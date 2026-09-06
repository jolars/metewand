//! Cross-definition validation for parsed manifests and problem contracts.

use std::{collections::BTreeMap, path::PathBuf};

use thiserror::Error;

use crate::{
    manifest::{
        DatasetDefinition, ExperimentDefinition, ImplementationCapability, Manifest, Name,
        ObservationKind, TimingScope,
    },
    problem_contract::{ProblemContract, ScientificBudget},
};

/// Validates references and compatibility across a manifest and its contracts.
///
/// `problem_contracts` associates each manifest-local problem name with the
/// contract parsed from that problem definition's declared path. This function
/// performs no filesystem access, schema loading, expansion, or worker launch.
///
/// Validation covers every implementation declaration and execution policy,
/// followed by experiments in manifest order. A dataset-free experiment's
/// empty dataset selection denotes the single built-in unit dataset that the
/// planner will bind during expansion.
///
/// # Errors
///
/// Returns the first error in deterministic problem, implementation, execution
/// policy, and experiment order.
pub fn validate_manifest_compatibility(
    manifest: &Manifest,
    problem_contracts: &BTreeMap<Name, ProblemContract>,
) -> Result<(), ManifestCompatibilityError> {
    for (problem, definition) in &manifest.problems {
        if !problem_contracts.contains_key(problem) {
            return Err(ManifestCompatibilityError::MissingProblemContract {
                problem: problem.clone(),
                contract_path: definition.contract.as_path().to_path_buf(),
            });
        }
    }

    for (implementation, definition) in &manifest.implementations {
        for problem in &definition.problem_contracts {
            if !manifest.problems.contains_key(problem) {
                return Err(ManifestCompatibilityError::UndefinedProblemDeclaration {
                    implementation: implementation.clone(),
                    problem: problem.clone(),
                });
            }
        }
    }

    for (execution_policy, policy) in &manifest.execution_policies {
        let worker_reuse = policy.resolved_worker_reuse();
        let warmup_runs = policy.resolved_warmup_runs();
        if warmup_runs > 0 && !worker_reuse {
            return Err(ManifestCompatibilityError::WarmupsRequireWorkerReuse {
                execution_policy: execution_policy.clone(),
                warmup_runs,
            });
        }
        if policy.resolved_timing_scope() == TimingScope::ColdEndToEnd
            && (worker_reuse || warmup_runs != 0)
        {
            return Err(
                ManifestCompatibilityError::ColdEndToEndRequiresFreshWorker {
                    execution_policy: execution_policy.clone(),
                },
            );
        }
    }

    for experiment in &manifest.experiments {
        if !manifest.problems.contains_key(&experiment.problem) {
            return Err(ManifestCompatibilityError::UndefinedProblem {
                experiment: experiment.name.clone(),
                problem: experiment.problem.clone(),
            });
        }
        let contract = problem_contracts
            .get(&experiment.problem)
            .expect("every defined problem contract was checked above");

        validate_dataset_selection(manifest, experiment, contract)?;

        let execution_policy = manifest
            .execution_policies
            .get(&experiment.execution_policy)
            .ok_or_else(|| ManifestCompatibilityError::UndefinedExecutionPolicy {
                experiment: experiment.name.clone(),
                execution_policy: experiment.execution_policy.clone(),
            })?;
        let timing_scope = execution_policy.resolved_timing_scope();
        if !contract.allowed_timing_scopes.contains(&timing_scope) {
            return Err(ManifestCompatibilityError::UnsupportedTimingScope {
                experiment: experiment.name.clone(),
                problem: experiment.problem.clone(),
                execution_policy: experiment.execution_policy.clone(),
                timing_scope,
            });
        }

        let observation_policy = manifest
            .observation_policies
            .get(&experiment.observation_policy)
            .ok_or_else(|| ManifestCompatibilityError::UndefinedObservationPolicy {
                experiment: experiment.name.clone(),
                observation_policy: experiment.observation_policy.clone(),
            })?;
        let scientific_budget = required_scientific_budget(observation_policy.kind);
        if !contract.supported_budgets.contains(&scientific_budget) {
            return Err(ManifestCompatibilityError::UnsupportedScientificBudget {
                experiment: experiment.name.clone(),
                problem: experiment.problem.clone(),
                observation_policy: experiment.observation_policy.clone(),
                scientific_budget,
            });
        }

        for implementation in &experiment.implementations {
            let definition = manifest
                .implementations
                .get(implementation)
                .ok_or_else(|| ManifestCompatibilityError::UndefinedImplementation {
                    experiment: experiment.name.clone(),
                    implementation: implementation.clone(),
                })?;
            if !definition.problem_contracts.contains(&experiment.problem) {
                return Err(ManifestCompatibilityError::UnsupportedProblem {
                    experiment: experiment.name.clone(),
                    implementation: implementation.clone(),
                    problem: experiment.problem.clone(),
                });
            }

            let required_capability = required_capability(observation_policy.kind);
            if !definition.capabilities.contains(&required_capability) {
                return Err(ManifestCompatibilityError::UnsupportedObservationKind {
                    experiment: experiment.name.clone(),
                    implementation: implementation.clone(),
                    observation_policy: experiment.observation_policy.clone(),
                    observation_kind: observation_policy.kind,
                });
            }
        }
    }

    Ok(())
}

