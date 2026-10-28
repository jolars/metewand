#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Duration,
};

use metewand_core::{
    problem_contract::{ProblemContract, parse_problem_contract},
    schema::SchemaCatalog,
};
use metewand_protocol::{
    Capability, EvaluationVerdict, ExecuteResponse, MaterializeResponse, RequestId, WorkerIdentity,
    WorkerRole,
};
use metewand_runtime::{
    worker_output::{WorkerOutputError, WorkerOutputValidator},
    worker_process::{PosixWorkerProcess, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerSession},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const CONTRACT_ID: &str =
    "mw1-problem-contract-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn reference_contracts_admit_correct_workers_and_reject_three_classes_of_bad_results() {
    let package = TempDir::new().unwrap();
    copy_tree(&root().join("packages/r"), &package.path().join("r"));
    let library = package.path().join("library");
    fs::create_dir(&library).unwrap();
    let output = Command::new("R")
        .args(["CMD", "INSTALL", "--preclean", "--clean"])
        .arg(format!("--library={}", library.display()))
        .arg(package.path().join("r"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    for family in ["sum", "argmin"] {
        let base = root()
            .join("fixtures/problem-contract/v1/suites")
            .join(family);
        let suite = read(&base.join("suite.json"));
        assert_eq!(suite["version"], 1);
        let catalog = SchemaCatalog::try_new(
            ["dataset", "parameters", "result", "metrics", "semantics"].map(|name| {
                let path = PathBuf::from(format!("schemas/{name}.json"));
                (path.clone(), read(&base.join(path)))
            }),
        )
        .unwrap();
        let contract = parse_problem_contract(
            Path::new("problem.yaml"),
            &fs::read_to_string(base.join("problem.yaml")).unwrap(),
            &catalog,
        )
        .unwrap();
        assert_eq!(contract.name.as_str(), family);
        let validator = WorkerOutputValidator::new(&catalog).unwrap();
        let cases = suite["cases"].as_array().unwrap();
        assert_eq!(cases.len(), contract.reference_cases.len());
        let mut evaluators: Vec<_> = ["python", "r"]
            .map(|language| {
                (
                    language,
                    connect(
                        language,
                        WorkerRole::ProblemEvaluator,
                        family,
                        "correct",
                        &library,
                    ),
                )
            })
            .into();

        for (case, reference) in cases.iter().zip(&contract.reference_cases) {
            let dataset = base.join(
                reference
                    .dataset
                    .as_ref()
                    .unwrap()
                    .reference_path()
                    .expect("file-backed fixture"),
            );
            let response = MaterializeResponse::new(id(), "dataset-manifest.json");
            validator
                .validate_materialized_dataset(
                    &dataset,
                    &response,
                    contract.dataset_schemas[0].as_path(),
                )
                .unwrap();
            catalog
                .validate(
                    contract.parameter_schema.as_path(),
                    &case["problem_parameters"],
                )
                .unwrap();
            let result = base.join(
                reference
                    .result
                    .reference_path()
                    .expect("file-backed fixture"),
            );
            let response = ExecuteResponse::new(id(), None, "result.json", serde_json::Map::new());
            validator
                .validate_execution_result(&result, &response, contract.result_schema.as_path())
                .unwrap();
            let metrics = base.join(
                reference
                    .expected_metrics
                    .reference_path()
                    .expect("file-backed fixture"),
            );
            let validated = validator
                .validate_metrics(
                    metrics.parent().unwrap(),
                    Path::new("metrics.json"),
                    contract.metric_schema.as_path(),
                )
                .unwrap();
            assert_eq!(
                validated.data().as_json(),
                &case["expected_metrics"]["correct"]
            );
            for (language, evaluator) in &mut evaluators {
                let (verdict, metrics) = evaluate(
                    evaluator,
                    &validator,
                    &contract,
                    &dataset,
                    &case["problem_parameters"],
                    &result,
                );
                assert_eq!(verdict, EvaluationVerdict::Reached, "{family}/{language}");
                assert_eq!(&metrics, validated.data().as_json(), "{family}/{language}");
            }
        }

        for language in ["python", "r"] {
            for variant in suite["variants"].as_array().unwrap() {
                let mode = variant["mode"].as_str().unwrap();
                let mut implementation =
                    connect(language, WorkerRole::Implementation, family, mode, &library);
                for (case, reference) in cases.iter().zip(&contract.reference_cases) {
                    let context = format!("{family}/{}/{language}/{mode}", case["name"]);
                    let dataset = base.join(
                        reference
                            .dataset
                            .as_ref()
                            .unwrap()
                            .reference_path()
                            .expect("file-backed fixture"),
                    );
                    let result = TempDir::new().unwrap();
                    implementation
                        .prepare(
                            dataset.to_str().unwrap(),
                            CONTRACT_ID,
                            case["problem_parameters"].clone(),
                            json!({}),
                            9_007_199_254_740_991,
                        )
                        .unwrap();
                    let response = implementation
                        .execute(result.path().to_str().unwrap())
                        .unwrap();
                    assert_eq!(
                        response.implementation_time_ns().is_some(),
                        language == "python",
                        "{context}"
                    );
                    let validated = validator.validate_execution_result(
                        result.path(),
                        &response,
                        contract.result_schema.as_path(),
                    );
                    if variant["validation"] == "malformed" {
                        assert!(
                            matches!(validated, Err(WorkerOutputError::ScientificSchema { .. })),
                            "{context}: {validated:?}"
                        );
                        continue;
                    }
                    let validated = validated.unwrap();
                    if mode == "correct" {
                        let expected = read(
                            &base
                                .join(
                                    reference
                                        .result
                                        .reference_path()
                                        .expect("file-backed fixture"),
                                )
                                .join("result.json"),
                        );
                        assert_eq!(validated.data().as_json(), &expected["data"], "{context}");
                    }
                    for (evaluator_language, evaluator) in &mut evaluators {
                        let (verdict, metrics) = evaluate(
                            evaluator,
                            &validator,
                            &contract,
                            &dataset,
                            &case["problem_parameters"],
                            result.path(),
                        );
                        assert_eq!(
                            serde_json::to_value(verdict).unwrap(),
                            variant["verdict"],
                            "{context}/{evaluator_language}"
                        );
                        assert_eq!(
                            verdict == EvaluationVerdict::Reached,
                            mode == "correct",
                            "{context}"
                        );
                        assert_eq!(
                            metrics, case["expected_metrics"][mode],
                            "{context}/{evaluator_language}"
                        );
                    }
                }
                assert!(implementation.shutdown().unwrap().status.success());
            }
        }
        for (_, evaluator) in evaluators {
            assert!(evaluator.shutdown().unwrap().status.success());
        }
    }
}

fn evaluate(
    evaluator: &mut WorkerSession,
    validator: &WorkerOutputValidator<'_>,
    contract: &ProblemContract,
    dataset: &Path,
    parameters: &Value,
    result: &Path,
) -> (EvaluationVerdict, Value) {
    let metrics = TempDir::new().unwrap();
    let response = evaluator
        .evaluate(
            dataset.to_str().unwrap(),
            CONTRACT_ID,
            parameters.clone(),
            contract.semantics.as_canonical().as_json().clone(),
            result.to_str().unwrap(),
            metrics.path().join("metrics.json").to_str().unwrap(),
        )
        .unwrap();
    let validated = validator
        .validate_metrics(
            metrics.path(),
            Path::new("metrics.json"),
            contract.metric_schema.as_path(),
        )
        .unwrap();
    (response.verdict(), validated.data().as_json().clone())
}

fn connect(
    language: &str,
    role: WorkerRole,
    family: &str,
    mode: &str,
    library: &Path,
) -> WorkerSession {
    let mut command = if language == "python" {
        let mut command = Command::new("python3");
        command
            .arg(root().join("fixtures/problem-contract/v1/suites/worker.py"))
            .env("PYTHONPATH", root().join("packages/python/src"))
            .env("PYTHONDONTWRITEBYTECODE", "1");
        command
    } else {
        let mut command = Command::new("Rscript");
        command
            .arg("--vanilla")
            .arg(root().join("fixtures/problem-contract/v1/suites/worker.R"))
            .env("R_LIBS_USER", library);
        command
    };
    command.args([
        if role == WorkerRole::Implementation {
            "implementation"
        } else {
            "problem_evaluator"
        },
        family,
        mode,
    ]);
    let adapter = TempDir::new().unwrap();
    let path = metewand_runtime::language_adapter::stage(language, adapter.path()).unwrap();
    command
        .env("METEWAND_ADAPTER_PATH", path)
        .env("METEWAND_PROTOCOL_TRANSPORT", "socket");
    let process = PosixWorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    let limit = Duration::from_secs(10);
    WorkerSession::connect(
        process,
        role,
        WorkerIdentity::from_str(
            "mw1-resolved-worker-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap(),
        if role == WorkerRole::Implementation {
            &[Capability::OneShot]
        } else {
            &[]
        },
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
    .unwrap()
}

fn id() -> RequestId {
    RequestId::from_str("reference").unwrap()
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
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
