use std::{fs, path::Path};

use metewand_core::schema::DRAFT_2020_12_DIALECT;
use metewand_runtime::repository::{CheckedRepository, RepositoryCheckError, check_repository};
use serde_json::{Value, json};
use tempfile::TempDir;

fn contract() -> Value {
    json!({
        "version": 1,
        "name": "sum",
        "family": "arithmetic",
        "parameter-schema": {"type": "object", "additionalProperties": false},
        "dataset-schemas": [{"type": "object"}],
        "result-schema": {"type": "object"},
        "metric-schema": {"type": "object"},
        "semantics-schema": {"type": "object"},
        "allowed-timing-scopes": ["prepare_and_execute"],
        "supported-budgets": ["none"],
        "semantics": {
            "preprocessing": {"allowed_outside_timed_scope": []},
            "validity": {"finite": true},
            "one_shot_completion": {"exact": true}
        },
        "reference-cases": [{
            "dataset": "data.json", "result": "result.json", "expected-metrics": "metrics.json"
        }]
    })
}

fn manifest() -> Value {
    json!({
        "version": 1, "name": "inline",
        "datasets": {"numbers": {"output-schema": {"type": "object"}, "sources": ["data.json"]}},
        "problems": {"sum": {"contract": contract(), "evaluator": {
            "runner": "command", "program": "evaluate", "environment": "local"
        }}},
        "implementations": {"sum": {
            "runner": "command", "program": "solve", "environment": "local",
            "parameter-schema": {"$ref": "schemas/parameters.json"},
            "parameter-defaults": {"iterations": 1},
            "problem-contracts": ["sum"], "capabilities": ["one_shot"]
        }},
        "environments": {"local": {"kind": "local"}},
        "execution-policies": {"local": {"version": 1}},
        "observation-policies": {"final": {"version": 1, "kind": "one_shot"}},
        "experiments": [{
            "name": "main", "problem": "sum", "datasets": ["numbers"], "implementations": ["sum"],
            "execution-policy": "local", "observation-policy": "final",
            "implementation-repetitions": 1, "measurement-repetitions": 1, "seed": 7
        }]
    })
}

fn write(root: &Path, path: &str, value: &Value) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn repository(manifest: &Value) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "metewand.yaml", manifest);
    for path in ["data.json", "result.json", "metrics.json"] {
        write(root.path(), path, &json!({}));
    }
    write(
        root.path(),
        "schemas/parameters.json",
        &json!({
            "$schema": DRAFT_2020_12_DIALECT,
            "type": "object", "properties": {"iterations": {"type": "integer", "minimum": 1}},
            "additionalProperties": false
        }),
    );
    root
}

fn check(root: &Path) -> CheckedRepository {
    check_repository(&root.join("metewand.yaml")).unwrap_or_else(|error| panic!("{error:?}"))
}

#[test]
fn inline_schemas_and_contracts_use_normalized_contents() {
    let mut manifest = manifest();
    let root = repository(&manifest);
    let inherited = check(root.path());
    assert_eq!(inherited.logical_plan().unwrap().experiments.len(), 1);
    for pointer in [
        "/datasets/numbers/output-schema",
        "/implementations/sum/parameter-schema",
        "/problems/sum/contract/parameter-schema",
        "/problems/sum/contract/dataset-schemas/0",
        "/problems/sum/contract/result-schema",
        "/problems/sum/contract/metric-schema",
        "/problems/sum/contract/semantics-schema",
    ] {
        manifest.pointer_mut(pointer).unwrap()["$schema"] = json!(DRAFT_2020_12_DIALECT);
    }
    write(root.path(), "metewand.yaml", &manifest);
    let explicit = check(root.path());
    assert_eq!(inherited.manifest_hash, explicit.manifest_hash);
    assert_eq!(
        inherited.resources.problem_contracts,
        explicit.resources.problem_contracts
    );
    assert_eq!(
        inherited.planning_catalog.implementation_definitions,
        explicit.planning_catalog.implementation_definitions
    );
    assert!(!root.path().join(".metewand").exists());
    fs::rename(
        root.path().join("metewand.yaml"),
        root.path().join("benchmark.yml"),
    )
    .unwrap();
    let renamed = check_repository(&root.path().join("benchmark.yml")).unwrap();
    assert_eq!(
        explicit.resources.problem_contracts,
        renamed.resources.problem_contracts
    );
    let contract_id = renamed.resources.problem_contracts.values().next().unwrap();
    assert_eq!(
        renamed.resources.records[&contract_id.to_string()].paths,
        ["benchmark.yml"]
    );
    manifest["problems"]["sum"]["contract"]["result-schema"]["additionalProperties"] = json!(false);
    write(root.path(), "benchmark.yml", &manifest);
    let changed = check_repository(&root.path().join("benchmark.yml")).unwrap();
    assert_ne!(renamed.manifest_hash, changed.manifest_hash);
    assert_ne!(
        renamed.resources.problem_contracts,
        changed.resources.problem_contracts
    );
}

