use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::core::{
    EXECUTION_SEMANTICS_VERSION, SCHEDULING_POLICY_VERSION,
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
        DatasetConfigurationDefinition, LogicalCandidatePlan, LogicalExperimentPlan, LogicalPlan,
        identify_builtin_unit_dataset_definition,
    },
    records::{
        AttemptSlotRole, ContentDigest, DatasetArtifact, DatasetInstanceRecord,
        ExecutorDefinitionRecord, IdentifiedRecord, LogicalAttemptSlotRecord,
        ProblemInstanceRecord, RecordId, ResolvedAttemptSlotRecord, ResolvedEnvironmentRecord,
        ResolvedLaunchRecord, ResolvedObservationSlotRecord, ResolvedOneShotSpecificationRecord,
        ResolvedWorkerRecord, SourceBundleArtifact, SourceBundleResource, WorkerWorkingDirectory,
    },
    scheduling::{
        ComparisonBlock, SchedulingSlot, derive_block_scheduling_priority, schedule_slots,
    },
};
use crate::protocol::{MaterializeResponse, WorkerIdentity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};

use crate::runtime::{
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
    pub schedule: Vec<(usize, usize)>,
    pub batches: Vec<ResolvedBatch>,
    pub private_root: TempDir,
}

pub(super) struct ResolvedBatch {
    pub slots: std::ops::Range<usize>,
    pub lane_count: usize,
}

pub(super) struct ResolvedCandidate {
    pub profile: Option<crate::core::profile::SelectedBufferedProfile>,
    pub streaming_profile: Option<crate::core::profile::SelectedStreamingProfile>,
    pub fresh_profile: Option<crate::core::profile::ProfilePolicy>,
    pub experiment: String,
    pub logical_selection_ids: Vec<String>,
    pub applicability: IdentifiedRecord<crate::core::applicability::ApplicabilityDecisionRecord>,
    pub dataset: Arc<DatasetSnapshot>,
    pub implementation: ResolvedWorker,
    pub evaluator: ResolvedWorker,
    pub measurements: Vec<ResolvedMeasurement>,
    pub contract_id: String,
    pub problem_parameters: Value,
    pub problem_semantics: Value,
    pub implementation_parameters: Value,
    pub result_schema: PathBuf,
    pub metric_schema: PathBuf,
    pub timeouts: PhaseTimeouts,
    pub timing: ExecutionTimingPolicy,
    pub reuse_key: crate::runtime::worker_reuse::WorkerReuseKey,
}

#[derive(Clone)]
pub(super) struct ResolvedMeasurement {
    pub fresh: Option<crate::core::profile::FreshControl>,
    pub observation_slots: Vec<IdentifiedRecord<ResolvedObservationSlotRecord>>,
    pub candidate: RecordId<crate::core::records::LogicalCandidateRecord>,
    pub specification: IdentifiedRecord<ResolvedOneShotSpecificationRecord>,
    pub selection_ids: Vec<String>,
    pub scheduling: SchedulingSlot,
    pub warmups: Vec<(u64, RecordId<ResolvedAttemptSlotRecord>)>,
    pub logical_slot: RecordId<LogicalAttemptSlotRecord>,
    pub attempt_slot: IdentifiedRecord<ResolvedAttemptSlotRecord>,
    pub observation_slot: IdentifiedRecord<ResolvedObservationSlotRecord>,
    pub seed: u64,
    pub provenance: CanonicalValue,
}

