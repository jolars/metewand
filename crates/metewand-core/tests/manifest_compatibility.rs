use std::{collections::BTreeMap, path::Path};

use metewand_core::{
    compatibility::{ManifestCompatibilityError, validate_manifest_compatibility},
    manifest::{Manifest, parse_manifest},
    problem_contract::{ProblemContract, parse_problem_contract},
    schema::SchemaCatalog,
};
use serde_json::Value;

const COMPLETE_CONTRACT: &str = include_str!("../../../fixtures/problem-contract/v1/complete.toml");

const COMPLETE_MANIFEST: &str = r#"
version = 1
name = "compatibility"

[datasets.input]
output_schema = "schemas/dataset.json"
sources = ["datasets/input.json"]

[problems.regression]
contract = "problems/regression.toml"
[problems.regression.evaluator]
runner = "command"
program = "evaluate"
environment = "local"

[implementations.solver]
runner = "command"
program = "solve"
environment = "local"
problem_contracts = ["regression"]
capabilities = ["one_shot"]

[[experiments]]
name = "main"
problem = "regression"
datasets = ["input"]
implementations = ["solver"]
execution_policy = "controlled"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[execution_policies.controlled]
version = 1
worker_reuse = false
warmup_runs = 0
timing_scope = "prepare_and_execute"

[observation_policies.final]
version = 1
kind = "one_shot"
"#;

fn schema_catalog() -> SchemaCatalog {
    let documents = [
        (
            "parameters",
            include_str!("../../../fixtures/problem-contract/v1/schemas/parameters.json"),
        ),
        (
            "dataset",
            include_str!("../../../fixtures/problem-contract/v1/schemas/dataset.json"),
        ),
        (
            "result",
            include_str!("../../../fixtures/problem-contract/v1/schemas/result.json"),
        ),
        (
            "metrics",
            include_str!("../../../fixtures/problem-contract/v1/schemas/metrics.json"),
        ),
        (
            "semantics",
            include_str!("../../../fixtures/problem-contract/v1/schemas/semantics.json"),
        ),
    ]
    .into_iter()
    .map(|(name, source)| {
        (
            format!("schemas/{name}.json").into(),
            serde_json::from_str::<Value>(source).unwrap(),
        )
    })
    .collect::<Vec<_>>();
    SchemaCatalog::try_new(documents).unwrap()
}

fn parse(source: &str) -> Manifest {
    parse_manifest(Path::new("metewand.toml"), source).unwrap()
}

fn contract(dataset_free: bool, allowed_timing_scopes: &str) -> ProblemContract {
    let mut source = COMPLETE_CONTRACT.replace(
        "allowed_timing_scopes = [\"cold_end_to_end\", \"prepare_and_execute\"]",
        allowed_timing_scopes,
    );
    if dataset_free {
        source = source
            .replace(
                "dataset_schemas = [\"schemas/dataset.json\"]",
                "dataset_schemas = []",
            )
            .replace("dataset = \"fixtures/small/dataset\"\n", "");
    }
    parse_problem_contract(
        Path::new("problems/regression.toml"),
        &source,
        &schema_catalog(),
    )
    .unwrap()
}

fn contracts_for(
    manifest: &Manifest,
    problem: &str,
    contract: ProblemContract,
) -> BTreeMap<metewand_core::manifest::Name, ProblemContract> {
    BTreeMap::from([(
        manifest.problems.get_key_value(problem).unwrap().0.clone(),
        contract,
    )])
}

#[test]
fn accepts_compatible_dataset_backed_experiments() {
    let manifest = parse(COMPLETE_MANIFEST);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(
            false,
            "allowed_timing_scopes = [\"cold_end_to_end\", \"prepare_and_execute\"]",
        ),
    );

    validate_manifest_compatibility(&manifest, &contracts).unwrap();
}

#[test]
fn requires_every_problem_contract_to_be_loaded() {
    let manifest = parse(COMPLETE_MANIFEST);

    assert!(matches!(
        validate_manifest_compatibility(&manifest, &BTreeMap::new()),
        Err(ManifestCompatibilityError::MissingProblemContract {
            problem,
            contract_path,
        }) if problem.as_str() == "regression"
            && contract_path == Path::new("problems/regression.toml")
    ));
}

#[test]
fn requires_every_experiment_selection_to_resolve() {
    let source =
        COMPLETE_MANIFEST.replace("problem = \"regression\"", "problem = \"missing-problem\"");
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UndefinedProblem { problem, .. })
            if problem.as_str() == "missing-problem"
    ));

    let source = COMPLETE_MANIFEST.replace("datasets = [\"input\"]", "datasets = [\"missing\"]");
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UndefinedDataset { dataset, .. })
            if dataset.as_str() == "missing"
    ));

    let source = COMPLETE_MANIFEST.replace(
        "implementations = [\"solver\"]",
        "implementations = [\"missing\"]",
    );
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UndefinedImplementation { implementation, .. })
            if implementation.as_str() == "missing"
    ));

    let source = COMPLETE_MANIFEST.replace(
        "execution_policy = \"controlled\"",
        "execution_policy = \"missing\"",
    );
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UndefinedExecutionPolicy {
            execution_policy,
            ..
        }) if execution_policy.as_str() == "missing"
    ));

    let source = COMPLETE_MANIFEST.replace(
        "observation_policy = \"final\"",
        "observation_policy = \"missing\"",
    );
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UndefinedObservationPolicy {
            observation_policy,
            ..
        }) if observation_policy.as_str() == "missing"
    ));
}

