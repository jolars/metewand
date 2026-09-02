use std::path::{Path, PathBuf};

use metewand_core::{
    canonical::CanonicalValue,
    parameters::{ParameterResolutionError, resolve_parameters},
    schema::{DRAFT_2020_12_DIALECT, SchemaCatalog, SchemaCatalogError, SchemaValidationError},
};
use serde_json::{Value, json};

const ROOT_SCHEMA: &str = include_str!("../../../fixtures/schema-validation/v1/root.json");
const DEFINITIONS_SCHEMA: &str =
    include_str!("../../../fixtures/schema-validation/v1/definitions.json");
const PARAMETERS_SCHEMA: &str =
    include_str!("../../../fixtures/schema-validation/v1/parameters.json");

fn parse_schema(source: &str) -> Value {
    serde_json::from_str(source).expect("fixture must be valid JSON")
}

fn schema_catalog() -> SchemaCatalog {
    SchemaCatalog::try_new([
        (
            PathBuf::from("schemas/root.json"),
            parse_schema(ROOT_SCHEMA),
        ),
        (
            PathBuf::from("schemas/definitions.json"),
            parse_schema(DEFINITIONS_SCHEMA),
        ),
        (
            PathBuf::from("schemas/parameters.json"),
            parse_schema(PARAMETERS_SCHEMA),
        ),
    ])
    .expect("fixtures must form a valid catalog")
}

fn canonical(value: Value) -> CanonicalValue {
    CanonicalValue::try_from(value).expect("test input must be canonical JSON")
}

#[test]
fn validates_against_repository_local_external_references() {
    let catalog = schema_catalog();

    catalog
        .validate(Path::new("schemas/root.json"), &json!({"mode": "fast"}))
        .unwrap();

    let error = catalog
        .validate(
            Path::new("schemas/root.json"),
            &json!({"mode": "approximate"}),
        )
        .unwrap_err();
    let SchemaValidationError::InvalidInstance { violations, .. } = error else {
        panic!("expected instance validation errors");
    };
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].instance_path, "/mode");
    assert!(!violations[0].schema_path.is_empty());
    assert!(!violations[0].message.is_empty());
}

#[test]
fn reports_all_violations_in_deterministic_order() {
    let catalog = schema_catalog();
    let error = catalog
        .validate(
            Path::new("schemas/parameters.json"),
            &json!({"iterations": 0, "unexpected": true}),
        )
        .unwrap_err();
    let SchemaValidationError::InvalidInstance { violations, .. } = error else {
        panic!("expected instance validation errors");
    };

    assert!(violations.len() > 1);
    assert!(violations.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn rejects_unregistered_and_network_references_offline() {
    for reference in ["missing.json", "https://example.invalid/schema.json"] {
        let schema = json!({
            "$schema": DRAFT_2020_12_DIALECT,
            "$ref": reference,
        });
        let error = SchemaCatalog::try_new([(PathBuf::from("schemas/root.json"), schema)])
            .expect_err("unregistered references must fail catalog construction");
        assert!(matches!(error, SchemaCatalogError::Compilation { .. }));
    }
}

#[test]
fn requires_explicit_supported_dialects_and_valid_schemas() {
    let missing = SchemaCatalog::try_new([(
        PathBuf::from("schemas/missing.json"),
        json!({"type": "string"}),
    )])
    .unwrap_err();
    assert!(matches!(missing, SchemaCatalogError::MissingDialect { .. }));

    let unsupported = SchemaCatalog::try_new([(
        PathBuf::from("schemas/old.json"),
        json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "string",
        }),
    )])
    .unwrap_err();
    assert!(matches!(
        unsupported,
        SchemaCatalogError::UnsupportedDialect { .. }
    ));

    let malformed = SchemaCatalog::try_new([(
        PathBuf::from("schemas/malformed.json"),
        json!({
            "$schema": DRAFT_2020_12_DIALECT,
            "type": 42,
        }),
    )])
    .unwrap_err();
    assert!(matches!(
        malformed,
        SchemaCatalogError::InvalidSchema { .. }
    ));
}

