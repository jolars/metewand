use std::{collections::BTreeMap, path::Path};

use metewand_core::{
    manifest::{Manifest, Name, parse_manifest},
    planning::{
        DatasetConfigurationDefinition, LogicalPlanningCatalog, expand_manifest_logical_plan,
    },
    problem_contract::{ProblemContract, ScientificBudget, parse_problem_contract},
    records::{
        AttemptSlotRole, DatasetDefinitionRecord, EnvironmentDefinitionRecord,
        ImplementationDefinitionRecord, ProblemDefinitionRecord, RecordId,
    },
    schema::SchemaCatalog,
};
use serde_json::Value;

const CONTRACT: &str = include_str!("../../../fixtures/problem-contract/v1/complete.toml");

const MANIFEST: &str = r#"
version = 1
name = "logical-plan"

[datasets.fixed]
output_schema = "schemas/dataset.json"
sources = ["datasets/fixed.json"]

[problems.regression]
contract = "problems/regression.toml"
[problems.regression.evaluator]
runner = "command"
program = "evaluate"
environment = "local"

[implementations.alpha]
runner = "command"
program = "alpha"
environment = "local"
problem_contracts = ["regression"]
capabilities = ["one_shot"]

[implementations.beta]
runner = "command"
program = "beta"
environment = "local"
problem_contracts = ["regression"]
capabilities = ["one_shot"]

[environments.local]
kind = "local"

[[experiments]]
name = "main"
problem = "regression"
datasets = ["fixed"]
implementations = ["alpha", "beta"]
execution_policy = "reused"
observation_policy = "final"
implementation_repetitions = 2
measurement_repetitions = 3
seed = 17

[execution_policies.reused]
version = 1
worker_reuse = true
warmup_runs = 2

[observation_policies.final]
version = 1
kind = "one_shot"
"#;

fn name(value: &str) -> Name {
    parse_manifest(
        Path::new("name.toml"),
        &format!("version = 1\nname = {value:?}\n"),
    )
    .unwrap()
    .name
}

fn id<T>(byte: u8) -> RecordId<T> {
    RecordId::from_digest([byte; 32])
}