#[test]
fn requires_implementation_contract_declarations_to_resolve() {
    let source = COMPLETE_MANIFEST.replace(
        "[implementations.solver]",
        r#"[implementations.invalid]
runner = "command"
program = "invalid"
environment = "local"
problem_contracts = ["missing"]
capabilities = ["one_shot"]

[implementations.solver]"#,
    );
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );

    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UndefinedProblemDeclaration {
            implementation,
            problem,
        }) if implementation.as_str() == "invalid" && problem.as_str() == "missing"
    ));
}

#[test]
fn rejects_implementation_problem_pairings_outside_the_declared_set() {
    let source = COMPLETE_MANIFEST
        .replace(
            "problem_contracts = [\"regression\"]",
            "problem_contracts = [\"other\"]",
        )
        .replace(
            "[implementations.solver]",
            r#"[problems.other]
contract = "problems/other.toml"
[problems.other.evaluator]
runner = "command"
program = "evaluate-other"
environment = "local"

[implementations.solver]"#,
        );
    let manifest = parse(&source);
    let mut contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    contracts.insert(
        manifest.problems.get_key_value("other").unwrap().0.clone(),
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );

    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UnsupportedProblem {
            experiment,
            implementation,
            problem,
        }) if experiment.as_str() == "main"
            && implementation.as_str() == "solver"
            && problem.as_str() == "regression"
    ));
}

#[test]
fn rejects_dataset_schemas_not_accepted_by_the_problem() {
    let manifest = parse(&COMPLETE_MANIFEST.replace(
        "output_schema = \"schemas/dataset.json\"",
        "output_schema = \"schemas/other-dataset.json\"",
    ));
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );

    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::IncompatibleDatasetSchema {
            experiment,
            dataset,
            problem,
            output_schema,
        }) if experiment.as_str() == "main"
            && dataset.as_str() == "input"
            && problem.as_str() == "regression"
            && output_schema == Path::new("schemas/other-dataset.json")
    ));
}

#[test]
fn enforces_dataset_free_and_dataset_backed_selection_rules() {
    let dataset_free_manifest = parse(COMPLETE_MANIFEST);
    let dataset_free_contracts = contracts_for(
        &dataset_free_manifest,
        "regression",
        contract(true, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&dataset_free_manifest, &dataset_free_contracts),
        Err(ManifestCompatibilityError::DatasetFreeProblemHasDatasets {
            experiment,
            problem,
        }) if experiment.as_str() == "main" && problem.as_str() == "regression"
    ));

    let omitted = COMPLETE_MANIFEST.replace("datasets = [\"input\"]\n", "");
    let dataset_free_manifest = parse(&omitted);
    let dataset_free_contracts = contracts_for(
        &dataset_free_manifest,
        "regression",
        contract(true, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    validate_manifest_compatibility(&dataset_free_manifest, &dataset_free_contracts).unwrap();

    let dataset_backed_manifest = parse(&omitted);
    let dataset_backed_contracts = contracts_for(
        &dataset_backed_manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&dataset_backed_manifest, &dataset_backed_contracts),
        Err(ManifestCompatibilityError::DatasetRequired {
            experiment,
            problem,
        }) if experiment.as_str() == "main" && problem.as_str() == "regression"
    ));
}

#[test]
fn rejects_timing_scopes_not_admitted_by_the_contract() {
    let manifest = parse(COMPLETE_MANIFEST);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"cold_end_to_end\"]"),
    );

    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UnsupportedTimingScope {
            experiment,
            problem,
            execution_policy,
            ..
        }) if experiment.as_str() == "main"
            && problem.as_str() == "regression"
            && execution_policy.as_str() == "controlled"
    ));
}

#[test]
fn applies_safe_defaults_when_validating_timing_and_warmups() {
    let source = COMPLETE_MANIFEST
        .replace("worker_reuse = false\n", "")
        .replace("warmup_runs = 0\n", "")
        .replace("timing_scope = \"prepare_and_execute\"\n", "");
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );

    validate_manifest_compatibility(&manifest, &contracts).unwrap();
}

#[test]
fn requires_process_local_warmups_to_reuse_the_worker() {
    let manifest = parse(&COMPLETE_MANIFEST.replace("warmup_runs = 0", "warmup_runs = 1"));
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );

    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::WarmupsRequireWorkerReuse {
            execution_policy,
            warmup_runs: 1,
        }) if execution_policy.as_str() == "controlled"
    ));
}

#[test]
fn cold_end_to_end_requires_a_fresh_worker_without_warmups() {
    let source = COMPLETE_MANIFEST
        .replace("worker_reuse = false", "worker_reuse = true")
        .replace(
            "timing_scope = \"prepare_and_execute\"",
            "timing_scope = \"cold_end_to_end\"",
        );
    let manifest = parse(&source);
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"cold_end_to_end\"]"),
    );

    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::ColdEndToEndRequiresFreshWorker {
            execution_policy,
        }) if execution_policy.as_str() == "controlled"
    ));
}

#[test]
fn validates_the_observation_capability_and_scientific_budget() {
    let mut manifest = parse(COMPLETE_MANIFEST);
    manifest
        .implementations
        .get_mut("solver")
        .unwrap()
        .capabilities
        .clear();
    let contracts = contracts_for(
        &manifest,
        "regression",
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]"),
    );
    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UnsupportedObservationKind {
            implementation,
            ..
        }) if implementation.as_str() == "solver"
    ));

    let manifest = parse(COMPLETE_MANIFEST);
    let mut unsupported_contract =
        contract(false, "allowed_timing_scopes = [\"prepare_and_execute\"]");
    unsupported_contract.supported_budgets.clear();
    let contracts = contracts_for(&manifest, "regression", unsupported_contract);
    assert!(matches!(
        validate_manifest_compatibility(&manifest, &contracts),
        Err(ManifestCompatibilityError::UnsupportedScientificBudget {
            problem,
            ..
        }) if problem.as_str() == "regression"
    ));
}
