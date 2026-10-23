use std::path::Path;

use metewand_core::{
    identity::{identify_record, record_id},
    manifest::{
        Enforcement, EnvironmentDefinition as ManifestEnvironmentDefinition, Manifest, PrimaryTime,
        RunOrder, TimingScope, parse_manifest,
    },
    manifest_hash::{ManifestHashError, canonical_manifest_bytes, hash_manifest},
    records::{
        EnvironmentDefinitionKind, EnvironmentDefinitionRecord, ExecutionPolicyRecord,
        ObservationPolicyRecord,
    },
};

const IMPLICIT_DEFAULTS: &str = r#"
version: 1
name: "stable"
environments:
  local:
    kind: "local"
problems:
  example:
    contract: "problems/example.yaml"
    evaluator:
      runner: "command"
      program: "evaluate"
      environment: "local"
implementations:
  solver:
    runner: "command"
    program: "solve"
    environment: "local"
    problem-contracts: ["example"]
    capabilities: ["one_shot"]
experiments:
  - name: "main"
    problem: "example"
    implementations: ["solver"]
    execution-policy: "default"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 1
    seed: 17
execution-policies:
  default:
    version: 1
observation-policies:
  final:
    version: 1
    kind: "one_shot"
"#;

const EXPLICIT_DEFAULTS_REORDERED: &str = r#"
# Formatting, comments, and mapping order are not hash inputs.
name: "stable"
version: 1
observation-policies:
  final:
    kind: "one_shot"
    version: 1
execution-policies:
  default:
    enforcement: "best_effort"
    max-concurrency: 1
    run-order: "sequential"
    primary-time: "timed_wall_time"
    timing-scope: "prepare_and_execute"
    warmup-runs: 0
    worker-reuse: false
    network: false
    version: 1
implementations:
  solver:
    capabilities: ["one_shot"]
    problem-contracts: ["example"]
    environment: "local"
    args: []
    program: "solve"
    runner: "command"
problems:
  example:
    contract: "problems/example.yaml"
    evaluator:
      environment: "local"
      args: []
      program: "evaluate"
      runner: "command"
environments:
  local:
    kind: "local"
experiments:
  - seed: 17
    measurement-repetitions: 1
    implementation-repetitions: 1
    observation-policy: "final"
    execution-policy: "default"
    implementations: ["solver"]
    problem: "example"
    name: "main"
"#;

fn parse(path: &str, source: &str) -> Manifest {
    parse_manifest(Path::new(path), source).unwrap()
}

fn component_ids(manifest: &Manifest) -> Vec<String> {
    let (environment_name, environment) = manifest
        .environments
        .get_key_value("local")
        .expect("fixture defines the local environment");
    assert!(matches!(
        environment,
        ManifestEnvironmentDefinition::Local {}
    ));
    let environment_id = identify_record(EnvironmentDefinitionRecord {
        name: environment_name.clone(),
        kind: EnvironmentDefinitionKind::Local,
    })
    .unwrap()
    .id
    .to_string();

    let (name, policy) = manifest
        .execution_policies
        .get_key_value("default")
        .unwrap();
    let execution_policy_id = identify_record(ExecutionPolicyRecord {
        max_concurrency: policy.resolved_max_concurrency(),
        name: name.clone(),
        cpus: policy.cpus,
        threads: policy.threads,
        memory: policy.memory.clone(),
        network: policy.resolved_network(),
        worker_reuse: policy.resolved_worker_reuse(),
        warmup_runs: policy.resolved_warmup_runs(),
        timeout: policy.timeout.clone(),
        timing_scope: policy.resolved_timing_scope(),
        primary_time: policy.resolved_primary_time(),
        run_order: policy.resolved_run_order(),
        enforcement: policy.resolved_enforcement(),
    })
    .unwrap()
    .id
    .to_string();

    let (name, policy) = manifest
        .observation_policies
        .get_key_value("final")
        .unwrap();
    let observation_policy_id = identify_record(ObservationPolicyRecord {
        name: name.clone(),
        kind: policy.kind,
    })
    .unwrap()
    .id
    .to_string();

    vec![environment_id, execution_policy_id, observation_policy_id]
}

