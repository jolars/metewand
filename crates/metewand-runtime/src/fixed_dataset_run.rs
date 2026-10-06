//! One private, non-published Gate-1 run of a resolved fixed dataset.

use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use metewand_core::{
    canonical::CanonicalValue, public_schemas::PublicSchemaCatalogError,
    records::ResolvedLaunchRecord, schema::SchemaCatalog,
};
use metewand_protocol::{Capability, WorkerIdentity, WorkerRole};
use serde_json::Value;
use tempfile::Builder;
use thiserror::Error;

use crate::{
    local_worker::{LocalWorkerLaunchError, launch_trusted_local_worker},
    worker_output::{WorkerOutputError, WorkerOutputValidator},
    worker_process::WorkerLogLimits,
    worker_session::{PhaseTimeouts, WorkerSession, WorkerSessionError},
};

/// Fully resolved inputs for one fixed-dataset, one-shot measurement.
///
/// `dataset_dir` and `private_root` must be absolute paths. The dataset is a
/// trusted local input that has already been resolved; this narrow Gate-1 path
/// does not materialize it or create a private dataset view.
#[derive(Debug)]
pub struct FixedDatasetRun<'a> {
    /// Directory containing the fixed local dataset consumed by both workers.
    pub dataset_dir: &'a Path,
    /// Exact selected problem-contract identity sent over the worker protocol.
    pub problem_contract_id: &'a str,
    /// Resolved problem parameters.
    pub problem_parameters: &'a Value,
    /// Resolved implementation parameters.
    pub implementation_parameters: &'a Value,
    /// Deterministic resolved implementation seed.
    pub implementation_seed: u64,
    /// Exact resolved implementation launch.
    pub implementation_launch: &'a ResolvedLaunchRecord,
    /// Identity expected from the implementation handshake.
    pub implementation_identity: &'a WorkerIdentity,
    /// Exact resolved problem-owned evaluator launch.
    pub evaluator_launch: &'a ResolvedLaunchRecord,
    /// Identity expected from the evaluator handshake.
    pub evaluator_identity: &'a WorkerIdentity,
    /// Selected problem contract's canonical result schema.
    pub result_schema: &'a Path,
    /// Selected problem contract's evaluator metric schema.
    pub metric_schema: &'a Path,
    /// Offline schemas from the checked repository.
    pub schemas: &'a SchemaCatalog,
    /// Parent of private worker, result, and metric directories.
    pub private_root: &'a Path,
    /// Fully resolved operational deadlines for each worker phase.
    pub timeouts: PhaseTimeouts,
    /// Bounds for diagnostic worker logs.
    pub log_limits: WorkerLogLimits,
}

/// Validated values and timing from a run that has not been durably accepted.
#[derive(Debug)]
pub struct FixedDatasetResult {
    /// Canonical result data validated against the selected problem schema.
    pub result: CanonicalValue,
    /// Independent evaluator metrics validated against the metric schema.
    pub metrics: CanonicalValue,
    /// Orchestrator time from immediately before `prepare` through `execute`.
    pub timed_wall_time: Duration,
    /// Optional worker-reported time, retained only as a diagnostic.
    pub implementation_time_ns: Option<u64>,
    /// Implementation-owned diagnostic statistics.
    pub statistics: Value,
}

