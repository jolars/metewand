//! Private execution of resolved fixed datasets.

mod buffered;
pub(crate) use buffered::{BufferedEvent, run_buffered};

use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use metewand_core::{
    canonical::CanonicalValue,
    manifest::TimingScope,
    measurements::{AccountingScope, ExecutionMeasurements, Measurement},
    public_schemas::PublicSchemaCatalogError,
    records::{AttemptDiagnostic, CompletionStatus, ContentDigest, ResolvedLaunchRecord},
    schema::SchemaCatalog,
};
use metewand_protocol::{Capability, EvaluationVerdict, WorkerIdentity, WorkerRole};
use serde_json::Value;
use tempfile::Builder;
use thiserror::Error;

use crate::{
    artifact_store::{DatasetView, verify_dataset_tree},
    local_tree::{LocalTreeHashError, hash_local_tree},
    local_worker::{LocalWorkerLaunchError, launch_trusted_local_worker},
    worker_output::{ValidatedMetrics, ValidatedResult, WorkerOutputError, WorkerOutputValidator},
    worker_process::{WorkerLogLimits, WorkerLogSnapshot},
    worker_session::{PhaseTimeouts, WorkerSession, WorkerSessionError},
};

/// Orchestrator timing scope and its independent scientific wall-time limit.
#[derive(Clone, Copy, Debug)]
pub struct ExecutionTimingPolicy {
    /// Scope authorized by the selected problem contract.
    pub scope: TimingScope,
    /// Total time allowed across the selected scope; `None` leaves only phase limits.
    pub timeout: Option<Duration>,
}

impl Default for ExecutionTimingPolicy {
    fn default() -> Self {
        Self {
            scope: TimingScope::PrepareAndExecute,
            timeout: None,
        }
    }
}

struct TimingEnvelope {
    policy: ExecutionTimingPolicy,
    started: Option<Instant>,
    deadline: Option<Instant>,
    finished: bool,
}

impl TimingEnvelope {
    fn new(policy: ExecutionTimingPolicy) -> Result<Self, FixedDatasetRunError> {
        if policy
            .timeout
            .is_some_and(|limit| limit.is_zero() || Instant::now().checked_add(limit).is_none())
        {
            return Err(FixedDatasetRunError::InvalidScientificTimeout);
        }
        Ok(Self {
            policy,
            started: None,
            deadline: None,
            finished: false,
        })
    }

    fn start(&mut self) -> Result<(), FixedDatasetRunError> {
        let started = Instant::now();
        self.deadline = self
            .policy
            .timeout
            .map(|limit| {
                started
                    .checked_add(limit)
                    .ok_or(FixedDatasetRunError::InvalidScientificTimeout)
            })
            .transpose()?;
        self.started = Some(started);
        Ok(())
    }

    fn finish(&mut self) -> Result<Duration, FixedDatasetRunError> {
        let finished = Instant::now();
        if self.deadline.is_some_and(|deadline| finished >= deadline) {
            return Err(FixedDatasetRunError::ScientificTimeout);
        }
        self.finished = true;
        Ok(finished.duration_since(self.started.expect("the envelope starts before execution")))
    }
}

/// Fully resolved inputs for one fixed-dataset, one-shot measurement.
///
/// `dataset_dir` and `private_root` must be absolute paths. The dataset is a
/// trusted local input that has already been resolved; this narrow Gate-1 path
/// does not materialize it or create a private dataset view.
#[derive(Debug)]
pub struct FixedDatasetRun<'a> {
    /// Selected canonical control, absent for one-shot execution.
    pub observation_control: Option<&'a Value>,
    /// Directory containing the fixed local dataset consumed by both workers.
    pub dataset_dir: &'a Path,
    /// Exact selected problem-contract identity sent over the worker protocol.
    pub problem_contract_id: &'a str,
    /// Resolved problem parameters.
    pub problem_parameters: &'a Value,
    /// Resolved scientific rules owned and hashed by the problem contract.
    pub problem_semantics: &'a Value,
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
    /// Selected timing scope and scientific deadline, independent of phase limits.
    pub timing: ExecutionTimingPolicy,
    /// Bounds for diagnostic worker logs.
    pub log_limits: WorkerLogLimits,
}

