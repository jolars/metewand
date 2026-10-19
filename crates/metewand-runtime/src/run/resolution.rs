use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use metewand_core::{
    EXECUTION_SEMANTICS_VERSION,
    canonical::CanonicalValue,
    identity::{
        CanonicalResource, IdentityRecord, canonical_identity_bytes, identify_canonical,
        identify_record,
    },
    manifest::{
        DatasetDefinition, EnvironmentDefinition, InterpretedRunner, PrimaryTime,
        ProtocolTransport, RepositoryPath, RunOrder, WorkerDefinition,
    },
    measurements::{Measurement, MissingReason},
    planning::{
        DatasetConfigurationDefinition, LogicalCandidatePlan, LogicalPlan,
        identify_builtin_unit_dataset_definition,
    },
    records::{
        AttemptSlotRole, ContentDigest, DatasetArtifact, DatasetInstanceRecord,
        ExecutionPolicyRecord, ExecutorDefinitionRecord, IdentifiedRecord,
        LogicalAttemptSlotRecord, ProblemInstanceRecord, RecordId, ResolvedAttemptSlotRecord,
        ResolvedEnvironmentRecord, ResolvedLaunchRecord, ResolvedObservationSlotRecord,
        ResolvedOneShotSpecificationRecord, ResolvedWorkerRecord, SourceBundleArtifact,
        SourceBundleResource, WorkerWorkingDirectory,
    },
};
use metewand_protocol::{MaterializeResponse, WorkerIdentity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};

use crate::{
    LOCAL_TREE_HASH_VERSION,
    artifact_store::{ArtifactStore, StoredArtifact},
    dataset_materialization::{
        DatasetMaterialization, identify_dataset_derivation, materialize_dataset,
    },
    fixed_dataset_run::ExecutionTimingPolicy,
    local_tree::hash_local_tree,
    repository::CheckedRepository,
    source_acquisition::HttpAcquirer,
    source_bundle::identify_source_bundle,
    source_resolution::{SourceRequest, SourceResolutionError, resolve_source},
    worker_output::WorkerOutputValidator,
    worker_process::WorkerLogLimits,
    worker_session::PhaseTimeouts,
};

use super::RunError;

pub(super) struct ResolvedRun {
    pub candidates: Vec<ResolvedCandidate>,
    pub private_root: TempDir,
}

pub(super) struct ResolvedCandidate {
    pub dataset: Arc<DatasetSnapshot>,
    pub implementation: ResolvedWorker,
    pub evaluator: ResolvedWorker,
    pub measurements: Vec<ResolvedMeasurement>,
    pub contract_id: String,
    pub problem_parameters: Value,
    pub implementation_parameters: Value,
    pub result_schema: PathBuf,
    pub metric_schema: PathBuf,
    pub timeouts: PhaseTimeouts,
    pub timing: ExecutionTimingPolicy,
    pub reuse_key: crate::worker_reuse::WorkerReuseKey,
}

#[derive(Clone)]
pub(super) struct ResolvedMeasurement {
    pub warmups: Vec<(u64, RecordId<ResolvedAttemptSlotRecord>)>,
    pub logical_slot: RecordId<LogicalAttemptSlotRecord>,
    pub attempt_slot: IdentifiedRecord<ResolvedAttemptSlotRecord>,
    pub observation_slot: IdentifiedRecord<ResolvedObservationSlotRecord>,
    pub seed: u64,
    pub provenance: CanonicalValue,
}

pub(super) struct ResolvedWorker {
    pub launch: ResolvedLaunchRecord,
    pub identity: WorkerIdentity,
    record: ResolvedWorkerRecord,
    sources: Option<Snapshot>,
    executable: PathBuf,
    executable_digest: String,
    provenance: Value,
}

pub(super) struct Snapshot {
    pub directory: TempDir,
    digest: ContentDigest,
    original: Option<(PathBuf, Vec<RepositoryPath>, RecordId<SourceBundleResource>)>,
}

pub(super) struct DatasetSnapshot {
    pub materialization_time_ns: Measurement,
    pub artifact: StoredArtifact,
    original: Option<Snapshot>,
}

impl DatasetSnapshot {
    fn verify(&self) -> Result<(), RunError> {
        self.artifact.verify()?;
        if let Some(original) = &self.original {
            original.verify()?;
        }
        Ok(())
    }
}

enum LaunchArgument {
    Literal(String),
    Source(PathBuf),
}