/// Launches the implementation and independent evaluator, then measures one run.
///
/// Worker launch and handshake happen before timing. The timer starts immediately
/// before `prepare` and stops after the complete `execute` response, which the
/// protocol sends only after result writing. Metewand validates the result and
/// runs and validates the evaluator outside the measured interval. All output
/// stays in private temporary directories and is removed on return or failure.
/// This function does not publish an observation or accept an attempt.
///
/// # Errors
///
/// Returns a role- or validation-specific error if any boundary fails.
pub fn run_fixed_dataset(
    request: FixedDatasetRun<'_>,
) -> Result<FixedDatasetResult, FixedDatasetRunError> {
    if !request.dataset_dir.is_absolute() || !request.private_root.is_absolute() {
        return Err(FixedDatasetRunError::NonAbsolutePath);
    }
    let dataset_dir = path_string(request.dataset_dir)?;
    let validator = WorkerOutputValidator::new(request.schemas)?;

    let result_dir = private_directory(request.private_root, "result-")?;
    let metrics_dir = private_directory(request.private_root, "metrics-")?;
    let result_path = path_string(result_dir.path())?;
    let metrics_path = metrics_dir.path().join("metrics.json");
    let metrics_path_string = path_string(&metrics_path)?;

    let implementation_process = launch_trusted_local_worker(
        request.implementation_launch,
        request.private_root,
        request.log_limits,
    )
    .map_err(|source| FixedDatasetRunError::ImplementationLaunch { source })?;
    let mut implementation = WorkerSession::connect(
        implementation_process,
        WorkerRole::Implementation,
        request.implementation_identity.clone(),
        &[Capability::OneShot],
        request.timeouts,
    )
    .map_err(|source| FixedDatasetRunError::Implementation { source })?;

    let evaluator_process = launch_trusted_local_worker(
        request.evaluator_launch,
        request.private_root,
        request.log_limits,
    )
    .map_err(|source| FixedDatasetRunError::EvaluatorLaunch { source })?;
    let mut evaluator = WorkerSession::connect(
        evaluator_process,
        WorkerRole::ProblemEvaluator,
        request.evaluator_identity.clone(),
        &[],
        request.timeouts,
    )
    .map_err(|source| FixedDatasetRunError::Evaluator { source })?;

    let started = Instant::now();
    implementation
        .prepare(
            dataset_dir.clone(),
            request.problem_contract_id,
            request.problem_parameters.clone(),
            request.implementation_parameters.clone(),
            request.implementation_seed,
        )
        .map_err(|source| FixedDatasetRunError::Implementation { source })?;
    let executed = implementation
        .execute(result_path.clone())
        .map_err(|source| FixedDatasetRunError::Implementation { source })?;
    let timed_wall_time = started.elapsed();

    let result = validator
        .validate_execution_result(result_dir.path(), &executed, request.result_schema)
        .map_err(|source| FixedDatasetRunError::InvalidResult { source })?;
    evaluator
        .evaluate(
            dataset_dir,
            request.problem_contract_id,
            request.problem_parameters.clone(),
            result_path,
            metrics_path_string,
        )
        .map_err(|source| FixedDatasetRunError::Evaluator { source })?;
    let metrics = validator
        .validate_metrics(
            metrics_dir.path(),
            Path::new("metrics.json"),
            request.metric_schema,
        )
        .map_err(|source| FixedDatasetRunError::InvalidMetrics { source })?;

    implementation
        .shutdown()
        .map_err(|source| FixedDatasetRunError::Implementation { source })?;
    evaluator
        .shutdown()
        .map_err(|source| FixedDatasetRunError::Evaluator { source })?;

    Ok(FixedDatasetResult {
        result: result.data().clone(),
        metrics: metrics.data().clone(),
        timed_wall_time,
        implementation_time_ns: executed.implementation_time_ns(),
        statistics: executed.statistics().clone(),
    })
}

fn private_directory(root: &Path, prefix: &str) -> Result<tempfile::TempDir, FixedDatasetRunError> {
    let directory = Builder::new()
        .prefix(prefix)
        .permissions(Permissions::from_mode(0o700))
        .tempdir_in(root)
        .map_err(FixedDatasetRunError::PrivateDirectory)?;
    fs::set_permissions(directory.path(), Permissions::from_mode(0o700))
        .map_err(FixedDatasetRunError::PrivateDirectory)?;
    Ok(directory)
}

fn path_string(path: &Path) -> Result<String, FixedDatasetRunError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| FixedDatasetRunError::NonUtf8Path(path.to_path_buf()))
}

/// A failed fixed-dataset worker run before durable acceptance.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FixedDatasetRunError {
    /// An input directory is not an absolute path.
    #[error("dataset and private-root paths must be absolute")]
    NonAbsolutePath,
    /// A worker protocol path cannot be encoded as UTF-8.
    #[error("path is not UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),
    /// A private output directory cannot be created or secured.
    #[error("failed to create private output: {0}")]
    PrivateDirectory(#[source] std::io::Error),
    /// The embedded public output schemas cannot be loaded.
    #[error("failed to load public output schemas: {0}")]
    PublicSchemas(#[from] PublicSchemaCatalogError),
    /// The resolved implementation cannot be launched.
    #[error("failed to launch implementation: {source}")]
    ImplementationLaunch {
        /// Underlying trusted-local launch error.
        #[source]
        source: LocalWorkerLaunchError,
    },
    /// The resolved evaluator cannot be launched.
    #[error("failed to launch evaluator: {source}")]
    EvaluatorLaunch {
        /// Underlying trusted-local launch error.
        #[source]
        source: LocalWorkerLaunchError,
    },
    /// The implementation protocol or lifecycle failed.
    #[error("implementation failed: {source}")]
    Implementation {
        /// Typed worker-session failure.
        #[source]
        source: WorkerSessionError,
    },
    /// The problem-owned evaluator protocol or lifecycle failed.
    #[error("evaluator failed: {source}")]
    Evaluator {
        /// Typed worker-session failure.
        #[source]
        source: WorkerSessionError,
    },
    /// The canonical result failed output or schema validation.
    #[error("invalid canonical result: {source}")]
    InvalidResult {
        /// Typed result-validation failure.
        #[source]
        source: WorkerOutputError,
    },
    /// The independent evaluator metrics failed output or schema validation.
    #[error("invalid evaluator metrics: {source}")]
    InvalidMetrics {
        /// Typed metrics-validation failure.
        #[source]
        source: WorkerOutputError,
    },
}
