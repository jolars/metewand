//! Deterministic expansion of manifest parameter matrices.

use std::{collections::BTreeMap, fmt};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    SCHEDULING_POLICY_VERSION,
    canonical::{CanonicalJsonError, CanonicalValue},
    compatibility::{ManifestCompatibilityError, validate_manifest_compatibility},
    identity::{IdentityError, identify_record},
    manifest::{
        DatasetDefinition, ExecutionPolicyDefinition, ExperimentCase, ExperimentDefinition,
        Manifest, Name, ObservationPolicyDefinition, ParameterAxes, ParameterAxis,
        WorkerDefinition,
    },
    parameters::{
        ParameterNamespaceError, ParameterResolutionError, resolve_parameters,
        validate_parameter_namespaces,
    },
    problem_contract::{ProblemContract, ScientificBudget},
    records::{
        AttemptSlotRole, ContentDigest, DatasetConfigurationRecord, DatasetDefinitionRecord,
        DerivedSeedRecord, EnvironmentDefinitionRecord, ExecutionPolicyRecord, IdentifiedRecord,
        ImplementationConfigurationRecord, ImplementationDefinitionRecord,
        LogicalAttemptSlotRecord, LogicalCandidateRecord, LogicalObservationSlotRecord,
        ObservationPolicyRecord, OneShotLogicalSpecificationRecord, ProblemConfigurationRecord,
        ProblemDefinitionRecord, RecordId,
    },
    schema::SchemaCatalog,
    seed::{
        SeedDerivationError, derive_dataset_seed, derive_implementation_seed,
        derive_scheduling_seed,
    },
};

/// Domain separator for version-1 attempt-slot scheduling priorities.
pub const SCHEDULING_PRIORITY_DOMAIN: &str = "metewand-scheduling-priority-v1";

/// Expands every experiment into fully resolved configuration candidates.
///
/// Literal `value` axes contribute one value. Each `grid` contributes its
/// values in declared order, and axes form a Cartesian product in dataset,
/// problem, and applicable implementation namespaces. Parameter names are
/// traversed lexicographically, while experiments, cases, datasets,
/// implementations, and grid members retain their declared order.
///
/// When an experiment has named cases, each case overlays the experiment-level
/// axes by parameter name and is expanded independently. The case expansions
/// are unioned in declared order. Case names are retained as diagnostic
/// provenance but do not distinguish logical candidates.
///
/// This function also runs the pure namespace and cross-definition validation
/// passes required by expansion. It performs no filesystem access and launches
/// no workers.
///
/// # Errors
///
/// Returns the first deterministic namespace, compatibility, parameter, or
/// duplicate-candidate error.
pub fn expand_manifest_configurations(
    manifest: &Manifest,
    problem_contracts: &BTreeMap<Name, ProblemContract>,
    schemas: &SchemaCatalog,
) -> Result<Vec<ExpandedExperiment>, ConfigurationExpansionError> {
    validate_parameter_namespaces(manifest)?;
    validate_manifest_compatibility(manifest, problem_contracts)?;

    manifest
        .experiments
        .iter()
        .map(|experiment| {
            expand_experiment(
                manifest,
                &problem_contracts[&experiment.problem],
                schemas,
                experiment,
            )
        })
        .collect()
}

/// One experiment after configuration expansion and duplicate checking.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpandedExperiment {
    /// Manifest-local experiment name.
    pub name: Name,
    /// Deterministically ordered configuration candidates.
    pub candidates: Vec<ExpandedCandidate>,
}

