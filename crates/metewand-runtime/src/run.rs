//! Manifest-driven execution of the trusted local Gate-1 vertical path.

mod resolution;

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use metewand_core::{
    EXECUTION_SEMANTICS_VERSION,
    canonical::CanonicalJsonError,
    identity::IdentityError,
    planning::LogicalPlan,
    public_schemas::PublicSchemaCatalogError,
    records::{AttemptDiagnostic, ProvenanceRecord, RunAttemptOutcome, RunAttemptRecord},
    run_output::{
        RunAttemptOutput, RunAttemptStatus, RunDiagnostic, RunEvent, RunEventKind, RunReport,
        RunSummary,
    },
};
use serde_json::json;
use thiserror::Error;

use crate::{
    fixed_dataset_run::{FixedDatasetRun, run_fixed_dataset},
    local_tree::LocalTreeHashError,
    publication::{
        PublicationError, PublicationRequest, finalize_failed_attempt, finalize_fixed_dataset,
    },
    repository::CheckedRepository,
    source_bundle::{SourceBundleError, SourceBundleIdentityError},
    source_resolution::SourceResolutionError,
    worker_output::WorkerOutputError,
    worker_process::WorkerLogLimits,
};

use resolution::{ResolvedCandidate, ResolvedMeasurement, canonical};

/// Resolves and executes every supported measurement in deterministic plan order.
///
/// `output_directory` must name a new directory beneath an existing parent.
/// Preflight validates the complete selection before launching any worker or
/// creating that directory. Inputs are copied to private temporary storage;
/// resolved identities bind their contents and normalized launch templates,
/// while actual host paths remain provenance. Every attempt is published before
/// its terminal event. A failed attempt does not stop later selected slots.
///
/// The event sink runs synchronously at each transition. JSON Lines consumers
/// can therefore flush each event before execution advances. Sink failure stops
/// further execution without changing already published outcomes.
///
/// # Errors
///
/// Returns a preflight, input, publication, or event-delivery error. Worker and
/// scientific failures instead become typed failed attempts in the report.
pub fn run_repository(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    output_directory: &Path,
    mut event_sink: impl FnMut(&RunEvent) -> io::Result<()>,
) -> Result<RunReport, RunError> {
    let output_directory = destination(output_directory)?;
    let resolved = resolution::resolve(repository, plan, &output_directory)?;
    for candidate in &resolved.candidates {
        candidate.validate_destination(&output_directory)?;
    }
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&output_directory)?;
    fs::set_permissions(&output_directory, fs::Permissions::from_mode(0o700))?;
    fs::File::open(output_directory.parent().expect("destination has a parent"))?.sync_all()?;
    let attempt_slots = resolved
        .candidates
        .iter()
        .map(|candidate| candidate.measurements.len())
        .sum();
    let mut events = Vec::new();
    emit(
        &mut events,
        &mut event_sink,
        RunEventKind::RunStarted { attempt_slots },
    )?;
    let mut attempts = Vec::new();
    for candidate in &resolved.candidates {
        for measurement in &candidate.measurements {
            emit(
                &mut events,
                &mut event_sink,
                RunEventKind::AttemptStarted {
                    logical_slot_id: measurement.logical_slot,
                    resolved_slot_id: measurement.attempt_slot.id,
                },
            )?;
            let attempt = execute(
                repository,
                candidate,
                measurement,
                resolved.private_root.path(),
                &output_directory,
            )?;
            emit(
                &mut events,
                &mut event_sink,
                RunEventKind::AttemptFinished {
                    attempt: Box::new(attempt.clone()),
                },
            )?;
            attempts.push(attempt);
        }
    }
    let accepted_attempts = attempts
        .iter()
        .filter(|attempt| attempt.status == RunAttemptStatus::Accepted)
        .count();
    let failed_attempts = attempts.len() - accepted_attempts;
    emit(
        &mut events,
        &mut event_sink,
        RunEventKind::RunFinished {
            accepted_attempts,
            failed_attempts,
        },
    )?;
    Ok(RunReport {
        command: "run",
        benchmark: repository.manifest.name.to_string(),
        manifest_hash: repository.manifest_hash.to_string(),
        output_directory: output_directory
            .to_str()
            .expect("destination was checked for UTF-8")
            .to_owned(),
        attempts,
        events,
        summary: RunSummary {
            attempt_slots,
            accepted_attempts,
            failed_attempts,
        },
    })
}

