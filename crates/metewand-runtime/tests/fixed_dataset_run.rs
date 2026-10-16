#![cfg(unix)]

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, Instant},
};

use metewand_core::{
    identity::IdentityKind,
    materialization::hex,
    records::{
        ContentDigest, RecordId, ResolvedEnvironmentRecord, ResolvedLaunchRecord,
        WorkerWorkingDirectory,
    },
    schema::SchemaCatalog,
};
use metewand_protocol::WorkerIdentity;
use metewand_runtime::{
    fixed_dataset_run::{
        FixedDatasetFailureKind, FixedDatasetRun, FixedDatasetRunError, FixedDatasetRunOutcome,
        run_fixed_dataset, run_fixed_dataset_outcome,
    },
    worker_process::WorkerLogLimits,
    worker_session::{PhaseTimeouts, WorkerPhase, WorkerSessionError},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn fixed_dataset_runs_through_implementation_and_independent_evaluator() {
    let private_root = TempDir::new().unwrap();
    let schemas = fixture_schema_catalog();
    let problem_parameters = json!({"offset": 2});
    let implementation_parameters = json!({"algorithm": "sum"});
    let implementation = launch("implementation");
    let mut evaluator = launch("problem-evaluator");
    evaluator.environment_variables.insert(
        "METEWAND_TEST_EVALUATE_DELAY_MS".to_owned(),
        "250".to_owned(),
    );
    let worker_id = WorkerIdentity::from_str(WORKER_ID).unwrap();

    let whole_call_started = Instant::now();
    let result = run_fixed_dataset(FixedDatasetRun {
        dataset_dir: &fixture_path("fixed-dataset"),
        problem_contract_id: "mw1-problem-contract-fixture",
        problem_parameters: &problem_parameters,
        implementation_parameters: &implementation_parameters,
        implementation_seed: 23,
        implementation_launch: &implementation,
        implementation_identity: &worker_id,
        evaluator_launch: &evaluator,
        evaluator_identity: &worker_id,
        result_schema: Path::new("schemas/result.json"),
        metric_schema: Path::new("schemas/metrics.json"),
        schemas: &schemas,
        private_root: private_root.path(),
        timeouts: timeouts(),
        log_limits: WorkerLogLimits::new(4096, 4096),
    })
    .unwrap();
    let whole_call_elapsed = whole_call_started.elapsed();

    assert_eq!(result.result.as_json(), &json!({"answer": 8}));
    assert_eq!(result.metrics.as_json(), &json!({"absolute_error": 0}));
    assert_eq!(result.statistics, json!({"input_count": 3}));
    assert!(result.timed_wall_time > Duration::ZERO);
    assert!(whole_call_elapsed >= result.timed_wall_time + Duration::from_millis(200));
    assert_eq!(result.implementation_time_ns, None);
    assert!(result.result_directory().join("result.json").is_file());
    assert!(result.metrics_document().is_file());
    assert_eq!(
        result.validated_result().manifest_path(),
        Path::new("result.json")
    );
    assert_eq!(fs::read_dir(private_root.path()).unwrap().count(), 2);
    drop(result);
    assert_eq!(fs::read_dir(private_root.path()).unwrap().count(), 0);
}

#[test]
fn rejected_prepare_does_not_leave_private_output() {
    let private_root = TempDir::new().unwrap();
    let schemas = fixture_schema_catalog();
    let problem_parameters = json!({"offset": 2});
    let implementation_parameters = json!({"algorithm": "wrong"});
    let implementation = launch("implementation");
    let evaluator = launch("problem-evaluator");
    let worker_id = WorkerIdentity::from_str(WORKER_ID).unwrap();

    let error = run_fixed_dataset(FixedDatasetRun {
        dataset_dir: &fixture_path("fixed-dataset"),
        problem_contract_id: "mw1-problem-contract-fixture",
        problem_parameters: &problem_parameters,
        implementation_parameters: &implementation_parameters,
        implementation_seed: 23,
        implementation_launch: &implementation,
        implementation_identity: &worker_id,
        evaluator_launch: &evaluator,
        evaluator_identity: &worker_id,
        result_schema: Path::new("schemas/result.json"),
        metric_schema: Path::new("schemas/metrics.json"),
        schemas: &schemas,
        private_root: private_root.path(),
        timeouts: timeouts(),
        log_limits: WorkerLogLimits::new(4096, 4096),
    })
    .unwrap_err();

    assert!(matches!(
        error,
        FixedDatasetRunError::Implementation {
            source: WorkerSessionError::WorkerFailure {
                phase: WorkerPhase::Prepare,
                ..
            }
        }
    ));
    assert_eq!(fs::read_dir(private_root.path()).unwrap().count(), 0);
}

#[test]
fn caller_owned_datasets_are_checked_around_every_worker_operation() {
    for (evaluator, phase) in [
        (false, "hello"),
        (false, "prepare"),
        (false, "execute"),
        (true, "hello"),
        (true, "evaluate"),
        (false, "shutdown"),
        (true, "shutdown"),
    ] {
        let (_root, outcome) = run_mutation_case(evaluator, phase, "bytes", None);
        let FixedDatasetRunOutcome::Failed { kind, diagnostic } = outcome else {
            panic!("mutation during {phase} must invalidate the attempt");
        };
        assert_eq!(kind, FixedDatasetFailureKind::DatasetMutation);
        assert_eq!(diagnostic.code, "dataset_mutation");
        assert!(diagnostic.message.contains(phase), "{diagnostic:?}");
    }
}

#[test]
fn dataset_mutations_override_worker_failure_crash_timeout_and_invalid_output() {
    for (phase, mutation, worker_mode) in [
        ("prepare", "missing", None),
        ("prepare", "symlink", None),
        ("execute", "directory", None),
        ("execute", "mode", None),
        ("execute", "bytes", Some("crash")),
        ("execute", "bytes", Some("timeout")),
        ("execute", "bytes", Some("malformed_protocol")),
        ("execute", "bytes", Some("invalid_result")),
    ] {
        let (_root, outcome) = run_mutation_case(false, phase, mutation, worker_mode);
        assert!(
            matches!(
                outcome,
                FixedDatasetRunOutcome::Failed {
                    kind: FixedDatasetFailureKind::DatasetMutation,
                    ..
                }
            ),
            "{phase}/{mutation}/{worker_mode:?}: {outcome:?}"
        );
    }
}

#[test]
fn caller_owned_dataset_links_are_rejected_before_launch() {
    let (_root, outcome) = run_mutation_case(false, "hello", "initial_symlink", None);
    let FixedDatasetRunOutcome::Failed { kind, diagnostic } = outcome else {
        panic!("a linked dataset must not reach a worker");
    };
    assert_eq!(kind, FixedDatasetFailureKind::DatasetMutation);
    assert!(
        diagnostic.message.contains("attempt start"),
        "{diagnostic:?}"
    );
    assert!(diagnostic.message.contains("unsafe"), "{diagnostic:?}");
}

fn run_mutation_case(
    mutate_evaluator: bool,
    phase: &str,
    mutation: &str,
    worker_mode: Option<&str>,
) -> (TempDir, FixedDatasetRunOutcome) {
    let private_root = TempDir::new().unwrap();
    let dataset = TempDir::new().unwrap();
    fs::copy(
        fixture_path("fixed-dataset/data.json"),
        dataset.path().join("data.json"),
    )
    .unwrap();
    if mutation == "initial_symlink" {
        fs::remove_file(dataset.path().join("data.json")).unwrap();
        std::os::unix::fs::symlink(
            fixture_path("fixed-dataset/data.json"),
            dataset.path().join("data.json"),
        )
        .unwrap();
    }
    let schemas = fixture_schema_catalog();
    let mut implementation = launch("implementation");
    let mut evaluator = launch("problem-evaluator");
    let launch = if mutate_evaluator {
        &mut evaluator
    } else {
        &mut implementation
    };
    for (key, value) in [
        ("METEWAND_TEST_MUTATION_PHASE", phase),
        (
            "METEWAND_TEST_MUTATION_PATH",
            dataset.path().join("data.json").to_str().unwrap(),
        ),
        ("METEWAND_TEST_MUTATION_KIND", mutation),
    ] {
        launch
            .environment_variables
            .insert(key.to_owned(), value.to_owned());
    }
    let mut deadlines = timeouts();
    if let Some(mode) = worker_mode {
        launch
            .environment_variables
            .insert("METEWAND_TEST_EXECUTE_MODE".to_owned(), mode.to_owned());
        if mode == "timeout" {
            deadlines.execute = Duration::from_millis(50);
        }
    }
    let worker_id = WorkerIdentity::from_str(WORKER_ID).unwrap();
    let outcome = run_fixed_dataset_outcome(FixedDatasetRun {
        dataset_dir: dataset.path(),
        problem_contract_id: "mw1-problem-contract-fixture",
        problem_parameters: &json!({"offset": 2}),
        implementation_parameters: &json!({"algorithm": "sum"}),
        implementation_seed: 23,
        implementation_launch: &implementation,
        implementation_identity: &worker_id,
        evaluator_launch: &evaluator,
        evaluator_identity: &worker_id,
        result_schema: Path::new("schemas/result.json"),
        metric_schema: Path::new("schemas/metrics.json"),
        schemas: &schemas,
        private_root: private_root.path(),
        timeouts: deadlines,
        log_limits: WorkerLogLimits::new(4096, 4096),
    });
    assert_eq!(fs::read_dir(private_root.path()).unwrap().count(), 0);
    (private_root, outcome)
}

#[test]
fn evaluator_rejects_a_schema_valid_but_incorrect_result() {
    let (_private_root, outcome) =
        run_case(Some(("METEWAND_TEST_ANSWER_OFFSET", "1")), None, timeouts());
    assert!(matches!(
        outcome,
        FixedDatasetRunOutcome::Failed {
            kind: FixedDatasetFailureKind::InvalidResult,
            ..
        }
    ));
}

#[test]
fn timing_includes_prepare_and_result_writing() {
    let (_private_root, outcome) = run_case(
        Some(("METEWAND_TEST_TIMING_DELAY_MS", "110")),
        None,
        timeouts(),
    );
    let FixedDatasetRunOutcome::Validated(result) = outcome else {
        panic!("the delayed implementation should complete");
    };
    assert!(result.timed_wall_time >= Duration::from_millis(200));
}

#[test]
fn timing_excludes_implementation_startup_evaluator_startup_and_evaluation() {
    for (implementation_mode, evaluator_mode) in [
        (Some(("METEWAND_TEST_STARTUP_DELAY_MS", "300")), None),
        (None, Some(("METEWAND_TEST_STARTUP_DELAY_MS", "300"))),
        (None, Some(("METEWAND_TEST_EVALUATE_DELAY_MS", "300"))),
    ] {
        let started = Instant::now();
        let (_private_root, outcome) = run_case(implementation_mode, evaluator_mode, timeouts());
        let elapsed = started.elapsed();
        let FixedDatasetRunOutcome::Validated(result) = outcome else {
            panic!("the delayed worker should complete");
        };
        assert!(
            elapsed >= result.timed_wall_time + Duration::from_millis(280),
            "delay must remain outside the timed interval: {implementation_mode:?}, {evaluator_mode:?}"
        );
    }
}

#[test]
fn one_shot_failures_keep_distinct_typed_diagnostics() {
    let cases = [
        (
            Some(("METEWAND_TEST_EXECUTE_MODE", "crash")),
            None,
            timeouts(),
            FixedDatasetFailureKind::ImplementationCrash,
        ),
        (
            Some(("METEWAND_TEST_EXECUTE_MODE", "invalid_result")),
            None,
            timeouts(),
            FixedDatasetFailureKind::InvalidResult,
        ),
        (
            Some(("METEWAND_TEST_EXECUTE_MODE", "malformed_protocol")),
            None,
            timeouts(),
            FixedDatasetFailureKind::ProtocolError,
        ),
        (
            None,
            Some(("METEWAND_TEST_EVALUATE_MODE", "failure")),
            timeouts(),
            FixedDatasetFailureKind::EvaluatorFailure,
        ),
        (
            None,
            Some(("METEWAND_TEST_EVALUATE_MODE", "invalid_metrics")),
            timeouts(),
            FixedDatasetFailureKind::EvaluatorFailure,
        ),
        (
            None,
            Some(("METEWAND_TEST_EVALUATE_MODE", "mutate_result")),
            timeouts(),
            FixedDatasetFailureKind::InvalidResult,
        ),
        (
            Some(("METEWAND_TEST_EXECUTE_MODE", "timeout")),
            None,
            PhaseTimeouts {
                execute: Duration::from_millis(20),
                ..timeouts()
            },
            FixedDatasetFailureKind::PhaseTimeout,
        ),
    ];

    for (implementation_mode, evaluator_mode, deadlines, expected) in cases {
        let (_private_root, outcome) = run_case(implementation_mode, evaluator_mode, deadlines);
        let FixedDatasetRunOutcome::Failed { kind, diagnostic } = outcome else {
            panic!("expected a failed one-shot outcome");
        };
        assert_eq!(kind, expected);
        assert_eq!(diagnostic.code, kind.code());
        assert!(!diagnostic.message.is_empty());
    }
}

fn run_case(
    implementation_mode: Option<(&str, &str)>,
    evaluator_mode: Option<(&str, &str)>,
    deadlines: PhaseTimeouts,
) -> (TempDir, FixedDatasetRunOutcome) {
    let private_root = TempDir::new().unwrap();
    let schemas = fixture_schema_catalog();
    let problem_parameters = json!({"offset": 2});
    let implementation_parameters = json!({"algorithm": "sum"});
    let mut implementation = launch("implementation");
    let mut evaluator = launch("problem-evaluator");
    if let Some((name, value)) = implementation_mode {
        implementation
            .environment_variables
            .insert(name.into(), value.into());
    }
    if let Some((name, value)) = evaluator_mode {
        evaluator
            .environment_variables
            .insert(name.into(), value.into());
    }
    let worker_id = WorkerIdentity::from_str(WORKER_ID).unwrap();

    let outcome = run_fixed_dataset_outcome(FixedDatasetRun {
        dataset_dir: &fixture_path("fixed-dataset"),
        problem_contract_id: "mw1-problem-contract-fixture",
        problem_parameters: &problem_parameters,
        implementation_parameters: &implementation_parameters,
        implementation_seed: 23,
        implementation_launch: &implementation,
        implementation_identity: &worker_id,
        evaluator_launch: &evaluator,
        evaluator_identity: &worker_id,
        result_schema: Path::new("schemas/result.json"),
        metric_schema: Path::new("schemas/metrics.json"),
        schemas: &schemas,
        private_root: private_root.path(),
        timeouts: deadlines,
        log_limits: WorkerLogLimits::new(4096, 4096),
    });
    if matches!(outcome, FixedDatasetRunOutcome::Failed { .. }) {
        assert_eq!(fs::read_dir(private_root.path()).unwrap().count(), 0);
    }
    (private_root, outcome)
}

fn launch(name: &str) -> ResolvedLaunchRecord {
    ResolvedLaunchRecord {
        environment: record_id::<ResolvedEnvironmentRecord>(1),
        program: fixture_path(name).to_str().unwrap().to_owned(),
        args: Vec::new(),
        environment_variables: BTreeMap::from([("PATH".to_owned(), env::var("PATH").unwrap())]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    }
}

fn record_id<T: IdentityKind>(byte: u8) -> RecordId<T> {
    RecordId::from_digest([byte; 32])
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/worker-protocol/v1")
        .join(name)
}

fn fixture_schema_catalog() -> SchemaCatalog {
    SchemaCatalog::try_new(["result.json", "metrics.json"].into_iter().map(|name| {
        let logical_path = PathBuf::from("schemas").join(name);
        let bytes = fs::read(fixture_path("schemas").join(name)).unwrap();
        (
            logical_path,
            serde_json::from_slice::<Value>(&bytes).unwrap(),
        )
    }))
    .unwrap()
}

fn timeouts() -> PhaseTimeouts {
    PhaseTimeouts {
        hello: Duration::from_secs(2),
        materialize: Duration::from_secs(2),
        prepare: Duration::from_secs(2),
        execute: Duration::from_secs(2),
        reset: Duration::from_secs(2),
        evaluate: Duration::from_secs(2),
        shutdown: Duration::from_secs(2),
    }
}

#[test]
fn valid_results_keep_the_evaluator_completion_decision() {
    use metewand_core::records::CompletionStatus;
    for (verdict, expected) in [
        ("reached", CompletionStatus::Reached),
        ("not_reached", CompletionStatus::NotReached),
        ("not_required", CompletionStatus::NotRequired),
    ] {
        let (_root, outcome) = run_case(None, Some(("METEWAND_TEST_VERDICT", verdict)), timeouts());
        let FixedDatasetRunOutcome::Validated(result) = outcome else {
            panic!("valid evaluation should retain a result");
        };
        assert_eq!(result.completion, expected);
    }
    let (_root, outcome) = run_case(None, Some(("METEWAND_TEST_VERDICT", "unknown")), timeouts());
    assert!(matches!(
        outcome,
        FixedDatasetRunOutcome::Failed {
            kind: FixedDatasetFailureKind::ProtocolError,
            ..
        }
    ));
}

#[test]
fn finalized_one_shot_has_independent_hash_complete_directories() {
    use metewand_core::{
        canonical::CanonicalValue,
        public_schemas::{PublicSchema, public_schema_catalog},
        records::RunAttemptOutcome,
    };
    use metewand_runtime::publication::{
        PublicationRequest, finalize_fixed_dataset, verify_finalized_directory,
    };

    let (_private, outcome) = run_case(
        Some(("METEWAND_TEST_RESULT_PAYLOAD", "1")),
        None,
        timeouts(),
    );
    let FixedDatasetRunOutcome::Validated(result) = outcome else {
        panic!("expected valid result")
    };
    let store = TempDir::new().unwrap();
    let published = finalize_fixed_dataset(
        &result,
        PublicationRequest {
            root: store.path(),
            attempt_slot: record_id(2),
            observation_slot: record_id(3),
            retry_index: 0,
            provenance_data: CanonicalValue::try_from(json!({"repository_commit": null})).unwrap(),
        },
    )
    .unwrap();
    assert!(
        matches!(published.attempt.record.outcome, RunAttemptOutcome::Accepted { observation } if observation == published.observation.id)
    );
    let schemas = public_schema_catalog().unwrap();
    assert_eq!(
        published.attempt.record.provenance.data.as_json()["execution"]["dataset_immutability"],
        json!({
            "view": "caller_provided", "enforcement": "best_effort", "verification": "each_worker_operation",
        })
    );
    for (directory, document, schema) in [
        (
            &published.observation_directory,
            "observation.json",
            PublicSchema::Observation,
        ),
        (
            &published.attempt_directory,
            "attempt.json",
            PublicSchema::Attempt,
        ),
    ] {
        verify_finalized_directory(directory).unwrap();
        let record: Value =
            serde_json::from_slice(&fs::read(directory.join(document)).unwrap()).unwrap();
        schemas.validate(schema.repository_path(), &record).unwrap();
        let bytes = fs::read(directory.join("manifest.json")).unwrap();
        assert_eq!(
            bytes,
            CanonicalValue::from_slice(&bytes)
                .unwrap()
                .to_canonical_bytes()
                .unwrap()
        );
        let marker: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(marker["complete"], true);
        let expected = if document == "observation.json" {
            vec![
                "metrics.json",
                "observation.json",
                "result/arrays/values.bin",
                "result/result.json",
            ]
        } else {
            vec![
                "attempt.json",
                "logs/implementation/stderr.log",
                "logs/implementation/stdout.log",
                "logs/problem-evaluator/stderr.log",
                "logs/problem-evaluator/stdout.log",
            ]
        };
        let files = marker["files"].as_array().unwrap();
        assert_eq!(
            files
                .iter()
                .map(|entry| entry["path"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        for entry in files {
            use sha2::{Digest, Sha256};
            let content = fs::read(directory.join(entry["path"].as_str().unwrap())).unwrap();
            assert_eq!(entry["size_bytes"], content.len());
            assert_eq!(
                entry["sha256"],
                hex(ContentDigest::new(Sha256::digest(&content).into()))
            );
        }
        assert!(
            marker["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == document)
        );
    }
    assert_eq!(
        fs::read(published.observation_directory.join("metrics.json")).unwrap(),
        fs::read(result.metrics_document()).unwrap()
    );
    assert_eq!(
        fs::read(published.observation_directory.join("result/result.json")).unwrap(),
        fs::read(result.result_directory().join("result.json")).unwrap()
    );
    for role in ["implementation", "problem-evaluator"] {
        for stream in ["stdout", "stderr"] {
            assert!(
                published
                    .attempt_directory
                    .join(format!("logs/{role}/{stream}.log"))
                    .is_file()
            );
        }
    }
    drop(result);
    assert_eq!(
        fs::read(
            published
                .observation_directory
                .join("result/arrays/values.bin")
        )
        .unwrap(),
        (0..=255).collect::<Vec<u8>>()
    );
    verify_finalized_directory(&published.observation_directory).unwrap();
    fs::write(published.observation_directory.join("metrics.json"), b"{}").unwrap();
    assert!(verify_finalized_directory(&published.observation_directory).is_err());
}

#[test]
fn a_missed_completion_target_keeps_an_observation_in_a_failed_attempt() {
    use metewand_core::{canonical::CanonicalValue, records::RunAttemptOutcome};
    use metewand_runtime::publication::{PublicationRequest, finalize_fixed_dataset};
    let (_private, outcome) = run_case(
        None,
        Some(("METEWAND_TEST_VERDICT", "not_reached")),
        timeouts(),
    );
    let FixedDatasetRunOutcome::Validated(result) = outcome else {
        panic!("expected valid result")
    };
    let store = TempDir::new().unwrap();
    let published = finalize_fixed_dataset(
        &result,
        PublicationRequest {
            root: store.path(),
            attempt_slot: record_id(2),
            observation_slot: record_id(3),
            retry_index: 0,
            provenance_data: CanonicalValue::try_from(json!({})).unwrap(),
        },
    )
    .unwrap();
    assert!(
        matches!(published.attempt.record.outcome, RunAttemptOutcome::Failed { observation: Some(id), .. } if id == published.observation.id)
    );
}

#[test]
fn changed_private_outputs_cannot_be_published() {
    use metewand_core::canonical::CanonicalValue;
    use metewand_runtime::publication::{
        PublicationError, PublicationRequest, finalize_fixed_dataset,
    };
    for changed in ["result", "metrics"] {
        let (_private, outcome) = run_case(None, None, timeouts());
        let FixedDatasetRunOutcome::Validated(result) = outcome else {
            panic!("expected valid result")
        };
        let path = if changed == "result" {
            result.result_directory().join("result.json")
        } else {
            result.metrics_document()
        };
        fs::write(path, b"{}").unwrap();
        let store = TempDir::new().unwrap();
        assert!(matches!(
            finalize_fixed_dataset(
                &result,
                PublicationRequest {
                    root: store.path(),
                    attempt_slot: record_id(2),
                    observation_slot: record_id(3),
                    retry_index: 0,
                    provenance_data: CanonicalValue::try_from(json!({})).unwrap(),
                }
            ),
            Err(PublicationError::ModifiedOutput)
        ));
        assert_eq!(
            fs::read_dir(store.path().join("observations"))
                .unwrap()
                .count(),
            0
        );
        assert!(!store.path().join("attempts").exists());
    }
}

#[test]
fn a_metrics_symlink_cannot_be_published_even_if_its_bytes_match() {
    use metewand_core::canonical::CanonicalValue;
    use metewand_runtime::publication::{
        PublicationError, PublicationRequest, finalize_fixed_dataset,
    };
    let (_private, outcome) = run_case(None, None, timeouts());
    let FixedDatasetRunOutcome::Validated(result) = outcome else {
        panic!("expected valid result")
    };
    let store = TempDir::new().unwrap();
    let target = store.path().join("metrics-copy.json");
    fs::copy(result.metrics_document(), &target).unwrap();
    fs::remove_file(result.metrics_document()).unwrap();
    std::os::unix::fs::symlink(target, result.metrics_document()).unwrap();
    assert!(matches!(
        finalize_fixed_dataset(
            &result,
            PublicationRequest {
                root: store.path(),
                attempt_slot: record_id(2),
                observation_slot: record_id(3),
                retry_index: 0,
                provenance_data: CanonicalValue::try_from(json!({})).unwrap(),
            }
        ),
        Err(PublicationError::UnsafeEntry(_))
    ));
    assert_eq!(
        fs::read_dir(store.path().join("observations"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn an_attempt_publication_failure_leaves_only_a_complete_observation() {
    use metewand_core::canonical::CanonicalValue;
    use metewand_runtime::publication::{
        PublicationRequest, finalize_fixed_dataset, verify_finalized_directory,
    };
    let (_private, outcome) = run_case(None, None, timeouts());
    let FixedDatasetRunOutcome::Validated(result) = outcome else {
        panic!("expected valid result")
    };
    let store = TempDir::new().unwrap();
    fs::write(store.path().join("attempts"), b"occupied").unwrap();
    assert!(
        finalize_fixed_dataset(
            &result,
            PublicationRequest {
                root: store.path(),
                attempt_slot: record_id(2),
                observation_slot: record_id(3),
                retry_index: 0,
                provenance_data: CanonicalValue::try_from(json!({})).unwrap(),
            }
        )
        .is_err()
    );
    let observations = fs::read_dir(store.path().join("observations"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(observations.len(), 1);
    assert!(
        observations[0]
            .file_name()
            .to_str()
            .unwrap()
            .starts_with("mw1-observation-")
    );
    verify_finalized_directory(&observations[0].path()).unwrap();
    assert_eq!(
        fs::read(store.path().join("attempts")).unwrap(),
        b"occupied"
    );
}

#[test]
fn publication_retains_role_scoped_log_prefixes_and_truncation_provenance() {
    use metewand_core::canonical::CanonicalValue;
    use metewand_runtime::publication::{PublicationRequest, finalize_fixed_dataset};
    let (_private, outcome) = run_case(
        Some(("METEWAND_TEST_LOGS", "1")),
        Some(("METEWAND_TEST_LOGS", "1")),
        timeouts(),
    );
    let FixedDatasetRunOutcome::Validated(result) = outcome else {
        panic!("expected valid result")
    };
    let store = TempDir::new().unwrap();
    let published = finalize_fixed_dataset(
        &result,
        PublicationRequest {
            root: store.path(),
            attempt_slot: record_id(2),
            observation_slot: record_id(3),
            retry_index: 0,
            provenance_data: CanonicalValue::try_from(json!({"repository_commit": null})).unwrap(),
        },
    )
    .unwrap();
    let document: Value = serde_json::from_slice(
        &fs::read(published.attempt_directory.join("attempt.json")).unwrap(),
    )
    .unwrap();
    for (role, message_role) in [
        ("implementation", "implementation"),
        ("problem-evaluator", "problem_evaluator"),
    ] {
        for stream in ["stdout", "stderr"] {
            let complete = format!("{message_role} {stream}\n")
                .repeat(1000)
                .into_bytes();
            assert_eq!(
                fs::read(
                    published
                        .attempt_directory
                        .join(format!("logs/{role}/{stream}.log"))
                )
                .unwrap(),
                complete[..4096]
            );
            let metadata = &document["provenance"]["data"]["logs"][role][stream];
            assert_eq!(metadata["retained_bytes"], 4096);
            assert_eq!(metadata["total_bytes"], complete.len());
            assert_eq!(metadata["truncated"], true);
        }
    }
    assert_eq!(
        document["provenance"]["data"]["context"]["repository_commit"],
        Value::Null
    );
    assert_eq!(
        document["provenance"]["data"]["execution"]["implementation"]["worker_id"],
        WORKER_ID
    );
}