/// Typed definition identities needed to turn expanded parameters into records.
///
/// Schema, source-bundle, contract, and environment resolution determines
/// definition identities before this pure logical-planning pass. The built-in
/// unit dataset, when needed, is keyed by
/// [`DatasetConfigurationDefinition::Unit`].
#[derive(Clone, Debug, Default)]
pub struct LogicalPlanningCatalog {
    /// Manifest and built-in dataset-definition identities.
    pub dataset_definitions:
        BTreeMap<DatasetConfigurationDefinition, RecordId<DatasetDefinitionRecord>>,
    /// Problem-definition identities keyed by manifest-local name.
    pub problem_definitions: BTreeMap<Name, RecordId<ProblemDefinitionRecord>>,
    /// Implementation-definition identities keyed by manifest-local name.
    pub implementation_definitions: BTreeMap<Name, RecordId<ImplementationDefinitionRecord>>,
    /// Environment-definition identities keyed by manifest-local name.
    pub environment_definitions: BTreeMap<Name, RecordId<EnvironmentDefinitionRecord>>,
}

/// A complete unresolved logical plan for all manifest experiments.
#[derive(Clone, Debug, PartialEq)]
pub struct LogicalPlan {
    /// Experiments in their declared order.
    pub experiments: Vec<LogicalExperimentPlan>,
}

/// One experiment expanded into identified logical records.
#[derive(Clone, Debug, PartialEq)]
pub struct LogicalExperimentPlan {
    /// Manifest-local experiment name.
    pub name: Name,
    /// Seed used to derive stable scheduling priorities.
    pub scheduling_seed: DerivedSeedRecord,
    /// Selected, fully defaulted execution policy.
    pub execution_policy: IdentifiedRecord<ExecutionPolicyRecord>,
    /// Selected one-shot observation policy.
    pub observation_policy: IdentifiedRecord<ObservationPolicyRecord>,
    /// Logical candidates in deterministic configuration-expansion order.
    pub candidates: Vec<LogicalCandidatePlan>,
}

/// One expanded configuration candidate with all logical identities attached.
#[derive(Clone, Debug, PartialEq)]
pub struct LogicalCandidatePlan {
    /// Experiment defaults or named case that produced this candidate.
    pub source: ConfigurationSource,
    /// Identified dataset configuration, including its derived seed.
    pub dataset_configuration: IdentifiedRecord<DatasetConfigurationRecord>,
    /// Identified problem configuration.
    pub problem_configuration: IdentifiedRecord<ProblemConfigurationRecord>,
    /// Identified implementation configuration.
    pub implementation_configuration: IdentifiedRecord<ImplementationConfigurationRecord>,
    /// Identified comparison candidate.
    pub candidate: IdentifiedRecord<LogicalCandidateRecord>,
    /// One run specification per implementation repetition.
    pub specifications: Vec<OneShotSpecificationPlan>,
}

/// One identified run specification and all of its planned slots.
#[derive(Clone, Debug, PartialEq)]
pub struct OneShotSpecificationPlan {
    /// One-shot logical run with a fixed implementation seed.
    pub specification: IdentifiedRecord<OneShotLogicalSpecificationRecord>,
    /// One independently evaluated slot per measurement repetition.
    pub observation_slots: Vec<IdentifiedRecord<LogicalObservationSlotRecord>>,
    /// Warm-up slots followed by one measured slot per observation.
    pub attempt_slots: Vec<IdentifiedRecord<LogicalAttemptSlotRecord>>,
}

/// Expands and identifies the complete unresolved one-shot logical plan.
///
/// This pass performs no filesystem access, materialization, environment
/// resolution, or worker launch. The supplied catalog is the boundary between
/// prior definition identity construction and logical planning.
///
/// # Errors
///
/// Returns the first deterministic configuration, catalog, seed, or identity
/// error in manifest expansion order.
pub fn expand_manifest_logical_plan(
    manifest: &Manifest,
    problem_contracts: &BTreeMap<Name, ProblemContract>,
    schemas: &SchemaCatalog,
    catalog: &LogicalPlanningCatalog,
) -> Result<LogicalPlan, LogicalPlanningError> {
    let expanded_experiments =
        expand_manifest_configurations(manifest, problem_contracts, schemas)?;
    let experiments = manifest
        .experiments
        .iter()
        .zip(expanded_experiments)
        .map(|(experiment, expanded)| plan_experiment(manifest, catalog, experiment, expanded))
        .collect::<Result<Vec<_>, LogicalPlanningError>>()?;

    Ok(LogicalPlan { experiments })
}