#[test]
fn pins_the_version_one_manifest_hash_bytes() {
    let manifest = parse("metewand.yaml", "version: 1\nname: \"empty\"\n");

    assert_eq!(
        canonical_manifest_bytes(&manifest).unwrap(),
        br#"{"datasets":{},"environments":{},"execution_policies":{},"experiments":[],"implementations":{},"name":"empty","observation_policies":{},"problems":{},"version":1}"#
    );
    assert_eq!(
        hash_manifest(&manifest).unwrap().to_string(),
        "6f1ccf8407899e4f01fac61cd309f6837327482f6b61613d129d8d3d07372a4b"
    );
}

#[test]
fn hashes_the_defaulted_typed_manifest_instead_of_yaml_spelling() {
    let implicit = parse("metewand.yaml", IMPLICIT_DEFAULTS);
    let explicit = parse("bench/renamed-input.yaml", EXPLICIT_DEFAULTS_REORDERED);

    assert_eq!(
        hash_manifest(&implicit).unwrap(),
        hash_manifest(&explicit).unwrap()
    );

    let canonical = String::from_utf8(canonical_manifest_bytes(&implicit).unwrap()).unwrap();
    assert!(canonical.contains(
        r#""default":{"cpus":null,"enforcement":"best_effort","max_concurrency":1,"memory":null,"network":false,"primary_time":"timed_wall_time","run_order":"sequential","threads":null,"timeout":null,"timing_scope":"prepare_and_execute","version":1,"warmup_runs":0,"worker_reuse":false}"#
    ));
    assert!(!canonical.contains("metewand.yaml"));
    assert!(!canonical.contains("Formatting"));
}

#[test]
fn concurrency_is_an_explicit_default_and_a_manifest_hash_dependency() {
    let serial = parse("metewand.yaml", IMPLICIT_DEFAULTS);
    let explicit = parse(
        "metewand.yaml",
        &IMPLICIT_DEFAULTS.replace("  default:", "  default:\n    max-concurrency: 1"),
    );
    let concurrent = parse(
        "metewand.yaml",
        &IMPLICIT_DEFAULTS.replace("  default:", "  default:\n    max-concurrency: 2"),
    );
    assert_eq!(
        hash_manifest(&serial).unwrap(),
        hash_manifest(&explicit).unwrap()
    );
    assert_ne!(
        hash_manifest(&serial).unwrap(),
        hash_manifest(&concurrent).unwrap()
    );
    assert_eq!(component_ids(&serial)[0], component_ids(&concurrent)[0]);
    assert_ne!(component_ids(&serial)[1], component_ids(&concurrent)[1]);
}

#[test]
fn hashes_every_version_one_manifest_variant() {
    let manifest = parse(
        "metewand.yaml",
        include_str!("../../../fixtures/manifest/v1/complete.yaml"),
    );

    let canonical = canonical_manifest_bytes(&manifest).unwrap();
    assert_eq!(hash_manifest(&manifest).unwrap().to_string().len(), 64);
    assert_eq!(canonical.first(), Some(&b'{'));
    assert_eq!(canonical.last(), Some(&b'}'));
}

#[test]
fn refuses_to_hash_a_manifest_before_name_resolution() {
    let manifest = parse(
        "metewand.yaml",
        r#"
version: 1
name: "unresolved"
problems:
  example:
    contract: "problems/example.yaml"
    evaluator:
      runner: "command"
      program: "evaluate"
      environment: "missing"
"#,
    );

    assert!(matches!(
        hash_manifest(&manifest),
        Err(ManifestHashError::UndefinedReference {
            field: "environment",
            ref target,
            ..
        }) if target == "missing"
    ));
}

