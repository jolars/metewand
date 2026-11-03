use std::path::Path;

use crate::core::{
    manifest::{DatasetDefinition, TimingScope, WorkerDefinition, parse_manifest},
    manifest_hash::canonical_manifest_bytes,
    problem_contract::ContractDeclaration,
    public_schemas::{PublicSchema, public_schema_catalog},
};

const BASIC: &str = r#"
version: 1
name: simple
datasets:
  numbers:
    function: benchmark.py:generate
problems:
  sum:
    result: {total: integer}
    metrics: {error: number}
    evaluator: {function: benchmark.py:evaluate}
implementations:
  loop:
    function: benchmark.py:solve
experiments:
  - name: comparison
    problem: sum
    datasets: [numbers]
    implementations: [loop]
    seed: 7
"#;

#[test]
fn named_experiment_defaults_and_unknown_names_are_checked() {
    let source = format!("{BASIC}\ndefault-experiment: comparison\n");
    let manifest = parse_manifest(Path::new("metewand.yaml"), &source).unwrap();
    assert_eq!(
        manifest.default_experiment.as_ref().unwrap().as_str(),
        "comparison"
    );
    assert!(
        parse_manifest(
            Path::new("metewand.yaml"),
            &source.replace(
                "default-experiment: comparison",
                "default-experiment: missing"
            )
        )
        .is_err()
    );
    assert!(
        parse_manifest(
            Path::new("metewand.yaml"),
            &format!("{BASIC}\ndefault-experiment: null\n")
        )
        .is_err()
    );
}

#[test]
fn public_schemas_accept_compact_profiles_and_explicit_worker_settings() {
    let catalog = public_schema_catalog().unwrap();
    let mut document: serde_json::Value = serde_saphyr::from_str(BASIC).unwrap();
    document["default-experiment"] = serde_json::json!("comparison");
    document["environments"] = serde_json::json!({"local": {"kind": "local", "environment-variables": {"OMP_NUM_THREADS": "1"}}});
    catalog
        .validate(PublicSchema::Manifest.repository_path(), &document)
        .unwrap();
    let policy = serde_json::json!({
        "version": 1, "kind": "profile", "metric": "gap", "direction": "at_most", "threshold": 1e-7,
        "max-observations": 4, "max-active-time": "1 s", "max-checkpoint-bytes": 4096,
        "implementations": {
            "fresh": {"delivery": "fresh_sequence", "controls": {"tolerance": {"type": "number", "exclusiveMinimum": 0}},
                "schedule": {"kind": "geometric", "field": "tolerance", "start": 0.1, "ratio": 0.1, "count": 3, "prefix": [{"initial": true}]}},
            "stream": {"delivery": "streaming_profile", "measurement-basis": "executor_observed",
                "check-schedule": {"kind": "geometric", "start": 1, "ratio": 2, "count": 3, "prefix": [0]}}
        }
    });
    catalog
        .validate(
            Path::new("schemas/v1/profile-observation-policy.schema.json"),
            &policy,
        )
        .unwrap();
}

#[test]
fn compact_profile_controls_include_prefixed_initial_observations() {
    use crate::core::profile::{CheckSchedule, FreshBinding};
    let binding: FreshBinding = serde_json::from_value(serde_json::json!({
        "delivery": "fresh_sequence",
        "controls": {"tolerance": {"type": "number", "exclusiveMinimum": 0}},
        "schedule": {"kind": "geometric", "field": "tolerance", "start": 0.1,
            "ratio": 0.1, "count": 3, "prefix": [{"initial": true}]}
    }))
    .unwrap();
    let controls = binding.schedule.expand(4).unwrap();
    assert_eq!(controls[0].as_json(), &serde_json::json!({"initial": true}));
    assert_eq!(
        controls[1].as_json(),
        &serde_json::json!({"tolerance": 0.1})
    );
    assert!(binding.schedule.expand(3).is_err());
    let schedule: CheckSchedule = serde_json::from_value(serde_json::json!({
        "kind": "geometric", "start": 1, "ratio": 2, "count": 4, "prefix": [0]
    }))
    .unwrap();
    assert_eq!(
        schedule,
        CheckSchedule::Explicit {
            indices: vec![0, 1, 2, 4, 8]
        }
    );
}

#[test]
fn metric_tolerances_preserve_structure_and_reject_invalid_bounds() {
    use crate::core::problem_contract::MetricTolerance;
    let tolerance: MetricTolerance =
        serde_json::from_value(serde_json::json!({"absolute": 1e-12, "relative": 1e-6})).unwrap();
    assert!(tolerance.matches(
        &serde_json::json!({"gap": 1e-13, "label": "ok"}),
        &serde_json::json!({"gap": 0, "label": "ok"})
    ));
    assert!(!tolerance.matches(
        &serde_json::json!({"gap": 1e-13}),
        &serde_json::json!({"gap": 0, "label": "ok"})
    ));
    assert!(!tolerance.matches(&serde_json::json!("1"), &serde_json::json!(1)));
    assert!(
        serde_json::from_value::<MetricTolerance>(serde_json::json!({"absolute": -1})).is_err()
    );
}

#[test]
fn checkpoint_shorthand_rejects_duplicate_or_unsafe_indices() {
    use crate::core::profile::CheckSchedule;
    for source in [
        serde_json::json!({"kind":"geometric", "start":1,"ratio":2,"count":4,"prefix":[1]}),
        serde_json::json!({"kind":"geometric", "start":1,"ratio":2,"count":54}),
        serde_json::json!({"kind":"geometric", "start":1,"ratio":1,"count":4}),
    ] {
        assert!(serde_json::from_value::<CheckSchedule>(source).is_err());
    }
}

