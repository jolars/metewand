use std::{collections::BTreeMap, path::Path};

use metewand_core::{
    manifest::{DatasetDefinition, Manifest, Name, parse_manifest},
    manifest_hash::canonical_manifest_bytes,
    planning::{ConfigurationExpansionError, ExpandedExperiment, expand_manifest_configurations},
    problem_contract::{ContractDeclaration, ProblemContract, parse_problem_contract_document},
    public_schemas::{PublicSchema, public_schema_catalog},
    schema::SchemaCatalog,
};
use serde_json::{Value, json};

fn document() -> Value {
    json!({
        "version": 1, "name": "compact-boundaries",
        "environments": {"local": {"kind": "local"}},
        "datasets": {"generated": {
            "runner": "command", "program": "generate", "environment": "local",
            "output-schema": {"type": "object"},
            "parameters": {"rows": {"type": "integer", "minimum": 1, "default": 10}}
        }},
        "problems": {"sum": {
            "contract": {
                "version": 1, "name": "sum", "family": "arithmetic",
                "parameters": {"offset": {"type": "integer", "minimum": 0, "default": 0}},
                "dataset-schemas": [{"type": "object"}],
                "result": {"answer": "integer", "trace": {"type": "string", "optional": true}},
                "metrics": {"error": {"type": "number", "minimum": 0}},
                "semantics-schema": {"type": "object"},
                "allowed-timing-scopes": ["prepare_and_execute"],
                "supported-budgets": ["none"],
                "semantics": {"validity": {"finite": true}, "one_shot_completion": {"exact": true}},
                "reference-cases": [{"dataset": "data.json", "result": "result.json", "expected-metrics": "metrics.json"}]
            },
            "evaluator": {"runner": "command", "program": "evaluate", "environment": "local"}
        }},
        "implementations": {"solver": {
            "runner": "command", "program": "solve", "environment": "local",
            "problem-contracts": ["sum"], "capabilities": ["one_shot"],
            "parameters": {
                "steps": {"type": "integer", "minimum": 1, "default": 1},
                "runner": {"type": "string", "default": "scientific-setting"}
            }
        }},
        "experiments": [{
            "name": "main", "problem": "sum", "datasets": ["generated"], "implementations": ["solver"],
            "execution-policy": "local", "observation-policy": "final",
            "implementation-repetitions": 1, "measurement-repetitions": 1, "seed": 7
        }],
        "execution-policies": {"local": {"version": 1, "timing-scope": "prepare_and_execute", "primary-time": "timed_wall_time"}},
        "observation-policies": {"final": {"version": 1, "kind": "one_shot"}}
    })
}

fn parse(document: &Value) -> Manifest {
    parse_manifest(Path::new("metewand.yaml"), &document.to_string()).unwrap()
}

