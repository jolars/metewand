//! Explicit worker verification and problem-owned reference conformance.

use std::{fs, io, path::Path};

use metewand_core::{
    canonical::CanonicalValue,
    manifest::{DatasetDefinition, RepositoryPath},
    schema::SchemaValidationError,
};
use metewand_protocol::{
    Capability, EvaluationVerdict, ExecuteResponse, MaterializeResponse, RequestId, WorkerRole,
};
use serde::Serialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use thiserror::Error;

use super::{
    RunError,
    resolution::{ResolvedWorker, resolve_worker},
};
use crate::{
    local_tree::hash_local_tree,
    local_worker::{LocalWorkerLaunchError, launch_trusted_local_worker},
    repository::{CheckedRepository, RepositoryCheckError, check_repository},
    source_bundle::identify_source_bundle,
    source_resolution::{SourceRequest, resolve_source},
    worker_output::{WorkerOutputError, WorkerOutputValidator},
    worker_process::WorkerLogLimits,
    worker_session::{PhaseTimeouts, WorkerSession, WorkerSessionError},
};

/// Evidence produced by an explicitly authorized worker check.
#[derive(Debug, Serialize)]
pub struct WorkerCheckReport {
    /// Handshake evidence in deterministic component order.
    pub workers: Vec<WorkerCheck>,
    /// Successful problem reference cases in contract order.
    pub reference_cases: Vec<ReferenceCaseCheck>,
}

/// A worker's negotiated capabilities, including unselected observations.
#[derive(Debug, Serialize)]
pub struct WorkerCheck {
    /// Manifest component name.
    pub component: String,
    /// Protocol role.
    pub role: WorkerRole,
    /// Resolved content identity.
    pub worker_id: String,
    /// Evidence freshness.
    pub evidence: &'static str,
    /// Capabilities explicitly selected from declarations.
    pub selected_capabilities: Vec<Capability>,
    /// All capabilities reported by the worker.
    pub reported_capabilities: Vec<Capability>,
}

/// An evaluator result that exactly matched a reference expectation.
#[derive(Debug, Serialize)]
pub struct ReferenceCaseCheck {
    /// Manifest problem name.
    pub problem: String,
    /// Zero-based index in the contract.
    pub case_index: usize,
    /// Evaluated canonical metric data.
    pub metrics: Value,
}

