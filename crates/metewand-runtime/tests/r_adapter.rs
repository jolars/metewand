#![cfg(any(unix, windows))]

use std::{io::Write, path::Path, process::Command};

use metewand_runtime::worker_process::{WorkerLogLimits, WorkerProcess};
use serde_json::{Value, json};
use tempfile::TempDir;

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn install_package(directory: &Path) {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/r");
    let output = Command::new("R")
        .args(["CMD", "INSTALL", "--preclean", "--clean"])
        .arg(format!("--library={}", directory.display()))
        .arg(package)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn spawn(role: &str, library: &Path, adapter: &Path) -> WorkerProcess {
    let mut command = Command::new("Rscript");
    command
        .args([
            "--vanilla",
            "-e",
            r#"
library(metewand)
role <- commandArgs(trailingOnly = TRUE)[[1L]]
if (role == "implementation") {
  serve_implementation(
    prepare = function(context) { cat("prepare\n"); NULL },
    execute = function(state, context) { cat("execute\n"); list(answer = 1) },
    check_applicability = function(context) stop("Unexpected applicability callback."),
    reset = function(state, context) stop("Unexpected reset callback."),
    shutdown = function(state, context) cat("shutdown\n"))
} else if (role == "dataset_materializer") {
  serve_dataset_materializer(function(context) { cat("materialize\n"); list(value = 1) })
} else {
  serve_evaluator(function(context) { cat("evaluate\n"); evaluation(list(error = 0), "not_required") })
}
"#,
            role,
        ])
        .env("R_LIBS_USER", library)
        .env("METEWAND_ADAPTER_PATH", adapter)
        .env("METEWAND_PROTOCOL_TRANSPORT", "socket");
    WorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap()
}

fn exchange(process: &mut WorkerProcess, request: &Value) -> Value {
    let mut bytes = serde_json::to_vec(request).unwrap();
    bytes.push(b'\n');
    process.protocol_writer().write_all(&bytes).unwrap();
    process.protocol_reader().read_frame().unwrap().unwrap()
}

fn hello(role: &str) -> Value {
    json!({"id":"hello", "method":"hello", "protocols":[1], "role":role, "worker_id":WORKER_ID})
}

#[test]
fn malformed_r_requests_do_not_invoke_callbacks_or_consume_preparation() {
    let library = TempDir::new().unwrap();
    install_package(library.path());
    let adapter = TempDir::new().unwrap();
    let adapter_path = metewand_runtime::language_adapter::stage("r", adapter.path()).unwrap();
    let prepare = json!({"id":"prepare", "method":"prepare", "dataset_dir":"dataset",
        "problem_contract_id":"contract", "problem_parameters":{},
        "implementation_parameters":{}, "implementation_seed":17});

    for (role, requests, expected_log) in [
        (
            "implementation",
            vec![
                prepare.clone(),
                json!({"id":"execute", "method":"execute", "result_dir":"result", "observation_control":{}}),
                json!({"id":"support", "method":"check_applicability", "dataset_dir":"dataset",
                    "dataset_manifest":"dataset/manifest.json", "problem_contract_id":"contract",
                    "problem_parameters":{}, "implementation_parameters":{}}),
                json!({"id":"reset", "method":"reset"}),
            ],
            "prepare\nexecute\nshutdown\n",
        ),
        (
            "dataset_materializer",
            vec![
                json!({"id":"materialize", "method":"materialize", "source_dir":"source",
                "dataset_parameters":{}, "dataset_seed":17, "output_dir":"output"}),
            ],
            "materialize\n",
        ),
        (
            "problem_evaluator",
            vec![
                json!({"id":"evaluate", "method":"evaluate", "dataset_dir":"dataset",
                "problem_contract_id":"contract", "problem_parameters":{}, "problem_semantics":{},
                "result_dir":"result", "metrics_path":"metrics/metrics.json"}),
            ],
            "evaluate\n",
        ),
    ] {
        let mut process = spawn(role, library.path(), &adapter_path);
        assert_eq!(exchange(&mut process, &hello(role))["ok"], true);
        if role == "implementation" {
            assert_eq!(exchange(&mut process, &prepare)["ok"], true);
        }
        for request in &requests {
            let mut invalid = vec![request.clone()];
            invalid[0]["unexpected"] = json!(true);
            for (field, value) in request.as_object().unwrap() {
                if field == "id" {
                    continue;
                }
                let mut missing = request.clone();
                missing.as_object_mut().unwrap().remove(field);
                if field != "observation_control" {
                    invalid.push(missing);
                }
                for replacement in [Value::Null, json!([]), json!(true)] {
                    if &replacement != value {
                        let mut wrong_type = request.clone();
                        wrong_type[field] = replacement;
                        invalid.push(wrong_type);
                    }
                }
            }
            if request.get("implementation_seed").is_some() || request.get("dataset_seed").is_some()
            {
                let field = if role == "implementation" {
                    "implementation_seed"
                } else {
                    "dataset_seed"
                };
                for seed in [json!(-1), json!(1.5), json!("17")] {
                    let mut invalid_seed = request.clone();
                    invalid_seed[field] = seed;
                    invalid.push(invalid_seed);
                }
            }
            for request in invalid {
                let response = exchange(&mut process, &request);
                assert_eq!(response["id"], request["id"]);
                assert_eq!(
                    response["error"]["code"], "invalid_request",
                    "{request}: {response}"
                );
            }
        }
        let output = TempDir::new().unwrap();
        let mut valid = if role == "implementation" {
            requests[1].clone()
        } else {
            requests[0].clone()
        };
        match role {
            "implementation" => valid["result_dir"] = json!(output.path()),
            "dataset_materializer" => valid["output_dir"] = json!(output.path()),
            _ => valid["metrics_path"] = json!(output.path().join("metrics.json")),
        }
        assert_eq!(exchange(&mut process, &valid)["ok"], true);
        assert_eq!(
            exchange(&mut process, &json!({"id":"stop", "method":"shutdown"}))["ok"],
            true
        );
        let output = process.wait().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            std::str::from_utf8(output.stdout.bytes())
                .unwrap()
                .replace("\r\n", "\n"),
            expected_log
        );
        assert!(output.stderr.bytes().is_empty(), "{output:?}");
    }
}

