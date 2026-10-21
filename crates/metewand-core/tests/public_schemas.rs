use std::collections::BTreeSet;

use metewand_core::{
    PUBLIC_SCHEMA_VERSION,
    canonical::CanonicalValue,
    public_schemas::{PUBLIC_SCHEMAS, PublicSchema, public_schema_catalog},
    schema::{DRAFT_2020_12_DIALECT, SchemaValidationError},
};
use serde::Deserialize;
use serde_json::Value;

const CONFORMANCE_CASES: &str = include_str!("../../../schemas/v1/fixtures/conformance.json");

#[test]
fn concurrency_schema_rejects_bounds_outside_the_canonical_integer_domain() {
    let catalog = public_schema_catalog().unwrap();
    let instance = serde_json::json!({
        "version": 1, "name": "concurrency",
        "execution_policies": {"local": {"version": 1, "max_concurrency": 9_007_199_254_740_992_u64}}
    });
    assert!(
        catalog
            .validate(PublicSchema::Manifest.repository_path(), &instance)
            .is_err()
    );
}

#[derive(Debug, Deserialize)]
struct ConformanceCases {
    version: u32,
    valid: Vec<ConformanceCase>,
    invalid: Vec<ConformanceCase>,
}

#[derive(Debug, Deserialize)]
struct ConformanceCase {
    name: String,
    schema: String,
    instance: Value,
}

#[test]
fn publishes_the_complete_version_one_schema_set() {
    assert_eq!(PUBLIC_SCHEMAS.len(), 24);

    let mut slugs = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut ids = BTreeSet::new();

    for schema in PUBLIC_SCHEMAS {
        assert!(slugs.insert(schema.slug()));
        assert!(paths.insert(schema.repository_path()));
        assert!(ids.insert(schema.id()));

        let document =
            CanonicalValue::from_slice(schema.source().as_bytes()).unwrap_or_else(|error| {
                panic!("schema `{}` is not canonical JSON: {error}", schema.slug())
            });
        assert_eq!(document.as_json()["$schema"], DRAFT_2020_12_DIALECT);
        assert_eq!(document.as_json()["$id"], schema.id());
        assert_eq!(
            document.as_json()["x-metewand-compatibility-version"],
            PUBLIC_SCHEMA_VERSION
        );
    }

    public_schema_catalog().expect("published schemas must compile as an offline catalog");
}

#[test]
fn accepts_all_version_one_conformance_cases() {
    let cases = conformance_cases();
    let catalog = public_schema_catalog().expect("published schemas must compile");
    let covered = cases
        .valid
        .iter()
        .map(|case| case.schema.as_str())
        .collect::<BTreeSet<_>>();
    let expected = PUBLIC_SCHEMAS
        .iter()
        .map(|schema| schema.slug())
        .collect::<BTreeSet<_>>();
    assert_eq!(covered, expected, "every public schema needs a valid case");

    for case in cases.valid {
        let schema = PublicSchema::from_slug(&case.schema)
            .unwrap_or_else(|| panic!("unknown schema slug `{}`", case.schema));
        catalog
            .validate(schema.repository_path().as_ref(), &case.instance)
            .unwrap_or_else(|error| panic!("valid case `{}` failed: {error}", case.name));
    }
}

#[test]
fn rejects_all_version_one_negative_cases() {
    let cases = conformance_cases();
    let catalog = public_schema_catalog().expect("published schemas must compile");
    let covered = cases
        .invalid
        .iter()
        .map(|case| case.schema.as_str())
        .collect::<BTreeSet<_>>();
    let expected = PUBLIC_SCHEMAS
        .iter()
        .map(|schema| schema.slug())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        covered, expected,
        "every public schema needs an invalid case"
    );

    for case in cases.invalid {
        let schema = PublicSchema::from_slug(&case.schema)
            .unwrap_or_else(|| panic!("unknown schema slug `{}`", case.schema));
        let error = catalog
            .validate(schema.repository_path().as_ref(), &case.instance)
            .unwrap_err();
        assert!(
            matches!(error, SchemaValidationError::InvalidInstance { .. }),
            "invalid case `{}` produced the wrong error: {error}",
            case.name
        );
    }
}

fn conformance_cases() -> ConformanceCases {
    let canonical = CanonicalValue::from_slice(CONFORMANCE_CASES.as_bytes())
        .expect("conformance cases must use canonical JSON values");
    let cases: ConformanceCases = serde_json::from_value(canonical.as_json().clone())
        .expect("conformance cases must have the expected shape");
    assert_eq!(cases.version, PUBLIC_SCHEMA_VERSION);
    cases
}

#[test]
fn measurement_scope_versions_preserve_existing_records() {
    let cases = conformance_cases();
    let template = cases
        .valid
        .into_iter()
        .find(|case| case.name == "observation-with-explicit-measurement-states")
        .unwrap()
        .instance;
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../schemas/v1/fixtures/measurement-scopes.json"
    ))
    .unwrap();
    assert_eq!(vectors["version"], 1);
    let catalog = public_schema_catalog().unwrap();
    for (kind, valid) in [("valid", true), ("invalid", false)] {
        for vector in vectors[kind].as_array().unwrap() {
            let mut instance = template.clone();
            let measured = &mut instance["provenance"]["data"]["measurements"];
            measured["version"] = vector["version"].clone();
            measured["cpu_time_scope"] = vector["cpu_time_scope"].clone();
            assert_eq!(
                catalog
                    .validate(PublicSchema::Observation.repository_path(), &instance)
                    .is_ok(),
                valid,
                "{vector}"
            );
        }
    }
}