/// Validated values and timing from a run that has not been durably accepted.
#[derive(Debug)]
pub struct FixedDatasetResult {
    /// Complete, partial, or unavailable phase and resource observations.
    pub measurements: ExecutionMeasurements,
    /// Canonical result data validated against the selected problem schema.
    pub result: CanonicalValue,
    /// Independent evaluator metrics validated against the metric schema.
    pub metrics: CanonicalValue,
    /// Problem-owned completion decision for this scientifically valid result.
    pub completion: CompletionStatus,
    /// Orchestrator wall time covering the complete selected timing envelope.
    pub timed_wall_time: Duration,
    /// Optional worker-reported time, retained only as a diagnostic.
    pub implementation_time_ns: Option<u64>,
    /// Implementation-owned diagnostic statistics.
    pub statistics: Value,
    /// Complete independent evaluator exchange, excluding metrics validation.
    pub evaluation_time: Duration,
    /// Wall-clock boundaries retained only as provenance.
    pub started_at: SystemTime,
    /// Wall-clock time when the private validated run finished.
    pub finished_at: SystemTime,
    /// Bounded process-lifetime logs; retained workers return incomplete snapshots.
    pub implementation_output: WorkerLogSnapshot,
    /// Completely drained evaluator logs for this execution.
    pub evaluator_output: WorkerLogSnapshot,
    pub(crate) result_tree_hash: ContentDigest,
    pub(crate) metrics_bytes: Vec<u8>,
    pub(crate) execution_provenance: Value,
    validated_result: ValidatedResult,
    validated_metrics: ValidatedMetrics,
    result_output: tempfile::TempDir,
    metrics_output: tempfile::TempDir,
}

impl FixedDatasetResult {
    /// Returns the private directory holding the complete validated result.
    #[must_use]
    pub fn result_directory(&self) -> &Path {
        self.result_output.path()
    }

    /// Returns the private evaluator metrics document.
    #[must_use]
    pub fn metrics_document(&self) -> PathBuf {
        self.metrics_output
            .path()
            .join(self.validated_metrics.metrics_path())
    }

    /// Returns the result's validated manifest path and file inventory.
    #[must_use]
    pub const fn validated_result(&self) -> &ValidatedResult {
        &self.validated_result
    }

    /// Returns the validated evaluator metrics data and relative path.
    #[must_use]
    pub const fn validated_metrics(&self) -> &ValidatedMetrics {
        &self.validated_metrics
    }
}

/// The private result of one one-shot execution before durable publication.
#[derive(Debug)]
pub enum FixedDatasetRunOutcome {
    /// Both documents passed validation and the evaluator declared the result valid.
    Validated(Box<FixedDatasetResult>),
    /// Execution ended without a result eligible for scientific review.
    Failed {
        /// Stable technical failure class.
        kind: FixedDatasetFailureKind,
        /// Diagnostic material for a later failed attempt record.
        diagnostic: AttemptDiagnostic,
    },
}

/// Technical failure classes retained before an attempt can be published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedDatasetFailureKind {
    InvalidBuffer,
    MetricDisagreement,
    ResourceLimit,
    /// A runtime control ceased to hold over the worker lifetime.
    ControlEnforcementLost,
    /// Inputs or local setup prevented execution.
    SetupFailure,
    /// A worker changed its dataset view or the canonical cache object.
    DatasetMutation,
    /// The implementation reported an operation failure.
    ImplementationFailure,
    /// The implementation exited before completing its protocol exchange.
    ImplementationCrash,
    /// The complete selected scientific timing scope exceeded its deadline.
    ScientificTimeout,
    /// A worker exceeded its phase deadline.
    PhaseTimeout,
    /// The canonical result failed manifest or schema validation.
    InvalidResult,
    /// The evaluator failed or produced invalid metrics.
    EvaluatorFailure,
    /// A worker violated protocol framing, correlation, or message shape.
    ProtocolError,
}