fn contracts_and_schemas(manifest: &Manifest) -> (BTreeMap<Name, ProblemContract>, SchemaCatalog) {
    let contracts = manifest
        .problems
        .iter()
        .map(|(name, problem)| {
            let ContractDeclaration::Inline(contract) = &problem.contract else {
                panic!("this test uses inline contracts");
            };
            (name.clone(), (**contract).clone())
        })
        .collect::<BTreeMap<_, _>>();
    let mut declarations = contracts
        .values()
        .flat_map(ProblemContract::schemas)
        .collect::<Vec<_>>();
    for dataset in manifest.datasets.values() {
        if let DatasetDefinition::Generated(dataset) = dataset {
            declarations.extend(dataset.parameter_schema.iter());
            declarations.push(&dataset.output_schema);
        }
    }
    for implementation in manifest.implementations.values() {
        declarations.extend(implementation.parameter_schema.iter());
    }
    let documents = declarations
        .into_iter()
        .map(|schema| {
            (
                schema.as_path().to_path_buf(),
                schema.inline_document().unwrap().as_json().clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    (contracts, SchemaCatalog::try_new(documents).unwrap())
}

fn expand(document: &Value) -> Vec<ExpandedExperiment> {
    let manifest = parse(document);
    let (contracts, schemas) = contracts_and_schemas(&manifest);
    expand_manifest_configurations(&manifest, &contracts, &schemas).unwrap()
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

#[test]
fn compact_boundaries_normalize_to_explicit_schemas_and_defaults() {
    let compact = document();
    let mut explicit = compact.clone();
    let dataset = explicit["datasets"]["generated"].as_object_mut().unwrap();
    dataset.remove("parameters");
    dataset.insert(
        "parameter-schema".into(),
        object_schema(
            json!({"rows": {"type": "integer", "minimum": 1}}),
            &["rows"],
        ),
    );
    dataset.insert("parameter-defaults".into(), json!({"rows": 10}));
    let problem = explicit["problems"]["sum"].as_object_mut().unwrap();
    problem.insert("parameter-defaults".into(), json!({"offset": 0}));
    let contract = problem
        .get_mut("contract")
        .unwrap()
        .as_object_mut()
        .unwrap();
    contract.remove("parameters");
    contract.remove("result");
    contract.remove("metrics");
    contract.insert(
        "parameter-schema".into(),
        object_schema(
            json!({"offset": {"type": "integer", "minimum": 0}}),
            &["offset"],
        ),
    );
    contract.insert(
        "result-schema".into(),
        object_schema(
            json!({"answer": {"type": "integer"}, "trace": {"type": "string"}}),
            &["answer"],
        ),
    );
    contract.insert(
        "metric-schema".into(),
        object_schema(
            json!({"error": {"type": "number", "minimum": 0}}),
            &["error"],
        ),
    );
    assert_eq!(
        canonical_manifest_bytes(&parse(&compact)).unwrap(),
        canonical_manifest_bytes(&parse(&explicit)).unwrap()
    );
    assert_eq!(expand(&compact), expand(&explicit));
    let candidate = &expand(&compact)[0].candidates[0];
    assert_eq!(candidate.dataset.parameters.as_json(), &json!({"rows": 10}));
    assert_eq!(
        candidate.problem.parameters.as_json(),
        &json!({"offset": 0})
    );
    assert_eq!(
        candidate.implementation.parameters.as_json(),
        &json!({"steps": 1, "runner": "scientific-setting"})
    );
}

#[test]
fn compact_results_and_metrics_require_fields_and_reject_unknown_fields() {
    let manifest = parse(&document());
    let (contracts, schemas) = contracts_and_schemas(&manifest);
    let contract = contracts.values().next().unwrap();
    for (schema, valid, invalid) in [
        (
            &contract.result_schema,
            json!({"answer": 1}),
            vec![
                json!({}),
                json!({"answer": 1, "extra": true}),
                json!({"answer": "one"}),
            ],
        ),
        (
            &contract.metric_schema,
            json!({"error": 0}),
            vec![
                json!({}),
                json!({"error": -1}),
                json!({"error": 0, "extra": true}),
            ],
        ),
    ] {
        schemas.validate(schema.as_path(), &valid).unwrap();
        for invalid in invalid {
            assert!(
                schemas.validate(schema.as_path(), &invalid).is_err(),
                "{invalid}"
            );
        }
    }
    schemas
        .validate(
            contract.result_schema.as_path(),
            &json!({"answer": 1, "trace": "done"}),
        )
        .unwrap();
}

fn grid_document() -> Value {
    let mut document = document();
    document["datasets"]["generated"]["parameters"]["rows"] =
        json!({"type": "integer", "minimum": 1, "grid": [10, 20]});
    document["problems"]["sum"]["contract"]["parameters"]["offset"] =
        json!({"type": "integer", "minimum": 0, "grid": [0, 2]});
    document["implementations"]["solver"]["parameters"]["steps"] =
        json!({"type": "integer", "minimum": 1, "grid": [1, 2]});
    document
}

#[test]
fn declaration_grids_expand_across_namespaces_without_narrowing_domains() {
    let mut document = grid_document();
    let expansion = expand(&document);
    let actual = expansion[0]
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.dataset.parameters.as_json()["rows"]
                    .as_i64()
                    .unwrap(),
                candidate.problem.parameters.as_json()["offset"]
                    .as_i64()
                    .unwrap(),
                candidate.implementation.parameters.as_json()["steps"]
                    .as_i64()
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            (10, 0, 1),
            (10, 0, 2),
            (10, 2, 1),
            (10, 2, 2),
            (20, 0, 1),
            (20, 0, 2),
            (20, 2, 1),
            (20, 2, 2)
        ]
    );

    document["experiments"][0]["dataset-parameters"] =
        json!({"generated": {"rows": {"value": 15}}});
    document["experiments"][0]["implementation-parameters"] =
        json!({"solver": {"steps": {"grid": [3, 4]}}});
    document["experiments"][0]["cases"] = json!([
        {"name": "first", "problem-parameters": {"offset": {"value": 3}}},
        {"name": "second", "dataset-parameters": {"generated": {"rows": {"value": 99}}},
         "problem-parameters": {"offset": {"value": 4}}, "implementation-parameters": {"solver": {"steps": {"value": 5}}}}
    ]);
    let expansion = expand(&document);
    let actual = expansion[0]
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.dataset.parameters.as_json().clone(),
                candidate.problem.parameters.as_json().clone(),
                candidate.implementation.parameters.as_json().clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            (
                json!({"rows": 15}),
                json!({"offset": 3}),
                json!({"steps": 3, "runner": "scientific-setting"})
            ),
            (
                json!({"rows": 15}),
                json!({"offset": 3}),
                json!({"steps": 4, "runner": "scientific-setting"})
            ),
            (
                json!({"rows": 99}),
                json!({"offset": 4}),
                json!({"steps": 5, "runner": "scientific-setting"})
            )
        ]
    );
}