#[test]
fn inline_and_referenced_contracts_share_effective_identity() {
    let mut manifest = manifest();
    let root = repository(&manifest);
    let inline = check(root.path());
    // Both documents own their inline schemas in the same repository directory.
    write(
        root.path(),
        "problem.yaml",
        &manifest["problems"]["sum"]["contract"],
    );
    manifest["problems"]["sum"]["contract"] = json!("problem.yaml");
    write(root.path(), "metewand.yaml", &manifest);
    let referenced = check(root.path());
    assert_eq!(
        inline.resources.problem_contracts,
        referenced.resources.problem_contracts
    );
    assert_eq!(
        inline.planning_catalog.problem_definitions,
        referenced.planning_catalog.problem_definitions
    );
}

fn compact_manifest() -> Value {
    let mut manifest = manifest();
    manifest["datasets"]["numbers"] = json!({
        "runner": "command", "program": "generate", "environment": "local",
        "output-schema": {"type": "object"},
        "parameters": {"rows": {"type": "integer", "minimum": 1, "default": 10}}
    });
    let contract = manifest["problems"]["sum"]["contract"]
        .as_object_mut()
        .unwrap();
    for key in ["parameter-schema", "result-schema", "metric-schema"] {
        contract.remove(key);
    }
    contract.insert(
        "parameters".into(),
        json!({"offset": {"type": "integer", "minimum": 0, "default": 2}}),
    );
    contract.insert("result".into(), json!({"answer": "integer"}));
    contract.insert(
        "metrics".into(),
        json!({"error": {"type": "number", "minimum": 0}}),
    );
    let implementation = manifest["implementations"]["sum"].as_object_mut().unwrap();
    implementation.remove("parameter-schema");
    implementation.remove("parameter-defaults");
    implementation.insert(
        "parameters".into(),
        json!({"iterations": {"type": "integer", "minimum": 1, "default": 1}}),
    );
    manifest
}

fn closed_schema(name: &str, field: Value) -> Value {
    json!({"type": "object", "properties": {name: field}, "required": [name], "additionalProperties": false})
}