/// Derives a stable priority for one logical attempt-slot role.
///
/// The transcript hashes the complete scheduling-seed digest, the role, and a
/// typed identity that already exists: the logical specification for a warm-up
/// or the logical observation slot for a measurement. Warm-up indices use
/// eight-byte big-endian encoding. No expansion position or unrelated member
/// participates.
#[must_use]
pub fn derive_attempt_scheduling_priority(
    scheduling_seed: &DerivedSeedRecord,
    specification: RecordId<OneShotLogicalSpecificationRecord>,
    role: &AttemptSlotRole,
) -> ContentDigest {
    let mut hasher = Sha256::new();
    hasher.update(SCHEDULING_PRIORITY_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(scheduling_seed.derivation_digest.bytes());
    hasher.update([0]);
    match role {
        AttemptSlotRole::Warmup { warmup_index } => {
            hasher.update(b"warmup");
            hasher.update([0]);
            hasher.update(specification.to_string().as_bytes());
            hasher.update([0]);
            hasher.update(warmup_index.to_be_bytes());
        }
        AttemptSlotRole::Measured { observation_slot } => {
            hasher.update(b"measured");
            hasher.update([0]);
            hasher.update(observation_slot.to_string().as_bytes());
        }
    }
    ContentDigest::new(hasher.finalize().into())
}

/// A deterministic logical-planning failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LogicalPlanningError {
    /// Configuration expansion or its pure validation failed.
    #[error(transparent)]
    Configuration(Box<ConfigurationExpansionError>),

    /// A selected definition was absent from the identity catalog.
    #[error("logical planning has no {kind} identity for `{name}`")]
    MissingDefinitionIdentity {
        /// Stable typed definition kind.
        kind: &'static str,
        /// Manifest-local definition name or the built-in unit label.
        name: String,
    },

    /// A component seed could not be derived.
    #[error(transparent)]
    Seed(#[from] SeedDerivationError),

    /// An expanded logical record could not be identified.
    #[error(transparent)]
    Identity(#[from] IdentityError),
}

impl From<ConfigurationExpansionError> for LogicalPlanningError {
    fn from(error: ConfigurationExpansionError) -> Self {
        Self::Configuration(Box::new(error))
    }
}

fn plan_experiment(
    manifest: &Manifest,
    catalog: &LogicalPlanningCatalog,
    experiment: &ExperimentDefinition,
    expanded: ExpandedExperiment,
) -> Result<LogicalExperimentPlan, LogicalPlanningError> {
    debug_assert_eq!(experiment.name, expanded.name);

    let execution_policy_definition = manifest
        .execution_policies
        .get(&experiment.execution_policy)
        .expect("configuration expansion validates execution-policy references");
    let execution_policy = identify_record(execution_policy_record(
        experiment.execution_policy.clone(),
        execution_policy_definition,
    ))?;
    let observation_policy_definition = manifest
        .observation_policies
        .get(&experiment.observation_policy)
        .expect("configuration expansion validates observation-policy references");
    let observation_policy = identify_record(observation_policy_record(
        experiment.observation_policy.clone(),
        observation_policy_definition,
    ))?;
    let scheduling_seed = derive_scheduling_seed(
        experiment.seed,
        &manifest.name,
        &experiment.name,
        SCHEDULING_POLICY_VERSION,
    )?;

    let candidates = expanded
        .candidates
        .into_iter()
        .map(|candidate| {
            plan_candidate(
                catalog,
                experiment,
                execution_policy.id,
                observation_policy.id,
                execution_policy.record.warmup_runs,
                &scheduling_seed,
                candidate,
            )
        })
        .collect::<Result<Vec<_>, LogicalPlanningError>>()?;

    Ok(LogicalExperimentPlan {
        name: expanded.name,
        scheduling_seed,
        execution_policy,
        observation_policy,
        candidates,
    })
}

#[allow(clippy::too_many_arguments)]
fn plan_candidate(
    catalog: &LogicalPlanningCatalog,
    experiment: &ExperimentDefinition,
    execution_policy: RecordId<ExecutionPolicyRecord>,
    observation_policy: RecordId<ObservationPolicyRecord>,
    warmup_runs: u64,
    scheduling_seed: &DerivedSeedRecord,
    expanded: ExpandedCandidate,
) -> Result<LogicalCandidatePlan, LogicalPlanningError> {
    let dataset_definition = catalog
        .dataset_definitions
        .get(&expanded.dataset.definition)
        .copied()
        .ok_or_else(|| LogicalPlanningError::MissingDefinitionIdentity {
            kind: "dataset-definition",
            name: expanded.dataset.definition.to_string(),
        })?;
    let problem_definition = named_definition_id(
        &catalog.problem_definitions,
        &expanded.problem.definition,
        "problem-definition",
    )?;
    let implementation_definition = named_definition_id(
        &catalog.implementation_definitions,
        &expanded.implementation.definition,
        "implementation-definition",
    )?;
    let environment_definition = named_definition_id(
        &catalog.environment_definitions,
        &expanded.implementation.environment,
        "environment-definition",
    )?;

    let dataset_seed = derive_dataset_seed(
        experiment.seed,
        dataset_definition,
        &expanded.dataset.parameters,
    )?;
    let dataset_configuration = identify_record(DatasetConfigurationRecord {
        definition: dataset_definition,
        parameters: expanded.dataset.parameters,
        seed: dataset_seed,
    })?;
    let problem_configuration = identify_record(ProblemConfigurationRecord {
        definition: problem_definition,
        parameters: expanded.problem.parameters,
    })?;
    let implementation_configuration = identify_record(ImplementationConfigurationRecord {
        definition: implementation_definition,
        parameters: expanded.implementation.parameters,
        environment: environment_definition,
    })?;
    let candidate = identify_record(LogicalCandidateRecord {
        dataset_configuration: dataset_configuration.id,
        problem_configuration: problem_configuration.id,
        implementation_configuration: implementation_configuration.id,
        execution_policy,
        observation_policy,
    })?;

    let specifications = (0..experiment.implementation_repetitions)
        .map(|implementation_repetition| {
            plan_specification(
                experiment,
                candidate.id,
                dataset_configuration.id,
                problem_configuration.id,
                implementation_repetition,
                warmup_runs,
                scheduling_seed,
            )
        })
        .collect::<Result<Vec<_>, LogicalPlanningError>>()?;

    Ok(LogicalCandidatePlan {
        source: expanded.source,
        dataset_configuration,
        problem_configuration,
        implementation_configuration,
        candidate,
        specifications,
    })
}

#[allow(clippy::too_many_arguments)]
fn plan_specification(
    experiment: &ExperimentDefinition,
    candidate: RecordId<LogicalCandidateRecord>,
    dataset_configuration: RecordId<DatasetConfigurationRecord>,
    problem_configuration: RecordId<ProblemConfigurationRecord>,
    implementation_repetition: u64,
    warmup_runs: u64,
    scheduling_seed: &DerivedSeedRecord,
) -> Result<OneShotSpecificationPlan, LogicalPlanningError> {
    let implementation_seed = derive_implementation_seed(
        experiment.seed,
        dataset_configuration,
        problem_configuration,
        implementation_repetition,
    )?;
    let specification = identify_record(OneShotLogicalSpecificationRecord {
        candidate,
        scientific_budget: ScientificBudget::None,
        implementation_repetition,
        implementation_seed,
    })?;

    let observation_slots = (0..experiment.measurement_repetitions)
        .map(|measurement_index| {
            identify_record(LogicalObservationSlotRecord {
                specification: specification.id,
                measurement_index,
            })
            .map_err(LogicalPlanningError::from)
        })
        .collect::<Result<Vec<_>, LogicalPlanningError>>()?;

    let mut attempt_slots = Vec::new();
    for warmup_index in 0..warmup_runs {
        let role = AttemptSlotRole::Warmup { warmup_index };
        attempt_slots.push(identify_record(LogicalAttemptSlotRecord {
            specification: specification.id,
            scheduling_priority: derive_attempt_scheduling_priority(
                scheduling_seed,
                specification.id,
                &role,
            ),
            role,
        })?);
    }
    for observation in &observation_slots {
        let role = AttemptSlotRole::Measured {
            observation_slot: observation.id,
        };
        attempt_slots.push(identify_record(LogicalAttemptSlotRecord {
            specification: specification.id,
            scheduling_priority: derive_attempt_scheduling_priority(
                scheduling_seed,
                specification.id,
                &role,
            ),
            role,
        })?);
    }

    Ok(OneShotSpecificationPlan {
        specification,
        observation_slots,
        attempt_slots,
    })
}

fn named_definition_id<T>(
    definitions: &BTreeMap<Name, RecordId<T>>,
    name: &Name,
    kind: &'static str,
) -> Result<RecordId<T>, LogicalPlanningError> {
    definitions
        .get(name)
        .copied()
        .ok_or_else(|| LogicalPlanningError::MissingDefinitionIdentity {
            kind,
            name: name.to_string(),
        })
}

fn execution_policy_record(
    name: Name,
    definition: &ExecutionPolicyDefinition,
) -> ExecutionPolicyRecord {
    ExecutionPolicyRecord {
        name,
        cpus: definition.cpus,
        threads: definition.threads,
        memory: definition.memory.clone(),
        network: definition.resolved_network(),
        worker_reuse: definition.resolved_worker_reuse(),
        warmup_runs: definition.resolved_warmup_runs(),
        timeout: definition.timeout.clone(),
        timing_scope: definition.resolved_timing_scope(),
        primary_time: definition.resolved_primary_time(),
        run_order: definition.resolved_run_order(),
        enforcement: definition.resolved_enforcement(),
    }
}

fn observation_policy_record(
    name: Name,
    definition: &ObservationPolicyDefinition,
) -> ObservationPolicyRecord {
    ObservationPolicyRecord {
        name,
        kind: definition.kind,
    }
}

/// One pre-identity logical candidate expressed through resolved parameters.
///
/// Definition and policy identities are attached in the subsequent logical
/// planning stage. The three explicit namespaces here prevent parameters with
/// the same name from being conflated.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpandedCandidate {
    /// Experiment defaults or named case that produced this candidate.
    pub source: ConfigurationSource,
    /// Selected dataset configuration.
    pub dataset: ExpandedDatasetConfiguration,
    /// Selected problem configuration.
    pub problem: ExpandedProblemConfiguration,
    /// Selected implementation configuration.
    pub implementation: ExpandedImplementationConfiguration,
}

/// The authored matrix member that produced an expanded candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigurationSource {
    /// The experiment has no named cases and was expanded directly.
    Experiment,
    /// A named case overlaid the experiment-level axes.
    Case(Name),
}