/// Worker-check failure. A failed check never claims verified evidence.
#[derive(Debug, Error)]
pub enum WorkerCheckError {
    /// Repository inputs changed or became invalid during checking.
    #[error(transparent)]
    Repository(#[from] RepositoryCheckError),
    /// Logical planning failed before worker launches.
    #[error(transparent)]
    Planning(#[from] metewand_core::planning::LogicalPlanningError),
    /// The local executor cannot provide required controls.
    #[error(transparent)]
    Executor(#[from] crate::executor::ExecutorError),
    /// Worker resolution or source integrity failure.
    #[error(transparent)]
    Resolution(#[from] RunError),
    /// Private staging failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Worker launch failed.
    #[error(transparent)]
    Launch(#[from] LocalWorkerLaunchError),
    /// Negotiation or evaluation failed.
    #[error(transparent)]
    Session(#[from] WorkerSessionError),
    /// A reference or worker output violated its schema.
    #[error(transparent)]
    Output(#[from] WorkerOutputError),
    /// Reference scientific data violated its schema.
    #[error(transparent)]
    Schema(#[from] SchemaValidationError),
    /// Reference conformance or input integrity failed.
    #[error("{0}")]
    Conformance(String),
}

/// Launches declared local workers and evaluates every problem reference case.
///
/// Sources and reference inputs are privately staged. No implementation is
/// timed or executed, no remote dataset is acquired, and no run is published.
/// Reference cases use the problem definition's parameter defaults (or `{}`).
///
/// # Errors
/// Returns an error for unsupported resolution, missing selected capabilities,
/// malformed reference artifacts, worker failures, input mutation, or metrics
/// that differ from the reference expectation.
pub fn check_repository_workers(
    repository: &CheckedRepository,
) -> Result<WorkerCheckReport, WorkerCheckError> {
    verify_repository(repository)?;
    let plan = repository.logical_plan()?;
    let executor =
        crate::executor::LocalProcessExecutor::new(std::time::Duration::from_millis(100));
    for experiment in &plan.experiments {
        executor.preflight_policy(&experiment.execution_policy.record)?;
    }
    let private = TempDir::new()?;
    let mut resolved = Vec::new();
    for (name, dataset) in &repository.manifest.datasets {
        if let DatasetDefinition::Generated(dataset) = dataset {
            resolved.push((
                name.to_string(),
                WorkerRole::DatasetMaterializer,
                vec![],
                resolve_worker(repository, &dataset.worker, private.path())?,
            ));
        }
    }
    for (name, implementation) in &repository.manifest.implementations {
        let capabilities = implementation
            .capabilities
            .iter()
            .map(|capability| match capability {
                metewand_core::manifest::ImplementationCapability::OneShot => Capability::OneShot,
            })
            .collect();
        resolved.push((
            name.to_string(),
            WorkerRole::Implementation,
            capabilities,
            resolve_worker(repository, &implementation.worker, private.path())?,
        ));
    }
    for (name, problem) in &repository.manifest.problems {
        resolved.push((
            name.to_string(),
            WorkerRole::ProblemEvaluator,
            vec![],
            resolve_worker(repository, &problem.evaluator, private.path())?,
        ));
    }
    let validator = WorkerOutputValidator::new(&repository.schemas).map_err(RunError::from)?;
    let mut report = WorkerCheckReport {
        workers: vec![],
        reference_cases: vec![],
    };
    for (name, role, capabilities, worker) in resolved {
        worker.verify()?;
        let mut session = connect(&worker, private.path(), role, &capabilities)?;
        let negotiated = session.negotiated();
        report.workers.push(WorkerCheck {
            component: name.clone(),
            role,
            worker_id: worker.identity.to_string(),
            evidence: "verified_now",
            selected_capabilities: negotiated.selected_capabilities().to_vec(),
            reported_capabilities: negotiated.reported_capabilities().to_vec(),
        });
        if role == WorkerRole::ProblemEvaluator {
            let contract = &repository.problem_contracts[name.as_str()];
            let definition = &repository.manifest.problems[name.as_str()];
            let parameters = definition
                .parameter_defaults
                .as_ref()
                .map_or_else(|| json!({}), |value| value.as_json().clone());
            repository
                .schemas
                .validate(contract.parameter_schema.as_path(), &parameters)?;
            for (case_index, case) in contract.reference_cases.iter().enumerate() {
                let staged = TempDir::new_in(private.path())?;
                let dataset = staged.path().join("dataset");
                fs::create_dir(&dataset)?;
                if let Some(source) = &case.dataset {
                    stage_reference(repository, source, &dataset, None, private.path())?;
                    if !dataset.join("dataset-manifest.json").exists() {
                        let files = super::resolution::file_inventory(&dataset)?;
                        fs::write(
                            dataset.join("dataset-manifest.json"),
                            serde_json::to_vec(
                                &json!({"schema_version": 1, "complete": true, "files": files}),
                            )
                            .expect("JSON inventory"),
                        )?;
                    }
                    let response = MaterializeResponse::new(request_id(), "dataset-manifest.json");
                    let mut valid = false;
                    for schema in &contract.dataset_schemas {
                        if validator
                            .validate_materialized_dataset(&dataset, &response, schema.as_path())
                            .is_ok()
                        {
                            valid = true;
                            break;
                        }
                    }
                    if !valid {
                        return Err(WorkerCheckError::Conformance(format!(
                            "problem `{name}` reference case {case_index}: dataset matches no accepted schema"
                        )));
                    }
                }
                let result = staged.path().join("result");
                fs::create_dir(&result)?;
                stage_reference(
                    repository,
                    &case.result,
                    &result,
                    Some(("result.json", contract.result_schema.as_path(), true)),
                    private.path(),
                )?;
                validator.validate_execution_result(
                    &result,
                    &ExecuteResponse::new(
                        request_id(),
                        None,
                        "result.json",
                        serde_json::Map::new(),
                    ),
                    contract.result_schema.as_path(),
                )?;
                let expected_root = staged.path().join("expected");
                fs::create_dir(&expected_root)?;
                stage_reference(
                    repository,
                    &case.expected_metrics,
                    &expected_root,
                    Some(("metrics.json", contract.metric_schema.as_path(), false)),
                    private.path(),
                )?;
                let expected = validator.validate_metrics(
                    &expected_root,
                    Path::new("metrics.json"),
                    contract.metric_schema.as_path(),
                )?;
                let before = hash_local_tree(staged.path()).map_err(RunError::from)?;
                let output = TempDir::new_in(private.path())?;
                let response = session.evaluate(
                    text(&dataset)?,
                    repository.resources.problem_contracts[name.as_str()].to_string(),
                    parameters.clone(),
                    text(&result)?,
                    text(&output.path().join("metrics.json"))?,
                )?;
                worker.verify()?;
                if hash_local_tree(staged.path()).map_err(RunError::from)? != before {
                    return Err(WorkerCheckError::Conformance(format!(
                        "problem `{name}` reference case {case_index}: evaluator mutated reference inputs"
                    )));
                }
                let metrics = validator.validate_metrics(
                    output.path(),
                    Path::new("metrics.json"),
                    contract.metric_schema.as_path(),
                )?;
                if response.verdict() != EvaluationVerdict::Reached
                    || metrics.data() != expected.data()
                {
                    return Err(WorkerCheckError::Conformance(format!(
                        "problem `{name}` reference case {case_index}: reference metrics disagree or verdict is not reached"
                    )));
                }
                report.reference_cases.push(ReferenceCaseCheck {
                    problem: name.clone(),
                    case_index,
                    metrics: metrics.data().as_json().clone(),
                });
            }
        }
        session.shutdown()?;
        worker.verify()?;
    }
    verify_repository(repository)?;
    Ok(report)
}

fn verify_repository(repository: &CheckedRepository) -> Result<(), WorkerCheckError> {
    let current = check_repository(&repository.manifest_path)?;
    if current.manifest_hash != repository.manifest_hash
        || current.resources.problem_contracts != repository.resources.problem_contracts
        || current.resources.schemas != repository.resources.schemas
        || current.resources.source_bundles != repository.resources.source_bundles
    {
        return Err(RunError::ChangedInput.into());
    }
    Ok(())
}

fn connect(
    worker: &ResolvedWorker,
    private: &Path,
    role: WorkerRole,
    capabilities: &[Capability],
) -> Result<WorkerSession, WorkerCheckError> {
    let process =
        launch_trusted_local_worker(&worker.launch, private, WorkerLogLimits::new(65536, 65536))?;
    Ok(WorkerSession::connect(
        process,
        role,
        worker.identity.clone(),
        capabilities,
        PhaseTimeouts::default(),
    )?)
}

fn request_id() -> RequestId {
    serde_json::from_value(json!("reference")).expect("fixed valid request identity")
}

fn text(path: &Path) -> Result<&str, WorkerCheckError> {
    path.to_str()
        .ok_or_else(|| WorkerCheckError::Conformance("reference path is not UTF-8".into()))
}

fn stage_reference(
    repository: &CheckedRepository,
    source: &RepositoryPath,
    destination: &Path,
    envelope: Option<(&str, &Path, bool)>,
    private: &Path,
) -> Result<(), WorkerCheckError> {
    let sources = std::slice::from_ref(source);
    let expected = identify_source_bundle(&repository.root, sources).map_err(RunError::from)?;
    let snapshot = resolve_source(
        &SourceRequest::Repository {
            root: &repository.root,
            sources,
            expected,
        },
        None,
        private,
    )
    .map_err(RunError::from)?;
    let path = snapshot.directory().join(source.as_path());
    if path.is_dir() {
        copy_tree(&path, destination)?;
    } else if let Some((filename, schema, result)) = envelope {
        let bytes = fs::read(&path)?;
        let data = CanonicalValue::from_slice(&bytes).map_err(RunError::from)?;
        let value = if data.as_json().get("schema_version").is_some() {
            data.as_json().clone()
        } else {
            let mut value = json!({"schema_version": 1, "schema": schema, "data": data.as_json()});
            if result {
                value["files"] = json!([]);
            }
            value
        };
        fs::write(
            destination.join(filename),
            serde_json::to_vec(&value).expect("JSON value"),
        )?;
    } else {
        fs::copy(
            path,
            destination.join(source.as_path().file_name().expect("repository file name")),
        )?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