impl FixedDatasetFailureKind {
    /// Returns the stable code suitable for an [`AttemptDiagnostic`].
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidBuffer => "invalid_buffer",
            Self::MetricDisagreement => "metric_disagreement",
            Self::ResourceLimit => "resource_limit",
            Self::ControlEnforcementLost => "control_enforcement_lost",
            Self::SetupFailure => "setup_failure",
            Self::DatasetMutation => "dataset_mutation",
            Self::ImplementationFailure => "implementation_failure",
            Self::ImplementationCrash => "implementation_crash",
            Self::ScientificTimeout => "scientific_timeout",
            Self::PhaseTimeout => "phase_timeout",
            Self::InvalidResult => "invalid_result",
            Self::EvaluatorFailure => "evaluator_failure",
            Self::ProtocolError => "protocol_error",
        }
    }
}

/// Runs a private one-shot attempt and retains its technical outcome.
///
/// The validated variant carries a problem-owned scientific verdict but must
/// not become `RunAttemptOutcome::Accepted` until durable publication succeeds.
#[must_use]
pub fn run_fixed_dataset_outcome(request: FixedDatasetRun<'_>) -> FixedDatasetRunOutcome {
    let mut measurements = ExecutionMeasurements::default();
    let timing = request.timing;
    let timeouts = request.timeouts;
    match run_checked(request, None, &mut measurements, None, None) {
        Ok(result) => FixedDatasetRunOutcome::Validated(Box::new(result)),
        Err(error) => {
            let kind = error.failure_kind();
            FixedDatasetRunOutcome::Failed {
                kind,
                diagnostic: AttemptDiagnostic {
                    code: kind.code().to_owned(),
                    message: error.to_string(),
                    details: Some(
                        CanonicalValue::try_from(serde_json::json!({
                            "measurements": measurements,
                            "timing_scope": timing.scope.to_string(),
                            "operation_scope": timing.scope == TimingScope::ExecuteOnly,
                            "scientific_timeout_ns": timing.timeout.map(|limit| limit.as_nanos().to_string()),
                            "operational_timeouts_ns": timeouts.provenance(),
                        }))
                        .expect("measurements use canonical safe integers"),
                    ),
                },
            }
        }
    }
}

/// Launches the implementation and independent evaluator, then measures one run.
///
/// The selected envelope includes launch, preparation, or only execution.
/// The scientific timeout applies continuously across that envelope. Operational
/// phase limits also bound worker exchanges outside it. Dataset integrity checks
/// between timed operations contribute to the continuous duration.
/// Metewand validates the result and runs and validates the evaluator outside
/// the measured interval. All output
/// stays in private temporary directories. Failed runs remove them immediately;
/// successful results own them until dropped so a later publication step can
/// copy the complete validated artifact.
/// This function does not publish an observation or accept an attempt.
///
/// # Errors
///
/// Returns a role- or validation-specific error if any boundary fails.
pub fn run_fixed_dataset(
    request: FixedDatasetRun<'_>,
) -> Result<FixedDatasetResult, FixedDatasetRunError> {
    run_checked(
        request,
        None,
        &mut ExecutionMeasurements::default(),
        None,
        None,
    )
}

pub(crate) fn run_fixed_dataset_in_view(
    request: FixedDatasetRun<'_>,
    view: &DatasetView,
    measurements: &mut ExecutionMeasurements,
    observer: Option<&mut PhaseObserver<'_>>,
) -> Result<FixedDatasetResult, FixedDatasetRunError> {
    run_checked(request, Some(view), measurements, None, observer)
}

pub(crate) fn run_fixed_dataset_with_session(
    request: FixedDatasetRun<'_>,
    view: &DatasetView,
    measurements: &mut ExecutionMeasurements,
    session: &mut WorkerSession,
    observer: Option<&mut PhaseObserver<'_>>,
) -> Result<FixedDatasetResult, FixedDatasetRunError> {
    run_checked(request, Some(view), measurements, Some(session), observer)
}

type PhaseObserver<'a> =
    dyn FnMut(crate::run_store::AttemptPhase) -> Result<(), crate::run_store::RunStoreError> + 'a;

