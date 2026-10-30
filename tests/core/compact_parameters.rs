use std::path::Path;

use crate::core::{
    manifest::{Manifest, parse_manifest},
    manifest_hash::canonical_manifest_bytes,
    parameters::resolve_parameters,
    public_schemas::{PublicSchema, public_schema_catalog},
    schema::{DRAFT_2020_12_DIALECT, SchemaCatalog},
};
use serde_json::{Value, json};

fn document(parameters: Value) -> Value {
    json!({
        "version": 1, "name": "compact",
        "environments": {"local": {"kind": "local"}},
        "problems": {"sum": {
            "contract": "problem.yaml",
            "evaluator": {"runner": "command", "program": "evaluate", "environment": "local"}
        }},
        "implementations": {"solver": {
            "runner": "command", "program": "solve", "environment": "local",
            "problem-contracts": ["sum"], "capabilities": ["one_shot"],
            "parameters": parameters
        }}
    })
}

fn parse(document: &Value) -> Manifest {
    parse_manifest(Path::new("bench/metewand.yaml"), &document.to_string()).unwrap()
}

fn resolve(manifest: &Manifest, supplied: Value) -> Result<Value, String> {
    let implementation = manifest.implementations.values().next().unwrap();
    let schema = implementation.parameter_schema.as_ref().unwrap();
    let catalog = SchemaCatalog::try_new([(
        schema.as_path().to_path_buf(),
        schema.inline_document().unwrap().as_json().clone(),
    )])
    .unwrap();
    resolve_parameters(
        &catalog,
        schema.as_path(),
        implementation.parameter_defaults.as_ref(),
        &supplied.try_into().unwrap(),
    )
    .map(|value| value.as_json().clone())
    .map_err(|error| error.to_string())
}

#[test]
fn compact_parameters_lower_to_existing_schema_and_defaults() {
    let compact = document(json!({
        "iterations": {"type": "integer", "minimum": 1, "default": 1000},
        "mode": {"type": "string", "enum": ["a", "b"], "default": "a"},
        "trace": {"type": "boolean", "optional": true},
        "weights": {"type": "array", "items": {"type": "number"}, "minItems": 1, "default": [1, 2]}
    }));
    let mut explicit = compact.clone();
    let implementation = explicit["implementations"]["solver"]
        .as_object_mut()
        .unwrap();
    implementation.remove("parameters");
    implementation.insert(
        "parameter-schema".into(),
        json!({
            "type": "object",
            "properties": {
                "iterations": {"type": "integer", "minimum": 1},
                "mode": {"type": "string", "enum": ["a", "b"]},
                "trace": {"type": "boolean"},
                "weights": {"type": "array", "items": {"type": "number"}, "minItems": 1}
            },
            "required": ["iterations", "mode", "weights"],
            "additionalProperties": false
        }),
    );
    implementation.insert(
        "parameter-defaults".into(),
        json!({"iterations": 1000, "mode": "a", "weights": [1, 2]}),
    );
    let compact = parse(&compact);
    let explicit = parse(&explicit);
    assert_eq!(
        canonical_manifest_bytes(&compact).unwrap(),
        canonical_manifest_bytes(&explicit).unwrap()
    );
    assert_eq!(
        resolve(&compact, json!({})).unwrap(),
        json!({"iterations": 1000, "mode": "a", "weights": [1, 2]})
    );
    assert_eq!(
        resolve(&compact, json!({"iterations": 5, "weights": [3]})).unwrap(),
        resolve(&explicit, json!({"iterations": 5, "weights": [3]})).unwrap()
    );
    for invalid in [
        json!({"iterations": 0}),
        json!({"mode": "c"}),
        json!({"weights": []}),
        json!({"weights": ["bad"]}),
        json!({"unknown": true}),
    ] {
        assert!(resolve(&compact, invalid).is_err());
    }
}

#[test]
fn shorthand_types_require_values_and_preserve_explicit_null_defaults() {
    let manifest = parse(&document(json!({
        "answer": "number", "nothing": {"type": "null", "default": null},
        "label": {"type": "string", "optional": true}
    })));
    assert!(resolve(&manifest, json!({})).is_err());
    assert_eq!(
        resolve(&manifest, json!({"answer": 2})).unwrap(),
        json!({"answer": 2, "nothing": null})
    );
}

