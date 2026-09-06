use std::path::Path;

use metewand_core::{
    identity::{identify_record, record_id},
    manifest::{Enforcement, Manifest, PrimaryTime, RunOrder, TimingScope, parse_manifest},
    manifest_hash::{ManifestHashError, canonical_manifest_bytes, hash_manifest},
    records::ExecutionPolicyRecord,
};

const IMPLICIT_DEFAULTS: &str = r#"
version = 1
name = "stable"

[environments.local]
kind = "local"

[problems.example]
contract = "problems/example.toml"

[problems.example.evaluator]
runner = "command"
program = "evaluate"
environment = "local"

[implementations.solver]
runner = "command"
program = "solve"
environment = "local"
problem_contracts = ["example"]
capabilities = ["one_shot"]

[[experiments]]
name = "main"
problem = "example"
implementations = ["solver"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 17

[execution_policies.default]
version = 1

[observation_policies.final]
version = 1
kind = "one_shot"
"#;

const EXPLICIT_DEFAULTS_REORDERED: &str = r#"
# Formatting, comments, and table order are not hash inputs.
name = "stable"
version = 1

[observation_policies.final]
kind = "one_shot"
version = 1

[execution_policies.default]
enforcement = "best_effort"
run_order = "sequential"
primary_time = "timed_wall_time"
timing_scope = "prepare_and_execute"
warmup_runs = 0
worker_reuse = false
network = false
version = 1

[implementations.solver]
capabilities = ["one_shot"]
problem_contracts = ["example"]
environment = "local"
args = []
program = "solve"
runner = "command"

[problems.example]
contract = "problems/example.toml"

[problems.example.evaluator]
environment = "local"
args = []
program = "evaluate"
runner = "command"

[environments.local]
kind = "local"

[[experiments]]
seed = 17
measurement_repetitions = 1
implementation_repetitions = 1
observation_policy = "final"
execution_policy = "default"
implementations = ["solver"]
problem = "example"
name = "main"
"#;

fn parse(path: &str, source: &str) -> Manifest {
    parse_manifest(Path::new(path), source).unwrap()
}

fn execution_policy_id(manifest: &Manifest) -> String {
    let (name, policy) = manifest
        .execution_policies
        .get_key_value("default")
        .unwrap();
    identify_record(ExecutionPolicyRecord {
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
    .to_string()
}

#[test]
fn pins_the_version_one_manifest_hash_bytes() {
    let manifest = parse("metewand.toml", "version = 1\nname = \"empty\"\n");

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
fn hashes_the_defaulted_typed_manifest_instead_of_toml_spelling() {
    let implicit = parse("metewand.toml", IMPLICIT_DEFAULTS);
    let explicit = parse("bench/renamed-input.toml", EXPLICIT_DEFAULTS_REORDERED);

    assert_eq!(
        hash_manifest(&implicit).unwrap(),
        hash_manifest(&explicit).unwrap()
    );

    let canonical = String::from_utf8(canonical_manifest_bytes(&implicit).unwrap()).unwrap();
    assert!(canonical.contains(
        r#""default":{"cpus":null,"enforcement":"best_effort","memory":null,"network":false,"primary_time":"timed_wall_time","run_order":"sequential","threads":null,"timeout":null,"timing_scope":"prepare_and_execute","version":1,"warmup_runs":0,"worker_reuse":false}"#
    ));
    assert!(!canonical.contains("metewand.toml"));
    assert!(!canonical.contains("Formatting"));
}

#[test]
fn hashes_every_version_one_manifest_variant() {
    let manifest = parse(
        "metewand.toml",
        include_str!("../../../fixtures/manifest/v1/complete.toml"),
    );

    let canonical = canonical_manifest_bytes(&manifest).unwrap();
    assert_eq!(hash_manifest(&manifest).unwrap().to_string().len(), 64);
    assert_eq!(canonical.first(), Some(&b'{'));
    assert_eq!(canonical.last(), Some(&b'}'));
}

#[test]
fn refuses_to_hash_a_manifest_before_name_resolution() {
    let manifest = parse(
        "metewand.toml",
        r#"
version = 1
name = "unresolved"

[problems.example]
contract = "problems/example.toml"
[problems.example.evaluator]
runner = "command"
program = "evaluate"
environment = "missing"
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
                "problem_contracts = [\"example\"]",
                "problem_contracts = [\"missing\"]",
            ),
            "problem_contracts",
            "missing",
        ),
        (
            "experiment problem",
            IMPLICIT_DEFAULTS.replace("problem = \"example\"", "problem = \"missing\""),
            "problem",
            "missing",
        ),
        (
            "experiment dataset",
            IMPLICIT_DEFAULTS.replace(
                "problem = \"example\"\nimplementations",
                "problem = \"example\"\ndatasets = [\"missing\"]\nimplementations",
            ),
            "datasets",
            "missing",
        ),
        (
            "experiment implementation",
            IMPLICIT_DEFAULTS.replace(
                "implementations = [\"solver\"]",
                "implementations = [\"missing\"]",
            ),
            "implementations",
            "missing",
        ),
        (
            "execution policy",
            IMPLICIT_DEFAULTS.replace(
                "execution_policy = \"default\"",
                "execution_policy = \"missing\"",
            ),
            "execution_policy",
            "missing",
        ),
        (
            "observation policy",
            IMPLICIT_DEFAULTS.replace(
                "observation_policy = \"final\"",
                "observation_policy = \"missing\"",
            ),
            "observation_policy",
            "missing",
        ),
        (
            "dataset parameter namespace",
            IMPLICIT_DEFAULTS.replace(
                "seed = 17",
                "seed = 17\n\n[experiments.dataset_parameters.missing]\nrows = { value = 1 }",
            ),
            "dataset_parameters",
            "missing",
        ),
        (
            "implementation parameter namespace",
            IMPLICIT_DEFAULTS.replace(
                "seed = 17",
                "seed = 17\n\n[experiments.implementation_parameters.missing]\ntolerance = { value = 1 }",
            ),
            "implementation_parameters",
            "missing",
        ),
    ];

    for (description, source, expected_field, expected_target) in cases {
        let manifest = parse("metewand.toml", &source);
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
    let original = parse("metewand.toml", IMPLICIT_DEFAULTS);
    let extended = parse(
        "metewand.toml",
        &format!(
            "{IMPLICIT_DEFAULTS}\n{}",
            r#"
[[experiments]]
name = "additional"
problem = "example"
implementations = ["solver"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 99
"#
        ),
    );

    assert_ne!(
        hash_manifest(&original).unwrap(),
        hash_manifest(&extended).unwrap()
    );
    assert_eq!(
        execution_policy_id(&original),
        execution_policy_id(&extended)
    );

    let policy = ExecutionPolicyRecord {
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
        execution_policy_id(&original)
    );
}
