use std::{collections::BTreeMap, path::Path};

use crate::core::{
    compatibility::validate_manifest_compatibility_with_schemas,
    manifest::{Manifest, Name, parse_manifest},
    manifest_hash::canonical_manifest_bytes,
    planning::{
        DatasetConfigurationDefinition, LogicalPlan, LogicalPlanningCatalog,
        expand_manifest_logical_plan, identify_builtin_unit_dataset_definition,
    },
    problem_contract::{ContractDeclaration, ProblemContract},
    public_schemas::{PublicSchema, public_schema_catalog},
    records::RecordId,
    schema::SchemaCatalog,
};
use serde_json::{Value, json};

fn buffered() -> Value {
    json!({
        "delivery": "buffered_profile", "measurement-basis": "instrumented",
        "instrumented-scope": "solve",
        "instrumentation-schema": {
            "type": "object", "properties": {"tolerance": {"type": "number", "exclusiveMinimum": 0}},
            "required": ["tolerance"], "additionalProperties": false
        },
        "instrumentation": {"tolerance": 0.01}, "check-schedule": {"every": 1}
    })
}

fn fresh() -> Value {
    json!({
        "delivery": "fresh_sequence", "controls": {"tolerance": {"type": "number", "exclusiveMinimum": 0}},
        "schedule": {"kind": "explicit", "controls": [{"tolerance": 0.01}, {"tolerance": 0.001}]}
    })
}

fn streaming() -> Value {
    json!({"delivery": "streaming_profile", "measurement-basis": "executor_observed", "check-schedule": {"every": 2}})
}

fn document() -> Value {
    let worker = json!({
        "runner": "command", "program": "solve", "environment": "local",
        "problem-contracts": ["sum"], "capabilities": ["buffered_profile", "fresh_sequence", "streaming_profile"]
    });
    json!({
        "version": 1, "name": "profile-defaults",
        "environments": {"local": {"kind": "local"}},
        "problems": {"sum": {
            "dataset-schemas": [],
            "result": {"value": "number"}, "metrics": {"gap": "number"},
            "instrumented-scopes": {"solve": {
                "clock-origin": "Before solving.", "included-work": "The complete solve.",
                "excluded-preparation": "Shared input loading."
            }},
            "evaluator": {"runner": "command", "program": "evaluate", "environment": "local"}
        }},
        "implementations": {
            "alpha": worker, "beta": worker,
            "unused": {"runner": "command", "program": "other", "environment": "local",
                "problem-contracts": ["sum"], "capabilities": ["one_shot"]}
        },
        "experiments": [
            {"name": "first", "problem": "sum", "implementations": ["alpha"], "observation-policy": "accuracy", "seed": 7},
            {"name": "second", "problem": "sum", "implementations": ["beta"], "observation-policy": "accuracy", "seed": 8},
            {"name": "other", "problem": "sum", "implementations": ["unused"], "seed": 9}
        ],
        "observation-policies": {"accuracy": {
            "kind": "profile", "max-observations": 2, "max-active-time": "1 s",
            "max-trace-points": 100, "max-buffer-bytes": 65536, "terminal-reserve-bytes": 8192,
            "max-checkpoint-bytes": 4096,
            "default-binding": buffered(), "implementations": {"beta": fresh()}
        }}
    })
}

fn parse(document: &Value) -> Manifest {
    parse_manifest(Path::new("bench/metewand.yaml"), &document.to_string()).unwrap()
}