#[test]
fn supplied_value_errors_report_the_parameter_and_constraint() {
    let manifest = parse(&document(
        json!({"iterations": {"type": "integer", "minimum": 1}}),
    ));
    let error = resolve(&manifest, json!({"iterations": 0})).unwrap_err();
    assert!(error.contains("/iterations"), "{error}");
    assert!(
        error.contains("minimum") || error.contains("less than"),
        "{error}"
    );
    assert!(!error.contains(".metewand-inline-"), "{error}");
}

#[test]
fn block_and_flow_declarations_normalize_identically() {
    let flow = document(json!({"iterations": {"type": "integer", "minimum": 1, "default": 10}}))
        .to_string();
    let block = "version: 1\nname: compact\nenvironments:\n  local: {kind: local}\nproblems:\n  sum:\n    contract: problem.yaml\n    evaluator: {runner: command, program: evaluate, environment: local}\nimplementations:\n  solver:\n    runner: command\n    program: solve\n    environment: local\n    problem-contracts: [sum]\n    capabilities: [one_shot]\n    parameters:\n      iterations:\n        type: integer\n        minimum: 1\n        default: 10\n";
    let parse = |source| parse_manifest(Path::new("bench/metewand.yaml"), source).unwrap();
    assert_eq!(
        canonical_manifest_bytes(&parse(&flow)).unwrap(),
        canonical_manifest_bytes(&parse(block)).unwrap()
    );
}

#[test]
fn invalid_compact_fields_name_the_authored_parameter() {
    for field in [
        json!({"minimum": 0}),
        json!({"type": "object"}),
        json!({"type": "integer", "minimum": 1, "default": 0}),
        json!({"type": "integer", "default": 1.5}),
        json!({"type": "number", "minimum": "zero"}),
        json!({"type": "string", "minimum": 0}),
        json!({"type": "string", "enum": []}),
        json!({"type": "number", "grid": []}),
        json!({"type": "array"}),
        json!({"type": "array", "items": {"type": "number", "default": 1}}),
        json!({"type": "array", "items": "integer", "default": [1, "bad"]}),
        json!({"type": "boolean", "optional": "yes"}),
    ] {
        let source = document(json!({"tolerance": field})).to_string();
        let error = parse_manifest(Path::new("bench/metewand.yaml"), &source).unwrap_err();
        assert!(
            error.message().contains("parameters.tolerance"),
            "{field}: {error}"
        );
        assert_eq!(
            error.source_span().unwrap().path,
            Path::new("bench/metewand.yaml")
        );
    }
}

#[test]
fn compact_and_explicit_parameter_declarations_cannot_compete() {
    for (key, value) in [
        ("parameter-schema", json!({"type": "object"})),
        ("parameter-defaults", json!({})),
    ] {
        let mut value_document = document(json!({}));
        value_document["implementations"]["solver"][key] = value;
        let error =
            parse_manifest(Path::new("metewand.yaml"), &value_document.to_string()).unwrap_err();
        assert!(
            error.message().contains("parameters") && error.message().contains(key),
            "{error}"
        );
    }
}

#[test]
fn empty_compact_parameters_are_closed_without_invented_defaults() {
    let manifest = parse(&document(json!({})));
    assert_eq!(resolve(&manifest, json!({})).unwrap(), json!({}));
    assert!(resolve(&manifest, json!({"surprise": 1})).is_err());
    let implementation = manifest.implementations.values().next().unwrap();
    assert!(implementation.parameter_defaults.is_none());
    assert_eq!(
        implementation
            .parameter_schema
            .as_ref()
            .unwrap()
            .inline_document()
            .unwrap()
            .as_json()["$schema"],
        DRAFT_2020_12_DIALECT
    );
}

#[test]
fn public_manifest_schema_describes_compact_parameter_syntax() {
    let catalog = public_schema_catalog().unwrap();
    for parameters in [
        json!({}),
        json!({"tolerance": "number"}),
        json!({"iterations": {"type": "integer", "minimum": 1, "default": 10}}),
        json!({"iterations": {"type": "integer", "minimum": 1, "grid": [1, 2]}}),
    ] {
        catalog
            .validate(
                PublicSchema::Manifest.repository_path(),
                &document(parameters),
            )
            .unwrap();
    }
    for field in [
        json!({"default": 1}),
        json!({"type": "object"}),
        json!({"type": "number", "grid": []}),
        json!({"type": "array"}),
    ] {
        assert!(
            catalog
                .validate(
                    PublicSchema::Manifest.repository_path(),
                    &document(json!({"test": field}))
                )
                .is_err()
        );
    }
}