#[test]
fn compact_boundaries_preserve_effective_resource_and_configuration_identities() {
    let mut manifest = compact_manifest();
    let root = repository(&manifest);
    let compact = check(root.path());

    let dataset = manifest["datasets"]["numbers"].as_object_mut().unwrap();
    dataset.remove("parameters");
    dataset.insert(
        "parameter-schema".into(),
        closed_schema("rows", json!({"type": "integer", "minimum": 1})),
    );
    dataset.insert("parameter-defaults".into(), json!({"rows": 10}));
    manifest["problems"]["sum"]["parameter-defaults"] = json!({"offset": 2});
    let contract = manifest["problems"]["sum"]["contract"]
        .as_object_mut()
        .unwrap();
    for key in ["parameters", "result", "metrics"] {
        contract.remove(key);
    }
    contract.insert(
        "parameter-schema".into(),
        closed_schema("offset", json!({"type": "integer", "minimum": 0})),
    );
    contract.insert(
        "result-schema".into(),
        closed_schema("answer", json!({"type": "integer"})),
    );
    contract.insert(
        "metric-schema".into(),
        closed_schema("error", json!({"type": "number", "minimum": 0})),
    );
    let implementation = manifest["implementations"]["sum"].as_object_mut().unwrap();
    implementation.remove("parameters");
    implementation.insert(
        "parameter-schema".into(),
        closed_schema("iterations", json!({"type": "integer", "minimum": 1})),
    );
    implementation.insert("parameter-defaults".into(), json!({"iterations": 1}));
    write(root.path(), "metewand.yaml", &manifest);
    let explicit = check(root.path());
    assert_eq!(compact.manifest_hash, explicit.manifest_hash);
    assert_eq!(compact.resources.schemas, explicit.resources.schemas);
    assert_eq!(
        compact.resources.problem_contracts,
        explicit.resources.problem_contracts
    );
    assert_eq!(
        compact.planning_catalog.dataset_definitions,
        explicit.planning_catalog.dataset_definitions
    );
    assert_eq!(
        compact.planning_catalog.problem_definitions,
        explicit.planning_catalog.problem_definitions
    );
    assert_eq!(
        compact.planning_catalog.implementation_definitions,
        explicit.planning_catalog.implementation_definitions
    );
    assert_eq!(
        compact.logical_plan().unwrap(),
        explicit.logical_plan().unwrap()
    );
}

#[test]
fn declaration_grids_preserve_definitions_and_match_experiment_axes() {
    for (pointer, axis) in [
        (
            "/datasets/numbers/parameters/rows",
            "/experiments/0/dataset-parameters",
        ),
        (
            "/problems/sum/contract/parameters/offset",
            "/experiments/0/problem-parameters",
        ),
        (
            "/implementations/sum/parameters/iterations",
            "/experiments/0/implementation-parameters",
        ),
    ] {
        let mut manifest = compact_manifest();
        let field = manifest
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap();
        field.remove("default");
        field.insert("grid".into(), json!([1, 2]));
        let root = repository(&manifest);
        let compact = check(root.path());
        let original_plan = compact.logical_plan().unwrap();
        assert_eq!(original_plan.experiments[0].candidates.len(), 2);

        manifest.pointer_mut(pointer).unwrap()["grid"] = json!([1, 2, 3]);
        write(root.path(), "metewand.yaml", &manifest);
        let extended = check(root.path());
        assert_eq!(compact.resources.schemas, extended.resources.schemas);
        assert_eq!(
            compact.resources.problem_contracts,
            extended.resources.problem_contracts
        );
        assert_eq!(
            compact.planning_catalog.dataset_definitions,
            extended.planning_catalog.dataset_definitions
        );
        assert_eq!(
            compact.planning_catalog.problem_definitions,
            extended.planning_catalog.problem_definitions
        );
        assert_eq!(
            compact.planning_catalog.implementation_definitions,
            extended.planning_catalog.implementation_definitions
        );
        let extended_plan = extended.logical_plan().unwrap();
        assert_eq!(extended_plan.experiments[0].candidates.len(), 3);
        assert_eq!(
            original_plan.experiments[0].candidates,
            extended_plan.experiments[0].candidates[..2]
        );

        manifest
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("grid");
        let (_, axis_name) = axis.rsplit_once('/').unwrap();
        manifest["experiments"][0][axis_name] = match axis_name {
            "dataset-parameters" => json!({"numbers": {"rows": {"grid": [1, 2]}}}),
            "problem-parameters" => json!({"offset": {"grid": [1, 2]}}),
            "implementation-parameters" => json!({"sum": {"iterations": {"grid": [1, 2]}}}),
            _ => unreachable!(),
        };
        write(root.path(), "metewand.yaml", &manifest);
        assert_eq!(original_plan, check(root.path()).logical_plan().unwrap());
    }
}

