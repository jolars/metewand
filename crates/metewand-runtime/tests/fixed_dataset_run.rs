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
    records::{RecordId, ResolvedEnvironmentRecord, ResolvedLaunchRecord, WorkerWorkingDirectory},
    schema::SchemaCatalog,
};
use metewand_protocol::WorkerIdentity;
use metewand_runtime::{
    fixed_dataset_run::{FixedDatasetRun, FixedDatasetRunError, run_fixed_dataset},
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
