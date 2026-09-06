use std::path::{Path, PathBuf};

use metewand_core::{
    problem_contract::{
        ProblemContractError, ProblemSchemaRole, ScientificBudget, parse_problem_contract,
        parse_problem_contract_document, validate_problem_contract,
    },
    schema::{DRAFT_2020_12_DIALECT, SchemaCatalog, SchemaValidationError},
};
use serde_json::{Value, json};

const COMPLETE_CONTRACT: &str = include_str!("../../../fixtures/problem-contract/v1/complete.toml");

fn schema_documents() -> Vec<(PathBuf, Value)> {
    [
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
            PathBuf::from(format!("schemas/{name}.json")),
            serde_json::from_str(source).expect("schema fixture must be valid JSON"),
        )
    })
    .collect()
}

fn schema_catalog() -> SchemaCatalog {
    SchemaCatalog::try_new(schema_documents()).expect("schema fixtures must form a valid catalog")
}

#[test]
fn parses_and_validates_a_complete_problem_contract() {
    let contract = parse_problem_contract(
        Path::new("problems/regression.toml"),
        COMPLETE_CONTRACT,
        &schema_catalog(),
    )
    .unwrap();

    assert_eq!(contract.version, 1);
    assert_eq!(contract.name.as_str(), "regression");
    assert_eq!(contract.family.as_str(), "optimization");
    assert_eq!(contract.dataset_schemas.len(), 1);
    assert_eq!(contract.allowed_timing_scopes.len(), 2);
    assert_eq!(contract.supported_budgets, [ScientificBudget::None]);
    assert_eq!(contract.reference_cases.len(), 1);
    assert!(contract.reference_cases[0].dataset.is_some());
    assert_eq!(contract.semantics.validity()["nonfinite"], "reject");
    assert_eq!(
        contract.semantics.one_shot_completion()["maximum_error"],
        1e-8
    );
}

#[test]
fn repository_loaders_can_parse_before_compiling_the_schema_catalog() {
    let path = Path::new("problems/regression.toml");
    let contract = parse_problem_contract_document(path, COMPLETE_CONTRACT).unwrap();
    assert_eq!(
        contract.semantics_schema.as_path(),
        Path::new("schemas/semantics.json")
    );

    let empty_catalog = SchemaCatalog::try_new([]).unwrap();
    let error = validate_problem_contract(path, &contract, &empty_catalog).unwrap_err();
    assert!(matches!(
        error,
        ProblemContractError::MissingSchema {
            role: ProblemSchemaRole::Parameter,
            ..
        }
    ));
}

#[test]
fn accepts_dataset_free_contracts_and_reference_cases() {
    let source = COMPLETE_CONTRACT
        .replace(
            "dataset_schemas = [\"schemas/dataset.json\"]",
            "dataset_schemas = []",
        )
        .replace("dataset = \"fixtures/small/dataset\"\n", "");

    let contract = parse_problem_contract(
        Path::new("problems/regression.toml"),
        &source,
        &schema_catalog(),
    )
    .unwrap();

    assert!(contract.dataset_schemas.is_empty());
    assert!(contract.reference_cases[0].dataset.is_none());
}