#[test]
fn referenced_compact_contract_defaults_reach_definitions_and_reject_competing_defaults() {
    let mut manifest = compact_manifest();
    let root = repository(&manifest);
    let inline = check(root.path());
    write(
        root.path(),
        "problem.yaml",
        &manifest["problems"]["sum"]["contract"],
    );
    manifest["problems"]["sum"]["contract"] = json!("problem.yaml");
    write(root.path(), "metewand.yaml", &manifest);
    let referenced = check(root.path());
    assert_eq!(
        inline.resources.problem_contracts,
        referenced.resources.problem_contracts
    );
    assert_eq!(
        inline.planning_catalog.problem_definitions,
        referenced.planning_catalog.problem_definitions
    );
    assert_eq!(
        inline.logical_plan().unwrap(),
        referenced.logical_plan().unwrap()
    );
    manifest["problems"]["sum"]["parameter-defaults"] = json!({});
    write(root.path(), "metewand.yaml", &manifest);
    let error = check_repository(&root.path().join("metewand.yaml"))
        .err()
        .unwrap();
    assert!(error.to_string().contains("cannot be combined"), "{error}");
}

#[cfg(unix)]
#[test]
fn worker_checks_detect_changed_selections_in_referenced_contracts() {
    use metewand_runtime::run::check_repository_workers;

    for selection in ["default", "grid"] {
        let mut manifest = compact_manifest();
        let root = repository(&manifest);
        let mut contract = manifest["problems"]["sum"]["contract"].take();
        if selection == "grid" {
            contract["parameters"]["offset"]
                .as_object_mut()
                .unwrap()
                .remove("default");
            contract["parameters"]["offset"]["grid"] = json!([1, 2]);
        }
        write(root.path(), "problem.yaml", &contract);
        manifest["problems"]["sum"]["contract"] = json!("problem.yaml");
        write(root.path(), "metewand.yaml", &manifest);
        let checked = check(root.path());
        contract["parameters"]["offset"][selection] = if selection == "grid" {
            json!([3, 4])
        } else {
            json!(3)
        };
        write(root.path(), "problem.yaml", &contract);
        let error = check_repository_workers(&checked).unwrap_err();
        assert!(error.to_string().contains("inputs changed"), "{error}");
    }
}

#[test]
fn schema_dependencies_affect_identities_and_parameters_are_validated() {
    let mut manifest = manifest();
    let root = repository(&manifest);
    let before = check(root.path());
    write(
        root.path(),
        "schemas/parameters.json",
        &json!({
            "$schema": DRAFT_2020_12_DIALECT, "type": "object",
            "properties": {"iterations": {"type": "integer", "minimum": 0}}, "additionalProperties": false
        }),
    );
    let after = check(root.path());
    assert_eq!(before.manifest_hash, after.manifest_hash);
    assert_ne!(
        before.planning_catalog.implementation_definitions,
        after.planning_catalog.implementation_definitions
    );
    manifest["implementations"]["sum"]["parameter-defaults"]["iterations"] = json!(-1);
    write(root.path(), "metewand.yaml", &manifest);
    assert!(check_repository(&root.path().join("metewand.yaml")).is_err());
}

#[test]
fn inline_schema_refs_use_the_owning_contract_directory() {
    let mut manifest = manifest();
    let root = repository(&manifest);
    let mut contract = contract();
    contract["parameter-schema"] = json!({"$ref": "parameters.json#/$defs/empty"});
    write(root.path(), "problems/sum.yaml", &contract);
    write(
        root.path(),
        "problems/parameters.json",
        &json!({
            "$schema": DRAFT_2020_12_DIALECT,
            "$defs": {"empty": {"type": "object", "additionalProperties": false}}
        }),
    );
    manifest["problems"]["sum"]["contract"] = json!("problems/sum.yaml");
    write(root.path(), "metewand.yaml", &manifest);
    check(root.path());
}