#[test]
fn resolves_every_manifest_reference_namespace_before_hashing() {
    let cases = [
        (
            "implementation contract",
            IMPLICIT_DEFAULTS.replace(
                "    problem-contracts: [\"example\"]",
                "    problem-contracts: [\"missing\"]",
            ),
            "problem_contracts",
            "missing",
        ),
        (
            "experiment problem",
            IMPLICIT_DEFAULTS.replace("    problem: \"example\"", "    problem: \"missing\""),
            "problem",
            "missing",
        ),
        (
            "experiment dataset",
            IMPLICIT_DEFAULTS.replace(
                "    problem: \"example\"\n    implementations: [\"solver\"]",
                "    problem: \"example\"\n    datasets: [\"missing\"]\n    implementations: [\"solver\"]",
            ),
            "datasets",
            "missing",
        ),
        (
            "experiment implementation",
            IMPLICIT_DEFAULTS.replace(
                "    implementations: [\"solver\"]",
                "    implementations: [\"missing\"]",
            ),
            "implementations",
            "missing",
        ),
        (
            "execution policy",
            IMPLICIT_DEFAULTS.replace(
                "    execution-policy: \"default\"",
                "    execution-policy: \"missing\"",
            ),
            "execution_policy",
            "missing",
        ),
        (
            "observation policy",
            IMPLICIT_DEFAULTS.replace(
                "    observation-policy: \"final\"",
                "    observation-policy: \"missing\"",
            ),
            "observation_policy",
            "missing",
        ),
        (
            "dataset parameter namespace",
            IMPLICIT_DEFAULTS.replace(
                "    seed: 17",
                "    seed: 17\n    dataset-parameters:\n      missing:\n        rows: { value: 1 }",
            ),
            "dataset_parameters",
            "missing",
        ),
        (
            "implementation parameter namespace",
            IMPLICIT_DEFAULTS.replace(
                "    seed: 17",
                "    seed: 17\n    implementation-parameters:\n      missing:\n        tolerance: { value: 1 }",
            ),
            "implementation_parameters",
            "missing",
        ),
    ];

    for (description, source, expected_field, expected_target) in cases {
        let manifest = parse("metewand.yaml", &source);
        let error = hash_manifest(&manifest).expect_err(description);
        assert!(
            matches!(
                error,
                ManifestHashError::UndefinedReference {
                    field,
                    ref target,
                    ..
                } if field == expected_field && target == expected_target
            ),
            "unexpected error for {description}: {error}"
        );
    }
}

#[test]
fn unrelated_experiments_change_only_the_whole_manifest_hash() {
    let original = parse("metewand.yaml", IMPLICIT_DEFAULTS);
    let extended = parse(
        "metewand.yaml",
        &IMPLICIT_DEFAULTS.replace(
            "execution-policies:",
            r#"  - name: "additional"
    problem: "example"
    implementations: ["solver"]
    execution-policy: "default"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 1
    seed: 99
execution-policies:
"#,
        ),
    );

    assert_ne!(
        hash_manifest(&original).unwrap(),
        hash_manifest(&extended).unwrap()
    );
    assert_eq!(component_ids(&original), component_ids(&extended));

    let policy = ExecutionPolicyRecord {
        max_concurrency: 1,
        name: original
            .execution_policies
            .get_key_value("default")
            .unwrap()
            .0
            .clone(),
        cpus: None,
        threads: None,
        memory: None,
        network: false,
        worker_reuse: false,
        warmup_runs: 0,
        timeout: None,
        timing_scope: TimingScope::PrepareAndExecute,
        primary_time: PrimaryTime::TimedWallTime,
        run_order: RunOrder::Sequential,
        enforcement: Enforcement::BestEffort,
    };
    assert_eq!(
        record_id(&policy).unwrap().to_string(),
        component_ids(&original)[1]
    );
}