#[test]
fn rejects_invalid_or_duplicate_repository_paths() {
    for path in [
        "/absolute.json",
        "schemas/../escape.json",
        "schemas/./root.json",
    ] {
        let error = SchemaCatalog::try_new([(
            PathBuf::from(path),
            json!({"$schema": DRAFT_2020_12_DIALECT}),
        )])
        .unwrap_err();
        assert!(matches!(error, SchemaCatalogError::InvalidPath { .. }));
    }

    let error = SchemaCatalog::try_new([
        (
            PathBuf::from("schemas/root.json"),
            json!({"$schema": DRAFT_2020_12_DIALECT}),
        ),
        (
            PathBuf::from("schemas/root.json"),
            json!({"$schema": DRAFT_2020_12_DIALECT}),
        ),
    ])
    .unwrap_err();
    assert!(matches!(error, SchemaCatalogError::DuplicatePath { .. }));
}

#[test]
fn reports_an_unknown_validation_schema() {
    let error = schema_catalog()
        .validate(Path::new("schemas/unknown.json"), &Value::Null)
        .unwrap_err();
    assert!(matches!(error, SchemaValidationError::UnknownSchema { .. }));
}

#[test]
fn merges_defaults_recursively_before_validation() {
    let catalog = schema_catalog();
    let defaults = canonical(json!({
        "iterations": 10,
        "nested": {"alpha": 1, "beta": 2},
        "literal_array": [1, 2],
        "table": {"left": {"x": 1}, "right": true},
        "nullable": "fallback"
    }));
    let supplied = canonical(json!({
        "nested": {"beta": 20, "gamma": 30},
        "literal_array": [[3]],
        "table": {"left": {"y": 2}},
        "nullable": null
    }));

    let resolved = resolve_parameters(
        &catalog,
        Path::new("schemas/parameters.json"),
        Some(&defaults),
        &supplied,
    )
    .unwrap();

    assert_eq!(
        resolved.to_canonical_bytes().unwrap(),
        br#"{"iterations":10,"literal_array":[[3]],"nested":{"alpha":1,"beta":20,"gamma":30},"nullable":null,"table":{"left":{"x":1,"y":2},"right":true}}"#
    );
}

#[test]
fn schema_defaults_are_annotations_only() {
    let catalog = schema_catalog();
    let supplied = canonical(json!({
        "iterations": 5,
        "nested": {"alpha": 1, "beta": 2},
        "literal_array": [],
        "table": {},
        "nullable": null
    }));

    let resolved = resolve_parameters(
        &catalog,
        Path::new("schemas/parameters.json"),
        None,
        &supplied,
    )
    .unwrap();

    assert_eq!(resolved, supplied);
    assert!(resolved.as_json().get("annotated_default").is_none());
}

#[test]
fn supplied_values_replace_defaults_and_are_then_validated() {
    let catalog = schema_catalog();
    let defaults = canonical(json!({
        "iterations": 10,
        "nested": {"alpha": 1, "beta": 2},
        "literal_array": [],
        "table": {},
        "nullable": null
    }));
    let supplied = canonical(json!({"iterations": 0}));

    let error = resolve_parameters(
        &catalog,
        Path::new("schemas/parameters.json"),
        Some(&defaults),
        &supplied,
    )
    .unwrap_err();

    assert!(matches!(error, ParameterResolutionError::Validation(_)));
}

#[test]
fn parameter_roots_must_be_objects() {
    let catalog = schema_catalog();
    let object = canonical(json!({}));

    let defaults_error = resolve_parameters(
        &catalog,
        Path::new("schemas/parameters.json"),
        Some(&canonical(json!([]))),
        &object,
    )
    .unwrap_err();
    assert!(matches!(
        defaults_error,
        ParameterResolutionError::DefaultsMustBeObject
    ));

    let supplied_error = resolve_parameters(
        &catalog,
        Path::new("schemas/parameters.json"),
        None,
        &canonical(Value::Null),
    )
    .unwrap_err();
    assert!(matches!(
        supplied_error,
        ParameterResolutionError::ParametersMustBeObject
    ));
}