#[test]
fn inline_and_file_dataset_declarations_match_by_effective_contents() {
    let mut manifest = manifest();
    let root = repository(&manifest);
    write(
        root.path(),
        "schemas/dataset.json",
        &json!({"$schema": DRAFT_2020_12_DIALECT, "type": "object"}),
    );
    manifest["datasets"]["numbers"]["output-schema"] = json!("schemas/dataset.json");
    write(root.path(), "metewand.yaml", &manifest);
    check(root.path());
    manifest["problems"]["sum"]["contract"]["dataset-schemas"][0] = json!({"type": "array"});
    write(root.path(), "metewand.yaml", &manifest);
    assert!(check_repository(&root.path().join("metewand.yaml")).is_err());
}

#[test]
fn rejects_invalid_inline_schemas_contracts_and_offline_references() {
    for (pointer, invalid) in [
        ("/implementations/sum/parameter-schema", json!({"type": 42})),
        (
            "/implementations/sum/parameter-schema",
            json!({"$schema": "https://json-schema.org/draft-07/schema"}),
        ),
        (
            "/implementations/sum/parameter-schema",
            json!({"$ref": "missing.json"}),
        ),
        (
            "/implementations/sum/parameter-schema",
            json!({"$ref": "https://example.invalid/schema.json"}),
        ),
        (
            "/implementations/sum/parameter-schema",
            json!({"$ref": "../escape.json"}),
        ),
        ("/implementations/sum/parameter-schema", Value::Null),
        (
            "/problems/sum/contract",
            json!({"version": 1, "name": "incomplete"}),
        ),
        (
            "/problems/sum/contract/semantics-schema",
            json!({"type": "object", "required": ["missing"]}),
        ),
        ("/problems/sum/contract/unknown-option", json!(true)),
    ] {
        let mut manifest = manifest();
        if pointer.ends_with("unknown-option") {
            manifest["problems"]["sum"]["contract"]["unknown-option"] = invalid;
        } else {
            *manifest.pointer_mut(pointer).unwrap() = invalid;
        }
        let root = repository(&manifest);
        assert!(
            check_repository(&root.path().join("metewand.yaml")).is_err(),
            "{pointer}: {manifest}"
        );
    }
}