fn inputs(manifest: &Manifest) -> (BTreeMap<Name, ProblemContract>, SchemaCatalog) {
    let (name, problem) = manifest.problems.first_key_value().unwrap();
    let ContractDeclaration::Inline(contract) = &problem.contract else {
        panic!("inline contract");
    };
    let declarations = contract.schemas().chain(
        manifest
            .observation_policies
            .values()
            .filter_map(|p| p.profile.as_ref())
            .flat_map(|p| p.implementations.values())
            .filter_map(|b| b.schema()),
    );
    let documents = declarations
        .map(|schema| {
            (
                schema.as_path().to_path_buf(),
                schema.inline_document().unwrap().as_json().clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    (
        BTreeMap::from([(name.clone(), (**contract).clone())]),
        SchemaCatalog::try_new(documents).unwrap(),
    )
}

fn plan(manifest: &Manifest) -> LogicalPlan {
    let (contracts, schemas) = inputs(manifest);
    let catalog = LogicalPlanningCatalog {
        dataset_definitions: BTreeMap::from([(
            DatasetConfigurationDefinition::Unit,
            identify_builtin_unit_dataset_definition().unwrap().id,
        )]),
        problem_definitions: manifest
            .problems
            .keys()
            .cloned()
            .map(|name| (name, RecordId::from_digest([1; 32])))
            .collect(),
        implementation_definitions: manifest
            .implementations
            .keys()
            .cloned()
            .enumerate()
            .map(|(index, name)| (name, RecordId::from_digest([index as u8 + 2; 32])))
            .collect(),
        environment_definitions: manifest
            .environments
            .keys()
            .cloned()
            .map(|name| (name, RecordId::from_digest([5; 32])))
            .collect(),
        ..Default::default()
    };
    expand_manifest_logical_plan(manifest, &contracts, &schemas, &catalog).unwrap()
}

#[test]
fn default_profile_bindings_preserve_explicit_canonical_bytes_and_plans() {
    for binding in [buffered(), fresh(), streaming()] {
        let mut compact = document();
        compact["observation-policies"]["accuracy"]["default-binding"] = binding.clone();
        let mut explicit = compact.clone();
        let policy = explicit["observation-policies"]["accuracy"]
            .as_object_mut()
            .unwrap();
        policy.remove("default-binding");
        policy["implementations"]["alpha"] = binding;
        let compact = parse(&compact);
        let explicit = parse(&explicit);
        assert_eq!(
            canonical_manifest_bytes(&compact).unwrap(),
            canonical_manifest_bytes(&explicit).unwrap()
        );
        assert_eq!(plan(&compact), plan(&explicit));
        let bindings = &compact.observation_policies["accuracy"]
            .profile
            .as_ref()
            .unwrap()
            .implementations;
        assert_eq!(bindings.len(), 2);
        assert!(!bindings.contains_key("unused"));
    }
}

#[test]
fn default_profile_bindings_cover_all_experiments_and_replace_whole_bindings() {
    let mut source = document();
    source["observation-policies"]["accuracy"]
        .as_object_mut()
        .unwrap()
        .remove("implementations");
    let manifest = parse(&source);
    let bindings = &manifest.observation_policies["accuracy"]
        .profile
        .as_ref()
        .unwrap()
        .implementations;
    assert_eq!(bindings.len(), 2);
    assert_eq!(
        bindings["alpha"].declaration_value(),
        bindings["beta"].declaration_value()
    );

    source["observation-policies"]["accuracy"]["implementations"] = json!({"beta": fresh()});
    let manifest = parse(&source);
    let beta = manifest.observation_policies["accuracy"]
        .profile
        .as_ref()
        .unwrap()
        .implementations["beta"]
        .declaration_value();
    assert_eq!(beta["delivery"], "fresh_sequence");
    assert!(beta.get("instrumentation").is_none());
    assert!(beta.get("instrumented-scope").is_none());
}

#[test]
fn resolved_default_profile_bindings_retain_contract_schema_and_capability_checks() {
    let valid = document();
    let manifest = parse(&valid);
    let (contracts, schemas) = inputs(&manifest);
    validate_manifest_compatibility_with_schemas(&manifest, &contracts, &schemas).unwrap();
    for (pointer, value, expected) in [
        (
            "/implementations/alpha/capabilities",
            json!(["fresh_sequence"]),
            "alpha",
        ),
        (
            "/implementations/beta/capabilities",
            json!(["buffered_profile"]),
            "beta",
        ),
        (
            "/observation-policies/accuracy/default-binding/instrumented-scope",
            json!("missing"),
            "does not authorize",
        ),
        (
            "/observation-policies/accuracy/default-binding/instrumentation/tolerance",
            json!("bad"),
            "does not satisfy schema",
        ),
        (
            "/observation-policies/accuracy/implementations/beta/schedule/controls/0/tolerance",
            json!(-1),
            "does not satisfy schema",
        ),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        let manifest = parse(&invalid);
        let (contracts, schemas) = inputs(&manifest);
        let error = validate_manifest_compatibility_with_schemas(&manifest, &contracts, &schemas)
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{pointer}: {error}");
    }
    let mut invalid = valid;
    invalid["observation-policies"]["accuracy"]["default-binding"] = fresh();
    invalid["observation-policies"]["accuracy"]["default-binding"]["schedule"]["controls"][0]["tolerance"] =
        json!(-1);
    let manifest = parse(&invalid);
    let (contracts, schemas) = inputs(&manifest);
    assert!(validate_manifest_compatibility_with_schemas(&manifest, &contracts, &schemas).is_err());
}

#[test]
fn profiles_without_a_default_still_require_every_selected_binding() {
    let mut source = document();
    source["observation-policies"]["accuracy"]
        .as_object_mut()
        .unwrap()
        .remove("default-binding");
    let manifest = parse(&source);
    let (contracts, schemas) = inputs(&manifest);
    let error =
        validate_manifest_compatibility_with_schemas(&manifest, &contracts, &schemas).unwrap_err();
    assert!(
        error.to_string().contains("missing binding for alpha"),
        "{error}"
    );
}

#[test]
fn public_schemas_accept_default_bindings_with_optional_complete_overrides() {
    let catalog = public_schema_catalog().unwrap();
    for binding in [buffered(), fresh(), streaming()] {
        for overrides in [None, Some(json!({})), Some(json!({"beta": fresh()}))] {
            let mut source = document();
            let policy = source["observation-policies"]["accuracy"]
                .as_object_mut()
                .unwrap();
            policy.insert("default-binding".into(), binding.clone());
            match overrides {
                Some(value) => {
                    policy.insert("implementations".into(), value);
                }
                None => {
                    policy.remove("implementations");
                }
            }
            if binding["delivery"] == "fresh_sequence" {
                for key in [
                    "max-trace-points",
                    "max-buffer-bytes",
                    "terminal-reserve-bytes",
                    "max-checkpoint-bytes",
                ] {
                    policy.remove(key);
                }
            }
            catalog
                .validate(PublicSchema::Manifest.repository_path(), &source)
                .unwrap();
            let manifest = parse(&source);
            let (contracts, schemas) = inputs(&manifest);
            validate_manifest_compatibility_with_schemas(&manifest, &contracts, &schemas).unwrap();
        }
    }
}

#[test]
fn default_profile_bindings_reject_partial_overrides_and_missing_bounds() {
    let catalog = public_schema_catalog().unwrap();
    let valid = document();
    let mut invalids = Vec::new();
    for (pointer, value) in [
        (
            "/observation-policies/accuracy/default-binding",
            Value::Null,
        ),
        (
            "/observation-policies/accuracy/implementations/beta",
            json!({"delivery": "fresh_sequence"}),
        ),
        (
            "/observation-policies/accuracy/implementations/beta",
            json!({"check-schedule": {"every": 2}}),
        ),
        (
            "/observation-policies/accuracy",
            json!({"kind": "one_shot", "default-binding": fresh()}),
        ),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        invalids.push(invalid);
    }
    for key in ["max-trace-points", "max-checkpoint-bytes"] {
        let mut invalid = valid.clone();
        if key == "max-checkpoint-bytes" {
            invalid["observation-policies"]["accuracy"]["default-binding"] = streaming();
        }
        invalid["observation-policies"]["accuracy"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        invalids.push(invalid);
    }
    for invalid in invalids {
        assert!(
            catalog
                .validate(PublicSchema::Manifest.repository_path(), &invalid)
                .is_err(),
            "{invalid}"
        );
        assert!(
            parse_manifest(Path::new("metewand.yaml"), &invalid.to_string()).is_err(),
            "{invalid}"
        );
    }
}

fn assert_observation_limit_inference(mut source: Value, expected: u64) {
    source["observation-policies"]["accuracy"]["max-observations"] = json!(expected);
    let explicit = parse(&source);
    source["observation-policies"]["accuracy"]
        .as_object_mut()
        .unwrap()
        .remove("max-observations");
    let inferred = parse(&source);
    assert_eq!(
        inferred.observation_policies["accuracy"]
            .profile
            .as_ref()
            .unwrap()
            .policy
            .max_observations,
        expected
    );
    assert_eq!(
        canonical_manifest_bytes(&inferred).unwrap(),
        canonical_manifest_bytes(&explicit).unwrap()
    );
    assert_eq!(plan(&inferred), plan(&explicit));
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::Manifest.repository_path(), &source)
        .unwrap();
}

#[test]
fn fresh_observation_limits_infer_explicit_linear_and_geometric_lengths_with_prefixes() {
    for schedule in [
        json!({"kind": "explicit", "controls": [{"tolerance": 1.0}, {"tolerance": 0.1}]}),
        json!({"kind": "linear", "field": "tolerance", "start": 0.1, "step": -0.01, "count": 1, "prefix": [{"initial": true}]}),
        json!({"kind": "geometric", "field": "tolerance", "start": 0.1, "ratio": 0.1, "count": 1, "prefix": [{"initial": true}]}),
    ] {
        for shared_default in [true, false] {
            let mut source = document();
            let mut binding = fresh();
            binding["schedule"] = schedule.clone();
            if shared_default {
                source["observation-policies"]["accuracy"]["default-binding"] = binding;
            } else {
                source["observation-policies"]["accuracy"]
                    .as_object_mut()
                    .unwrap()
                    .remove("default-binding");
                source["observation-policies"]["accuracy"]["implementations"]["alpha"] = binding;
            }
            assert_observation_limit_inference(source, 2);
        }
    }
    let mut source = document();
    source["observation-policies"]["accuracy"]["default-binding"] = fresh();
    source["observation-policies"]["accuracy"]
        .as_object_mut()
        .unwrap()
        .remove("implementations");
    assert_observation_limit_inference(source, 2);
}

#[test]
fn fresh_observation_limits_ignore_defaults_replaced_for_every_selected_implementation() {
    let mut longer = fresh();
    longer["schedule"]["controls"]
        .as_array_mut()
        .unwrap()
        .push(json!({"tolerance": 0.0001}));
    for default in [longer, buffered(), streaming()] {
        let mut source = document();
        source["observation-policies"]["accuracy"]["default-binding"] = default;
        source["observation-policies"]["accuracy"]["implementations"]["alpha"] = fresh();
        assert_observation_limit_inference(source, 2);
    }
}

#[test]
fn fresh_observation_limits_require_unambiguous_nonempty_safe_schedules() {
    let mut fresh_source = document();
    fresh_source["observation-policies"]["accuracy"]["default-binding"] = fresh();
    fresh_source["observation-policies"]["accuracy"]
        .as_object_mut()
        .unwrap()
        .remove("max-observations");
    for binding in [buffered(), streaming()] {
        let mut invalid = fresh_source.clone();
        invalid["observation-policies"]["accuracy"]["implementations"]["beta"] = binding.clone();
        assert!(parse_manifest(Path::new("metewand.yaml"), &invalid.to_string()).is_err());
        invalid["observation-policies"]["accuracy"]["default-binding"] = binding;
        invalid["observation-policies"]["accuracy"]
            .as_object_mut()
            .unwrap()
            .remove("implementations");
        assert!(parse_manifest(Path::new("metewand.yaml"), &invalid.to_string()).is_err());
    }
    for schedule in [
        json!({"kind": "explicit", "controls": []}),
        json!({"kind": "explicit", "controls": [{}]}),
        json!({"kind": "linear", "field": "tolerance", "start": 1, "step": 1, "count": 0}),
        json!({"kind": "linear", "field": "tolerance", "start": 1, "step": 1, "count": 9007199254740992u64}),
        json!({"kind": "geometric", "field": "tolerance", "start": 0.1, "ratio": 0.1, "count": 2, "prefix": [{"initial": true}]}),
    ] {
        let mut invalid = fresh_source.clone();
        invalid["observation-policies"]["accuracy"]["default-binding"]["schedule"] = schedule;
        let error = parse_manifest(Path::new("metewand.yaml"), &invalid.to_string()).unwrap_err();
        assert!(
            error.to_string().contains("max-observations")
                || error.to_string().contains("outside the inclusive range"),
            "{error}"
        );
    }
    for schedule in [
        json!({"kind": "explicit", "controls": []}),
        json!({"kind": "linear", "field": "tolerance", "start": 1, "step": 1, "count": 9007199254740991u64, "prefix": [{"initial": true}]}),
    ] {
        let mut invalid = fresh_source.clone();
        invalid["observation-policies"]["accuracy"]
            .as_object_mut()
            .unwrap()
            .remove("implementations");
        invalid["observation-policies"]["accuracy"]["default-binding"]["schedule"] = schedule;
        let error = parse_manifest(Path::new("metewand.yaml"), &invalid.to_string()).unwrap_err();
        assert!(
            error.to_string().contains("positive safe integer"),
            "{error}"
        );
    }
}

#[test]
fn explicit_fresh_observation_limits_keep_existing_validation_and_precedence() {
    let mut source = document();
    source["observation-policies"]["accuracy"]["default-binding"] = fresh();
    source["observation-policies"]["accuracy"]["metric"] = json!("gap");
    source["observation-policies"]["accuracy"]["direction"] = json!("at_most");
    source["observation-policies"]["accuracy"]["threshold"] = json!(0.001);
    source["observation-policies"]["accuracy"]["implementations"]["beta"]["schedule"]["controls"]
        .as_array_mut()
        .unwrap()
        .push(json!({"tolerance": 0.0001}));
    source["observation-policies"]["accuracy"]["max-observations"] = json!(4);
    let manifest = parse(&source);
    assert_eq!(
        manifest.observation_policies["accuracy"]
            .profile
            .as_ref()
            .unwrap()
            .policy
            .max_observations,
        4
    );
    plan(&manifest);
    for limit in [json!(0), Value::Null, json!(1), json!(9007199254740992u64)] {
        source["observation-policies"]["accuracy"]["max-observations"] = limit;
        assert!(parse_manifest(Path::new("metewand.yaml"), &source.to_string()).is_err());
    }
}

#[test]
fn persisted_profile_policies_still_require_explicit_observation_limits() {
    let policy = json!({"max-active-time": "1 s"});
    assert!(serde_json::from_value::<crate::core::profile::ProfilePolicy>(policy.clone()).is_err());
    let schema: Value = serde_json::from_str(include_str!(
        "../../schemas/v1/profile-observation-policy.schema.json"
    ))
    .unwrap();
    let mut persisted_schema = schema["$defs"]["policy"].clone();
    persisted_schema["$schema"] = schema["$schema"].clone();
    let catalog =
        SchemaCatalog::try_new([("policy.schema.json".into(), persisted_schema)]).unwrap();
    assert!(
        catalog
            .validate(Path::new("policy.schema.json"), &policy)
            .is_err()
    );
    catalog
        .validate(
            Path::new("policy.schema.json"),
            &json!({"max-observations": 2, "max-active-time": "1 s"}),
        )
        .unwrap();
}