fn launch_argument(argument: &str) -> Result<LaunchArgument, RunError> {
    if let Some(relative) = argument.strip_prefix("{sources}/") {
        let path: RepositoryPath = serde_json::from_value(json!(relative)).map_err(|_| {
            unsupported("`{sources}/` arguments require a normalized repository-relative path")
        })?;
        Ok(LaunchArgument::Source(path.as_path().to_path_buf()))
    } else if argument.contains("{sources}") {
        Err(unsupported(
            "pass a source reference as a separate `{sources}/PATH` argument",
        ))
    } else {
        Ok(LaunchArgument::Literal(argument.to_owned()))
    }
}

impl Snapshot {
    pub fn verify(&self) -> Result<(), RunError> {
        if hash_local_tree(self.directory.path())? != self.digest {
            return Err(RunError::ChangedInput);
        }
        if let Some((root, patterns, expected)) = &self.original
            && identify_source_bundle(root, patterns)? != *expected
        {
            return Err(RunError::ChangedInput);
        }
        Ok(())
    }

    fn artifact<T: CanonicalResource>(&self) -> Result<RecordId<T>, RunError> {
        Ok(identify_canonical(&canonical(json!({
            "tree_hash_version": LOCAL_TREE_HASH_VERSION,
            "tree_sha256": hex(self.digest.bytes()),
        }))?)?)
    }
}

impl ResolvedWorker {
    pub fn verify(&self) -> Result<(), RunError> {
        if let Some(sources) = &self.sources {
            sources.verify()?;
        }
        if file_digest(&self.executable)? != self.executable_digest {
            return Err(RunError::ChangedInput);
        }
        Ok(())
    }
}

impl ResolvedCandidate {
    pub fn verify(&self) -> Result<(), RunError> {
        self.dataset.verify()?;
        self.implementation.verify()?;
        self.evaluator.verify()
    }

    pub fn validate_destination(&self, output: &Path) -> Result<(), RunError> {
        for snapshot in self
            .dataset
            .original
            .iter()
            .chain(self.implementation.sources.iter())
            .chain(self.evaluator.sources.iter())
        {
            validate_snapshot_destination(snapshot, output)?;
        }
        Ok(())
    }
}