#[test]
fn r_hello_validates_protocol_offers_and_worker_identity() {
    let library = TempDir::new().unwrap();
    install_package(library.path());
    let adapter = TempDir::new().unwrap();
    let adapter_path = metewand_runtime::language_adapter::stage("r", adapter.path()).unwrap();
    for (field, value) in [
        ("protocols", json!([])),
        ("protocols", json!([1, 1])),
        ("protocols", json!([1, 1.5])),
        ("protocols", json!([1, 4_294_967_296_u64])),
        ("protocols", json!({"version":1})),
        ("worker_id", json!("invalid")),
        ("unexpected", json!(true)),
    ] {
        let mut process = spawn("implementation", library.path(), &adapter_path);
        let mut request = hello("implementation");
        request[field] = value;
        let response = exchange(&mut process, &request);
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "{request}: {response}"
        );
        assert!(process.wait().unwrap().status.success());
    }
}

#[test]
fn malformed_r_envelopes_terminate_serving_without_fabricating_an_id() {
    let library = TempDir::new().unwrap();
    install_package(library.path());
    let adapter = TempDir::new().unwrap();
    let adapter_path = metewand_runtime::language_adapter::stage("r", adapter.path()).unwrap();
    for frame in [
        "[]\n",
        "{\"identifier\":\"request\",\"method\":\"shutdown\"}\n",
        "{\"id\":true,\"method\":\"shutdown\"}\n",
        "{\"id\":\"\",\"method\":\"shutdown\"}\n",
        "{\"id\":\"one\",\"id\":\"two\",\"method\":\"shutdown\"}\n",
    ] {
        let mut process = spawn("implementation", library.path(), &adapter_path);
        process
            .protocol_writer()
            .write_all(frame.as_bytes())
            .unwrap();
        assert!(
            process.protocol_reader().read_frame().unwrap().is_none(),
            "{frame}"
        );
        let output = process.wait().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.bytes().is_empty());
    }
}

#[test]
fn non_json_r_frames_terminate_serving_without_invoking_callbacks() {
    let library = TempDir::new().unwrap();
    install_package(library.path());
    let adapter = TempDir::new().unwrap();
    let adapter_path = metewand_runtime::language_adapter::stage("r", adapter.path()).unwrap();
    let prepare = json!({"id":"prepare", "method":"prepare", "dataset_dir":"dataset",
        "problem_contract_id":"contract", "problem_parameters":{},
        "implementation_parameters":{}, "implementation_seed":17});

    for negotiated in [false, true] {
        let request = if negotiated {
            prepare.clone()
        } else {
            hello("implementation")
        };
        let source = serde_json::to_string(&request).unwrap();
        for frame in [
            format!("\u{feff}{source}\n"),
            format!("/*comment*/{source}\n"),
            format!("{source}/*comment*/\n"),
            format!("{source}//comment\n"),
            format!("{}\n", source.replacen(':', ":/*comment*/", 1)),
        ] {
            let mut process = spawn("implementation", library.path(), &adapter_path);
            if negotiated {
                assert_eq!(exchange(&mut process, &hello("implementation"))["ok"], true);
            }
            process
                .protocol_writer()
                .write_all(frame.as_bytes())
                .unwrap();
            assert!(
                process.protocol_reader().read_frame().unwrap().is_none(),
                "{frame}"
            );
            let output = process.wait().unwrap();
            assert!(!output.status.success(), "{output:?}");
            assert!(output.stdout.bytes().is_empty(), "{output:?}");
        }
    }
}