#[test]
fn rejects_unknown_fields_with_a_source_span() {
    let source = COMPLETE_CONTRACT.replace(
        "family = \"optimization\"",
        "family = \"optimization\"\nunexpected = true",
    );

    let error = parse_problem_contract(
        Path::new("bench/problems/regression.toml"),
        &source,
        &schema_catalog(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("unknown field `unexpected`"));
    let span = error
        .source_span()
        .expect("parse errors must retain a span");
    assert_eq!(span.path, Path::new("bench/problems/regression.toml"));
    assert_eq!(span.line, 4);
}

#[test]
fn requires_distinct_validity_and_one_shot_completion_rules() {
    for (heading, expected) in [
        (
            "[semantics.validity]\nnonfinite = \"reject\"\n\n",
            "validity",
        ),
        (
            "[semantics.one_shot_completion]\nmaximum_error = 1e-8\n\n",
            "one_shot_completion",
        ),
    ] {
        let source = COMPLETE_CONTRACT.replace(heading, "");
        let error = parse_problem_contract(
            Path::new("problems/regression.toml"),
            &source,
            &schema_catalog(),
        )
        .unwrap_err();

        assert!(error.to_string().contains(expected), "{error}");
        assert!(error.source_span().is_some());
    }
}

#[test]
fn rejects_empty_operational_rule_objects() {
    for assignment in [
        "[semantics.validity]\nnonfinite = \"reject\"",
        "[semantics.one_shot_completion]\nmaximum_error = 1e-8",
    ] {
        let heading = assignment.lines().next().unwrap();
        let source = COMPLETE_CONTRACT.replace(assignment, heading);
        let error = parse_problem_contract(
            Path::new("problems/regression.toml"),
            &source,
            &schema_catalog(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("cannot be empty"), "{error}");
    }
}

#[test]
fn validates_family_specific_semantics_against_its_declared_schema() {
    let source = COMPLETE_CONTRACT.replace("nonfinite = \"reject\"", "nonfinite = \"allow\"");
    let error = parse_problem_contract(
        Path::new("problems/regression.toml"),
        &source,
        &schema_catalog(),
    )
    .unwrap_err();

    let ProblemContractError::InvalidSemantics { source, .. } = error else {
        panic!("expected invalid semantics");
    };
    let SchemaValidationError::InvalidInstance { violations, .. } = source else {
        panic!("expected schema violations");
    };
    assert_eq!(violations[0].instance_path, "/validity/nonfinite");
}

#[test]
fn requires_every_declared_schema_to_be_available() {
    for (missing, expected_role) in [
        ("schemas/parameters.json", ProblemSchemaRole::Parameter),
        ("schemas/dataset.json", ProblemSchemaRole::Dataset),
        ("schemas/result.json", ProblemSchemaRole::Result),
        ("schemas/metrics.json", ProblemSchemaRole::Metric),
        ("schemas/semantics.json", ProblemSchemaRole::Semantics),
    ] {
        let documents = schema_documents()
            .into_iter()
            .filter(|(path, _)| path != Path::new(missing));
        let catalog = SchemaCatalog::try_new(documents).unwrap();

        let error = parse_problem_contract(
            Path::new("problems/regression.toml"),
            COMPLETE_CONTRACT,
            &catalog,
        )
        .unwrap_err();

        assert!(
            matches!(
                error,
                ProblemContractError::MissingSchema {
                    role,
                    schema_path,
                    ..
                } if role == expected_role && schema_path == Path::new(missing)
            ),
            "wrong missing-schema error for {missing}"
        );
    }
}

#[test]
fn enforces_common_envelope_constraints() {
    let mutations = [
        ("version = 1", "version = 2"),
        (
            "dataset_schemas = [\"schemas/dataset.json\"]",
            "dataset_schemas = [\"schemas/dataset.json\", \"schemas/dataset.json\"]",
        ),
        (
            "allowed_timing_scopes = [\"cold_end_to_end\", \"prepare_and_execute\"]",
            "allowed_timing_scopes = []",
        ),
        ("supported_budgets = [\"none\"]", "supported_budgets = []"),
    ];

    for (from, to) in mutations {
        let source = COMPLETE_CONTRACT.replace(from, to);
        let error = parse_problem_contract(
            Path::new("problems/regression.toml"),
            &source,
            &schema_catalog(),
        )
        .expect_err("invalid common envelope must be rejected");
        assert!(error.source_span().is_some(), "{error}");
    }
}

#[test]
fn requires_reference_case_datasets_exactly_when_the_contract_uses_datasets() {
    let missing_dataset = COMPLETE_CONTRACT.replace("dataset = \"fixtures/small/dataset\"\n", "");
    let error = parse_problem_contract(
        Path::new("problems/regression.toml"),
        &missing_dataset,
        &schema_catalog(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("requires `dataset`"), "{error}");

    let dataset_free_with_dataset = COMPLETE_CONTRACT.replace(
        "dataset_schemas = [\"schemas/dataset.json\"]",
        "dataset_schemas = []",
    );
    let error = parse_problem_contract(
        Path::new("problems/regression.toml"),
        &dataset_free_with_dataset,
        &schema_catalog(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("must omit `dataset`"), "{error}");
}

#[test]
fn rejects_contract_values_outside_the_canonical_domain() {
    let source = COMPLETE_CONTRACT.replace(
        "maximum_error = 1e-8",
        "maximum_error = 1979-05-27T07:32:00Z",
    );
    let error = parse_problem_contract(
        Path::new("problems/regression.toml"),
        &source,
        &schema_catalog(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("version-1 Metewand JSON domain"));
    assert!(error.source_span().is_some());
}

#[test]
fn family_schema_fixture_explicitly_owns_the_operational_rules() {
    let schema = schema_documents()
        .into_iter()
        .find(|(path, _)| path == Path::new("schemas/semantics.json"))
        .unwrap()
        .1;

    assert_eq!(schema["$schema"], DRAFT_2020_12_DIALECT);
    assert_eq!(
        schema["required"],
        json!(["objective", "validity", "one_shot_completion"])
    );
}
