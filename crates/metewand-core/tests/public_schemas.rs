use std::{collections::BTreeSet, path::Path};

use metewand_core::{
    PUBLIC_SCHEMA_VERSION,
    canonical::CanonicalValue,
    manifest::{ObservationPolicyDefinition, parse_manifest},
    problem_contract::parse_problem_contract_document,
    public_schemas::{PUBLIC_SCHEMAS, PublicSchema, public_schema_catalog},
    schema::{DRAFT_2020_12_DIALECT, SchemaValidationError},
};
use serde::Deserialize;
use serde_json::Value;

const CONFORMANCE_CASES: &str = include_str!("../../../schemas/v1/fixtures/conformance.json");

#[test]
fn native_path_boundaries_accept_windows_and_preserve_unix_records() {
    let catalog = public_schema_catalog().unwrap();
    for directory in [
        "/worker",
        r"C:\worker space\data",
        r"\\?\C:\worker\data",
        r"\\host\share\data",
    ] {
        let unix = serde_json::json!({"version": 1, "working_directory": directory,
            "environment_variables": {}, "umask": "0077"});
        catalog
            .validate(PublicSchema::WorkerEnvironment.repository_path(), &unix)
            .unwrap();
        let windows = serde_json::json!({"version": 2, "working_directory": directory,
            "environment_variables": {}, "umask": null,
            "directory_access": "current_user_and_system"});
        catalog
            .validate(PublicSchema::WorkerEnvironment.repository_path(), &windows)
            .unwrap();
    }
    for directory in ["worker", r"C:worker", r"\worker", ""] {
        let instance = serde_json::json!({"version": 2, "working_directory": directory,
            "environment_variables": {}, "umask": null,
            "directory_access": "current_user_and_system"});
        assert!(
            catalog
                .validate(PublicSchema::WorkerEnvironment.repository_path(), &instance)
                .is_err()
        );
    }
}

#[test]
fn concurrency_schema_rejects_bounds_outside_the_canonical_integer_domain() {
    let catalog = public_schema_catalog().unwrap();
    let instance = serde_json::json!({
        "version": 1, "name": "concurrency",
        "execution-policies": {"local": {"version": 1, "max-concurrency": 9_007_199_254_740_992_u64}}
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
    assert_eq!(PUBLIC_SCHEMAS.len(), 31);

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
            .expect_err(&format!("negative case {} was accepted", case.name));
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
            measured
                .as_object_mut()
                .unwrap()
                .extend(vector.as_object().unwrap().clone());
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

#[derive(Deserialize)]
struct Gate3Extensions {
    version: u32,
    extensions: Vec<Gate3Extension>,
}

#[derive(Deserialize)]
struct Gate3Extension {
    #[serde(default)]
    valid: bool,
    name: String,
    template: String,
    pointer: String,
    field: String,
    value: Value,
}

fn typed_document_is_valid(schema: PublicSchema, instance: &Value) -> bool {
    let source = serde_json::to_string(instance).unwrap();
    match schema {
        PublicSchema::Manifest => parse_manifest(Path::new("metewand.yaml"), &source).is_ok(),
        PublicSchema::ProblemContract => {
            parse_problem_contract_document(Path::new("problem.yaml"), &source).is_ok()
        }
        PublicSchema::OneShotObservationPolicy => {
            serde_json::from_value::<ObservationPolicyDefinition>(instance.clone()).is_ok()
        }
        _ => panic!("uncovered typed document: {schema:?}"),
    }
}

#[test]
fn gate3_extensions_require_new_contracts_in_both_schemas_and_typed_parsers() {
    let cases = conformance_cases();
    let vectors: Gate3Extensions = serde_json::from_str(include_str!(
        "../../../schemas/v1/fixtures/gate3-extensions.json"
    ))
    .unwrap();
    assert_eq!(vectors.version, PUBLIC_SCHEMA_VERSION);
    let catalog = public_schema_catalog().unwrap();

    for vector in vectors.extensions {
        let template = cases
            .valid
            .iter()
            .find(|case| case.name == vector.template)
            .unwrap();
        let schema = PublicSchema::from_slug(&template.schema).unwrap();
        catalog
            .validate(schema.repository_path(), &template.instance)
            .unwrap();
        assert!(typed_document_is_valid(schema, &template.instance));

        let mut instance = template.instance.clone();
        instance
            .pointer_mut(&vector.pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(vector.field, vector.value);
        assert_eq!(
            catalog
                .validate(schema.repository_path(), &instance)
                .is_ok(),
            vector.valid,
            "schema disagreed for {}",
            vector.name
        );
        assert_eq!(
            typed_document_is_valid(schema, &instance),
            vector.valid,
            "typed parser disagreed for {}",
            vector.name
        );
    }
}

#[test]
fn worker_schema_matches_the_gate3_protocol_compatibility_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../fixtures/worker-protocol/v1/compatibility.json"
    ))
    .unwrap();
    assert_eq!(vectors["version"], PUBLIC_SCHEMA_VERSION);
    let catalog = public_schema_catalog().unwrap();
    for vector in vectors["messages"].as_array().unwrap() {
        let message: Value = serde_json::from_str(vector["frame"].as_str().unwrap()).unwrap();
        assert_eq!(
            catalog
                .validate(
                    PublicSchema::WorkerProtocolMessage.repository_path(),
                    &message
                )
                .is_ok(),
            vector["valid"].as_bool().unwrap(),
            "{}",
            vector["name"]
        );
    }
}

#[test]
fn applicability_schema_accepts_only_checked_outcomes_and_exact_input_shapes() {
    let catalog = public_schema_catalog().unwrap();
    let schema = PublicSchema::WorkerProtocolMessage.repository_path();
    let request = serde_json::json!({"id":"1","method":"check_applicability",
        "dataset_dir":"/private/data","dataset_manifest":"dataset-manifest.json",
        "problem_contract_id":"contract","problem_parameters":{"intercept":true},
        "implementation_parameters":{}});
    catalog.validate(schema, &request).unwrap();
    let response = serde_json::json!({"id":"1","ok":true,"outcome":"not_applicable",
        "code":"unsupported_intercept","reason":"Only zero intercepts are supported."});
    catalog.validate(schema, &response).unwrap();
    let mut applicable = response.clone();
    applicable["outcome"] = serde_json::json!("applicable");
    applicable["details"] = serde_json::json!({"intercept":true});
    catalog.validate(schema, &applicable).unwrap();
    for (field, value) in [
        ("outcome", serde_json::json!("unchecked")),
        ("code", serde_json::json!("")),
        ("reason", serde_json::json!("")),
        ("details", serde_json::json!([])),
    ] {
        let mut invalid = response.clone();
        invalid[field] = value;
        assert!(catalog.validate(schema, &invalid).is_err());
    }
    for field in [
        "dataset_dir",
        "dataset_manifest",
        "problem_contract_id",
        "problem_parameters",
        "implementation_parameters",
    ] {
        let mut invalid = request.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(catalog.validate(schema, &invalid).is_err());
    }
}