fn schemas() -> SchemaCatalog {
    let documents = ["parameters", "dataset", "result", "metrics", "semantics"]
        .into_iter()
        .map(|name| {
            let source = match name {
                "parameters" => {
                    include_str!("../../../fixtures/problem-contract/v1/schemas/parameters.json")
                }
                "dataset" => {
                    include_str!("../../../fixtures/problem-contract/v1/schemas/dataset.json")
                }
                "result" => {
                    include_str!("../../../fixtures/problem-contract/v1/schemas/result.json")
                }
                "metrics" => {
                    include_str!("../../../fixtures/problem-contract/v1/schemas/metrics.json")
                }
                "semantics" => {
                    include_str!("../../../fixtures/problem-contract/v1/schemas/semantics.json")
                }
                _ => unreachable!(),
            };
            (
                Path::new("schemas").join(format!("{name}.json")),
                serde_json::from_str::<Value>(source).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    SchemaCatalog::try_new(documents).unwrap()
}

fn manifest(source: &str) -> Manifest {
    parse_manifest(Path::new("metewand.toml"), source).unwrap()
}

fn contracts(manifest: &Manifest) -> BTreeMap<Name, ProblemContract> {
    let contract =
        parse_problem_contract(Path::new("problems/regression.toml"), CONTRACT, &schemas())
            .unwrap();
    BTreeMap::from([(manifest.problems.keys().next().unwrap().clone(), contract)])
}

fn dataset_free_contracts(manifest: &Manifest) -> BTreeMap<Name, ProblemContract> {
    let source = CONTRACT
        .replace(
            "dataset_schemas = [\"schemas/dataset.json\"]",
            "dataset_schemas = []",
        )
        .replace("dataset = \"fixtures/small/dataset\"\n", "");
    let contract =
        parse_problem_contract(Path::new("problems/regression.toml"), &source, &schemas()).unwrap();
    BTreeMap::from([(manifest.problems.keys().next().unwrap().clone(), contract)])
}

fn catalog() -> LogicalPlanningCatalog {
    LogicalPlanningCatalog {
        dataset_definitions: BTreeMap::from([(
            DatasetConfigurationDefinition::Named(name("fixed")),
            id::<DatasetDefinitionRecord>(1),
        )]),
        problem_definitions: BTreeMap::from([(
            name("regression"),
            id::<ProblemDefinitionRecord>(2),
        )]),
        implementation_definitions: BTreeMap::from([
            (name("alpha"), id::<ImplementationDefinitionRecord>(3)),
            (name("beta"), id::<ImplementationDefinitionRecord>(4)),
        ]),
        environment_definitions: BTreeMap::from([(
            name("local"),
            id::<EnvironmentDefinitionRecord>(5),
        )]),
    }
}

#[test]
fn expands_repetitions_into_identified_one_shot_records() {
    let manifest = manifest(MANIFEST);
    let plan =
        expand_manifest_logical_plan(&manifest, &contracts(&manifest), &schemas(), &catalog())
            .unwrap();

    assert_eq!(plan.experiments.len(), 1);
    let experiment = &plan.experiments[0];
    assert_eq!(experiment.name, name("main"));
    assert_eq!(experiment.candidates.len(), 2);
    assert_eq!(experiment.scheduling_seed.value, 8_960_666_142_325_237);
    assert_eq!(experiment.execution_policy.record.warmup_runs, 2);
    assert!(experiment.execution_policy.record.worker_reuse);

    for candidate in &experiment.candidates {
        assert_eq!(candidate.specifications.len(), 2);
        assert_eq!(
            candidate.candidate.record.dataset_configuration,
            candidate.dataset_configuration.id
        );
        assert_eq!(
            candidate.candidate.record.problem_configuration,
            candidate.problem_configuration.id
        );
        assert_eq!(
            candidate.candidate.record.implementation_configuration,
            candidate.implementation_configuration.id
        );

        for (repetition, specification) in candidate.specifications.iter().enumerate() {
            assert_eq!(
                specification.specification.record.implementation_repetition,
                repetition as u64
            );
            assert_eq!(
                specification.specification.record.scientific_budget,
                ScientificBudget::None
            );
            assert_eq!(specification.observation_slots.len(), 3);
            assert_eq!(specification.attempt_slots.len(), 5);

            for (warmup_index, slot) in specification.attempt_slots[..2].iter().enumerate() {
                assert_eq!(
                    slot.record.role,
                    AttemptSlotRole::Warmup {
                        warmup_index: warmup_index as u64
                    }
                );
            }

            for (measurement_index, (observation, attempt)) in specification
                .observation_slots
                .iter()
                .zip(&specification.attempt_slots[2..])
                .enumerate()
            {
                assert_eq!(
                    observation.record.measurement_index,
                    measurement_index as u64
                );
                assert_eq!(
                    attempt.record.role,
                    AttemptSlotRole::Measured {
                        observation_slot: observation.id
                    }
                );
            }
        }
    }

    for repetition in 0..2 {
        let alpha = &experiment.candidates[0].specifications[repetition];
        let beta = &experiment.candidates[1].specifications[repetition];
        assert_eq!(
            alpha.specification.record.implementation_seed,
            beta.specification.record.implementation_seed
        );
    }
    assert_ne!(
        experiment.candidates[0].specifications[0]
            .specification
            .record
            .implementation_seed,
        experiment.candidates[0].specifications[1]
            .specification
            .record
            .implementation_seed
    );

    let alpha = &experiment.candidates[0];
    let first = &alpha.specifications[0];
    let ids = [
        alpha.dataset_configuration.id.to_string(),
        alpha.problem_configuration.id.to_string(),
        alpha.implementation_configuration.id.to_string(),
        alpha.candidate.id.to_string(),
        first.specification.id.to_string(),
        first.observation_slots[0].id.to_string(),
        first.attempt_slots[0].id.to_string(),
        first.attempt_slots[2].id.to_string(),
    ];
    assert_eq!(
        ids,
        [
            "mw1-dataset-configuration-63dacca29102e59c87cb42f66e9adfa353d5dc2bc0445e3dfecf35a9c4791556",
            "mw1-problem-configuration-2adda94616ec04811ceb238ed626c643936159588362aa0378ca1e7cf727677c",
            "mw1-implementation-configuration-f26c4511d148fbd79ae9d0230728c343b55eaf28cbbcde34ace2afd80413cb00",
            "mw1-logical-candidate-920758e9aa960f4ac9858fd3c28dea0599bdcc1c9815270d4fb5deba593b716b",
            "mw1-logical-specification-bfba247e61d3b2444296a379d08f947c4664a41d9172a240b79b762cc06eed3e",
            "mw1-logical-observation-slot-e6f837544e5c8d0b28a4e2d87d1f2cc7470e688c4617dfa17c50f8949c2a2f72",
            "mw1-logical-attempt-slot-d568ee1f86ac3d7a0a5e54789c59e232350c493abeaaaa3ddba71a9a87393340",
            "mw1-logical-attempt-slot-cd99b53afcb7937265885ce5c8361356148690375e5e12694c5c83610b0acb17",
        ]
    );
}

#[test]
fn existing_ids_survive_unrelated_candidates_and_more_measurements() {
    let baseline_manifest = manifest(MANIFEST);
    let baseline = expand_manifest_logical_plan(
        &baseline_manifest,
        &contracts(&baseline_manifest),
        &schemas(),
        &catalog(),
    )
    .unwrap();

    let expanded_source = MANIFEST
        .replace(
            "[implementations.beta]",
            r#"[implementations.gamma]
runner = "command"
program = "gamma"
environment = "local"
problem_contracts = ["regression"]
capabilities = ["one_shot"]

[implementations.beta]"#,
        )
        .replace(
            "implementations = [\"alpha\", \"beta\"]",
            "implementations = [\"beta\", \"gamma\", \"alpha\"]",
        )
        .replace("measurement_repetitions = 3", "measurement_repetitions = 4");
    let expanded_manifest = manifest(&expanded_source);
    let mut expanded_catalog = catalog();
    expanded_catalog
        .implementation_definitions
        .insert(name("gamma"), id::<ImplementationDefinitionRecord>(6));
    let expanded = expand_manifest_logical_plan(
        &expanded_manifest,
        &contracts(&expanded_manifest),
        &schemas(),
        &expanded_catalog,
    )
    .unwrap();

    let baseline_alpha = &baseline.experiments[0].candidates[0];
    let expanded_alpha = &expanded.experiments[0].candidates[2];
    assert_eq!(baseline_alpha.candidate.id, expanded_alpha.candidate.id);
    for (before, after) in baseline_alpha
        .specifications
        .iter()
        .zip(&expanded_alpha.specifications)
    {
        assert_eq!(before.specification.id, after.specification.id);
        for (before, after) in before
            .observation_slots
            .iter()
            .zip(&after.observation_slots)
        {
            assert_eq!(before.id, after.id);
        }
        for (before, after) in before.attempt_slots.iter().zip(&after.attempt_slots) {
            assert_eq!(before.id, after.id);
            assert_eq!(
                before.record.scheduling_priority,
                after.record.scheduling_priority
            );
        }
    }
}

#[test]
fn identifies_the_built_in_unit_dataset_configuration() {
    let source = MANIFEST
        .replace("datasets = [\"fixed\"]\n", "")
        .replace("measurement_repetitions = 3", "measurement_repetitions = 1")
        .replace(
            "implementation_repetitions = 2",
            "implementation_repetitions = 1",
        );
    let manifest = manifest(&source);
    let mut catalog = catalog();
    catalog
        .dataset_definitions
        .insert(DatasetConfigurationDefinition::Unit, id(9));
    let plan = expand_manifest_logical_plan(
        &manifest,
        &dataset_free_contracts(&manifest),
        &schemas(),
        &catalog,
    )
    .unwrap();

    for candidate in &plan.experiments[0].candidates {
        assert_eq!(candidate.dataset_configuration.record.definition, id(9));
        assert_eq!(
            candidate.dataset_configuration.record.parameters.as_json(),
            &serde_json::json!({})
        );
    }
}

#[test]
fn reports_a_missing_typed_definition_identity() {
    let manifest = manifest(MANIFEST);
    let mut catalog = catalog();
    catalog.implementation_definitions.remove(&name("beta"));
    let error =
        expand_manifest_logical_plan(&manifest, &contracts(&manifest), &schemas(), &catalog)
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "logical planning has no implementation-definition identity for `beta`"
    );
}