impl fmt::Display for ConfigurationSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Experiment => formatter.write_str("experiment parameters"),
            Self::Case(name) => write!(formatter, "case `{name}`"),
        }
    }
}

/// A dataset definition selected during configuration expansion.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DatasetConfigurationDefinition {
    /// The versioned built-in unit dataset for a dataset-free problem.
    Unit,
    /// A manifest-defined dataset.
    Named(Name),
}

impl fmt::Display for DatasetConfigurationDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => formatter.write_str("<unit>"),
            Self::Named(name) => name.fmt(formatter),
        }
    }
}

/// One dataset definition bound to complete canonical parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpandedDatasetConfiguration {
    /// Selected manifest definition or built-in unit definition.
    pub definition: DatasetConfigurationDefinition,
    /// Resolved parameters after recursive default merging and validation.
    pub parameters: CanonicalValue,
}

/// One problem definition bound to complete canonical parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpandedProblemConfiguration {
    /// Selected manifest problem definition.
    pub definition: Name,
    /// Resolved parameters after recursive default merging and validation.
    pub parameters: CanonicalValue,
}

/// One implementation definition bound to complete canonical parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpandedImplementationConfiguration {
    /// Selected manifest implementation definition.
    pub definition: Name,
    /// Environment selected by the implementation worker.
    pub environment: Name,
    /// Resolved parameters after recursive default merging and validation.
    pub parameters: CanonicalValue,
}