#[test]
fn array_enum_boolean_and_null_grids_preserve_literal_values() {
    let mut document = document();
    document["implementations"]["solver"]["parameters"] = json!({
        "weights": {"type": "array", "items": {"type": "integer", "minimum": 0}, "grid": [[1], [2, 3]]},
        "mode": {"type": "string", "enum": ["a", "b", "c"], "grid": ["a", "b"]},
        "flag": {"type": "boolean", "grid": [true, false]},
        "nothing": {"type": "null", "grid": [null]}
    });
    let expanded = expand(&document);
    let candidates = &expanded[0].candidates;
    assert_eq!(candidates.len(), 8);
    assert_eq!(
        candidates[0].implementation.parameters.as_json(),
        &json!({
            "weights": [1], "mode": "a", "flag": true, "nothing": null
        })
    );
    assert_eq!(
        candidates[7].implementation.parameters.as_json(),
        &json!({
            "weights": [2, 3], "mode": "b", "flag": false, "nothing": null
        })
    );
    document["experiments"][0]["implementation-parameters"] =
        json!({"solver": {"mode": {"value": "c"}}});
    assert_eq!(expand(&document)[0].candidates.len(), 4);
}

#[test]
fn declaration_grids_retain_duplicate_detection_and_required_parameters() {
    let mut document = grid_document();
    document["datasets"]["generated"]["parameters"]["rows"]["grid"] = json!([10, 10]);
    let manifest = parse(&document);
    let (contracts, schemas) = contracts_and_schemas(&manifest);
    assert!(matches!(
        expand_manifest_configurations(&manifest, &contracts, &schemas),
        Err(ConfigurationExpansionError::DuplicateLogicalCandidate(_))
    ));
    document["datasets"]["generated"]["parameters"]["rows"]
        .as_object_mut()
        .unwrap()
        .remove("grid");
    let manifest = parse(&document);
    let (contracts, schemas) = contracts_and_schemas(&manifest);
    let error = expand_manifest_configurations(&manifest, &contracts, &schemas).unwrap_err();
    assert!(error.to_string().contains("rows"), "{error}");
}

