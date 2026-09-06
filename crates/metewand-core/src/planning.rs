//! Deterministic expansion of manifest parameter matrices.

use std::{collections::BTreeMap, fmt};

use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    canonical::{CanonicalJsonError, CanonicalValue},
    compatibility::{ManifestCompatibilityError, validate_manifest_compatibility},
    manifest::{
        DatasetDefinition, ExperimentCase, ExperimentDefinition, Manifest, Name, ParameterAxes,
        ParameterAxis, WorkerDefinition,
    },
    parameters::{
        ParameterNamespaceError, ParameterResolutionError, resolve_parameters,
        validate_parameter_namespaces,
    },
    problem_contract::ProblemContract,
    schema::SchemaCatalog,
};

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