#[test]
fn basic_functions_need_no_policies_artifact_schemas_or_scientific_rules() {
    let document: serde_json::Value = serde_saphyr::from_str(BASIC).unwrap();
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::Manifest.repository_path(), &document)
        .unwrap();
    let mut explicit_empty_sources = document.clone();
    explicit_empty_sources["implementations"]["loop"]["sources"] = serde_json::json!([]);
    public_schema_catalog()
        .unwrap()
        .validate(
            PublicSchema::Manifest.repository_path(),
            &explicit_empty_sources,
        )
        .unwrap();
    parse_manifest(
        Path::new("metewand.yaml"),
        &explicit_empty_sources.to_string(),
    )
    .unwrap();
    let manifest = parse_manifest(Path::new("metewand.yaml"), BASIC).unwrap();
    let experiment = &manifest.experiments[0];
    assert_eq!(
        manifest.execution_policies[&experiment.execution_policy].resolved_timing_scope(),
        TimingScope::PrepareAndExecute
    );
    let ContractDeclaration::Inline(contract) = &manifest.problems["sum"].contract else {
        panic!("inline contract");
    };
    assert_eq!(contract.family.as_str(), "sum");
    assert!(contract.reference_cases.is_empty());
    assert_eq!(contract.semantics.validity(), &serde_json::json!({}));
    assert_eq!(
        contract.semantics.one_shot_completion(),
        &serde_json::json!({})
    );
    assert_eq!(
        manifest.implementations["loop"].problem_contracts[0].as_str(),
        "sum"
    );
    let DatasetDefinition::Generated(dataset) = &manifest.datasets["numbers"] else {
        panic!("generated dataset");
    };
    let WorkerDefinition::Command(worker) = &dataset.worker else {
        panic!("lowered package launch");
    };
    assert_eq!(worker.program, "python3");
    assert!(worker.args.contains(&"{sources}/benchmark.py".to_owned()));
    assert_eq!(
        worker.sources.as_ref().unwrap()[0].as_path(),
        Path::new("benchmark.py")
    );
}

#[test]
fn function_defaults_normalize_like_explicit_declarations() {
    let explicit = BASIC
        .replace(
            "function: benchmark.py:solve",
            "function: benchmark.py:solve\n    environment: local\n    sources: [benchmark.py]\n    problem-contracts: [sum]\n    capabilities: [one_shot]",
        )
        .replace(
            "    result: {total: integer}",
            "    family: sum\n    allowed-timing-scopes: [prepare_and_execute]\n    supported-budgets: [none]\n    semantics: {preprocessing: {}, validity: {}, one_shot_completion: {}}\n    reference-cases: []\n    result: {total: integer}",
        );
    let basic = parse_manifest(Path::new("metewand.yaml"), BASIC).unwrap();
    let explicit = parse_manifest(Path::new("metewand.yaml"), &explicit).unwrap();
    assert_eq!(
        canonical_manifest_bytes(&basic).unwrap(),
        canonical_manifest_bytes(&explicit).unwrap()
    );
}

#[test]
fn omitted_policies_always_select_builtins_instead_of_custom_controls() {
    let source = format!(
        "{BASIC}\nexecution-policies:\n  custom: {{worker-reuse: true}}\nobservation-policies:\n  custom: {{kind: one_shot}}\n"
    );
    let manifest = parse_manifest(Path::new("metewand.yaml"), &source).unwrap();
    let experiment = &manifest.experiments[0];
    assert_ne!(experiment.execution_policy.as_str(), "custom");
    assert_ne!(experiment.observation_policy.as_str(), "custom");
    assert!(!manifest.execution_policies[&experiment.execution_policy].resolved_worker_reuse());
}

#[test]
fn function_support_requires_a_problem_binding_when_ambiguous() {
    let source = BASIC.replace("\nimplementations:\n", "\n  other:\n    result: {total: integer}\n    metrics: {error: number}\n    evaluator: {function: benchmark.py:evaluate}\nimplementations:\n");
    let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
    assert!(error.to_string().contains("problem-contracts"), "{error}");
}

#[test]
fn references_can_be_inline_and_have_an_expected_verdict() {
    let source = BASIC.replace("    evaluator:", "    reference-cases:\n      - dataset: {values: [1, 2]}\n        result: {total: 3}\n        expected-metrics: {error: 0}\n        expected-verdict: not_required\n    evaluator:");
    let manifest = parse_manifest(Path::new("metewand.yaml"), &source).unwrap();
    let document: serde_json::Value = serde_saphyr::from_str(&source).unwrap();
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::Manifest.repository_path(), &document)
        .unwrap();
    let ContractDeclaration::Inline(contract) = &manifest.problems["sum"].contract else {
        panic!("inline contract");
    };
    assert_eq!(contract.reference_cases.len(), 1);
}

#[test]
fn function_launches_reject_conflicting_low_level_fields() {
    for addition in [
        "runner: python",
        "program: python3",
        "entrypoint: other.py",
        "args: [bad]",
    ] {
        let source = BASIC.replace(
            "function: benchmark.py:solve",
            &format!("function: benchmark.py:solve\n    {addition}"),
        );
        let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
        assert!(error.to_string().contains("function"), "{error}");
    }
}