#[test]
fn boolean_schemas_and_generated_dataset_parameters_use_the_catalog() {
    let mut manifest = manifest();
    manifest["implementations"]["sum"]["parameter-schema"] = json!(true);
    manifest["datasets"]["numbers"] = json!({
        "output-schema": {"type": "object"},
        "runner": "command", "program": "generate", "environment": "local",
        "parameter-schema": {"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}}},
        "parameter-defaults": {"n": 2}
    });
    let root = repository(&manifest);
    check(root.path());
    manifest["implementations"]["sum"]["parameter-schema"] = json!(false);
    write(root.path(), "metewand.yaml", &manifest);
    assert!(check_repository(&root.path().join("metewand.yaml")).is_err());
    manifest["implementations"]["sum"]["parameter-schema"] = json!(true);
    manifest["datasets"]["numbers"]["parameter-defaults"]["n"] = json!(0);
    write(root.path(), "metewand.yaml", &manifest);
    assert!(check_repository(&root.path().join("metewand.yaml")).is_err());
}

#[test]
fn resolves_resource_ids_dynamic_refs_cycles_and_fragments_offline() {
    let mut manifest = manifest();
    manifest["implementations"]["sum"]["parameter-schema"] = json!({
        "$id": "schemas/virtual.json", "$dynamicRef": "parameters.json#parameters"
    });
    manifest["problems"]["sum"]["contract"]["result-schema"] = json!({
        "$defs": {"result": {"type": "object"}}, "$ref": "#/$defs/result"
    });
    let root = repository(&manifest);
    write(
        root.path(),
        "schemas/parameters.json",
        &json!({
            "$schema": DRAFT_2020_12_DIALECT, "$dynamicAnchor": "parameters",
            "type": "object", "properties": {"iterations": {"type": "integer"}, "child": {"$ref": "virtual.json"}}
        }),
    );
    let before = check(root.path());
    before
        .schemas
        .validate(
            before.problem_contracts["sum"].result_schema.as_path(),
            &json!({}),
        )
        .unwrap();
    assert!(
        before
            .schemas
            .validate(
                before.problem_contracts["sum"].result_schema.as_path(),
                &json!(1)
            )
            .is_err()
    );
    write(
        root.path(),
        "schemas/parameters.json",
        &json!({
            "$schema": DRAFT_2020_12_DIALECT, "$dynamicAnchor": "parameters",
            "type": "object", "properties": {"iterations": {"type": "integer", "minimum": 0}, "child": {"$ref": "virtual.json"}}
        }),
    );
    let after = check(root.path());
    assert_ne!(
        before.planning_catalog.implementation_definitions,
        after.planning_catalog.implementation_definitions
    );
}

#[test]
fn references_in_literal_schema_data_are_not_dependencies() {
    let mut manifest = manifest();
    manifest["implementations"]["sum"]["parameter-schema"] = json!({
        "type": "object", "properties": {"iterations": {"type": "integer"}},
        "$comment": "The following annotations are literal JSON data.",
        "default": {"$ref": "missing-default.json"},
        "examples": [{"$ref": "missing-example.json"}],
        "properties-with-ref": {"$ref": "missing-extension.json"}
    });
    let root = repository(&manifest);
    check(root.path());
}

#[test]
fn resource_ids_can_be_declared_in_a_transitively_loaded_file() {
    let mut manifest = manifest();
    manifest["implementations"]["sum"]["parameter-schema"] = json!({"$ref": "schemas/bundle.json"});
    manifest["problems"]["sum"]["contract"]["result-schema"] =
        json!({"$ref": "schemas/virtual.json"});
    let root = repository(&manifest);
    write(
        root.path(),
        "schemas/bundle.json",
        &json!({
            "$schema": DRAFT_2020_12_DIALECT, "$id": "virtual.json", "type": "object"
        }),
    );
    check(root.path());
    assert!(!root.path().join("schemas/virtual.json").exists());
}

#[test]
fn a_missing_declared_schema_keeps_its_file_access_diagnostic() {
    let mut manifest = manifest();
    manifest["implementations"]["sum"]["parameter-schema"] = json!("schemas/missing.json");
    let root = repository(&manifest);
    assert!(
        matches!(check_repository(&root.path().join("metewand.yaml")),
        Err(RepositoryCheckError::RepositoryFileAccess { path, .. }) if path == Path::new("schemas/missing.json"))
    );
}

#[test]
fn absolute_ids_resolve_only_to_admitted_documents_and_bind_their_contents() {
    let mut manifest = manifest();
    manifest["implementations"]["sum"]["parameter-schema"] =
        json!({"$ref": "https://example.invalid/parameters"});
    manifest["problems"]["sum"]["contract"]["parameter-schema"] = json!("schemas/parameters.json");
    manifest["problems"]["sum"]["parameter-defaults"] = json!({"iterations": 1});
    let root = repository(&manifest);
    let mut previous = None;
    for minimum in [1, 0] {
        write(
            root.path(),
            "schemas/parameters.json",
            &json!({
                "$schema": DRAFT_2020_12_DIALECT, "$id": "https://example.invalid/parameters",
                "type": "object", "properties": {"iterations": {"type": "integer", "minimum": minimum}}
            }),
        );
        let checked = check(root.path());
        let id = checked
            .planning_catalog
            .implementation_definitions
            .values()
            .next()
            .unwrap();
        if let Some(previous) = previous {
            assert_ne!(previous, *id);
        }
        previous = Some(*id);
    }
}
