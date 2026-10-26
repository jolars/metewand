#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Duration,
};

use metewand_core::{
    manifest::parse_manifest, problem_contract::ContractDeclaration, schema::SchemaCatalog,
};
use metewand_protocol::{Capability, EvaluationVerdict, WorkerIdentity, WorkerRole};
use metewand_runtime::{
    worker_output::WorkerOutputValidator,
    worker_process::{PosixWorkerProcess, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerSession},
};
use serde_json::json;
use tempfile::TempDir;

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn python_sdk_unit_and_canonical_contracts() {
    let output = Command::new("python3")
        .args(["-m", "unittest", "discover", "-s"])
        .arg(root().join("sdk/python/tests"))
        .env("PYTHONPATH", root().join("sdk/python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn python_sdk_roles_complete_a_validated_chain_over_dedicated_pipes() {
    let source = TempDir::new().unwrap();
    let dataset = TempDir::new().unwrap();
    let result = TempDir::new().unwrap();
    let metrics = TempDir::new().unwrap();
    let manifest = parse_manifest(
        Path::new("metewand.yaml"),
        &fs::read_to_string(root().join("examples/python-sdk/metewand.yaml")).unwrap(),
    )
    .unwrap();
    let ContractDeclaration::Inline(contract) = &manifest.problems["sum"].contract else {
        panic!("the Python SDK example has an inline contract");
    };
    let catalog = SchemaCatalog::try_new(contract.schemas().map(|schema| {
        (
            schema.as_path().to_path_buf(),
            schema.inline_document().unwrap().as_json().clone(),
        )
    }))
    .unwrap();
    let validator = WorkerOutputValidator::new(&catalog).unwrap();

    let mut materializer = connect("dataset_materializer", WorkerRole::DatasetMaterializer);
    let response = materializer
        .materialize(
            source.path().to_str().unwrap(),
            json!({"values": [1, 2, 3]}),
            17,
            dataset.path().to_str().unwrap(),
        )
        .unwrap();
    validator
        .validate_materialized_dataset(
            dataset.path(),
            &response,
            contract.dataset_schemas[0].as_path(),
        )
        .unwrap();
    assert_logs(materializer.shutdown().unwrap(), "dataset_materializer");

    let mut implementation = connect("implementation", WorkerRole::Implementation);
    for seed in [23, 29] {
        implementation
            .prepare(
                dataset.path().to_str().unwrap(),
                "mw1-problem-contract-fixture",
                json!({"offset": 2}),
                json!({}),
                seed,
            )
            .unwrap();
        let response = implementation
            .execute(result.path().to_str().unwrap())
            .unwrap();
        assert!(response.implementation_time_ns().is_some());
        assert_eq!(response.statistics(), &json!({"input_count": 3}));
        let validated = validator
            .validate_execution_result(result.path(), &response, contract.result_schema.as_path())
            .unwrap();
        assert_eq!(validated.data().as_json(), &json!({"answer": 8}));
        implementation.reset().unwrap();
    }
    assert_logs(implementation.shutdown().unwrap(), "implementation");

    let mut evaluator = connect("problem_evaluator", WorkerRole::ProblemEvaluator);
    let metrics_path = metrics.path().join("metrics.json");
    let response = evaluator
        .evaluate(
            dataset.path().to_str().unwrap(),
            "mw1-problem-contract-fixture",
            json!({"offset": 2}),
            contract.semantics.as_canonical().as_json().clone(),
            result.path().to_str().unwrap(),
            metrics_path.to_str().unwrap(),
        )
        .unwrap();
    assert_eq!(response.verdict(), EvaluationVerdict::Reached);
    let validated = validator
        .validate_metrics(
            metrics.path(),
            Path::new("metrics.json"),
            contract.metric_schema.as_path(),
        )
        .unwrap();
    assert_eq!(validated.data().as_json(), &json!({"absolute_error": 0}));
    assert_logs(evaluator.shutdown().unwrap(), "problem_evaluator");
}

fn connect(role_name: &str, role: WorkerRole) -> WorkerSession {
    let mut command = Command::new("python3");
    command
        .arg(root().join("examples/python-sdk/worker.py"))
        .arg(role_name)
        .env("PYTHONPATH", root().join("sdk/python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    let process = PosixWorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    let capabilities = if role == WorkerRole::Implementation {
        &[Capability::OneShot][..]
    } else {
        &[]
    };
    let limit = Duration::from_secs(5);
    let session = WorkerSession::connect(
        process,
        role,
        WorkerIdentity::from_str(WORKER_ID).unwrap(),
        capabilities,
        PhaseTimeouts {
            hello: limit,
            materialize: limit,
            prepare: limit,
            execute: limit,
            reset: limit,
            evaluate: limit,
            shutdown: limit,
        },
    )
    .unwrap();
    let sdk = session.negotiated().sdk().unwrap();
    assert_eq!(sdk.name(), "metewand-python");
    assert_eq!(sdk.version(), "0.1.0");
    session
}

fn assert_logs(output: metewand_runtime::worker_process::WorkerProcessOutput, role: &str) {
    assert!(output.status.success());
    assert_eq!(output.stdout.bytes(), format!("{role} stdout\n").as_bytes());
    assert_eq!(output.stderr.bytes(), format!("{role} stderr\n").as_bytes());
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