fn run_checked(
    request: FixedDatasetRun<'_>,
    view: Option<&DatasetView>,
    measurements: &mut ExecutionMeasurements,
    session: Option<&mut WorkerSession>,
    observer: Option<&mut PhaseObserver<'_>>,
) -> Result<FixedDatasetResult, FixedDatasetRunError> {
    if !request.dataset_dir.is_absolute() || !request.private_root.is_absolute() {
        return Err(FixedDatasetRunError::NonAbsolutePath);
    }
    let mut timing = TimingEnvelope::new(request.timing)?;
    let digest = match view {
        Some(view) => view.digest(),
        None => hash_local_tree(request.dataset_dir)
            .map_err(|source| FixedDatasetRunError::DatasetTreeHash { source })?,
    };
    let integrity = DatasetIntegrity {
        directory: request.dataset_dir,
        digest,
        view,
    };
    integrity.verify("attempt start")?;
    let result = run_workers(
        request,
        &integrity,
        measurements,
        &mut timing,
        session,
        observer,
    );
    if let Some(started) = timing.started
        && !timing.finished
    {
        measurements.timed_wall_time_ns = Measurement::elapsed(started.elapsed(), false);
    }
    // Owned sessions have stopped here; failures on a borrowed session cause
    // its lifecycle owner to discard it after this final integrity check.
    let verified = integrity.verify("attempt finalization");
    match result {
        Err(error) if error.failure_kind() == FixedDatasetFailureKind::DatasetMutation => {
            Err(error)
        }
        result => {
            verified?;
            result
        }
    }
}

