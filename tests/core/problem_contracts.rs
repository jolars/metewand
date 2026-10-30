use std::path::{Path, PathBuf};

use crate::core::{
    problem_contract::{
        ProblemContractError, ProblemSchemaRole, ScientificBudget, parse_problem_contract,
        parse_problem_contract_document, validate_problem_contract,
    },
    schema::{DRAFT_2020_12_DIALECT, SchemaCatalog, SchemaValidationError},
};
use serde_json::{Value, json};

const COMPLETE_CONTRACT: &str = include_str!("../../fixtures/problem-contract/v1/complete.yaml");

fn schema_documents() -> Vec<(PathBuf, Value)> {
    [
        (
            "parameters",
            include_str!("../../fixtures/problem-contract/v1/schemas/parameters.json"),
        ),
        (
            "dataset",
            include_str!("../../fixtures/problem-contract/v1/schemas/dataset.json"),
        ),
        (
            "result",
            include_str!("../../fixtures/problem-contract/v1/schemas/result.json"),
        ),
        (
            "metrics",
            include_str!("../../fixtures/problem-contract/v1/schemas/metrics.json"),
        ),
        (
            "semantics",
            include_str!("../../fixtures/problem-contract/v1/schemas/semantics.json"),
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
fn scientific_rule_groups_are_optional_but_explicit_values_must_be_objects() {
    let mut source: Value = serde_saphyr::from_str(COMPLETE_CONTRACT).unwrap();
    source.as_object_mut().unwrap().remove("semantics-schema");
    source["semantics"]["preprocessing"] = json!({"allowed_outside_timed_scope": []});
    let path = Path::new("problems/regression.yaml");
    let catalog = SchemaCatalog::try_new(
        schema_documents()
            .into_iter()
            .filter(|(path, _)| path != Path::new("schemas/semantics.json")),
    )
    .unwrap();
    parse_problem_contract(path, &source.to_string(), &catalog).unwrap();
    for group in ["preprocessing", "validity", "one_shot_completion"] {
        for replacement in [None, Some(Value::Null), Some(json!([]))] {
            let mut invalid = source.clone();
            let rules = invalid["semantics"].as_object_mut().unwrap();
            let omitted = replacement.is_none();
            match replacement {
                None => {
                    rules.remove(group);
                }
                Some(value) => {
                    rules.insert(group.to_owned(), value);
                }
            }
            if omitted {
                let contract =
                    parse_problem_contract(path, &invalid.to_string(), &catalog).unwrap();
                assert_eq!(
                    contract.semantics.as_canonical().as_json()[group],
                    json!({})
                );
            } else {
                let error =
                    parse_problem_contract(path, &invalid.to_string(), &catalog).unwrap_err();
                assert!(error.to_string().contains(group), "{error}");
            }
        }
    }
    source["semantics-schema"] = Value::Null;
    assert!(parse_problem_contract(path, &source.to_string(), &catalog).is_err());
}

#[test]
fn parses_and_validates_a_complete_problem_contract() {
    let contract = parse_problem_contract(
        Path::new("problems/regression.yaml"),
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
    let path = Path::new("problems/regression.yaml");
    let contract = parse_problem_contract_document(path, COMPLETE_CONTRACT).unwrap();
    assert_eq!(
        contract.semantics_schema.as_ref().unwrap().as_path(),
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
            "dataset-schemas: [\"schemas/dataset.json\"]",
            "dataset-schemas: []",
        )
        .replace("    dataset: \"fixtures/small/dataset\"\n", "");

    let contract = parse_problem_contract(
        Path::new("problems/regression.yaml"),
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
        "family: \"optimization\"",
        "family: \"optimization\"\nunexpected: true",
    );

    let error = parse_problem_contract(
        Path::new("bench/problems/regression.yaml"),
        &source,
        &schema_catalog(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("unknown field `unexpected`"));
    let span = error
        .source_span()
        .expect("parse errors must retain a span");
    assert_eq!(span.path, Path::new("bench/problems/regression.yaml"));
    assert_eq!(span.line, 4);
}

#[test]
fn declared_family_schemas_can_require_scientific_rules() {
    for heading in [
        "  validity:\n    nonfinite: \"reject\"\n",
        "  one_shot_completion:\n    maximum_error: 1e-8\n",
    ] {
        let source = COMPLETE_CONTRACT.replace(heading, "");
        let error = parse_problem_contract(
            Path::new("problems/regression.yaml"),
            &source,
            &schema_catalog(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ProblemContractError::InvalidSemantics { .. }
        ));
    }
}

#[test]
fn declared_family_schemas_can_reject_empty_rule_objects() {
    for assignment in [
        "  validity:\n    nonfinite: \"reject\"",
        "  one_shot_completion:\n    maximum_error: 1e-8",
    ] {
        let heading = assignment.lines().next().unwrap();
        let source = COMPLETE_CONTRACT.replace(assignment, &format!("{heading} {{}}"));
        let error = parse_problem_contract(
            Path::new("problems/regression.yaml"),
            &source,
            &schema_catalog(),
        )
        .unwrap_err();

        assert!(
            matches!(error, ProblemContractError::InvalidSemantics { .. }),
            "{error}"
        );
    }
}

#[test]
fn validates_family_specific_semantics_against_its_declared_schema() {
    let source = COMPLETE_CONTRACT.replace("    nonfinite: \"reject\"", "    nonfinite: \"allow\"");
    let error = parse_problem_contract(
        Path::new("problems/regression.yaml"),
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
            Path::new("problems/regression.yaml"),
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
        ("version: 1\n", "version: 2\n"),
        (
            "dataset-schemas: [\"schemas/dataset.json\"]",
            "dataset-schemas: [\"schemas/dataset.json\", \"schemas/dataset.json\"]",
        ),
        (
            "allowed-timing-scopes: [\"cold_end_to_end\", \"prepare_and_execute\"]",
            "allowed-timing-scopes: []",
        ),
        ("supported-budgets: [\"none\"]", "supported-budgets: []"),
    ];

    for (from, to) in mutations {
        let source = COMPLETE_CONTRACT.replace(from, to);
        let error = parse_problem_contract(
            Path::new("problems/regression.yaml"),
            &source,
            &schema_catalog(),
        )
        .expect_err("invalid common envelope must be rejected");
        assert!(error.source_span().is_some(), "{error}");
    }
}

#[test]
fn requires_reference_case_datasets_exactly_when_the_contract_uses_datasets() {
    let missing_dataset =
        COMPLETE_CONTRACT.replace("    dataset: \"fixtures/small/dataset\"\n", "");
    let error = parse_problem_contract(
        Path::new("problems/regression.yaml"),
        &missing_dataset,
        &schema_catalog(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("requires `dataset`"), "{error}");

    let dataset_free_with_dataset = COMPLETE_CONTRACT.replace(
        "dataset-schemas: [\"schemas/dataset.json\"]",
        "dataset-schemas: []",
    );
    let error = parse_problem_contract(
        Path::new("problems/regression.yaml"),
        &dataset_free_with_dataset,
        &schema_catalog(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("must omit `dataset`"), "{error}");
}

#[test]
fn rejects_contract_values_outside_the_canonical_domain() {
    let source = COMPLETE_CONTRACT.replace("    maximum_error: 1e-8", "    maximum_error: .nan");
    let error = parse_problem_contract(
        Path::new("problems/regression.yaml"),
        &source,
        &schema_catalog(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("non-finite"));
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