/// A scientific parameter namespace being resolved for one configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigurationNamespace {
    /// Dataset-owned parameters.
    Dataset,
    /// Problem-owned parameters.
    Problem,
    /// Implementation-owned parameters.
    Implementation,
}

impl fmt::Display for ConfigurationNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Dataset => "dataset",
            Self::Problem => "problem",
            Self::Implementation => "implementation",
        })
    }
}

/// A deterministic configuration-expansion failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigurationExpansionError {
    /// A parameter namespace does not belong to its selected definition.
    #[error(transparent)]
    ParameterNamespace(#[from] ParameterNamespaceError),

    /// Selected manifest definitions or contracts are incompatible.
    #[error(transparent)]
    ManifestCompatibility(#[from] ManifestCompatibilityError),

    /// A parameter combination could not be merged or validated.
    #[error(
        "failed to resolve {namespace} configuration `{definition}` from {source_location} in experiment `{experiment}`: {error}"
    )]
    ParameterResolution {
        /// Enclosing experiment name.
        experiment: Name,
        /// Experiment defaults or named case being expanded.
        source_location: ConfigurationSource,
        /// Parameter namespace being resolved.
        namespace: ConfigurationNamespace,
        /// Definition that owns the parameter schema.
        definition: Name,
        /// Underlying canonicalization or schema-validation error.
        #[source]
        error: ParameterResolutionError,
    },

    /// Two expansion paths produced the same logical candidate.
    #[error(transparent)]
    DuplicateLogicalCandidate(Box<DuplicateLogicalCandidateError>),

    /// Canonical serialization unexpectedly failed while comparing candidates.
    #[error("failed to canonicalize an expanded configuration: {0}")]
    Canonicalization(#[from] CanonicalJsonError),
}