fn run_workers(
    request: FixedDatasetRun<'_>,
    integrity: &DatasetIntegrity<'_>,
    measurements: &mut ExecutionMeasurements,
    timing: &mut TimingEnvelope,
    session: Option<&mut WorkerSession>,
    mut observer: Option<&mut PhaseObserver<'_>>,
) -> Result<FixedDatasetResult, FixedDatasetRunError> {
    measurements.cpu_time_scope = match request.timing.scope {
        TimingScope::ColdEndToEnd => AccountingScope::WorkerLifetimeThroughExecute,
        TimingScope::PrepareAndExecute => AccountingScope::PrepareAndExecute,
        TimingScope::ExecuteOnly => AccountingScope::ExecuteOnly,
    };
    let started_at = SystemTime::now();
    let dataset_dir = path_string(request.dataset_dir)?;
    let validator = WorkerOutputValidator::new(request.schemas)?;

    let result_dir = private_directory(request.private_root, "result-")?;
    let metrics_dir = private_directory(request.private_root, "metrics-")?;
    let result_path = path_string(result_dir.path())?;
    let metrics_path = metrics_dir.path().join("metrics.json");
    let metrics_path_string = path_string(&metrics_path)?;

    let mut owned_implementation = if session.is_none() {
        Some(integrity.operation("implementation hello", || {
            measure(&mut measurements.worker_startup_time_ns, || {
                if request.timing.scope == TimingScope::ColdEndToEnd {
                    timing.start()?;
                }
                let implementation_process = launch_trusted_local_worker(
                    request.implementation_launch,
                    request.private_root,
                    request.log_limits,
                )
                .map_err(|source| FixedDatasetRunError::ImplementationLaunch { source })?;
                WorkerSession::connect_with_deadline(
                    implementation_process,
                    WorkerRole::Implementation,
                    request.implementation_identity.clone(),
                    &[if request.observation_control.is_some() {
                        Capability::FreshSequence
                    } else {
                        Capability::OneShot
                    }],
                    request.timeouts,
                    timing.deadline,
                )
                .map_err(|source| FixedDatasetRunError::Implementation { source })
            })
        })?)
    } else {
        measurements.worker_startup_time_ns = Measurement::elapsed(Duration::ZERO, true);
        None
    };
    let implementation = match session {
        Some(session) => session,
        None => owned_implementation
            .as_mut()
            .expect("a fresh execution owns its session"),
    };

    if let Some(observer) = observer.as_mut() {
        observer(crate::run_store::AttemptPhase::Preparing)?;
    }
    let mut baseline = None;
    integrity.operation("implementation prepare", || {
        if request.timing.scope == TimingScope::PrepareAndExecute {
            baseline = Some(implementation.process_usage());
            timing.start()?;
        }
        implementation.set_scientific_deadline(timing.deadline);
        let prepared = measure(&mut measurements.prepare_time_ns, || {
            implementation
                .prepare(
                    dataset_dir.clone(),
                    request.problem_contract_id,
                    request.problem_parameters.clone(),
                    request.implementation_parameters.clone(),
                    request.implementation_seed,
                )
                .map_err(|source| FixedDatasetRunError::Implementation { source })
        });
        if timing.started.is_some() {
            let (cpu, memory) = implementation.process_usage().since_optional(baseline);
            measurements.cpu_time_ns = cpu;
            measurements.peak_rss_bytes = memory;
        }
        prepared
    })?;
    if let Some(observer) = observer.as_mut() {
        observer(crate::run_store::AttemptPhase::Executing)?;
    }
    let (executed, timed_wall_time) = integrity.operation("implementation execute", || {
        if request.timing.scope == TimingScope::ExecuteOnly {
            baseline = Some(implementation.process_usage());
            timing.start()?;
        }
        implementation.set_scientific_deadline(timing.deadline);
        let executed = measure(&mut measurements.execute_request_time_ns, || {
            match request.observation_control {
                Some(control) => implementation.execute_fresh(result_path.clone(), control.clone()),
                None => implementation.execute(result_path.clone()),
            }
            .map_err(|source| FixedDatasetRunError::Implementation { source })
        });
        let elapsed = if executed.is_ok() {
            match timing.finish() {
                Ok(elapsed) => Some(elapsed),
                Err(error) => {
                    implementation.discard_after_timeout();
                    return Err(error);
                }
            }
        } else {
            None
        };
        let (cpu, memory) = implementation.process_usage().since_optional(baseline);
        measurements.cpu_time_ns = cpu;
        measurements.peak_rss_bytes = memory;
        let executed = executed?;
        let elapsed = elapsed.expect("a successful exchange completed the envelope");
        measurements.timed_wall_time_ns = Measurement::elapsed(elapsed, true);
        Ok((executed, elapsed))
    })?;
    implementation.set_scientific_deadline(None);

    let mut evaluator = integrity.operation("evaluator hello", || {
        measure(&mut measurements.evaluator_startup_time_ns, || {
            let evaluator_process = launch_trusted_local_worker(
                request.evaluator_launch,
                request.private_root,
                request.log_limits,
            )
            .map_err(|source| FixedDatasetRunError::EvaluatorLaunch { source })?;
            WorkerSession::connect(
                evaluator_process,
                WorkerRole::ProblemEvaluator,
                request.evaluator_identity.clone(),
                &[],
                request.timeouts,
            )
            .map_err(|source| FixedDatasetRunError::Evaluator { source })
        })
    })?;

    let execution_provenance = serde_json::json!({
        "environment_class": "development",
        "executor": "trusted_local",
        "architecture": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "implementation": worker_provenance(request.implementation_launch, implementation),
        "evaluator": worker_provenance(request.evaluator_launch, &evaluator),
        "problem_contract_id": request.problem_contract_id,
        "problem_parameters": request.problem_parameters,
        "implementation_parameters": request.implementation_parameters,
        "implementation_seed": request.implementation_seed,
        "timing_scope": request.timing.scope.to_string(),
        "operation_scope": request.timing.scope == TimingScope::ExecuteOnly,
        "scientific_timeout_ns": request.timing.timeout.map(|limit| limit.as_nanos().to_string()),
        "operational_timeouts_ns": request.timeouts.provenance(),
        "timing_dataset_integrity": if request.timing.scope == TimingScope::ExecuteOnly { "outside_execute" } else { "included_between_prepare_and_execute" },
        "timing_resource_accounting": if request.timing.scope == TimingScope::ExecuteOnly { "outside_execute" } else { "included_between_prepare_and_execute" },
        "timing_coordination": if observer.is_none() { "none" } else if request.timing.scope == TimingScope::ExecuteOnly { "outside_execute" } else { "included_between_prepare_and_execute" },
        "timing_excluded_phases": excluded_phases(request.timing.scope),
        "dataset_directory": request.dataset_dir,
        "dataset_tree_sha256": hex_digest(integrity.digest),
        "dataset_immutability": {
            "view": if integrity.view.is_some() { "copy" } else { "caller_provided" },
            "enforcement": "best_effort",
            "verification": "each_worker_operation",
        },
        "result_schema": request.result_schema,
        "metric_schema": request.metric_schema,
        "controls": metewand_core::controls::default_local_control_report(),
    });

    if let Some(observer) = observer.as_mut() {
        observer(crate::run_store::AttemptPhase::Validating)?;
    }
    let result = validator
        .validate_execution_result(result_dir.path(), &executed, request.result_schema)
        .map_err(|source| FixedDatasetRunError::InvalidResult { source })?;
    let result_tree_hash = hash_local_tree(result_dir.path())
        .map_err(|source| FixedDatasetRunError::ResultTreeHash { source })?;
    if let Some(observer) = observer.as_mut() {
        observer(crate::run_store::AttemptPhase::Evaluating)?;
    }
    let (evaluated, evaluation_time) = integrity.operation("evaluator evaluate", || {
        let started = Instant::now();
        let evaluated = evaluator
            .evaluate(
                dataset_dir,
                request.problem_contract_id,
                request.problem_parameters.clone(),
                request.problem_semantics.clone(),
                result_path,
                metrics_path_string,
            )
            .map_err(|source| FixedDatasetRunError::Evaluator { source });
        let elapsed = started.elapsed();
        measurements.evaluation_time_ns = Measurement::elapsed(elapsed, evaluated.is_ok());
        evaluated.map(|evaluated| (evaluated, elapsed))
    })?;
    verify_result_tree(result_dir.path(), result_tree_hash)?;
    let metrics = validator
        .validate_metrics(
            metrics_dir.path(),
            Path::new("metrics.json"),
            request.metric_schema,
        )
        .map_err(|source| FixedDatasetRunError::InvalidMetrics { source })?;

    let metrics_bytes = fs::read(&metrics_path).map_err(FixedDatasetRunError::PrivateDirectory)?;
    let implementation_snapshot = implementation.log_snapshot();
    let implementation_output = match owned_implementation.take() {
        Some(implementation) => integrity.operation("implementation shutdown", || {
            implementation
                .shutdown()
                .map(WorkerLogSnapshot::from)
                .map_err(|source| FixedDatasetRunError::Implementation { source })
        })?,
        None => implementation_snapshot,
    };
    let evaluator_output = integrity.operation("evaluator shutdown", || {
        evaluator
            .shutdown()
            .map(WorkerLogSnapshot::from)
            .map_err(|source| FixedDatasetRunError::Evaluator { source })
    })?;
    verify_result_tree(result_dir.path(), result_tree_hash)?;
    if fs::read(&metrics_path).map_err(FixedDatasetRunError::PrivateDirectory)? != metrics_bytes {
        return Err(FixedDatasetRunError::MetricsChanged);
    }

    let completion = match evaluated.verdict() {
        EvaluationVerdict::Invalid => return Err(FixedDatasetRunError::ScientificInvalidResult),
        EvaluationVerdict::Reached => CompletionStatus::Reached,
        EvaluationVerdict::NotReached => CompletionStatus::NotReached,
        EvaluationVerdict::NotRequired => CompletionStatus::NotRequired,
    };

    Ok(FixedDatasetResult {
        measurements: measurements.clone(),
        result: result.data().clone(),
        metrics: metrics.data().clone(),
        completion,
        timed_wall_time,
        implementation_time_ns: executed.implementation_time_ns(),
        statistics: executed.statistics().clone(),
        evaluation_time,
        started_at,
        finished_at: SystemTime::now(),
        implementation_output,
        evaluator_output,
        result_tree_hash,
        metrics_bytes,
        execution_provenance,
        validated_result: result,
        validated_metrics: metrics,
        result_output: result_dir,
        metrics_output: metrics_dir,
    })
}