fn validate_dataset_selection(
    manifest: &Manifest,
    experiment: &ExperimentDefinition,
    contract: &ProblemContract,
) -> Result<(), ManifestCompatibilityError> {
    if contract.dataset_schemas.is_empty() {
        if experiment.datasets.is_empty() {
            return Ok(());
        }
        return Err(ManifestCompatibilityError::DatasetFreeProblemHasDatasets {
            experiment: experiment.name.clone(),
            problem: experiment.problem.clone(),
        });
    }
    if experiment.datasets.is_empty() {
        return Err(ManifestCompatibilityError::DatasetRequired {
            experiment: experiment.name.clone(),
            problem: experiment.problem.clone(),
        });
    }

    for dataset in &experiment.datasets {
        let definition = manifest.datasets.get(dataset).ok_or_else(|| {
            ManifestCompatibilityError::UndefinedDataset {
                experiment: experiment.name.clone(),
                dataset: dataset.clone(),
            }
        })?;
        let output_schema = match definition {
            DatasetDefinition::Fixed(definition) => &definition.output_schema,
            DatasetDefinition::Generated(definition) => &definition.output_schema,
        };
        if !contract.dataset_schemas.contains(output_schema) {
            return Err(ManifestCompatibilityError::IncompatibleDatasetSchema {
                experiment: experiment.name.clone(),
                dataset: dataset.clone(),
                problem: experiment.problem.clone(),
                output_schema: output_schema.as_path().to_path_buf(),
            });
        }
    }

    Ok(())
}

fn required_capability(kind: ObservationKind) -> ImplementationCapability {
    match kind {
        ObservationKind::OneShot => ImplementationCapability::OneShot,
    }
}

fn required_scientific_budget(kind: ObservationKind) -> ScientificBudget {
    match kind {
        ObservationKind::OneShot => ScientificBudget::None,
    }
}