pub(crate) struct ResolvedWorker {
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
    manifest: Option<String>,
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
    pub(crate) fn lock_record(
        &self,
        environment: &crate::core::manifest::Name,
    ) -> crate::core::lockfile::LockedWorker {
        crate::core::lockfile::LockedWorker {
            environment: environment.to_string(),
            program: self.provenance["locked_program"]["program"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| self.executable.to_string_lossy().into_owned()),
            executable_sha256: self.provenance["locked_program"]["sha256"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| self.executable_digest.clone()),
            launch: self.provenance["launch"].clone(),
        }
    }

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
    pub(super) fn environment_class(&self) -> &str {
        self.implementation.provenance["environment"]["fingerprint"]["class"]
            .as_str()
            .expect("resolved environment class")
    }

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
    locked: Option<&crate::runtime::lockfile::VerifiedLock>,
) -> Result<ResolvedRun, RunError> {
    for experiment in &plan.experiments {
        let policy = &experiment.execution_policy.record;
        if policy.max_concurrency == 0
            || policy.max_concurrency > crate::core::canonical::MAX_SAFE_INTEGER
        {
            return Err(unsupported(
                "max_concurrency must be a positive safe integer",
            ));
        }
        if policy.primary_time != PrimaryTime::TimedWallTime {
            return Err(unsupported(format!(
                "execution policy `{}` requires timing outside the trusted local path; use wall timing",
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
        configuration: canonical({
            let mut configuration = executor_configuration();
            if locked.is_some() {
                configuration["environment_class"] = json!("locked");
            }
            configuration
        })?,
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
            let implementation = resolve_worker_with_lock(
                repository,
                &repository.manifest.implementations[implementation_name].worker,
                private_root.path(),
                locked,
            )?;
            let evaluator = resolve_worker_with_lock(
                repository,
                &repository.manifest.problems[problem_name].evaluator,
                private_root.path(),
                locked,
            )?;
            let dataset = prepare_dataset(repository, candidate, private_root.path(), locked)?;
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

            prepared.push((candidate, experiment, implementation, evaluator, dataset));
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
    for (candidate, experiment, implementation, evaluator, dataset_plan) in prepared {
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
                            manifest: snapshot
                                .directory
                                .path()
                                .join("dataset-manifest.json")
                                .exists()
                                .then(|| "dataset-manifest.json".into()),
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
                    let (artifact, provenance, materialization_time_ns, manifest) =
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
                                cached.manifest.as_path().to_string_lossy().into_owned(),
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
                                materialized
                                    .manifest
                                    .manifest_path()
                                    .to_string_lossy()
                                    .into_owned(),
                            )
                        };
                    materializer.verify()?;
                    (
                        Arc::new(DatasetSnapshot {
                            materialization_time_ns,
                            manifest: Some(manifest),
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
            experiment,
            executor.id,
            implementation,
            evaluator,
            &datasets[&key],
        )?);
    }
    let runner = crate::runtime::build_evidence::executable_evidence(&env::current_exe()?)?;
    let host = crate::runtime::build_evidence::host_observation();
    let checkout = crate::runtime::build_evidence::repository_observation(&repository.root);
    let mut slots = BTreeSet::new();
    for candidate in &mut candidates {
        for measurement in &mut candidate.measurements {
            let mut context = measurement.provenance.as_json().clone();
            context["runner"] = runner.clone();
            context["host_observation"] = host.clone();
            context["repository"] = checkout.clone();
            measurement.provenance = canonical(context)?;
            if !slots.insert(measurement.attempt_slot.id) {
                return Err(unsupported(
                    "the plan selects a resolved attempt slot more than once; use distinct experiment seeds for independent executions",
                ));
            }
        }
    }
    let mut schedule = Vec::new();
    let mut batches = Vec::new();
    let mut candidate_offset = 0;
    for experiment in &plan.experiments {
        let mut positions = Vec::new();
        let mut scheduling_slots = Vec::new();
        for (candidate_index, candidate) in candidates
            .iter()
            .enumerate()
            .skip(candidate_offset)
            .take(experiment.candidates.len())
        {
            for (measurement_index, measurement) in candidate.measurements.iter().enumerate() {
                positions.push((candidate_index, measurement_index));
                scheduling_slots.push(measurement.scheduling);
            }
        }
        let ordered = schedule_slots(
            experiment.execution_policy.record.run_order,
            &experiment.scheduling_seed,
            &scheduling_slots,
        )
        .into_iter()
        .map(|index| positions[index])
        .collect::<Vec<_>>();
        if !ordered.is_empty() {
            // Bound allocation by selected work even when the authored limit is huge.
            let max_concurrency = experiment.execution_policy.record.max_concurrency;
            let lane_count = max_concurrency.min(ordered.len() as u64) as usize;
            for (offset, &(candidate, measurement)) in ordered.iter().enumerate() {
                let batch_index = offset / lane_count;
                let batch_size = lane_count.min(ordered.len() - batch_index * lane_count);
                let provenance = &mut candidates[candidate].measurements[measurement].provenance;
                let mut value = provenance.as_json().clone();
                value["concurrency"] = json!({
                    "max_concurrency": max_concurrency,
                    "dispatch": "fixed_batches",
                    "batch_index": batch_index,
                    "batch_size": batch_size,
                    "lane_index": offset % lane_count,
                });
                *provenance = canonical(value)?;
            }
            let start = schedule.len();
            let end = start + ordered.len();
            batches.extend((start..end).step_by(lane_count).map(|start| ResolvedBatch {
                slots: start..(start + lane_count).min(end),
                lane_count,
            }));
        }
        schedule.extend(ordered);
        candidate_offset += experiment.candidates.len();
    }
    Ok(ResolvedRun {
        candidates,
        schedule,
        batches,
        private_root,
    })
}

enum DatasetPlan<'a> {
    Ready(Snapshot, RecordId<crate::core::records::SchemaResource>),
    Generated(
        &'a crate::core::manifest::GeneratedDatasetDefinition,
        Box<ResolvedWorker>,
    ),
}

fn prepare_dataset<'a>(
    repository: &'a CheckedRepository,
    candidate: &LogicalCandidatePlan,
    private_root: &Path,
    locked: Option<&crate::runtime::lockfile::VerifiedLock>,
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
                    let worker = resolve_worker_with_lock(
                        repository,
                        &definition.worker,
                        private_root,
                        locked,
                    )?;
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
    output_schema: RecordId<crate::core::records::SchemaResource>,
    provenance: Option<CanonicalValue>,
}

fn resolve_candidate(
    repository: &CheckedRepository,
    candidate: &LogicalCandidatePlan,
    experiment: &LogicalExperimentPlan,
    executor: RecordId<ExecutorDefinitionRecord>,
    implementation: ResolvedWorker,
    evaluator: ResolvedWorker,
    dataset: &ResolvedDataset,
) -> Result<ResolvedCandidate, RunError> {
    let policy = experiment.execution_policy.record.clone();
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
    let implementation_name = catalog
        .implementation_definitions
        .iter()
        .find(|(_, id)| **id == candidate.implementation_configuration.record.definition)
        .expect("checked implementation definition")
        .0;
    let dataset_name = catalog
        .dataset_definitions
        .iter()
        .find(|(_, id)| **id == candidate.dataset_configuration.record.definition)
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| "unit".to_owned());
    let comparison = json!({"experiment": experiment.name.as_str(), "dataset": dataset_name,
        "problem": problem_name.as_str(), "implementation": implementation_name.as_str(),
        "case": match &candidate.source { crate::core::planning::ConfigurationSource::Case(name) => Some(name.as_str()), _ => None },
        "parameters": {"dataset": candidate.dataset_configuration.record.parameters.as_json(),
            "problem": candidate.problem_configuration.record.parameters.as_json(),
            "implementation": candidate.implementation_configuration.record.parameters.as_json()}});
    let declaration = repository
        .manifest
        .experiments
        .iter()
        .find(|e| e.name == experiment.name)
        .expect("planned experiment declaration");
    let analysis_settings = json!({
        "benchmark": repository.manifest.name.as_str(),
        "implementation_repetitions": declaration.implementation_repetitions,
        "measurement_repetitions": declaration.measurement_repetitions,
        "observation_policy": record_document(candidate.profile_policy.as_ref()
            .map_or(&experiment.observation_policy.record, |p| &p.record))?,
    });

    let definition = repository.manifest.observation_policies
        [&experiment.observation_policy.record.name]
        .profile
        .as_ref();
    let binding = definition.map(|p| {
        let name = catalog
            .implementation_definitions
            .iter()
            .find(|(_, id)| **id == candidate.implementation_configuration.record.definition)
            .expect("checked implementation")
            .0;
        (&p.policy, &p.implementations[name])
    });
    let profile = binding.and_then(|(policy, binding)| match binding {
        crate::core::profile::ProfileBinding::Buffered(binding) => {
            Some(crate::core::profile::SelectedBufferedProfile {
                policy: policy.clone(),
                instrumented_scope: binding.instrumented_scope.clone(),
                scope: contract.instrumented_scopes[&binding.instrumented_scope].clone(),
                instrumentation: binding.instrumentation.clone(),
                check_schedule: binding.check_schedule.clone(),
            })
        }
        _ => None,
    });
    let streaming_profile = binding.and_then(|(policy, binding)| match binding {
        crate::core::profile::ProfileBinding::Streaming(binding) => {
            Some(crate::core::profile::SelectedStreamingProfile {
                policy: policy.clone(),
                check_schedule: binding.check_schedule.clone(),
            })
        }
        _ => None,
    });
    let fresh_profile = binding.and_then(|(policy, binding)| {
        matches!(binding, crate::core::profile::ProfileBinding::Fresh(_)).then(|| policy.clone())
    });
    let applicability = resolve_applicability(
        candidate,
        &implementation,
        &dataset,
        dataset_instance.id,
        repository.resources.problem_contracts[problem_name].to_string(),
        &policy,
    )?;
    let mut measurements = Vec::new();
    for specification in candidate.specifications.iter().filter(|_| {
        applicability.record.outcome
            != crate::core::applicability::ApplicabilityOutcome::NotApplicable
    }) {
        let resolved = identify_record(ResolvedOneShotSpecificationRecord {
            applicability: applicability
                .record
                .checked_inputs
                .as_ref()
                .map(|_| applicability.id),
            logical_specification: specification.specification.id,
            problem_instance: problem_instance.id,
            implementation: implementation.record.clone(),
            evaluator: evaluator.record.clone(),
            executor,
            wire_protocol_version: crate::protocol::WIRE_PROTOCOL_VERSION,
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
        for slot in specification
            .attempt_slots
            .iter()
            .filter(|slot| matches!(slot.record.role, AttemptSlotRole::Measured { .. }))
        {
            let anchor = match slot.record.role {
                AttemptSlotRole::Measured { observation_slot } => observation_slot,
                _ => unreachable!(),
            };
            let logical_observation = specification
                .observation_slots
                .iter()
                .find(|o| o.id == anchor)
                .expect("planned observation");
            let AttemptSlotRole::Measured { observation_slot } = slot.record.role else {
                unreachable!("only measured slots were selected");
            };
            let scheduling = SchedulingSlot {
                observation_round: logical_observation
                    .record
                    .fresh
                    .as_ref()
                    .map_or(0, |f| f.schedule_index),
                logical_slot: slot.id,
                comparison_block: ComparisonBlock {
                    dataset_instance: dataset_instance.id,
                    problem_configuration: candidate.problem_configuration.id,
                    implementation_repetition: specification
                        .specification
                        .record
                        .implementation_repetition,
                    measurement_index: streaming_profile.as_ref().map_or(
                        logical_observation.record.measurement_index,
                        |profile| {
                            profile.measurement_index(logical_observation.record.measurement_index)
                        },
                    ),
                },
                scheduling_priority: slot.record.scheduling_priority,
            };
            let block = &scheduling.comparison_block;
            let attempt_slot = identify_record(ResolvedAttemptSlotRecord {
                logical_slot: slot.id,
                specification: resolved.id,
            })?;
            let observation_slot = identify_record(ResolvedObservationSlotRecord {
                logical_slot: observation_slot,
                specification: resolved.id,
            })?;
            let mut provenance = canonical(json!({
                "comparison": comparison,
                "analysis": analysis_settings,
                "applicability": applicability.record,
                "controls": crate::core::controls::local_control_report(&policy),
                "execution_policy": record_document(&policy)?,
                "manifest_hash": repository.manifest_hash.to_string(),
                "timing_scope": timing.scope.to_string(),
                "operation_scope": timing.scope == crate::core::manifest::TimingScope::ExecuteOnly,
                "scientific_timeout_ns": timing.timeout.map(|limit| limit.as_nanos().to_string()),
                "operational_timeouts_ns": PhaseTimeouts::default().provenance(),
                "logical_candidate_id": candidate.candidate.id,
                "logical_slot_id": slot.id,
                "implementation_seed": specification.specification.record.implementation_seed.value,
                "scheduling": {
                    "run_order": match policy.run_order {
                        RunOrder::Sequential => "sequential",
                        RunOrder::Randomized => "randomized",
                    },
                    "policy_version": SCHEDULING_POLICY_VERSION,
                    "seed": experiment.scheduling_seed.value,
                    "seed_digest": hex(experiment.scheduling_seed.derivation_digest.bytes()),
                    "comparison_block": {
                        "dataset_instance": block.dataset_instance,
                        "problem_configuration": block.problem_configuration,
                        "implementation_repetition": block.implementation_repetition,
                        "measurement_index": block.measurement_index,
                    },
                    "block_priority": hex(derive_block_scheduling_priority(&experiment.scheduling_seed, block).bytes()),
                    "slot_priority": hex(scheduling.scheduling_priority.bytes()),
                },
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
            if let Some(streaming) = &streaming_profile {
                let mut value = provenance.as_json().clone();
                value["streaming_profile"] =
                    serde_json::to_value(streaming).expect("selected stream");
                provenance = canonical(value)?;
            }
            let bound = profile
                .as_ref()
                .map(|p| p.policy.max_observations)
                .or_else(|| {
                    streaming_profile
                        .as_ref()
                        .map(|p| p.policy.max_observations)
                })
                .unwrap_or(1);
            let observation_slots = specification
                .observation_slots
                .iter()
                .filter(|o| {
                    if fresh_profile.is_some() {
                        o.id == logical_observation.id
                    } else {
                        o.record.measurement_index >= logical_observation.record.measurement_index
                            && o.record.measurement_index
                                < logical_observation.record.measurement_index + bound
                    }
                })
                .map(|o| {
                    identify_record(ResolvedObservationSlotRecord {
                        logical_slot: o.id,
                        specification: resolved.id,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut selection_ids = vec![
                candidate.candidate.id.to_string(),
                specification.specification.id.to_string(),
                resolved.id.to_string(),
                slot.id.to_string(),
                attempt_slot.id.to_string(),
                logical_observation.id.to_string(),
                observation_slot.id.to_string(),
                if applicability.record.checked_inputs.is_some() {
                    applicability.id.to_string()
                } else {
                    format!("run-store-v1:unconditional:{}", candidate.candidate.id)
                },
            ];
            for observation in &observation_slots {
                selection_ids.push(observation.id.to_string());
                selection_ids.push(observation.record.logical_slot.to_string());
            }
            measurements.push(ResolvedMeasurement {
                fresh: logical_observation.record.fresh.clone(),
                observation_slots,
                candidate: candidate.candidate.id,
                specification: resolved.clone(),
                selection_ids,
                scheduling,
                warmups: warmups.clone(),
                logical_slot: slot.id,
                attempt_slot,
                observation_slot,
                seed: specification.specification.record.implementation_seed.value,
                provenance,
            });
        }
    }
    let reuse_key = crate::runtime::worker_reuse::WorkerReuseKey {
        selected_capabilities: vec![if fresh_profile.is_some() {
            crate::protocol::Capability::FreshSequence
        } else if streaming_profile.is_some() {
            crate::protocol::Capability::StreamingProfile
        } else if profile.is_some() {
            crate::protocol::Capability::BufferedProfile
        } else {
            crate::protocol::Capability::OneShot
        }],
        role: crate::protocol::WorkerRole::Implementation,
        definition: candidate.implementation_configuration.record.definition,
        resolved: implementation.record.clone(),
        policy,
    };
    Ok(ResolvedCandidate {
        profile,
        fresh_profile,
        streaming_profile,
        experiment: experiment.name.to_string(),
        applicability,
        logical_selection_ids: std::iter::once(candidate.candidate.id.to_string())
            .chain(candidate.specifications.iter().flat_map(|s| {
                std::iter::once(s.specification.id.to_string())
                    .chain(s.attempt_slots.iter().map(|slot| slot.id.to_string()))
                    .chain(s.observation_slots.iter().map(|slot| slot.id.to_string()))
            }))
            .collect(),
        reuse_key,
        dataset,
        implementation,
        evaluator,
        measurements,
        contract_id: repository.resources.problem_contracts[problem_name].to_string(),
        problem_semantics: contract.semantics.as_canonical().as_json().clone(),
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

fn resolve_applicability(
    candidate: &LogicalCandidatePlan,
    implementation: &ResolvedWorker,
    dataset: &DatasetSnapshot,
    dataset_instance: RecordId<DatasetInstanceRecord>,
    contract_id: String,
    policy: &crate::core::records::ExecutionPolicyRecord,
) -> Result<IdentifiedRecord<crate::core::applicability::ApplicabilityDecisionRecord>, RunError> {
    use crate::core::{
        applicability::{ApplicabilityDecisionRecord, ApplicabilityOutcome},
        manifest::ImplementationCapability,
    };
    let declared = candidate
        .implementation_capabilities
        .iter()
        .any(|c| c.capability == ImplementationCapability::Applicability);
    if !declared {
        return Ok(identify_record(ApplicabilityDecisionRecord {
            candidate_id: candidate.candidate.id,
            outcome: ApplicabilityOutcome::Applicable,
            code: "unconditional_one_shot".into(),
            reason: "The implementation declares support for every admitted configuration.".into(),
            checked_inputs: None,
            evidence: None,
            details: None,
        })?);
    }
    let private = Builder::new().prefix("metewand-applicability-").tempdir()?;
    let view = dataset.artifact.private_view(private.path())?;
    let manifest_path = if let Some(path) = &dataset.manifest {
        view.directory().join(path)
    } else {
        let path = private.path().join("unit-dataset-manifest.json");
        fs::write(
            &path,
            b"{\"complete\":true,\"files\":[],\"schema_version\":1}",
        )?;
        path
    };
    let manifest = CanonicalValue::from_slice(&fs::read(&manifest_path)?)?;
    let problem_parameters = candidate
        .problem_configuration
        .record
        .parameters
        .as_json()
        .clone();
    let implementation_parameters = candidate
        .implementation_configuration
        .record
        .parameters
        .as_json()
        .clone();
    let checked_inputs = canonical(json!({
        "dataset_instance": dataset_instance,
        "dataset_manifest": manifest.as_json(),
        "dataset_manifest_path": dataset.manifest,
        "problem_contract_id": contract_id,
        "problem_parameters": problem_parameters,
        "implementation_parameters": implementation_parameters,
    }))?;
    implementation.verify()?;
    dataset.verify()?;
    let checked = (|| {
        let process = crate::runtime::local_worker::launch_trusted_local_worker(
            &implementation.launch,
            private.path(),
            WorkerLogLimits::new(64 * 1024, 64 * 1024),
        )?;
        let mut session = crate::runtime::worker_session::WorkerSession::connect(
            process,
            crate::protocol::WorkerRole::Implementation,
            implementation.identity.clone(),
            &[crate::protocol::Capability::Applicability],
            PhaseTimeouts::default(),
        )?;
        let environment = session.environment().map(|environment| {
            let directory = environment.working_directory.to_string_lossy();
            let private_directory = private.path().to_string_lossy();
            let variables: BTreeMap<_, _> = environment
                .environment_variables
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .replace(directory.as_ref(), "{worker}")
                            .replace(private_directory.as_ref(), "{private}"),
                    )
                })
                .collect();
            {
                let mut record =
                    serde_json::to_value(environment).expect("typed worker environment");
                record["working_directory"] = json!("{worker}");
                record["environment_variables"] = json!(variables);
                record
            }
        });
        let checked = session.check_applicability(
            view.directory().to_string_lossy().into_owned(),
            manifest_path.to_string_lossy().into_owned(),
            contract_id,
            problem_parameters,
            implementation_parameters,
        );
        let stopped = session.shutdown();
        let (_, response) = checked?;
        stopped?;
        Ok::<_, RunError>((response, environment))
    })();
    // Verify after every exit path, including handshake and callback failures.
    view.verify()?;
    dataset.verify()?;
    implementation.verify()?;
    if CanonicalValue::from_slice(&fs::read(&manifest_path)?)? != manifest {
        return Err(RunError::ChangedInput);
    }
    let (response, environment) = checked?;
    let outcome = match response.outcome {
        crate::protocol::ApplicabilityOutcome::Applicable => ApplicabilityOutcome::Applicable,
        crate::protocol::ApplicabilityOutcome::NotApplicable => {
            ApplicabilityOutcome::NotApplicable
        }
    };
    Ok(identify_record(ApplicabilityDecisionRecord {
        candidate_id: candidate.candidate.id,
        outcome,
        code: response.code,
        reason: response.reason,
        checked_inputs: Some(checked_inputs.as_json().clone()),
        details: response.details,
        evidence: Some(json!({"worker": record_document(&implementation.record)?,
            "wire_protocol_version": crate::protocol::WIRE_PROTOCOL_VERSION,
            "environment": environment, "controls": crate::core::controls::local_control_report(policy)})),
    })?)
}

pub(super) fn resolve_worker(
    repository: &CheckedRepository,
    worker: &WorkerDefinition,
    private_root: &Path,
) -> Result<ResolvedWorker, RunError> {
    resolve_worker_with_lock(repository, worker, private_root, None)
}

pub(crate) fn resolve_worker_with_lock(
    repository: &CheckedRepository,
    worker: &WorkerDefinition,
    private_root: &Path,
    locked: Option<&crate::runtime::lockfile::VerifiedLock>,
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
    if locked.is_none()
        && !matches!(
            repository.manifest.environments[environment],
            EnvironmentDefinition::Local {}
        )
    {
        return Err(unsupported(format!(
            "environment `{environment}` requires preparation with metewand lock and execution with run --locked"
        )));
    }
    if transport == Some(ProtocolTransport::Stdio) {
        return Err(unsupported(
            "local workers require dedicated protocol pipes",
        ));
    }
    let sources = patterns
        .map(|patterns| snapshot(repository, patterns, private_root))
        .transpose()?;
    let locked_environment = locked
        .map(|lock| lock.environment(environment))
        .transpose()?;
    let find_executable = |name: &str| -> Result<PathBuf, RunError> {
        if let Some(environment) = locked_environment {
            Ok(crate::runtime::lockfile::locked_program(environment, name)?)
        } else {
            find_program(&repository.root, name)
        }
    };
    let source_program = !Path::new(&program).is_absolute()
        && Path::new(&program).components().count() > 1
        && repository.root.join(&program).is_file()
        && patterns.is_some();
    let original_executable = if source_program {
        find_program(&repository.root, &program)?
    } else {
        find_executable(&program)?
    };
    let mut arguments = arguments;
    let mut executable = original_executable.clone();
    // Windows virtual environments copy their interpreters. Moving a PATH-selected
    // interpreter into a source snapshot would separate it from pyvenv.cfg.
    let repository_program = !cfg!(windows)
        || Path::new(&program).is_absolute()
        || Path::new(&program).components().count() > 1;
    if repository_program
        && let Ok(relative) = fs::canonicalize(&original_executable)?.strip_prefix(&repository.root)
    {
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
        if locked_environment.is_some() && !bytes.starts_with(b"#!") {
            return Err(RunError::Unsupported(
                "locked native executables must come from a prepared environment; source executables require a locked shebang interpreter".into(),
            ));
        }
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
            executable = find_executable(if locked_environment.is_some() {
                Path::new(interpreter)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| unsupported("invalid shebang interpreter"))?
            } else {
                interpreter
            })?;
            if fs::canonicalize(&executable)?.starts_with(&repository.root) {
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
        fingerprint: canonical(if let Some(environment) = locked_environment {
            environment.fingerprint()
        } else {
            json!({"class": "development", "os": env::consts::OS, "architecture": env::consts::ARCH, "program_sha256": executable_digest, "dependency_closure": null})
        })?,
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
            normalized_arguments.push(format!(
                "{{sources}}/{}",
                crate::runtime::platform_fs::relative_path(&relative)?
            ));
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
    let mut variables = crate::runtime::worker_environment::worker_environment_template();
    variables.insert(
        "PATH".to_owned(),
        worker_search_path(None, source_executable)?,
    );
    let function = match worker {
        WorkerDefinition::Command(worker) => {
            worker
                .args
                .starts_with(&["-m".to_owned(), "metewand".to_owned()])
                || worker
                    .args
                    .get(2)
                    .is_some_and(|arg| arg == "metewand::serve_function()")
        }
        _ => false,
    };
    if transport == Some(ProtocolTransport::Socket) || function {
        let language = adapter_language(worker, &executable)?;
        variables.insert(
            "METEWAND_PROTOCOL_TRANSPORT".to_owned(),
            "socket".to_owned(),
        );
        variables.insert("METEWAND_ADAPTER_LANGUAGE".to_owned(), language.to_owned());
        variables.insert(
            "METEWAND_ADAPTER_SHA256".to_owned(),
            crate::runtime::language_adapter::digest(language)?,
        );
    }
    let normalized_program = if let Some(sources) = &sources
        && let Ok(relative) = executable.strip_prefix(sources.directory.path())
    {
        format!(
            "{{sources}}/{}",
            crate::runtime::platform_fs::relative_path(relative)?
        )
    } else {
        format!("sha256:{executable_digest}")
    };
    let uv_environment = locked_environment.and_then(|environment| match environment {
        crate::core::lockfile::LockedEnvironment::Uv(environment) => Some(environment),
        _ => None,
    });
    let normalized_program = if let Some(environment) = uv_environment {
        if source_executable {
            normalized_program
        } else {
            crate::runtime::locked_uv::program_identity(environment, &executable)?.0
        }
    } else {
        normalized_program
    };
    let locked_search_path = if let Some(environment) = locked_environment {
        let mut paths = vec![executable.parent().expect("executable parent").to_owned()];
        if let Some(environment) = environment.nix() {
            paths.extend(environment.outputs.iter().map(|p| Path::new(p).join("bin")));
        }
        paths.dedup();
        Some(
            env::join_paths(paths)
                .map_err(|_| unsupported("invalid locked search path"))?
                .into_string()
                .map_err(|_| unsupported("locked paths must be UTF-8"))?,
        )
    } else {
        None
    };
    if uv_environment.is_some() {
        variables.insert(
            "PYTHONPYCACHEPREFIX".into(),
            "{worker}/cache/python-bytecode".into(),
        );
    }
    if let Some(path) = &locked_search_path {
        variables.insert(
            "PATH".into(),
            if uv_environment.is_some() {
                if cfg!(windows) {
                    "{environment}/Scripts"
                } else {
                    "{environment}/bin"
                }
                .to_owned()
            } else {
                path.clone()
            },
        );
    }
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
    let program_directory = if let Some(path) = locked_search_path {
        path
    } else {
        worker_search_path(executable.parent(), source_executable)?
    };
    variables.insert("PATH".to_owned(), program_directory);
    let launch = ResolvedLaunchRecord {
        environment: environment_record.id,
        program: text_path(&executable)?,
        args: actual_arguments,
        environment_variables: variables,
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let executable_evidence = crate::runtime::build_evidence::executable_evidence(if source_executable {
        &original_executable
    } else {
        &executable
    })?;
    if executable_evidence["executable_sha256"] != executable_digest {
        return Err(RunError::ChangedInput);
    }
    let mut provenance = json!({"environment": record_document(&environment_record.record)?, "launch": record_document(&normalized_launch.record)?, "worker": record_document(&record)?, "executable": executable_evidence, "undeclared_file_access": "unsupported"});
    if let Some(environment) = uv_environment {
        let (program, digest) = if source_executable {
            (
                normalized_launch.record.program.clone(),
                executable_digest.clone(),
            )
        } else {
            crate::runtime::locked_uv::program_identity(environment, &executable)?
        };
        provenance["locked_program"] = json!({"program":program,"sha256":digest});
        provenance["uv_preparation"] = json!({"generation":environment.preparation,"receipt_sha256":environment.receipt_sha256});
    }
    if let Some(lock) = locked {
        provenance["lockfile_sha256"] = json!(lock.digest);
    }
    let resolved = ResolvedWorker {
        launch,
        identity,
        record,
        sources,
        executable,
        executable_digest,
        provenance,
    };
    if let Some(lock) = locked
        && !lock.lock.workers.is_empty()
    {
        lock.verify_worker(repository, worker, &resolved.lock_record(environment))?;
    }
    Ok(resolved)
}

fn adapter_language(
    worker: &WorkerDefinition,
    executable: &Path,
) -> Result<&'static str, RunError> {
    if let WorkerDefinition::Interpreted(worker) = worker {
        return match worker.runner {
            InterpretedRunner::Python => Ok("python"),
            InterpretedRunner::R => Ok("r"),
            InterpretedRunner::Julia => Err(unsupported(
                "managed socket adapters require Python or R workers",
            )),
        };
    }
    let name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    if name == "rscript" {
        Ok("r")
    } else if name.starts_with("python") {
        Ok("python")
    } else {
        Err(unsupported(
            "managed socket adapters require Python or R workers",
        ))
    }
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

pub(super) fn file_inventory(root: &Path) -> Result<Vec<Value>, RunError> {
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
                files.insert(relative.clone(), json!({"path": crate::runtime::platform_fs::relative_path(&relative)?, "sha256": file_digest(&path)?, "size_bytes": entry.metadata()?.len(), "media_type": if path.extension().is_some_and(|extension| extension == "json") { "application/json" } else { "application/octet-stream" }}));
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
                    .flat_map(|path| executable_candidates(&path, program))
                    .find(|path| {
                        path.is_file()
                            && fs::metadata(path).is_ok_and(|metadata| runnable_file(&metadata))
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
    #[cfg(windows)]
    let path = if !path.is_file() && path.extension().is_none() {
        path.with_extension("exe")
    } else {
        path
    };
    let selected_path = std::path::absolute(path)?;
    let path = fs::canonicalize(&selected_path)?;
    if !Path::new(program).is_absolute()
        && Path::new(program).components().count() > 1
        && !path.starts_with(root)
    {
        return Err(unsupported(
            "repository command programs must remain inside the repository; use an absolute path for an external executable",
        ));
    }
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || !runnable_file(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("program `{program}` is not an executable regular file"),
        )
        .into());
    }
    // Python discovers a virtual environment beside the selected interpreter,
    // so following its final symlink would silently change the software context.
    if cfg!(windows) && Path::new(program).components().count() == 1 {
        Ok(selected_path)
    } else if path.starts_with(root) {
        Ok(path)
    } else {
        Ok(selected_path)
    }
}

fn runnable_file(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        crate::runtime::platform_fs::is_executable(metadata)
    }
    #[cfg(windows)]
    {
        crate::runtime::platform_fs::is_regular_file(metadata)
    }
}

fn executable_candidates(directory: &Path, program: &str) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        vec![directory.join(program)]
    }
    #[cfg(windows)]
    {
        if program.eq_ignore_ascii_case("python3") {
            vec![directory.join("python3.exe"), directory.join("python.exe")]
        } else if Path::new(program).extension().is_none() {
            vec![directory.join(format!("{program}.exe"))]
        } else {
            vec![directory.join(program)]
        }
    }
}

fn worker_search_path(
    program_directory: Option<&Path>,
    source_executable: bool,
) -> Result<String, RunError> {
    #[cfg(unix)]
    {
        Ok(if source_executable {
            "/usr/bin:/bin".to_owned()
        } else if let Some(directory) = program_directory {
            format!("{}:/usr/bin:/bin", text_path(directory)?)
        } else {
            "{program_directory}:/usr/bin:/bin".to_owned()
        })
    }
    #[cfg(windows)]
    {
        let mut paths = Vec::new();
        if !source_executable {
            paths.push(
                program_directory
                    .map_or_else(|| PathBuf::from("{program_directory}"), Path::to_path_buf),
            );
        }
        paths.extend([
            PathBuf::from("{system_root}/System32"),
            PathBuf::from("{system_root}"),
        ]);
        env::join_paths(paths)
            .map_err(|_| unsupported("worker search path cannot be represented"))?
            .into_string()
            .map_err(|_| unsupported("worker search path must be UTF-8"))
    }
}

fn executor_configuration() -> Value {
    let configuration = json!({"environment_class": "development", "isolation": "unsupported",
        "worker_cache": {"policy": "clear_all_between_slots"},
        "timeout_termination": {"interrupt": "SIGINT", "grace_ms": 100, "force": "SIGKILL", "exit_wait_ms": 1000}});
    #[cfg(windows)]
    let configuration = {
        let mut configuration = configuration;
        configuration["timeout_termination"] = json!({"interrupt": null, "grace_ms": 0,
            "force": "TerminateJobObject", "exit_wait_ms": 1000});
        configuration["transport"] = json!("windows_inherited_handles");
        configuration["storage"] = serde_json::to_value(crate::runtime::platform_fs::storage_evidence())
            .expect("typed storage evidence");
        configuration
    };
    configuration
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
    fn command_adapter_selection_uses_only_the_executable_name() {
        use crate::core::manifest::{CommandWorker, WorkerDefinition};
        let worker = WorkerDefinition::Command(CommandWorker {
            program: "unused".to_owned(),
            args: vec![],
            sources: None,
            environment: serde_json::from_value(serde_json::json!("local")).unwrap(),
            protocol_transport: None,
        });
        for (path, language) in [
            ("rscript-env/bin/python3", "python"),
            ("rscript-env/bin/python3.14t", "python"),
            ("rscript-env/Python.exe", "python"),
            ("python-env/bin/Rscript", "r"),
            ("python-env/Rscript.exe", "r"),
        ] {
            assert_eq!(
                super::adapter_language(&worker, std::path::Path::new(path)).unwrap(),
                language
            );
        }
        assert!(
            super::adapter_language(&worker, std::path::Path::new("python-env/solver")).is_err()
        );
    }

    #[test]
    #[cfg(unix)]
    fn local_interpreter_preserves_symlinked_virtual_environment() {
        let root = tempfile::tempdir().unwrap();
        let venv = root.path().join(".venv");
        let output = std::process::Command::new("python3")
            .args(["-m", "venv", "--without-pip", "--symlinks"])
            .arg(&venv)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let selected = venv.join("bin/python3");
        assert!(
            std::fs::symlink_metadata(&selected)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let output = std::process::Command::new(&selected)
            .args([
                "-c",
                "import sysconfig; print(sysconfig.get_path('purelib'))",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let packages = String::from_utf8(output.stdout).unwrap();
        std::fs::write(
            std::path::Path::new(packages.trim()).join("metewand_venv_probe.py"),
            "VALUE = 42\n",
        )
        .unwrap();
        let resolved = super::find_program(root.path(), selected.to_str().unwrap()).unwrap();
        assert_eq!(resolved, selected);
        let output = std::process::Command::new(resolved)
            .args([
                "-I",
                "-c",
                "import metewand_venv_probe; print(metewand_venv_probe.VALUE)",
            ])
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        assert_eq!(output.stdout, b"42\n");
    }

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