fn excluded_phases(scope: TimingScope) -> Vec<&'static str> {
    let mut excluded = vec![
        "materialization",
        "evaluator_startup",
        "result_validation",
        "evaluation",
        "metrics_validation",
        "shutdown",
    ];
    if scope != TimingScope::ColdEndToEnd {
        excluded.push("worker_startup");
    }
    if scope == TimingScope::ExecuteOnly {
        excluded.push("prepare");
    }
    excluded
}

struct DatasetIntegrity<'a> {
    directory: &'a Path,
    digest: ContentDigest,
    view: Option<&'a DatasetView>,
}

impl DatasetIntegrity<'_> {
    fn verify(&self, operation: &'static str) -> Result<(), FixedDatasetRunError> {
        match self.view {
            Some(view) => view.verify(),
            None => verify_dataset_tree(self.directory, self.digest),
        }
        .map_err(|error| FixedDatasetRunError::DatasetMutation {
            operation,
            message: error.to_string(),
        })
    }

    fn operation<T>(
        &self,
        operation: &'static str,
        action: impl FnOnce() -> Result<T, FixedDatasetRunError>,
    ) -> Result<T, FixedDatasetRunError> {
        self.verify(operation)?;
        let result = action();
        // A mutation invalidates the attempt even when the worker also fails.
        self.verify(operation)?;
        result
    }
}

