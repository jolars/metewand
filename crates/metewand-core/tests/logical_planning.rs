use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use metewand_core::{
    manifest::{ImplementationCapability, Manifest, Name, parse_manifest},
    planning::{
        DatasetConfigurationDefinition, LogicalPlanningCatalog, expand_manifest_logical_plan,
        identify_builtin_unit_dataset_definition,
    },
    problem_contract::{ProblemContract, ScientificBudget, parse_problem_contract},
    records::{
        AttemptSlotRole, CapabilityEvidence, DatasetDefinitionRecord, EnvironmentDefinitionRecord,
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
fn builtin_unit_dataset_has_one_repository_independent_identity() {
    let first = identify_builtin_unit_dataset_definition().unwrap();
    let second = identify_builtin_unit_dataset_definition().unwrap();

    assert_eq!(first, second);
    assert!(matches!(
        first.record.kind,
        metewand_core::records::DatasetDefinitionKind::Unit
    ));
    assert_eq!(first.record.name.as_str(), "unit");
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
        assert_eq!(candidate.implementation_capabilities.len(), 1);
        assert_eq!(
            candidate.implementation_capabilities[0].capability,
            ImplementationCapability::OneShot
        );
        assert_eq!(
            candidate.implementation_capabilities[0].evidence,
            CapabilityEvidence::Declared
        );
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
            "mw1-logical-candidate-d00b8333a7a13b19535a406986f05222cb431e9978b27610398a85a373a68875",
            "mw1-logical-specification-f84ad430ec72ae97710ecbc4ecc208dd10f8186467a7fe24f6a4b600f5932e33",
            "mw1-logical-observation-slot-9bad8a54c8103f6c7f0f9b727f4e7c439fad859dd634456bf90855cc7fdb05cf",
            "mw1-logical-attempt-slot-9bfa93af33b020f6a9cad40a626132cfa77490c5ac996804aa937181a32481dd",
            "mw1-logical-attempt-slot-a387bde5dae47ec3b7e04f8e3a92d6f884af1846626f5ebfdc11e36e3b5e5568",
        ]
    );
}

#[test]
fn concurrency_binds_run_identities_without_changing_scientific_seeds() {
    let plans = [None, Some(1), Some(2), Some(3)].map(|max_concurrency| {
        let mut manifest = manifest(MANIFEST);
        manifest
            .execution_policies
            .get_mut(&name("reused"))
            .unwrap()
            .max_concurrency = max_concurrency;
        expand_manifest_logical_plan(&manifest, &contracts(&manifest), &schemas(), &catalog())
            .unwrap()
    });
    assert_eq!(plans[0], plans[1]);
    let baseline = &plans[0].experiments[0];
    assert_eq!(baseline.execution_policy.record.max_concurrency, 1);
    for (index, plan) in plans[2..].iter().enumerate() {
        let experiment = &plan.experiments[0];
        assert_eq!(
            experiment.execution_policy.record.max_concurrency,
            index as u64 + 2
        );
        assert_ne!(experiment.execution_policy.id, baseline.execution_policy.id);
        assert_eq!(experiment.scheduling_seed, baseline.scheduling_seed);
        for (before, after) in baseline.candidates.iter().zip(&experiment.candidates) {
            assert_eq!(before.dataset_configuration, after.dataset_configuration);
            assert_eq!(before.problem_configuration, after.problem_configuration);
            assert_eq!(
                before.implementation_configuration,
                after.implementation_configuration
            );
            assert_ne!(before.candidate.id, after.candidate.id);
            for (before, after) in before.specifications.iter().zip(&after.specifications) {
                assert_eq!(
                    before.specification.record.implementation_seed,
                    after.specification.record.implementation_seed
                );
                assert_ne!(before.specification.id, after.specification.id);
                assert!(
                    before
                        .attempt_slots
                        .iter()
                        .zip(&after.attempt_slots)
                        .all(|(before, after)| before.id != after.id)
                );
                assert!(
                    before
                        .observation_slots
                        .iter()
                        .zip(&after.observation_slots)
                        .all(|(before, after)| before.id != after.id)
                );
            }
        }
    }
    assert_ne!(
        plans[2].experiments[0].execution_policy.id,
        plans[3].experiments[0].execution_policy.id
    );
}