fn validate_snapshot_destination(snapshot: &Snapshot, output: &Path) -> Result<(), RunError> {
    if let Some((root, patterns, _)) = &snapshot.original {
        for pattern in patterns {
            let mut prefix = PathBuf::new();
            let mut has_glob = false;
            for component in pattern.as_path().components() {
                if component
                    .as_os_str()
                    .to_string_lossy()
                    .contains(['*', '?', '[', '{'])
                {
                    has_glob = true;
                    break;
                }
                prefix.push(component);
            }
            let prefix = root.join(prefix);
            if (has_glob || prefix.is_dir()) && output.starts_with(prefix) {
                return Err(unsupported(
                    "choose an output directory outside declared source directories and glob prefixes",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn resolve(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    output: &Path,
    cache_directory: Option<&Path>,
) -> Result<ResolvedRun, RunError> {
    for experiment in &plan.experiments {
        let policy = &experiment.execution_policy.record;
        if policy.primary_time != PrimaryTime::TimedWallTime
            || policy.run_order != RunOrder::Sequential
        {
            return Err(unsupported(format!(
                "execution policy `{}` requires timing or scheduling outside the trusted local path; use wall timing and sequential workers",
                policy.name,
            )));
        }
        scientific_timeout(policy.timeout.as_deref())?;
    }
    if plan
        .experiments
        .iter()
        .all(|experiment| experiment.candidates.is_empty())
    {
        return Err(unsupported(
            "`run` requires at least one measured attempt slot",
        ));
    }

    let mut logical_slots = BTreeSet::new();
    for experiment in &plan.experiments {
        for candidate in &experiment.candidates {
            for specification in &candidate.specifications {
                for slot in &specification.attempt_slots {
                    if !logical_slots.insert(slot.id) {
                        return Err(unsupported(
                            "the plan selects a resolved attempt slot more than once; use distinct experiment seeds for independent executions",
                        ));
                    }
                }
            }
        }
    }
    let private_root = Builder::new().prefix("metewand-run-").tempdir()?;
    let executor = identify_record(ExecutorDefinitionRecord {
        kind: serde_json::from_value(json!("trusted_local")).expect("static valid name"),
        configuration: canonical(
            json!({"environment_class": "development", "isolation": "unsupported",
                "worker_cache": {"policy": "clear_all_between_slots"},
                "timeout_termination": {"interrupt": "SIGINT", "grace_ms": 100, "force": "SIGKILL", "exit_wait_ms": 1000}}),
        )?,
    })?;
    // Resolve every launch and fixed dataset before any acquisition or worker runs.
    let mut prepared = Vec::new();
    for experiment in &plan.experiments {
        for candidate in &experiment.candidates {
            let implementation_name = repository
                .planning_catalog
                .implementation_definitions
                .iter()
                .find(|(_, id)| **id == candidate.implementation_configuration.record.definition)
                .expect("checked implementation")
                .0;
            let problem_name = repository
                .planning_catalog
                .problem_definitions
                .iter()
                .find(|(_, id)| **id == candidate.problem_configuration.record.definition)
                .expect("checked problem")
                .0;
            let implementation = resolve_worker(
                repository,
                &repository.manifest.implementations[implementation_name].worker,
                private_root.path(),
            )?;
            let evaluator = resolve_worker(
                repository,
                &repository.manifest.problems[problem_name].evaluator,
                private_root.path(),
            )?;
            let dataset = prepare_dataset(repository, candidate, private_root.path())?;
            for sources in implementation
                .sources
                .iter()
                .chain(evaluator.sources.iter())
            {
                validate_snapshot_destination(sources, output)?;
            }
            match &dataset {
                DatasetPlan::Ready(snapshot, _) => validate_snapshot_destination(snapshot, output)?,
                DatasetPlan::Generated(_, materializer) => {
                    if let Some(sources) = &materializer.sources {
                        validate_snapshot_destination(sources, output)?;
                    }
                }
            }

            prepared.push((
                candidate,
                experiment.execution_policy.record.clone(),
                implementation,
                evaluator,
                dataset,
            ));
        }
    }
    let temporary_cache = private_root.path().join("artifacts");
    let cache_directory = cache_directory.unwrap_or(&temporary_cache);
    if cache_directory.starts_with(output) || output.starts_with(cache_directory) {
        return Err(unsupported(
            "artifact cache and run output must be separate directories",
        ));
    }
    for (_, _, implementation, evaluator, dataset) in &prepared {
        for snapshot in implementation
            .sources
            .iter()
            .chain(evaluator.sources.iter())
        {
            validate_snapshot_destination(snapshot, cache_directory)?;
        }
        match dataset {
            DatasetPlan::Ready(snapshot, _) => {
                validate_snapshot_destination(snapshot, cache_directory)?
            }
            DatasetPlan::Generated(_, materializer) => {
                if let Some(snapshot) = &materializer.sources {
                    validate_snapshot_destination(snapshot, cache_directory)?;
                }
            }
        }
    }
    let store = ArtifactStore::open(cache_directory)?;
    let mut datasets = BTreeMap::new();
    let mut candidates = Vec::new();
    for (candidate, policy, implementation, evaluator, dataset_plan) in prepared {
        let timeouts = PhaseTimeouts::default();
        let key = candidate.dataset_configuration.id;
        if let std::collections::btree_map::Entry::Vacant(entry) = datasets.entry(key) {
            let (snapshot, output_schema, provenance) = match dataset_plan {
                DatasetPlan::Ready(snapshot, schema) => {
                    snapshot.verify()?;
                    let artifact = store.import_tree(snapshot.directory.path(), snapshot.digest)?;
                    (
                        Arc::new(DatasetSnapshot {
                            materialization_time_ns: Measurement::unavailable(
                                MissingReason::ProvidedDataset,
                            ),
                            artifact,
                            original: Some(snapshot),
                        }),
                        schema,
                        None,
                    )
                }
                DatasetPlan::Generated(definition, materializer) => {
                    materializer.verify()?;
                    let source = definition
                        .source
                        .as_ref()
                        .map(|source| {
                            let backend =
                                HttpAcquirer::new(timeouts.materialize, 1024 * 1024 * 1024)?;
                            resolve_source(
                                &SourceRequest::PinnedUrl(source),
                                Some(&backend),
                                private_root.path(),
                            )
                        })
                        .transpose()?;
                    let derivation = identify_dataset_derivation(
                        &candidate.dataset_configuration.record,
                        source.as_ref(),
                        &materializer.record,
                    )?;
                    let (artifact, provenance, materialization_time_ns) =
                        if let Some(cached) = store.lookup_dataset(&derivation)? {
                            WorkerOutputValidator::new(&repository.schemas)?
                                .validate_materialized_dataset(
                                    cached.artifact.directory(),
                                    &MaterializeResponse::new(
                                        "cache".parse().expect("static request ID"),
                                        cached.manifest.as_path().to_str().expect("UTF-8 manifest"),
                                    ),
                                    definition.output_schema.as_path(),
                                )?;
                            (
                                cached.artifact,
                                cached.provenance,
                                Measurement::unavailable(MissingReason::CacheHit),
                            )
                        } else {
                            let materialized = materialize_dataset(DatasetMaterialization {
                                configuration: &candidate.dataset_configuration.record,
                                source: source.as_ref(),
                                launch: &materializer.launch,
                                worker: &materializer.record,
                                output_schema: definition.output_schema.as_path(),
                                schemas: &repository.schemas,
                                validator: None,
                                private_root: private_root.path(),
                                timeouts,
                                log_limits: WorkerLogLimits::new(64 * 1024, 64 * 1024),
                            })?;
                            let artifact = store.publish_dataset(&materialized)?;
                            (
                                artifact,
                                materialized.provenance.clone(),
                                materialized.materialization_time_ns,
                            )
                        };
                    materializer.verify()?;
                    (
                        Arc::new(DatasetSnapshot {
                            materialization_time_ns,
                            artifact,
                            original: None,
                        }),
                        repository.resources.schemas[definition.output_schema.as_path()],
                        Some(provenance),
                    )
                }
            };
            entry.insert(ResolvedDataset {
                snapshot,
                output_schema,
                provenance,
            });
        }
        candidates.push(resolve_candidate(
            repository,
            candidate,
            policy,
            executor.id,
            implementation,
            evaluator,
            &datasets[&key],
        )?);
    }
    let mut slots = BTreeSet::new();
    for candidate in &candidates {
        for measurement in &candidate.measurements {
            if !slots.insert(measurement.attempt_slot.id) {
                return Err(unsupported(
                    "the plan selects a resolved attempt slot more than once; use distinct experiment seeds for independent executions",
                ));
            }
        }
    }
    Ok(ResolvedRun {
        candidates,
        private_root,
    })
}

enum DatasetPlan<'a> {
    Ready(Snapshot, RecordId<metewand_core::records::SchemaResource>),
    Generated(
        &'a metewand_core::manifest::GeneratedDatasetDefinition,
        Box<ResolvedWorker>,
    ),
}

fn prepare_dataset<'a>(
    repository: &'a CheckedRepository,
    candidate: &LogicalCandidatePlan,
    private_root: &Path,
) -> Result<DatasetPlan<'a>, RunError> {
    let catalog = &repository.planning_catalog;
    let dataset_definition = catalog
        .dataset_definitions
        .iter()
        .find(|(_, id)| **id == candidate.dataset_configuration.record.definition)
        .expect("checked plan dataset definition")
        .0;
    match dataset_definition {
        DatasetConfigurationDefinition::Unit => {
            let directory = Builder::new().prefix("unit-").tempdir_in(private_root)?;
            let digest = hash_local_tree(directory.path())?;
            Ok(DatasetPlan::Ready(
                Snapshot {
                    directory,
                    digest,
                    original: None,
                },
                identify_builtin_unit_dataset_definition()?
                    .record
                    .output_schema,
            ))
        }
        DatasetConfigurationDefinition::Named(name) => {
            let definition = match &repository.manifest.datasets[name] {
                DatasetDefinition::Fixed(definition) => definition,
                DatasetDefinition::Generated(definition) => {
                    if let Some(source) = &definition.source {
                        let url = reqwest::Url::parse(&source.url)
                            .map_err(|_| unsupported("invalid materializer source URL"))?;
                        if !matches!(url.scheme(), "http" | "https")
                            || !url.username().is_empty()
                            || url.password().is_some()
                        {
                            return Err(unsupported(
                                "materializer URLs require HTTP(S) without credentials",
                            ));
                        }
                    }
                    let worker = resolve_worker(repository, &definition.worker, private_root)?;
                    return Ok(DatasetPlan::Generated(definition, Box::new(worker)));
                }
            };
            let mut snapshot = snapshot(repository, &definition.sources, private_root)?;
            let manifest_path = snapshot.directory.path().join("dataset-manifest.json");
            if manifest_path.exists() {
                return Err(unsupported(
                    "fixed data reserves `dataset-manifest.json` for its generated inventory",
                ));
            }
            let files = file_inventory(snapshot.directory.path())?;
            fs::write(
                &manifest_path,
                canonical(json!({"schema_version": 1, "complete": true, "files": files}))?
                    .to_canonical_bytes()?,
            )?;
            WorkerOutputValidator::new(&repository.schemas)?.validate_materialized_dataset(
                snapshot.directory.path(),
                &MaterializeResponse::new(
                    "fixed".parse().expect("static request ID"),
                    "dataset-manifest.json",
                ),
                definition.output_schema.as_path(),
            )?;
            snapshot.digest = hash_local_tree(snapshot.directory.path())?;
            Ok(DatasetPlan::Ready(
                snapshot,
                repository.resources.schemas[definition.output_schema.as_path()],
            ))
        }
    }
}

struct ResolvedDataset {
    snapshot: Arc<DatasetSnapshot>,
    output_schema: RecordId<metewand_core::records::SchemaResource>,
    provenance: Option<CanonicalValue>,
}

fn resolve_candidate(
    repository: &CheckedRepository,
    candidate: &LogicalCandidatePlan,
    policy: ExecutionPolicyRecord,
    executor: RecordId<ExecutorDefinitionRecord>,
    implementation: ResolvedWorker,
    evaluator: ResolvedWorker,
    dataset: &ResolvedDataset,
) -> Result<ResolvedCandidate, RunError> {
    let timing = ExecutionTimingPolicy {
        scope: policy.timing_scope,
        timeout: scientific_timeout(policy.timeout.as_deref())?,
    };
    let output_schema = dataset.output_schema;
    let dataset_provenance = dataset.provenance.as_ref();
    let dataset = Arc::clone(&dataset.snapshot);
    let catalog = &repository.planning_catalog;
    let dataset_instance = identify_record(DatasetInstanceRecord {
        configuration: candidate.dataset_configuration.id,
        artifact: identify_canonical::<DatasetArtifact>(&canonical(json!({
            "tree_hash_version": LOCAL_TREE_HASH_VERSION,
            "tree_sha256": hex(dataset.artifact.digest().bytes()),
        }))?)?,
        output_schema,
    })?;
    let problem_instance = identify_record(ProblemInstanceRecord {
        dataset: dataset_instance.id,
        configuration: candidate.problem_configuration.id,
    })?;
    let problem_name = catalog
        .problem_definitions
        .iter()
        .find(|(_, id)| **id == candidate.problem_configuration.record.definition)
        .expect("checked plan problem definition")
        .0;
    let contract = &repository.problem_contracts[problem_name];
    let mut measurements = Vec::new();
    for specification in &candidate.specifications {
        let resolved = identify_record(ResolvedOneShotSpecificationRecord {
            logical_specification: specification.specification.id,
            problem_instance: problem_instance.id,
            implementation: implementation.record.clone(),
            evaluator: evaluator.record.clone(),
            executor,
            wire_protocol_version: metewand_protocol::WIRE_PROTOCOL_VERSION,
            execution_semantics_version: EXECUTION_SEMANTICS_VERSION,
        })?;
        let warmups = specification
            .attempt_slots
            .iter()
            .filter_map(|slot| {
                if let AttemptSlotRole::Warmup { warmup_index } = slot.record.role {
                    Some(
                        identify_record(ResolvedAttemptSlotRecord {
                            logical_slot: slot.id,
                            specification: resolved.id,
                        })
                        .map(|resolved| (warmup_index, resolved.id)),
                    )
                } else {
                    None
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        for slot in &specification.attempt_slots {
            let AttemptSlotRole::Measured { observation_slot } = slot.record.role else {
                continue;
            };
            let attempt_slot = identify_record(ResolvedAttemptSlotRecord {
                logical_slot: slot.id,
                specification: resolved.id,
            })?;
            let observation_slot = identify_record(ResolvedObservationSlotRecord {
                logical_slot: observation_slot,
                specification: resolved.id,
            })?;
            let provenance = canonical(json!({
                "controls": metewand_core::controls::local_control_report(&policy),
                "manifest_hash": repository.manifest_hash.to_string(),
                "timing_scope": timing.scope.to_string(),
                "operation_scope": timing.scope == metewand_core::manifest::TimingScope::ExecuteOnly,
                "scientific_timeout_ns": timing.timeout.map(|limit| limit.as_nanos().to_string()),
                "operational_timeouts_ns": PhaseTimeouts::default().provenance(),
                "logical_candidate_id": candidate.candidate.id,
                "logical_slot_id": slot.id,
                "dataset_materialization": dataset_provenance.map(CanonicalValue::as_json),
                "dataset_immutability": {
                    "view": "copy", "enforcement": "best_effort",
                    "verification": "each_worker_operation",
                },
                "dataset_instance": record_document(&dataset_instance.record)?,
                "problem_instance": record_document(&problem_instance.record)?,
                "resolved_specification": record_document(&resolved.record)?,
                "resolved_attempt_slot": record_document(&attempt_slot.record)?,
                "resolved_observation_slot": record_document(&observation_slot.record)?,
                "implementation": implementation.provenance,
                "evaluator": evaluator.provenance,
                "repository_commit": null, "dirty_tree_hash": null, "build_fingerprint": null,
                "cpu_model": null, "selected_cores": null, "memory": null, "kernel": null,
                "language_runtime": null, "blas": null, "thread_settings": null,
            }))?;
            measurements.push(ResolvedMeasurement {
                warmups: warmups.clone(),
                logical_slot: slot.id,
                attempt_slot,
                observation_slot,
                seed: specification.specification.record.implementation_seed.value,
                provenance,
            });
        }
    }
    let reuse_key = crate::worker_reuse::WorkerReuseKey {
        role: metewand_protocol::WorkerRole::Implementation,
        definition: candidate.implementation_configuration.record.definition,
        resolved: implementation.record.clone(),
        policy,
    };
    Ok(ResolvedCandidate {
        reuse_key,
        dataset,
        implementation,
        evaluator,
        measurements,
        contract_id: repository.resources.problem_contracts[problem_name].to_string(),
        problem_parameters: candidate
            .problem_configuration
            .record
            .parameters
            .as_json()
            .clone(),
        implementation_parameters: candidate
            .implementation_configuration
            .record
            .parameters
            .as_json()
            .clone(),
        result_schema: contract.result_schema.as_path().to_path_buf(),
        metric_schema: contract.metric_schema.as_path().to_path_buf(),
        timeouts: PhaseTimeouts::default(),
        timing,
    })
}

fn resolve_worker(
    repository: &CheckedRepository,
    worker: &WorkerDefinition,
    private_root: &Path,
) -> Result<ResolvedWorker, RunError> {
    let (program, arguments, patterns, environment, transport) = match worker {
        WorkerDefinition::Command(worker) => (
            worker.program.clone(),
            worker
                .args
                .iter()
                .map(|argument| launch_argument(argument))
                .collect::<Result<Vec<_>, _>>()?,
            worker.sources.as_deref(),
            &worker.environment,
            worker.protocol_transport,
        ),
        WorkerDefinition::Interpreted(worker) => {
            let program = match worker.runner {
                InterpretedRunner::Python => "python3",
                InterpretedRunner::R => "Rscript",
                InterpretedRunner::Julia => "julia",
            };
            let mut args = vec![LaunchArgument::Source(
                worker.entrypoint.as_path().to_path_buf(),
            )];
            args.extend(
                worker
                    .args
                    .iter()
                    .map(|argument| launch_argument(argument))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            (
                program.to_owned(),
                args,
                Some(worker.sources.as_slice()),
                &worker.environment,
                worker.protocol_transport,
            )
        }
    };
    if !matches!(
        repository.manifest.environments[environment],
        EnvironmentDefinition::Local {}
    ) {
        return Err(unsupported(format!(
            "environment `{environment}` requires provisioning; Gate 1 supports `kind = \"local\"`"
        )));
    }
    if transport == Some(ProtocolTransport::Stdio) {
        return Err(unsupported(
            "Gate 1 workers require dedicated POSIX protocol pipes",
        ));
    }
    let sources = patterns
        .map(|patterns| snapshot(repository, patterns, private_root))
        .transpose()?;
    let original_executable = find_program(&repository.root, &program)?;
    let mut arguments = arguments;
    let mut executable = original_executable.clone();
    if let Ok(relative) = original_executable.strip_prefix(&repository.root) {
        let sources = sources.as_ref().ok_or_else(|| {
            unsupported("repository command programs must be included in `sources`")
        })?;
        executable = sources.directory.path().join(relative);
        if !executable.is_file() {
            return Err(unsupported(
                "the repository command program is absent from its declared `sources`",
            ));
        }
        let bytes = fs::read(&executable)?;
        if bytes.starts_with(b"#!") {
            let line = std::str::from_utf8(
                bytes
                    .split(|byte| *byte == b'\n')
                    .next()
                    .expect("nonempty shebang"),
            )
            .map_err(|_| unsupported("worker shebang must be UTF-8"))?;
            let parts = line[2..].split_whitespace().collect::<Vec<_>>();
            let interpreter = match parts.as_slice() {
                ["/usr/bin/env", name] => *name,
                [name] => *name,
                _ => {
                    return Err(unsupported(
                        "use a single interpreter in a command worker's shebang, or declare an explicit interpreter program and arguments",
                    ));
                }
            };
            executable = find_program(&repository.root, interpreter)?;
            if executable.starts_with(&repository.root) {
                return Err(unsupported(
                    "a shebang interpreter must come from the local environment",
                ));
            }
            arguments.insert(0, LaunchArgument::Source(relative.to_path_buf()));
        }
    }
    let executable_digest = file_digest(&executable)?;
    let environment_record = identify_record(ResolvedEnvironmentRecord {
        definition: repository.planning_catalog.environment_definitions[environment],
        fingerprint: canonical(
            json!({"class": "development", "os": env::consts::OS, "architecture": env::consts::ARCH, "program_sha256": executable_digest, "dependency_closure": null}),
        )?,
    })?;
    let mut normalized_arguments = Vec::new();
    let mut actual_arguments = Vec::new();
    for argument in arguments {
        if let LaunchArgument::Source(relative) = argument {
            let staged = sources
                .as_ref()
                .map(|sources| sources.directory.path().join(&relative))
                .filter(|path| path.exists())
                .ok_or_else(|| {
                    unsupported(format!(
                        "worker source argument `{}` is absent from its declared `sources`",
                        relative.display()
                    ))
                })?;
            normalized_arguments.push(format!("{{sources}}/{}", text_path(&relative)?));
            actual_arguments.push(text_path(&staged)?);
        } else if let LaunchArgument::Literal(argument) = argument {
            if argument.contains(&text_path(&repository.root)?)
                || argument.contains(&text_path(private_root)?)
            {
                return Err(unsupported(
                    "worker arguments may not embed workspace or temporary paths; pass declared source paths as separate arguments",
                ));
            }
            normalized_arguments.push(argument.clone());
            actual_arguments.push(argument);
        }
    }
    let source_executable = sources
        .as_ref()
        .is_some_and(|sources| executable.starts_with(sources.directory.path()));
    let mut variables = crate::worker_environment::worker_environment_template();
    variables.insert(
        "PATH".to_owned(),
        if source_executable {
            "/usr/bin:/bin"
        } else {
            "{program_directory}:/usr/bin:/bin"
        }
        .to_owned(),
    );
    let normalized_program = if let Some(sources) = &sources
        && let Ok(relative) = executable.strip_prefix(sources.directory.path())
    {
        format!("{{sources}}/{}", text_path(relative)?)
    } else {
        format!("sha256:{executable_digest}")
    };
    let normalized_launch = identify_record(ResolvedLaunchRecord {
        environment: environment_record.id,
        program: normalized_program,
        args: normalized_arguments,
        environment_variables: variables.clone(),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    })?;
    let record = ResolvedWorkerRecord {
        source_bundle: sources
            .as_ref()
            .map(Snapshot::artifact::<SourceBundleArtifact>)
            .transpose()?,
        environment: environment_record.id,
        launch: normalized_launch.id,
    };
    let identity = identify_record(record.clone())?
        .id
        .to_string()
        .parse()
        .expect("typed resolved worker ID");
    let program_directory = if source_executable {
        "/usr/bin:/bin".to_owned()
    } else {
        format!(
            "{}:/usr/bin:/bin",
            text_path(
                executable
                    .parent()
                    .expect("absolute executable has a parent")
            )?
        )
    };
    variables.insert("PATH".to_owned(), program_directory);
    let launch = ResolvedLaunchRecord {
        environment: environment_record.id,
        program: text_path(&executable)?,
        args: actual_arguments,
        environment_variables: variables,
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let provenance = json!({"environment": record_document(&environment_record.record)?, "launch": record_document(&normalized_launch.record)?, "worker": record_document(&record)?, "undeclared_file_access": "unsupported"});
    Ok(ResolvedWorker {
        launch,
        identity,
        record,
        sources,
        executable,
        executable_digest,
        provenance,
    })
}

fn snapshot(
    repository: &CheckedRepository,
    patterns: &[RepositoryPath],
    private_root: &Path,
) -> Result<Snapshot, RunError> {
    let mut key = patterns
        .iter()
        .map(|path| path.as_path().to_path_buf())
        .collect::<Vec<_>>();
    key.sort();
    key.dedup();
    let expected = repository.resources.source_bundles[&key];
    let source = resolve_source(
        &SourceRequest::Repository {
            root: &repository.root,
            sources: patterns,
            expected,
        },
        None,
        private_root,
    )
    .map_err(|error| match error {
        SourceResolutionError::ChangedSource => RunError::ChangedInput,
        SourceResolutionError::UnsupportedEntry { .. } => unsupported(
            "Gate 1 input snapshots require regular files and directories; symbolic links and special files are not supported",
        ),
        SourceResolutionError::Io(error) => RunError::Io(error),
        error => RunError::SourceResolution(error),
    })?;
    let digest = source.tree_digest();
    let directory = source.into_directory();
    Ok(Snapshot {
        directory,
        digest,
        original: Some((repository.root.clone(), patterns.to_vec(), expected)),
    })
}

fn file_inventory(root: &Path) -> Result<Vec<Value>, RunError> {
    fn visit(
        root: &Path,
        path: &Path,
        files: &mut BTreeMap<PathBuf, Value>,
    ) -> Result<(), RunError> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                visit(root, &path, files)?;
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("inventory stays beneath root")
                    .to_path_buf();
                files.insert(relative.clone(), json!({"path": relative, "sha256": file_digest(&path)?, "size_bytes": entry.metadata()?.len(), "media_type": if path.extension().is_some_and(|extension| extension == "json") { "application/json" } else { "application/octet-stream" }}));
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files.into_values().collect())
}

fn find_program(root: &Path, program: &str) -> Result<PathBuf, RunError> {
    let path = if Path::new(program).is_absolute() {
        PathBuf::from(program)
    } else if Path::new(program).components().count() > 1 {
        root.join(program)
    } else if Path::new(program).components().count() == 1 {
        env::var_os("PATH")
            .and_then(|paths| {
                env::split_paths(&paths)
                    .map(|path| path.join(program))
                    .find(|path| {
                        path.is_file()
                            && fs::metadata(path)
                                .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
                    })
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("cannot resolve local executable `{program}`"),
                )
            })?
    } else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cannot resolve repository executable `{program}`"),
        )
        .into());
    };
    let path = fs::canonicalize(path)?;
    if !Path::new(program).is_absolute()
        && Path::new(program).components().count() > 1
        && !path.starts_with(root)
    {
        return Err(unsupported(
            "repository command programs must remain inside the repository; use an absolute path for an external executable",
        ));
    }
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("program `{program}` is not an executable regular file"),
        )
        .into());
    }
    Ok(path)
}