fn measure<T, E>(
    measurement: &mut Measurement,
    action: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let started = Instant::now();
    let result = action();
    *measurement = Measurement::elapsed(started.elapsed(), result.is_ok());
    result
}

fn worker_provenance(launch: &ResolvedLaunchRecord, session: &WorkerSession) -> Value {
    serde_json::json!({
        "program": launch.program,
        "args": launch.args,
        "environment_variables": launch.environment_variables,
        "worker_environment": session.environment(),
        "environment_id": launch.environment,
        "worker_id": session.negotiated().worker_id(),
        "sdk": session.negotiated().sdk(),
        "capabilities": session.negotiated().reported_capabilities(),
    })
}

pub(crate) fn hex_digest(digest: ContentDigest) -> String {
    digest
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verify_result_tree(root: &Path, expected: ContentDigest) -> Result<(), FixedDatasetRunError> {
    let observed =
        hash_local_tree(root).map_err(|source| FixedDatasetRunError::ResultTreeHash { source })?;
    if observed != expected {
        return Err(FixedDatasetRunError::ResultTreeChanged);
    }
    Ok(())
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
    #[error("invalid buffered profile: {0}")]
    InvalidBuffer(String),
    #[error("provisional metrics disagree with independent evaluation: {0}")]
    MetricDisagreement(String),
    /// Durable execution ownership or phase recording failed.
    #[error(transparent)]
    Coordination(#[from] crate::run_store::RunStoreError),
    /// The scientific timeout cannot form a positive monotonic deadline.
    #[error("scientific timeout must be positive and representable by the monotonic clock")]
    InvalidScientificTimeout,
    /// The selected timing envelope expired between worker exchanges.
    #[error("scientific timeout expired over the selected timing envelope")]
    ScientificTimeout,
    /// A dataset cannot be verified against its pre-launch content identity.
    #[error("dataset mutation around {operation}: {message}")]
    DatasetMutation {
        /// Worker operation whose integrity check detected the change.
        operation: &'static str,
        /// Private-view or canonical-object verification failure.
        message: String,
    },
    /// The evaluator rejected the result under problem-owned validity rules.
    #[error("problem evaluator rejected canonical result")]
    ScientificInvalidResult,
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
    /// The complete canonical result tree could not be rechecked.
    #[error("failed to hash canonical result tree: {source}")]
    ResultTreeHash {
        /// Result-tree hashing failure.
        #[source]
        source: LocalTreeHashError,
    },
    /// The fixed dataset could not be fingerprinted for provenance.
    #[error("failed to hash fixed dataset: {source}")]
    DatasetTreeHash {
        /// Dataset-tree hashing failure.
        #[source]
        source: LocalTreeHashError,
    },
    /// The evaluator or worker changed the validated canonical result tree.
    #[error("canonical result tree changed after validation")]
    ResultTreeChanged,
    /// The evaluator changed its metrics after validation.
    #[error("evaluator metrics changed after validation")]
    MetricsChanged,
    /// The independent evaluator metrics failed output or schema validation.
    #[error("invalid evaluator metrics: {source}")]
    InvalidMetrics {
        /// Typed metrics-validation failure.
        #[source]
        source: WorkerOutputError,
    },
}

impl FixedDatasetRunError {
    /// Classifies a failed private run for a later attempt diagnostic.
    #[must_use]
    pub fn failure_kind(&self) -> FixedDatasetFailureKind {
        match self {
            Self::InvalidBuffer(_) => FixedDatasetFailureKind::InvalidBuffer,
            Self::MetricDisagreement(_) => FixedDatasetFailureKind::MetricDisagreement,
            Self::DatasetMutation { .. } => FixedDatasetFailureKind::DatasetMutation,
            Self::ScientificTimeout => FixedDatasetFailureKind::ScientificTimeout,
            Self::Coordination(_)
            | Self::InvalidScientificTimeout
            | Self::NonAbsolutePath
            | Self::NonUtf8Path(_)
            | Self::PrivateDirectory(_)
            | Self::PublicSchemas(_)
            | Self::DatasetTreeHash { .. }
            | Self::ImplementationLaunch { .. }
            | Self::EvaluatorLaunch { .. } => FixedDatasetFailureKind::SetupFailure,
            Self::ScientificInvalidResult
            | Self::InvalidResult { .. }
            | Self::ResultTreeHash { .. }
            | Self::ResultTreeChanged => FixedDatasetFailureKind::InvalidResult,
            Self::InvalidMetrics { .. } | Self::MetricsChanged => {
                FixedDatasetFailureKind::EvaluatorFailure
            }
            Self::Implementation { source } => classify_session(source, true),
            Self::Evaluator { source } => classify_session(source, false),
        }
    }
}

fn classify_session(source: &WorkerSessionError, implementation: bool) -> FixedDatasetFailureKind {
    match source {
        WorkerSessionError::WorkerFailure { failure, .. }
            if failure.code() == metewand_protocol::WorkerFailureCode::ResourceLimit =>
        {
            FixedDatasetFailureKind::ResourceLimit
        }
        WorkerSessionError::ControlEnforcementLost { .. }
        | WorkerSessionError::Process {
            source: crate::worker_process::WorkerProcessError::LogDrainDeadline { .. },
            ..
        } => FixedDatasetFailureKind::ControlEnforcementLost,
        WorkerSessionError::Timeout { .. } => FixedDatasetFailureKind::PhaseTimeout,
        WorkerSessionError::ScientificTimeout { .. } => FixedDatasetFailureKind::ScientificTimeout,
        WorkerSessionError::ObservationControlMismatch
        | WorkerSessionError::ProtocolRead { .. }
        | WorkerSessionError::Response { .. }
        | WorkerSessionError::Handshake { .. }
        | WorkerSessionError::Encode { .. }
        | WorkerSessionError::Request { .. }
        | WorkerSessionError::InvalidRole { .. }
        | WorkerSessionError::CapabilityNotSelected { .. } => {
            FixedDatasetFailureKind::ProtocolError
        }
        WorkerSessionError::ProtocolClosed { .. }
        | WorkerSessionError::ProtocolThreadStopped { .. }
        | WorkerSessionError::ProtocolWrite { .. }
        | WorkerSessionError::Process { .. }
        | WorkerSessionError::UnsuccessfulExit { .. }
            if implementation =>
        {
            FixedDatasetFailureKind::ImplementationCrash
        }
        _ if implementation => FixedDatasetFailureKind::ImplementationFailure,
        _ => FixedDatasetFailureKind::EvaluatorFailure,
    }
}