fn execute(
    repository: &CheckedRepository,
    candidate: &ResolvedCandidate,
    measurement: &ResolvedMeasurement,
    private_root: &Path,
    output_root: &Path,
) -> Result<RunAttemptOutput, RunError> {
    let started_at = SystemTime::now();
    if let Err(error) = candidate.verify() {
        return failed_attempt(
            measurement,
            output_root,
            started_at,
            "input_changed",
            error.to_string(),
        );
    }
    let result = run_fixed_dataset(FixedDatasetRun {
        dataset_dir: candidate.dataset.directory.path(),
        problem_contract_id: &candidate.contract_id,
        problem_parameters: &candidate.problem_parameters,
        implementation_parameters: &candidate.implementation_parameters,
        implementation_seed: measurement.seed,
        implementation_launch: &candidate.implementation.launch,
        implementation_identity: &candidate.implementation.identity,
        evaluator_launch: &candidate.evaluator.launch,
        evaluator_identity: &candidate.evaluator.identity,
        result_schema: &candidate.result_schema,
        metric_schema: &candidate.metric_schema,
        schemas: &repository.schemas,
        private_root,
        timeouts: candidate.timeouts,
        log_limits: WorkerLogLimits::new(64 * 1024, 64 * 1024),
    });
    if let Err(error) = candidate.verify() {
        return failed_attempt(
            measurement,
            output_root,
            started_at,
            "input_changed",
            error.to_string(),
        );
    }
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            return failed_attempt(
                measurement,
                output_root,
                started_at,
                error.failure_kind().code(),
                error.to_string(),
            );
        }
    };
    let published = finalize_fixed_dataset(
        &result,
        PublicationRequest {
            root: output_root,
            attempt_slot: measurement.attempt_slot.id,
            observation_slot: measurement.observation_slot.id,
            retry_index: 0,
            provenance_data: measurement.provenance.clone(),
        },
    )?;
    let (status, diagnostic) = match &published.attempt.record.outcome {
        RunAttemptOutcome::Accepted { .. } => (RunAttemptStatus::Accepted, None),
        RunAttemptOutcome::Failed { diagnostic, .. } => (
            RunAttemptStatus::Failed,
            Some(RunDiagnostic {
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
            }),
        ),
    };
    Ok(RunAttemptOutput {
        logical_slot_id: measurement.logical_slot,
        resolved_slot_id: measurement.attempt_slot.id,
        attempt_id: published.attempt.id,
        status,
        observation_id: Some(published.observation.id),
        attempt_directory: format!("attempts/{}", published.attempt.id),
        observation_directory: Some(format!("observations/{}", published.observation.id)),
        result: Some(result.result.as_json().clone()),
        metrics: Some(result.metrics.as_json().clone()),
        diagnostic,
    })
}

fn failed_attempt(
    measurement: &ResolvedMeasurement,
    output_root: &Path,
    started_at: SystemTime,
    code: &str,
    message: String,
) -> Result<RunAttemptOutput, RunError> {
    let diagnostic = RunDiagnostic {
        code: code.to_owned(),
        message: message.clone(),
    };
    let (attempt, _) = finalize_failed_attempt(
        output_root,
        RunAttemptRecord {
            slot: measurement.attempt_slot.id,
            retry_index: 0,
            outcome: RunAttemptOutcome::Failed {
                diagnostic: AttemptDiagnostic {
                    code: code.to_owned(),
                    message,
                    details: None,
                },
                observation: None,
            },
            started_at,
            finished_at: SystemTime::now(),
            provenance: ProvenanceRecord {
                tool_version: env!("CARGO_PKG_VERSION").to_owned(),
                wire_protocol_version: metewand_protocol::WIRE_PROTOCOL_VERSION,
                execution_semantics_version: EXECUTION_SEMANTICS_VERSION,
                data: canonical(
                    json!({"context": measurement.provenance.as_json(), "logs_available": false}),
                )?,
            },
        },
    )?;
    Ok(RunAttemptOutput {
        logical_slot_id: measurement.logical_slot,
        resolved_slot_id: measurement.attempt_slot.id,
        attempt_id: attempt.id,
        status: RunAttemptStatus::Failed,
        observation_id: None,
        attempt_directory: format!("attempts/{}", attempt.id),
        observation_directory: None,
        result: None,
        metrics: None,
        diagnostic: Some(diagnostic),
    })
}

fn emit(
    events: &mut Vec<RunEvent>,
    sink: &mut impl FnMut(&RunEvent) -> io::Result<()>,
    kind: RunEventKind,
) -> Result<(), RunError> {
    let event = RunEvent {
        command: "run",
        sequence: events.len(),
        kind,
    };
    sink(&event).map_err(RunError::Event)?;
    events.push(event);
    Ok(())
}

fn destination(path: &Path) -> Result<PathBuf, RunError> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "output directory must name a new child directory",
        )
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let destination = fs::canonicalize(parent)?.join(name);
    if destination.to_str().is_none() {
        return Err(RunError::Unsupported(
            "output directory must be UTF-8".to_owned(),
        ));
    }
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "output directory already exists; choose a new `--output-dir` path",
            )
            .into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(destination)
}

/// A failure outside an individual worker's terminal outcome.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunError {
    /// Dataset construction or contract validation failed.
    #[error(transparent)]
    Materialization(#[from] crate::dataset_materialization::MaterializationError),
    /// The selected manifest requires a later execution gate.
    #[error("{0}")]
    Unsupported(String),
    /// An input changed after repository validation or resolution.
    #[error("declared execution inputs changed after validation or during execution")]
    ChangedInput,
    /// Local resolution or output-directory setup failed.
    #[error("run setup failed: {0}")]
    Io(#[from] io::Error),
    /// The event consumer stopped accepting output.
    #[error("failed to deliver run event: {0}")]
    Event(#[source] io::Error),
    /// A canonical identity could not be constructed.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// A resolved value is outside the canonical JSON domain.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// Declared input expansion failed.
    #[error(transparent)]
    Sources(#[from] SourceBundleError),
    /// Declared input hashing failed.
    #[error(transparent)]
    SourceIdentity(#[from] SourceBundleIdentityError),
    /// Source acquisition or independent artifact verification failed.
    #[error(transparent)]
    SourceResolution(#[from] SourceResolutionError),
    /// A private input tree could not be hashed.
    #[error(transparent)]
    Tree(#[from] LocalTreeHashError),
    /// The embedded validation contracts could not be loaded.
    #[error(transparent)]
    PublicSchemas(#[from] PublicSchemaCatalogError),
    /// A fixed dataset does not satisfy its output schema.
    #[error(transparent)]
    Dataset(#[from] WorkerOutputError),
    /// An observation or attempt could not be durably finalized.
    #[error(transparent)]
    Publication(#[from] PublicationError),
}