pub(super) fn canonical(value: Value) -> Result<CanonicalValue, RunError> {
    Ok(CanonicalValue::try_from(value)?)
}

fn record_document<T: IdentityRecord>(record: &T) -> Result<Value, RunError> {
    Ok(
        serde_json::from_slice(&canonical_identity_bytes(record)?)
            .expect("identity bytes are JSON"),
    )
}

fn file_digest(path: &Path) -> Result<String, io::Error> {
    Ok(hex(&Sha256::digest(fs::read(path)?)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn text_path(path: &Path) -> Result<String, RunError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| unsupported("Gate 1 launch paths must be UTF-8"))
}

fn scientific_timeout(expression: Option<&str>) -> Result<Option<Duration>, RunError> {
    let Some(expression) = expression else {
        return Ok(None);
    };
    let (number, multiplier) = [("ms", 1), ("s", 1000), ("min", 60_000), ("h", 3_600_000)]
        .into_iter().find_map(|(suffix, multiplier)| expression.trim().strip_suffix(suffix)
            .map(|number| (number.trim(), multiplier)))
        .ok_or_else(|| unsupported("scientific timeouts require a positive integer followed by `ms`, `s`, `min`, or `h`"))?;
    let milliseconds = number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .filter(|number| *number > 0)
        .ok_or_else(|| unsupported("scientific timeout is zero, invalid, or too large"))?;
    let duration = Duration::from_millis(milliseconds);
    if std::time::Instant::now().checked_add(duration).is_none() {
        return Err(unsupported(
            "scientific timeout cannot be represented by the monotonic clock",
        ));
    }
    Ok(Some(duration))
}

fn unsupported(message: impl Into<String>) -> RunError {
    RunError::Unsupported(message.into())
}

#[cfg(test)]
mod tests {
    use super::scientific_timeout;
    use std::time::Duration;

    #[test]
    fn scientific_limits_have_no_implicit_budget_and_parse_checked_units() {
        assert_eq!(scientific_timeout(None).unwrap(), None);
        for (text, milliseconds) in [
            ("1ms", 1),
            (" 2 s ", 2000),
            ("3 min", 180_000),
            ("4h", 14_400_000),
        ] {
            assert_eq!(
                scientific_timeout(Some(text)).unwrap(),
                Some(Duration::from_millis(milliseconds))
            );
        }
        for text in [
            "",
            "0 s",
            "-1 s",
            "1.5s",
            "1",
            "1 day",
            "18446744073709551615 h",
        ] {
            assert!(scientific_timeout(Some(text)).is_err(), "{text}");
        }
    }
}