/// Details about two expansion paths that produced one logical candidate.
#[derive(Debug, Error)]
#[error(
    "{duplicate_source} in experiment `{experiment}` duplicates the logical candidate first produced by {first_source} for dataset `{dataset}`, problem `{problem}`, and implementation `{implementation}`"
)]
pub struct DuplicateLogicalCandidateError {
    /// Enclosing experiment name.
    pub experiment: Name,
    /// First matrix member that produced the candidate.
    pub first_source: ConfigurationSource,
    /// Later matrix member that reproduced the candidate.
    pub duplicate_source: ConfigurationSource,
    /// Selected dataset definition.
    pub dataset: DatasetConfigurationDefinition,
    /// Selected problem definition.
    pub problem: Name,
    /// Selected implementation definition.
    pub implementation: Name,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CandidateKey {
    dataset: DatasetConfigurationDefinition,
    dataset_parameters: Vec<u8>,
    problem: Name,
    problem_parameters: Vec<u8>,
    implementation: Name,
    implementation_parameters: Vec<u8>,
}

fn expand_experiment(
    manifest: &Manifest,
    contract: &ProblemContract,
    schemas: &SchemaCatalog,
    experiment: &ExperimentDefinition,
) -> Result<ExpandedExperiment, ConfigurationExpansionError> {
    let mut candidates = Vec::new();
    let mut seen = BTreeMap::<CandidateKey, ConfigurationSource>::new();

    if experiment.cases.is_empty() {
        expand_source(
            manifest,
            contract,
            schemas,
            experiment,
            None,
            ConfigurationSource::Experiment,
            &mut candidates,
            &mut seen,
        )?;
    } else {
        for case in &experiment.cases {
            expand_source(
                manifest,
                contract,
                schemas,
                experiment,
                Some(case),
                ConfigurationSource::Case(case.name.clone()),
                &mut candidates,
                &mut seen,
            )?;
        }
    }

    Ok(ExpandedExperiment {
        name: experiment.name.clone(),
        candidates,
    })
}

#[allow(clippy::too_many_arguments)]
fn expand_source(
    manifest: &Manifest,
    contract: &ProblemContract,
    schemas: &SchemaCatalog,
    experiment: &ExperimentDefinition,
    case: Option<&ExperimentCase>,
    source: ConfigurationSource,
    candidates: &mut Vec<ExpandedCandidate>,
    seen: &mut BTreeMap<CandidateKey, ConfigurationSource>,
) -> Result<(), ConfigurationExpansionError> {
    let problem_parameters = expand_resolved_parameters(
        schemas,
        contract.parameter_schema.as_path(),
        manifest.problems[&experiment.problem]
            .parameter_defaults
            .as_ref(),
        Some(&experiment.problem_parameters),
        case.map(|case| &case.problem_parameters),
    )
    .map_err(|error| ConfigurationExpansionError::ParameterResolution {
        experiment: experiment.name.clone(),
        source_location: source.clone(),
        namespace: ConfigurationNamespace::Problem,
        definition: experiment.problem.clone(),
        error,
    })?;

    let dataset_parameters = if experiment.datasets.is_empty() {
        vec![(
            DatasetConfigurationDefinition::Unit,
            vec![empty_parameters()],
        )]
    } else {
        experiment
            .datasets
            .iter()
            .map(|name| {
                let definition = &manifest.datasets[name];
                let parameters = match definition {
                    DatasetDefinition::Fixed(_) => Ok(vec![empty_parameters()]),
                    DatasetDefinition::Generated(definition) => {
                        match &definition.parameter_schema {
                            Some(schema) => expand_resolved_parameters(
                                schemas,
                                schema.as_path(),
                                definition.parameter_defaults.as_ref(),
                                experiment.dataset_parameters.get(name),
                                case.and_then(|case| case.dataset_parameters.get(name)),
                            ),
                            None => Ok(vec![empty_parameters()]),
                        }
                    }
                }
                .map_err(|error| {
                    ConfigurationExpansionError::ParameterResolution {
                        experiment: experiment.name.clone(),
                        source_location: source.clone(),
                        namespace: ConfigurationNamespace::Dataset,
                        definition: name.clone(),
                        error,
                    }
                })?;
                Ok((
                    DatasetConfigurationDefinition::Named(name.clone()),
                    parameters,
                ))
            })
            .collect::<Result<Vec<_>, ConfigurationExpansionError>>()?
    };

    let implementation_parameters = experiment
        .implementations
        .iter()
        .map(|name| {
            let definition = &manifest.implementations[name];
            let parameters = match &definition.parameter_schema {
                Some(schema) => expand_resolved_parameters(
                    schemas,
                    schema.as_path(),
                    definition.parameter_defaults.as_ref(),
                    experiment.implementation_parameters.get(name),
                    case.and_then(|case| case.implementation_parameters.get(name)),
                ),
                None => Ok(vec![empty_parameters()]),
            }
            .map_err(|error| ConfigurationExpansionError::ParameterResolution {
                experiment: experiment.name.clone(),
                source_location: source.clone(),
                namespace: ConfigurationNamespace::Implementation,
                definition: name.clone(),
                error,
            })?;
            Ok((
                name.clone(),
                worker_environment(&definition.worker).clone(),
                parameters,
            ))
        })
        .collect::<Result<Vec<_>, ConfigurationExpansionError>>()?;

    for (dataset_definition, dataset_parameters) in dataset_parameters {
        for dataset_parameters in dataset_parameters {
            for problem_parameters in &problem_parameters {
                for (implementation_definition, environment, implementation_parameters) in
                    &implementation_parameters
                {
                    for implementation_parameters in implementation_parameters {
                        let candidate = ExpandedCandidate {
                            source: source.clone(),
                            dataset: ExpandedDatasetConfiguration {
                                definition: dataset_definition.clone(),
                                parameters: dataset_parameters.clone(),
                            },
                            problem: ExpandedProblemConfiguration {
                                definition: experiment.problem.clone(),
                                parameters: problem_parameters.clone(),
                            },
                            implementation: ExpandedImplementationConfiguration {
                                definition: implementation_definition.clone(),
                                environment: environment.clone(),
                                parameters: implementation_parameters.clone(),
                            },
                        };
                        insert_candidate(experiment, candidate, candidates, seen)?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn expand_resolved_parameters(
    schemas: &SchemaCatalog,
    schema_path: &std::path::Path,
    defaults: Option<&CanonicalValue>,
    base: Option<&ParameterAxes>,
    overlay: Option<&ParameterAxes>,
) -> Result<Vec<CanonicalValue>, ParameterResolutionError> {
    let mut axes = base
        .into_iter()
        .flat_map(|axes| axes.iter())
        .map(|(name, axis)| (name.as_str(), axis))
        .collect::<BTreeMap<_, _>>();
    if let Some(overlay) = overlay {
        axes.extend(overlay.iter().map(|(name, axis)| (name.as_str(), axis)));
    }

    let supplied = expand_axes(&axes)?;
    supplied
        .iter()
        .map(|parameters| resolve_parameters(schemas, schema_path, defaults, parameters))
        .collect()
}

fn expand_axes(
    axes: &BTreeMap<&str, &ParameterAxis>,
) -> Result<Vec<CanonicalValue>, CanonicalJsonError> {
    let mut combinations = vec![Map::new()];
    for (name, axis) in axes {
        let values = match axis {
            ParameterAxis::Value(value) => std::slice::from_ref(value),
            ParameterAxis::Grid(values) => values.as_slice(),
        };
        combinations = combinations
            .into_iter()
            .flat_map(|combination| {
                values.iter().map(move |value| {
                    let mut expanded = combination.clone();
                    expanded.insert((*name).to_owned(), value.as_json().clone());
                    expanded
                })
            })
            .collect();
    }

    combinations
        .into_iter()
        .map(|parameters| CanonicalValue::try_from(Value::Object(parameters)))
        .collect()
}

fn empty_parameters() -> CanonicalValue {
    CanonicalValue::try_from(Value::Object(Map::new()))
        .expect("the empty object is always a canonical value")
}

fn worker_environment(worker: &WorkerDefinition) -> &Name {
    match worker {
        WorkerDefinition::Interpreted(worker) => &worker.environment,
        WorkerDefinition::Command(worker) => &worker.environment,
    }
}

fn insert_candidate(
    experiment: &ExperimentDefinition,
    candidate: ExpandedCandidate,
    candidates: &mut Vec<ExpandedCandidate>,
    seen: &mut BTreeMap<CandidateKey, ConfigurationSource>,
) -> Result<(), ConfigurationExpansionError> {
    let key = CandidateKey {
        dataset: candidate.dataset.definition.clone(),
        dataset_parameters: candidate.dataset.parameters.to_canonical_bytes()?,
        problem: candidate.problem.definition.clone(),
        problem_parameters: candidate.problem.parameters.to_canonical_bytes()?,
        implementation: candidate.implementation.definition.clone(),
        implementation_parameters: candidate.implementation.parameters.to_canonical_bytes()?,
    };

    if let Some(first_source) = seen.get(&key) {
        return Err(ConfigurationExpansionError::DuplicateLogicalCandidate(
            Box::new(DuplicateLogicalCandidateError {
                experiment: experiment.name.clone(),
                first_source: first_source.clone(),
                duplicate_source: candidate.source,
                dataset: key.dataset,
                problem: key.problem,
                implementation: key.implementation,
            }),
        ));
    }

    seen.insert(key, candidate.source.clone());
    candidates.push(candidate);
    Ok(())
}