/// A cross-definition manifest or problem-contract incompatibility.
#[derive(Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum ManifestCompatibilityError {
    /// A manifest problem has no associated parsed contract.
    #[error("problem `{problem}` has no loaded contract from `{}`", contract_path.display())]
    MissingProblemContract {
        /// Manifest-local problem name.
        problem: Name,
        /// Contract path declared by the problem definition.
        contract_path: PathBuf,
    },

    /// An implementation declaration names no manifest problem.
    #[error("implementation `{implementation}` declares undefined problem `{problem}`")]
    UndefinedProblemDeclaration {
        /// Manifest-local implementation name.
        implementation: Name,
        /// Missing problem name.
        problem: Name,
    },

    /// An experiment selects no manifest problem.
    #[error("experiment `{experiment}` selects undefined problem `{problem}`")]
    UndefinedProblem {
        /// Experiment name.
        experiment: Name,
        /// Missing problem name.
        problem: Name,
    },

    /// An experiment selects a dataset for a dataset-free problem.
    #[error(
        "experiment `{experiment}` selects datasets for dataset-free problem `{problem}`; omit `datasets` to bind the built-in unit dataset"
    )]
    DatasetFreeProblemHasDatasets {
        /// Experiment name.
        experiment: Name,
        /// Dataset-free problem name.
        problem: Name,
    },

    /// An experiment omits datasets for a dataset-backed problem.
    #[error("experiment `{experiment}` must select a dataset for problem `{problem}`")]
    DatasetRequired {
        /// Experiment name.
        experiment: Name,
        /// Dataset-backed problem name.
        problem: Name,
    },

    /// An experiment selects no manifest dataset.
    #[error("experiment `{experiment}` selects undefined dataset `{dataset}`")]
    UndefinedDataset {
        /// Experiment name.
        experiment: Name,
        /// Missing dataset name.
        dataset: Name,
    },

    /// A selected dataset's output schema is not accepted by the problem.
    #[error(
        "dataset `{dataset}` in experiment `{experiment}` emits schema `{}`, which problem `{problem}` does not accept",
        output_schema.display()
    )]
    IncompatibleDatasetSchema {
        /// Experiment name.
        experiment: Name,
        /// Selected dataset name.
        dataset: Name,
        /// Selected problem name.
        problem: Name,
        /// Dataset output schema.
        output_schema: PathBuf,
    },

    /// An experiment selects no manifest execution policy.
    #[error("experiment `{experiment}` selects undefined execution policy `{execution_policy}`")]
    UndefinedExecutionPolicy {
        /// Experiment name.
        experiment: Name,
        /// Missing execution-policy name.
        execution_policy: Name,
    },

    /// An execution policy requests warm-ups without worker reuse.
    #[error(
        "execution policy `{execution_policy}` requests {warmup_runs} warm-up runs with `worker_reuse = false`"
    )]
    WarmupsRequireWorkerReuse {
        /// Execution-policy name.
        execution_policy: Name,
        /// Requested warm-up count.
        warmup_runs: u64,
    },

    /// A cold end-to-end policy does not use fresh, non-warmed workers.
    #[error(
        "execution policy `{execution_policy}` uses `cold_end_to_end`, which requires `worker_reuse = false` and `warmup_runs = 0`"
    )]
    ColdEndToEndRequiresFreshWorker {
        /// Execution-policy name.
        execution_policy: Name,
    },

    /// A problem contract does not admit the selected timing scope.
    #[error(
        "execution policy `{execution_policy}` selects timing scope `{timing_scope}` for experiment `{experiment}`, but problem `{problem}` does not allow it"
    )]
    UnsupportedTimingScope {
        /// Experiment name.
        experiment: Name,
        /// Selected problem name.
        problem: Name,
        /// Selected execution-policy name.
        execution_policy: Name,
        /// Timing scope requested by the execution policy.
        timing_scope: TimingScope,
    },

    /// An experiment selects no manifest observation policy.
    #[error(
        "experiment `{experiment}` selects undefined observation policy `{observation_policy}`"
    )]
    UndefinedObservationPolicy {
        /// Experiment name.
        experiment: Name,
        /// Missing observation-policy name.
        observation_policy: Name,
    },

    /// A problem contract does not support the policy's scientific budget.
    #[error(
        "observation policy `{observation_policy}` requires scientific budget `{scientific_budget}` for experiment `{experiment}`, but problem `{problem}` does not support it"
    )]
    UnsupportedScientificBudget {
        /// Experiment name.
        experiment: Name,
        /// Selected problem name.
        problem: Name,
        /// Selected observation-policy name.
        observation_policy: Name,
        /// Scientific budget required by the policy.
        scientific_budget: ScientificBudget,
    },

    /// An experiment selects no manifest implementation.
    #[error("experiment `{experiment}` selects undefined implementation `{implementation}`")]
    UndefinedImplementation {
        /// Experiment name.
        experiment: Name,
        /// Missing implementation name.
        implementation: Name,
    },

    /// An implementation does not declare support for the selected problem.
    #[error(
        "implementation `{implementation}` in experiment `{experiment}` does not declare problem `{problem}`"
    )]
    UnsupportedProblem {
        /// Experiment name.
        experiment: Name,
        /// Selected implementation name.
        implementation: Name,
        /// Selected problem name.
        problem: Name,
    },

    /// An implementation lacks the capability required by the observation policy.
    #[error(
        "implementation `{implementation}` in experiment `{experiment}` does not support `{observation_kind}` observation policy `{observation_policy}`"
    )]
    UnsupportedObservationKind {
        /// Experiment name.
        experiment: Name,
        /// Selected implementation name.
        implementation: Name,
        /// Selected observation-policy name.
        observation_policy: Name,
        /// Observation kind required by the policy.
        observation_kind: ObservationKind,
    },
}
