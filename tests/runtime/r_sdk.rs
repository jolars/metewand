#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Duration,
};

use crate::core::schema::SchemaCatalog;
use crate::protocol::{Capability, EvaluationVerdict, WorkerIdentity, WorkerRole};
use crate::runtime::{
    worker_output::WorkerOutputValidator,
    worker_process::{PosixWorkerProcess, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerSession},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn r_sdk_contracts_and_roles_complete_a_validated_chain() {
    let package = TempDir::new().unwrap();
    copy_tree(&root().join("packages/r"), &package.path().join("r"));
    let library = package.path().join("library");
    fs::create_dir(&library).unwrap();
    let output = Command::new("R")
        .args(["CMD", "INSTALL", "--preclean", "--clean", "--install-tests"])
        .env("CC", "false")
        .env("CXX", "false")
        .env("FC", "false")
        .arg(format!("--library={}", library.display()))
        .arg(package.path().join("r"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!library.join("metewand/libs").exists());

    let output = Command::new("Rscript")
        .args([
            "--vanilla",
            "-e",
            ".libPaths(c(Sys.getenv('METEWAND_TEST_R_LIBRARY'), .libPaths())); testthat::test_package('metewand', stop_on_failure = TRUE)",
        ])
        .env("METEWAND_REPOSITORY", root())
        .env("METEWAND_TEST_R_LIBRARY", &library)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let source = TempDir::new().unwrap();
    let dataset = TempDir::new().unwrap();
    let result = TempDir::new().unwrap();
    let metrics = TempDir::new().unwrap();
    let catalog = SchemaCatalog::try_new(["dataset", "result", "metrics"].map(|name| {
        let path = PathBuf::from(format!("schemas/{name}.json"));
        let value: Value = serde_json::from_slice(
            &fs::read(root().join("examples/r-package").join(&path)).unwrap(),
        )
        .unwrap();
        (path, value)
    }))
    .unwrap();
    let validator = WorkerOutputValidator::new(&catalog).unwrap();

    let mut materializer = connect(
        "dataset_materializer",
        WorkerRole::DatasetMaterializer,
        &library,
    );
    let response = materializer
        .materialize(
            source.path().to_str().unwrap(),
            json!({"values": [1, 2, 3]}),
            17,
            dataset.path().to_str().unwrap(),
        )
        .unwrap();
    validator
        .validate_materialized_dataset(dataset.path(), &response, Path::new("schemas/dataset.json"))
        .unwrap();
    assert_logs(materializer.shutdown().unwrap(), "dataset_materializer");

    let mut implementation = connect("implementation", WorkerRole::Implementation, &library);
    for seed in [23, 9_007_199_254_740_991] {
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
        assert!(response.implementation_time_ns().is_none());
        assert_eq!(response.statistics(), &json!({"input_count": 3}));
        let validated = validator
            .validate_execution_result(result.path(), &response, Path::new("schemas/result.json"))
            .unwrap();
        assert_eq!(validated.data().as_json(), &json!({"answer": 8}));
        implementation.reset().unwrap();
        if seed == 23 {
            fs::remove_file(result.path().join("result.json")).unwrap();
        }
    }
    assert_logs(implementation.shutdown().unwrap(), "implementation");

    let mut evaluator = connect("problem_evaluator", WorkerRole::ProblemEvaluator, &library);
    let metrics_path = metrics.path().join("metrics.json");
    let response = evaluator
        .evaluate(
            dataset.path().to_str().unwrap(),
            "mw1-problem-contract-fixture",
            json!({"offset": 2}),
            json!({"validity": {"exact_sum": true}, "one_shot_completion": {"absolute_error": 0}}),
            result.path().to_str().unwrap(),
            metrics_path.to_str().unwrap(),
        )
        .unwrap();
    assert_eq!(response.verdict(), EvaluationVerdict::Reached);
    let validated = validator
        .validate_metrics(
            metrics.path(),
            Path::new("metrics.json"),
            Path::new("schemas/metrics.json"),
        )
        .unwrap();
    assert_eq!(validated.data().as_json(), &json!({"absolute_error": 0}));
    assert_logs(evaluator.shutdown().unwrap(), "problem_evaluator");
}

fn connect(role_name: &str, role: WorkerRole, library: &Path) -> WorkerSession {
    let mut command = Command::new("Rscript");
    command
        .arg("--vanilla")
        .arg(root().join("examples/r-package/worker.R"))
        .arg(role_name)
        .env("R_LIBS_USER", library);
    let adapter = TempDir::new().unwrap();
    let path = crate::runtime::language_adapter::stage("r", adapter.path()).unwrap();
    command
        .env("METEWAND_ADAPTER_PATH", path)
        .env("METEWAND_PROTOCOL_TRANSPORT", "socket");
    let process = PosixWorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    let capabilities = if role == WorkerRole::Implementation {
        &[Capability::OneShot][..]
    } else {
        &[]
    };
    let limit = Duration::from_secs(10);
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
    assert_eq!(sdk.name(), "metewand-r");
    let description = fs::read_to_string(root().join("packages/r/DESCRIPTION")).unwrap();
    let version = description
        .lines()
        .find_map(|line| line.strip_prefix("Version: "))
        .unwrap();
    assert_eq!(sdk.version(), version);
    session
}

fn assert_logs(output: crate::runtime::worker_process::WorkerProcessOutput, role: &str) {
    assert!(output.status.success());
    assert_eq!(output.stdout.bytes(), format!("{role} stdout\n").as_bytes());
    assert_eq!(output.stderr.bytes(), format!("{role} stderr\n").as_bytes());
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