#[test]
fn compact_and_referenced_schemas_can_be_chosen_independently() {
    for (compact, explicit) in [
        ("parameters", "parameter-schema"),
        ("result", "result-schema"),
        ("metrics", "metric-schema"),
    ] {
        let mut document = document();
        let contract = document["problems"]["sum"]["contract"]
            .as_object_mut()
            .unwrap();
        contract.remove(compact);
        contract.insert(explicit.into(), json!("schema.json"));
        parse(&document);
        public_schema_catalog()
            .unwrap()
            .validate(PublicSchema::Manifest.repository_path(), &document)
            .unwrap();
    }
}

#[test]
fn grids_are_selections_and_only_change_the_manifest_selection_hash() {
    let document = grid_document();
    let mut changed = document.clone();
    changed["datasets"]["generated"]["parameters"]["rows"]["grid"] = json!([30]);
    assert_ne!(
        canonical_manifest_bytes(&parse(&document)).unwrap(),
        canonical_manifest_bytes(&parse(&changed)).unwrap()
    );
    let schema = |document: &Value| {
        let manifest = parse(document);
        let DatasetDefinition::Generated(dataset) = manifest.datasets.values().next().unwrap()
        else {
            unreachable!()
        };
        dataset
            .parameter_schema
            .as_ref()
            .unwrap()
            .declaration_value()
    };
    assert_eq!(schema(&document), schema(&changed));
    assert_eq!(
        schema(&document)["properties"]["rows"],
        json!({"type": "integer", "minimum": 1})
    );
}

#[test]
fn declarations_normalize_identically_in_block_and_flow_yaml() {
    let document = grid_document();
    let flow = document.to_string();
    let block = serde_saphyr::to_string(&document).unwrap();
    assert!(block.contains("parameters:\n"), "{block}");
    let flow = parse_manifest(Path::new("metewand.yaml"), &flow).unwrap();
    let block = parse_manifest(Path::new("metewand.yaml"), &block).unwrap();
    assert_eq!(
        canonical_manifest_bytes(&flow).unwrap(),
        canonical_manifest_bytes(&block).unwrap()
    );
}

#[test]
fn invalid_parameter_selections_report_the_authored_field() {
    for pointer in [
        "/datasets/generated/parameters",
        "/problems/sum/contract/parameters",
        "/implementations/solver/parameters",
    ] {
        for field in [
            json!({"type": "integer", "grid": []}),
            json!({"type": "integer", "grid": 1}),
            json!({"type": "integer", "minimum": 1, "grid": [0, 1]}),
            json!({"type": "integer", "grid": [1, "two"]}),
            json!({"type": "integer", "default": 1, "grid": [1, 2]}),
            json!({"grid": [1, 2]}),
            json!({"type": "array", "items": {"type": "integer", "grid": [1]}}),
        ] {
            let mut document = document();
            *document.pointer_mut(pointer).unwrap() = json!({"invalid": field});
            let error =
                parse_manifest(Path::new("metewand.yaml"), &document.to_string()).unwrap_err();
            assert!(error.message().contains("parameters.invalid"), "{error}");
        }
    }
}

#[test]
fn output_fields_cannot_supply_defaults_or_grids() {
    for boundary in ["result", "metrics"] {
        for field in [
            json!({"type": "number", "default": 0}),
            json!({"type": "number", "grid": [0, 1]}),
            json!({"type": "array", "items": {"type": "integer", "default": 1}}),
        ] {
            let mut document = document();
            document["problems"]["sum"]["contract"][boundary] = json!({"invalid": field});
            let error =
                parse_manifest(Path::new("metewand.yaml"), &document.to_string()).unwrap_err();
            assert!(
                error.message().contains(&format!("{boundary}.invalid")),
                "{error}"
            );
        }
    }
}

#[test]
fn compact_and_explicit_boundaries_cannot_compete() {
    for (pointer, explicit) in [
        ("/datasets/generated", "parameter-schema"),
        ("/datasets/generated", "parameter-defaults"),
        ("/problems/sum/contract", "parameter-schema"),
        ("/problems/sum/contract", "result-schema"),
        ("/problems/sum/contract", "metric-schema"),
        ("/problems/sum", "parameter-defaults"),
    ] {
        let mut document = document();
        document.pointer_mut(pointer).unwrap()[explicit] = if explicit == "parameter-defaults" {
            json!({})
        } else {
            json!("schema.json")
        };
        let error = parse_manifest(Path::new("metewand.yaml"), &document.to_string()).unwrap_err();
        assert!(error.message().contains(explicit), "{error}");
    }
}