#[test]
fn parameterized_matrices_have_exact_slot_counts_and_seed_sharing() {
    let source = MANIFEST
        .replace(
            "[datasets.fixed]",
            r#"[datasets.generated]
parameter_schema = "schemas/parameters.json"
output_schema = "schemas/dataset.json"
runner = "command"
program = "generate"
environment = "local"

[datasets.fixed]"#,
        )
        .replace(
            "[implementations.alpha]",
            "[implementations.alpha]\nparameter_schema = \"schemas/parameters.json\"",
        )
        .replace(
            "datasets = [\"fixed\"]",
            "datasets = [\"fixed\", \"generated\"]",
        )
        .replace(
            "seed = 17",
            r#"seed = 17

[experiments.dataset_parameters.generated]
replicate = { grid = [0, 1] }
[experiments.problem_parameters]
lambda = { grid = [0.1, 0.2] }
[experiments.implementation_parameters.alpha]
method = { grid = ["fast", "accurate"] }"#,
        );
    let mut catalog = catalog();
    catalog.dataset_definitions.insert(
        DatasetConfigurationDefinition::Named(name("generated")),
        id(6),
    );
    let mut dataset_seeds = BTreeMap::new();
    let mut implementation_seeds = BTreeMap::new();

    for seed in [17, 18] {
        for (implementation_repetitions, measurements, warmups) in
            [(1, 1, 0), (1, 3, 2), (2, 1, 2), (2, 3, 0)]
        {
            let mut manifest = manifest(&source);
            let experiment = &mut manifest.experiments[0];
            experiment.seed = seed;
            experiment.implementation_repetitions = implementation_repetitions;
            experiment.measurement_repetitions = measurements;
            manifest
                .execution_policies
                .get_mut(&name("reused"))
                .unwrap()
                .warmup_runs = Some(warmups);
            let plan = expand_manifest_logical_plan(
                &manifest,
                &contracts(&manifest),
                &schemas(),
                &catalog,
            )
            .unwrap();
            let experiment = &plan.experiments[0];
            // Alpha contributes two configurations; beta contributes one.
            let candidates = 3 * 2 * 3;
            assert_eq!(experiment.candidates.len(), candidates);
            let mut observations = BTreeSet::new();
            let mut measured_slots = BTreeSet::new();
            let mut warmup_slots = BTreeSet::new();
            let mut seed_blocks = BTreeMap::<_, BTreeSet<_>>::new();

            for candidate in &experiment.candidates {
                let dataset = &candidate.dataset_configuration;
                let dataset_key = (
                    seed,
                    dataset.record.definition,
                    dataset.record.parameters.to_canonical_bytes().unwrap(),
                );
                if let Some(previous) = dataset_seeds.insert(dataset_key, dataset.record.seed) {
                    assert_eq!(previous, dataset.record.seed);
                }
                assert_eq!(
                    candidate.specifications.len() as u64,
                    implementation_repetitions
                );
                for specification in &candidate.specifications {
                    let record = &specification.specification.record;
                    let block = (
                        seed,
                        dataset.id,
                        candidate.problem_configuration.id,
                        record.implementation_repetition,
                    );
                    if let Some(previous) =
                        implementation_seeds.insert(block, record.implementation_seed)
                    {
                        assert_eq!(previous, record.implementation_seed);
                    }
                    assert!(
                        seed_blocks
                            .entry(block)
                            .or_default()
                            .insert(candidate.implementation_configuration.id)
                    );
                    assert_eq!(specification.observation_slots.len() as u64, measurements);
                    for observation in &specification.observation_slots {
                        assert!(observations.insert(observation.id));
                    }
                    for attempt in &specification.attempt_slots {
                        match attempt.record.role {
                            AttemptSlotRole::Measured { observation_slot } => {
                                assert!(measured_slots.insert(attempt.id));
                                assert!(
                                    specification
                                        .observation_slots
                                        .iter()
                                        .any(|observation| observation.id == observation_slot)
                                );
                            }
                            AttemptSlotRole::Warmup { .. } => {
                                assert!(warmup_slots.insert(attempt.id));
                            }
                        }
                    }
                }
            }
            let specifications = candidates as u64 * implementation_repetitions;
            assert_eq!(observations.len() as u64, specifications * measurements);
            assert_eq!(measured_slots.len(), observations.len());
            assert_eq!(warmup_slots.len() as u64, specifications * warmups);
            assert_eq!(seed_blocks.len() as u64, 3 * 2 * implementation_repetitions);
            assert!(
                seed_blocks
                    .values()
                    .all(|implementations| implementations.len() == 3)
            );
        }
    }

    assert_eq!(dataset_seeds.len(), 2 * 3);
    assert_eq!(implementation_seeds.len(), 2 * 3 * 2 * 2);
    // These fixture inputs must distinguish every seed block, including its full digest.
    for seeds in [
        dataset_seeds.values().collect::<Vec<_>>(),
        implementation_seeds.values().collect(),
    ] {
        assert_eq!(
            seeds
                .iter()
                .map(|seed| seed.value)
                .collect::<BTreeSet<_>>()
                .len(),
            seeds.len()
        );
        assert_eq!(
            seeds
                .iter()
                .map(|seed| seed.derivation_digest)
                .collect::<BTreeSet<_>>()
                .len(),
            seeds.len()
        );
    }
}

#[test]
fn models_each_capability_evidence_state_with_a_stable_label() {
    let evidence = [
        CapabilityEvidence::Declared,
        CapabilityEvidence::VerifiedNow,
        CapabilityEvidence::PreviouslyVerified,
    ];

    assert_eq!(
        evidence.map(|state| state.to_string()),
        ["declared", "verified_now", "previously_verified"]
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

#[test]
fn required_local_controls_fail_during_logical_planning() {
    let manifest = manifest(&MANIFEST.replace(
        "worker_reuse = true",
        "worker_reuse = true\nenforcement = \"required\"",
    ));
    let error =
        expand_manifest_logical_plan(&manifest, &contracts(&manifest), &schemas(), &catalog())
            .unwrap_err();
    assert!(matches!(
        error,
        metewand_core::planning::LogicalPlanningError::Control(_)
    ));
    assert!(error.to_string().contains("network"));
}

#[test]
fn best_effort_resource_requests_survive_planning_and_are_reported() {
    let manifest = manifest(&MANIFEST.replace(
        "worker_reuse = true",
        "worker_reuse = true\ncpus = 2\nthreads = 4\nmemory = \"8 GiB\"",
    ));
    let plan =
        expand_manifest_logical_plan(&manifest, &contracts(&manifest), &schemas(), &catalog())
            .unwrap();
    let report =
        metewand_core::controls::local_control_report(&plan.experiments[0].execution_policy.record);
    assert_eq!(report.len(), 9);
    assert_eq!(
        serde_json::to_value(&report[6]).unwrap(),
        serde_json::json!({
            "name": "cpus", "requested": 2, "required": false, "status": "unsupported", "scope": "worker_lifetime"
        })
    );
    assert_eq!(report[7].requested, serde_json::json!(4));
    assert_eq!(report[8].requested, serde_json::json!("8 GiB"));
}