#[test]
fn standalone_contracts_share_the_compact_parser() {
    let contract = document()["problems"]["sum"]["contract"].clone();
    let parsed =
        parse_problem_contract_document(Path::new("problem.yaml"), &contract.to_string()).unwrap();
    assert_eq!(
        parsed.result_schema.inline_document().unwrap().as_json()["required"],
        json!(["answer"])
    );
}

#[test]
fn public_schemas_accept_compact_boundaries_and_reject_competing_or_output_selections() {
    let schemas = public_schema_catalog().unwrap();
    let document = grid_document();
    schemas
        .validate(PublicSchema::Manifest.repository_path(), &document)
        .unwrap();
    schemas
        .validate(
            PublicSchema::ProblemContract.repository_path(),
            &document["problems"]["sum"]["contract"],
        )
        .unwrap();
    for (pointer, value) in [
        ("/datasets/generated/parameter-schema", json!("schema.json")),
        ("/problems/sum/contract/result-schema", json!("schema.json")),
        (
            "/problems/sum/contract/result/answer",
            json!({"type": "integer", "default": 0}),
        ),
        (
            "/problems/sum/contract/metrics/error",
            json!({"type": "number", "grid": [0, 1]}),
        ),
        (
            "/implementations/solver/parameters/steps",
            json!({"type": "integer", "default": 1, "grid": [1]}),
        ),
    ] {
        let mut invalid = document.clone();
        let (parent, name) = pointer.rsplit_once('/').unwrap();
        invalid.pointer_mut(parent).unwrap()[name] = value;
        assert!(
            schemas
                .validate(PublicSchema::Manifest.repository_path(), &invalid)
                .is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn omitted_parameter_boundaries_are_closed_and_match_empty_declarations() {
    let mut implicit = document();
    implicit["datasets"]["generated"]
        .as_object_mut()
        .unwrap()
        .remove("parameters");
    implicit["problems"]["sum"]["contract"]
        .as_object_mut()
        .unwrap()
        .remove("parameters");
    implicit["implementations"]["solver"]
        .as_object_mut()
        .unwrap()
        .remove("parameters");
    let public_schemas = public_schema_catalog().unwrap();
    public_schemas
        .validate(PublicSchema::Manifest.repository_path(), &implicit)
        .unwrap();
    public_schemas
        .validate(
            PublicSchema::ProblemContract.repository_path(),
            &implicit["problems"]["sum"]["contract"],
        )
        .unwrap();
    let contract = parse_problem_contract_document(
        Path::new("problem.yaml"),
        &implicit["problems"]["sum"]["contract"].to_string(),
    )
    .unwrap();
    assert_eq!(
        contract
            .parameter_schema
            .inline_document()
            .unwrap()
            .as_json(),
        &json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object", "properties": {}, "required": [], "additionalProperties": false
        })
    );
    let mut explicit = implicit.clone();
    explicit["datasets"]["generated"]["parameters"] = json!({});
    explicit["problems"]["sum"]["contract"]["parameters"] = json!({});
    explicit["implementations"]["solver"]["parameters"] = json!({});
    assert_eq!(
        canonical_manifest_bytes(&parse(&implicit)).unwrap(),
        canonical_manifest_bytes(&parse(&explicit)).unwrap()
    );
    assert_eq!(expand(&implicit), expand(&explicit));
    for (namespace, axes) in [
        (
            "dataset-parameters",
            json!({"generated": {"extra": {"value": 1}}}),
        ),
        ("problem-parameters", json!({"extra": {"value": 1}})),
        (
            "implementation-parameters",
            json!({"solver": {"extra": {"value": 1}}}),
        ),
    ] {
        let mut invalid = implicit.clone();
        invalid["experiments"][0][namespace] = axes;
        let manifest = parse(&invalid);
        let (contracts, schemas) = contracts_and_schemas(&manifest);
        assert!(
            expand_manifest_configurations(&manifest, &contracts, &schemas).is_err(),
            "{namespace}"
        );
    }
    for pointer in [
        "/datasets/generated",
        "/problems/sum",
        "/implementations/solver",
    ] {
        let mut invalid = implicit.clone();
        invalid.pointer_mut(pointer).unwrap()["parameter-defaults"] = json!({"extra": 1});
        let manifest = parse(&invalid);
        let (contracts, schemas) = contracts_and_schemas(&manifest);
        assert!(
            expand_manifest_configurations(&manifest, &contracts, &schemas).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn local_environment_and_one_shot_defaults_normalize_before_hashing() {
    let mut explicit = document();
    explicit["execution-policies"]["local"]["primary-time"] = json!("timed_wall_time");
    let mut implicit = explicit.clone();
    implicit.as_object_mut().unwrap().remove("environments");
    implicit["datasets"]["generated"]
        .as_object_mut()
        .unwrap()
        .remove("environment");
    implicit["problems"]["sum"]["evaluator"]
        .as_object_mut()
        .unwrap()
        .remove("environment");
    implicit["implementations"]["solver"]
        .as_object_mut()
        .unwrap()
        .remove("environment");
    implicit["execution-policies"]["local"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    implicit["observation-policies"]["final"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    implicit["experiments"][0]
        .as_object_mut()
        .unwrap()
        .remove("implementation-repetitions");
    implicit["experiments"][0]
        .as_object_mut()
        .unwrap()
        .remove("measurement-repetitions");
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::Manifest.repository_path(), &implicit)
        .unwrap();
    assert_eq!(
        canonical_manifest_bytes(&parse(&implicit)).unwrap(),
        canonical_manifest_bytes(&parse(&explicit)).unwrap()
    );
    assert_eq!(expand(&implicit), expand(&explicit));
    for environments in [
        json!({"managed": {"kind": "uv", "project": ".", "lockfile": "uv.lock"}}),
        json!({"first": {"kind": "local"}, "second": {"kind": "local"}}),
    ] {
        let mut invalid = implicit.clone();
        invalid["environments"] = environments;
        assert!(parse_manifest(Path::new("metewand.yaml"), &invalid.to_string()).is_err());
    }
    implicit["environments"] = json!({"host": {"kind": "local"}});
    let manifest = parse(&implicit);
    let metewand_core::manifest::WorkerDefinition::Command(worker) =
        &manifest.implementations["solver"].worker
    else {
        panic!("expected command worker");
    };
    assert_eq!(worker.environment.as_str(), "host");
    implicit["environments"]["managed"] =
        json!({"kind": "uv", "project": ".", "lockfile": "uv.lock"});
    implicit["implementations"]["solver"]["environment"] = json!("managed");
    let manifest = parse(&implicit);
    let metewand_core::manifest::WorkerDefinition::Command(worker) =
        &manifest.implementations["solver"].worker
    else {
        panic!("expected command worker");
    };
    assert_eq!(worker.environment.as_str(), "managed");
}

#[test]
fn selected_timing_and_profile_choices_remain_explicit() {
    let mut valid = document();
    valid["execution-policies"]["local"]["primary-time"] = json!("timed_wall_time");
    for field in ["timing-scope", "primary-time"] {
        let mut invalid = valid.clone();
        invalid["execution-policies"]["local"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let manifest = parse(&invalid);
        let (contracts, schemas) = contracts_and_schemas(&manifest);
        let error = expand_manifest_configurations(&manifest, &contracts, &schemas).unwrap_err();
        assert!(error.to_string().contains(field), "{error}");
    }
    for field in ["observation-policy", "execution-policy"] {
        let mut invalid = valid.clone();
        invalid["experiments"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(parse_manifest(Path::new("metewand.yaml"), &invalid.to_string()).is_err());
    }
    valid["observation-policies"]["final"]
        .as_object_mut()
        .unwrap()
        .remove("kind");
    assert!(parse_manifest(Path::new("metewand.yaml"), &valid.to_string()).is_err());
}
