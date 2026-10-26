#![cfg(unix)]

use std::{fs, io::Cursor, path::Path, process::Command, str::FromStr, time::Duration};

use metewand_protocol::{
    Capability, EvaluationVerdict, PrepareRequest, WorkerFailure, WorkerFailureCode,
    WorkerIdentity, WorkerRole,
};
use metewand_runtime::worker_sdk::{
    Evaluation, Implementation, WorkerError, WorkerResult, serve_dataset_materializer_on,
    serve_evaluator_on, serve_implementation, serve_implementation_on, write_dataset_manifest,
    write_json,
};
use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Default)]
struct Sum {
    offset: i64,
}

impl Implementation for Sum {
    type Output = Value;

    fn prepare(&mut self, request: &PrepareRequest) -> WorkerResult<()> {
        self.offset = request.problem_parameters()["offset"].as_i64().unwrap();
        Ok(())
    }

    fn execute(&mut self) -> WorkerResult<Value> {
        Ok(json!({"answer": 6 + self.offset}))
    }
}

fn hello(role: &str) -> Value {
    json!({"id":"0","method":"hello","protocols":[1],"role":role,
        "worker_id":format!("mw1-resolved-worker-{}", "a".repeat(64))})
}

fn prepare() -> Value {
    json!({"id":"1","method":"prepare","dataset_dir":"dataset",
        "problem_contract_id":"c","problem_parameters":{"offset":2},
        "implementation_parameters":{},"implementation_seed":23})
}

fn frames(requests: Vec<Value>) -> Cursor<Vec<u8>> {
    Cursor::new(
        requests
            .into_iter()
            .flat_map(|r| {
                let mut bytes = serde_json::to_vec(&r).unwrap();
                bytes.push(b'\n');
                bytes
            })
            .collect(),
    )
}

fn responses(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| {
            let canonical = metewand_core::canonical::CanonicalValue::from_slice(l).unwrap();
            assert_eq!(canonical.to_canonical_bytes().unwrap(), l);
            canonical.as_json().clone()
        })
        .collect()
}

#[test]
fn materializers_write_canonical_inventories_and_validate_requests() {
    let directory = TempDir::new().unwrap();
    let request = json!({"id":"1", "method":"materialize", "source_dir":"source",
        "dataset_parameters":{"values":[1,2,3]}, "dataset_seed":17,
        "output_dir":directory.path()});
    let mut invalid = request.clone();
    invalid["dataset_seed"] = json!(true);
    let mut output = Vec::new();
    serve_dataset_materializer_on(
        &mut |request| {
            assert_eq!(request.dataset_seed(), 17);
            write_json(
                &directory.path().join("data.json"),
                request.dataset_parameters().clone(),
            )?;
            write_dataset_manifest(directory.path(), &[("data.json", "application/json")])
        },
        frames(vec![
            hello("dataset_materializer"),
            invalid,
            request,
            json!({"id":"2", "method":"execute"}),
            json!({"id":"3", "method":"shutdown"}),
        ]),
        &mut output,
    )
    .unwrap();
    let replies = responses(&output);
    assert_eq!(replies[0]["sdk"]["name"], "metewand-rust");
    assert_eq!(replies[1]["error"]["code"], "invalid_request");
    assert_eq!(replies[2]["dataset"]["manifest"], "dataset-manifest.json");
    assert_eq!(replies[3]["error"]["code"], "unsupported_operation");
    let bytes = fs::read(directory.path().join("dataset-manifest.json")).unwrap();
    assert_eq!(bytes, br#"{"complete":true,"files":[{"media_type":"application/json","path":"data.json","sha256":"abe0a9abcc957f9f5aa04d68c4f49fadee7c4b62343e403a3a1c0b646519d53c","size_bytes":18}],"schema_version":1}"#);
    let catalog = metewand_core::schema::SchemaCatalog::try_new([(
        "dataset.json".into(),
        json!({"$schema":"https://json-schema.org/draft/2020-12/schema", "type":"object"}),
    )])
    .unwrap();
    let response: metewand_protocol::MaterializeResponse =
        metewand_protocol::decode_message(replies[2].clone()).unwrap();
    metewand_runtime::worker_output::WorkerOutputValidator::new(&catalog)
        .unwrap()
        .validate_materialized_dataset(directory.path(), &response, Path::new("dataset.json"))
        .unwrap();
}

#[test]
fn materializers_report_callback_and_inventory_failures() {
    let directory = TempDir::new().unwrap();
    write_json(&directory.path().join("data.json"), json!({})).unwrap();
    std::os::unix::fs::symlink("data.json", directory.path().join("link.json")).unwrap();
    for files in [
        vec![("../escape", "application/json")],
        vec![("data.json", "")],
        vec![
            ("data.json", "application/json"),
            ("data.json", "application/json"),
        ],
        vec![("dataset-manifest.json", "application/json")],
        vec![("missing", "application/json")],
        vec![("link.json", "application/json")],
    ] {
        assert!(write_dataset_manifest(directory.path(), &files).is_err());
    }
    for result in [
        Ok("missing.json".to_owned()),
        Ok("../escape".to_owned()),
        Err(WorkerError::operation_failed("TODO: Author the dataset.")),
    ] {
        let mut result = Some(result);
        let mut output = Vec::new();
        serve_dataset_materializer_on(&mut |_| result.take().unwrap(), frames(vec![
            hello("dataset_materializer"),
            json!({"id":"1", "method":"materialize", "source_dir":"source", "dataset_parameters":{}, "dataset_seed":1, "output_dir":directory.path()}),
            json!({"id":"2", "method":"shutdown"})]), &mut output).unwrap();
        assert_eq!(responses(&output)[1]["error"]["code"], "operation_failed");
        assert_eq!(responses(&output)[2]["ok"], true);
    }
}

#[test]
fn dataset_inventories_have_the_same_bytes_for_every_input_order() {
    let directory = TempDir::new().unwrap();
    write_json(&directory.path().join("a.json"), json!({"a": 1})).unwrap();
    write_json(&directory.path().join("z.json"), json!({"z": 2})).unwrap();
    let mut files = [
        ("z.json", "application/json"),
        ("a.json", "application/json"),
    ];
    let manifest = write_dataset_manifest(directory.path(), &files).unwrap();
    let original = fs::read(directory.path().join(&manifest)).unwrap();
    files.reverse();
    write_dataset_manifest(directory.path(), &files).unwrap();
    assert_eq!(original, fs::read(directory.path().join(manifest)).unwrap());
    let document: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(document["files"][0]["path"], "a.json");
    assert_eq!(document["files"][1]["path"], "z.json");
}

#[test]
fn lifecycle_failures_invalidate_preparation_and_preserve_errors() {
    struct Broken(&'static str);
    impl Broken {
        fn check(&self, stage: &str) -> WorkerResult<()> {
            if self.0 == stage {
                return Err(WorkerFailure::new(
                    WorkerFailureCode::OperationFailed,
                    "bad input",
                    Some(json!({"stage":stage})),
                )
                .unwrap()
                .into());
            }
            if stage == "execute" {
                match self.0 {
                    "panic" => panic!("solver panicked"),
                    "panic_long" => std::panic::panic_any("é".repeat(1024 * 1024)),
                    "panic_payload" => std::panic::panic_any(42_u32),
                    _ => {}
                }
            }
            Ok(())
        }
    }
    impl Implementation for Broken {
        type Output = Value;
        fn prepare(&mut self, _: &PrepareRequest) -> WorkerResult<()> {
            self.check("prepare")
        }
        fn execute(&mut self) -> WorkerResult<Value> {
            self.check("execute")?;
            Ok(json!({"answer":8}))
        }
        fn statistics(&mut self, _: &Value) -> WorkerResult<Value> {
            self.check("statistics")?;
            Ok(json!({}))
        }
        fn write_result(&mut self, output: Value, directory: &Path) -> WorkerResult<String> {
            self.check("writer")?;
            metewand_runtime::worker_sdk::write_result(directory, output)
        }
        fn reset(&mut self) -> WorkerResult<()> {
            self.check("reset")
        }
    }
    for stage in [
        "prepare",
        "execute",
        "statistics",
        "writer",
        "reset",
        "panic",
        "panic_long",
        "panic_payload",
    ] {
        let directory = TempDir::new().unwrap();
        let execute = json!({"id":"2","method":"execute","result_dir":directory.path()});
        let mut calls = vec![hello("implementation"), prepare()];
        if stage == "reset" {
            calls.push(json!({"id":"2","method":"reset"}));
        } else if stage != "prepare" {
            calls.push(execute.clone());
        }
        calls.extend([execute, json!({"id":"3","method":"shutdown"})]);
        let mut output = Vec::new();
        serve_implementation_on(&mut Broken(stage), frames(calls), &mut output).unwrap();
        let replies = responses(&output);
        let failure_index = if stage == "prepare" { 1 } else { 2 };
        assert_eq!(
            replies[failure_index]["error"]["code"], "operation_failed",
            "{stage}"
        );
        if stage.starts_with("panic") {
            let message = replies[failure_index]["error"]["message"].as_str().unwrap();
            assert!(message.len() < 65536);
            let expected = match stage {
                "panic_long" => "[panic truncated]",
                "panic_payload" => "Non-string panic payload.",
                _ => "solver panicked",
            };
            assert!(
                replies[failure_index]["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(expected)
            );
        } else {
            assert_eq!(
                replies[failure_index]["error"]["details"],
                json!({"stage":stage})
            );
        }
        assert_eq!(
            replies[replies.len() - 2]["error"]["code"],
            "invalid_request",
            "{stage}"
        );
    }
}

#[test]
fn handshake_and_request_shapes_are_strict() {
    for change in [
        json!({"protocols":[2]}),
        json!({"protocols":[1,1]}),
        json!({"role":"problem_evaluator"}),
        json!({"worker_id":"wrong"}),
        json!({"extra":true}),
    ] {
        let mut greeting = hello("implementation");
        greeting
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        let mut output = Vec::new();
        serve_implementation_on(&mut Sum::default(), frames(vec![greeting]), &mut output).unwrap();
        assert_eq!(responses(&output).len(), 1);
        assert_eq!(responses(&output)[0]["ok"], false);
    }
    let mut output = Vec::new();
    let mut bad_prepare = prepare();
    bad_prepare["implementation_seed"] = json!(true);
    serve_implementation_on(
        &mut Sum::default(),
        frames(vec![
            hello("implementation"),
            bad_prepare,
            json!({"id":"2","method":"reset","extra":true}),
            json!({"id":"3","method":"execute_stream"}),
            json!({"id":"4","method":"reset"}),
            json!({"id":"5","method":"shutdown"}),
        ]),
        &mut output,
    )
    .unwrap();
    let replies = responses(&output);
    for index in [1, 2] {
        assert_eq!(replies[index]["error"]["code"], "invalid_request");
    }
    for index in [3, 4] {
        assert_eq!(replies[index]["error"]["code"], "unsupported_operation");
    }
}

#[test]
fn evaluator_requires_rules_and_bounds_callback_failures() {
    let directory = TempDir::new().unwrap();
    let base = json!({"id":"1","method":"evaluate","dataset_dir":"dataset",
        "problem_contract_id":"c","problem_parameters":{},"result_dir":"result",
        "metrics_path":directory.path().join("metrics.json")});
    for rules in [None, Some(json!([])), Some(json!({}))] {
        let mut request = base.clone();
        if let Some(rules) = rules.clone() {
            request["problem_semantics"] = rules;
        }
        let mut output = Vec::new();
        serve_evaluator_on(
            &mut |_: &metewand_protocol::EvaluateRequest| -> WorkerResult<Evaluation> {
                Err(WorkerError::operation_failed("x".repeat(1024 * 1024)))
            },
            frames(vec![
                hello("problem_evaluator"),
                request,
                json!({"id":"2","method":"shutdown"}),
            ]),
            &mut output,
        )
        .unwrap();
        let replies = responses(&output);
        assert_eq!(
            replies[1]["error"]["code"],
            if rules == Some(json!({})) {
                "internal_error"
            } else {
                "invalid_request"
            }
        );
        assert!(!directory.path().join("metrics.json").exists());
    }
}

#[test]
fn native_worker_entry() {
    if std::env::var_os("METEWAND_TEST_RUST_WORKER").is_none() {
        return;
    }
    println!("native worker stdout");
    eprintln!("native worker stderr");
    serve_implementation(&mut Sum::default()).unwrap();
    for name in ["METEWAND_PROTOCOL_READ_FD", "METEWAND_PROTOCOL_WRITE_FD"] {
        let descriptor = std::env::var(name).unwrap().parse::<i32>().unwrap();
        // The SDK must close only its duplicates of the caller's descriptors.
        assert_ne!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
    }
}

#[test]
fn native_worker_uses_dedicated_pipes_and_runtime_validation() {
    use metewand_runtime::{
        worker_output::WorkerOutputValidator,
        worker_process::{PosixWorkerProcess, WorkerLogLimits},
        worker_session::{PhaseTimeouts, WorkerSession},
    };
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "native_worker_entry", "--nocapture"])
        .env("METEWAND_TEST_RUST_WORKER", "1");
    let process = PosixWorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    let timeout = Duration::from_secs(5);
    let mut session = WorkerSession::connect(
        process,
        WorkerRole::Implementation,
        WorkerIdentity::from_str(&format!("mw1-resolved-worker-{}", "a".repeat(64))).unwrap(),
        &[Capability::OneShot],
        PhaseTimeouts {
            hello: timeout,
            prepare: timeout,
            execute: timeout,
            reset: timeout,
            evaluate: timeout,
            materialize: timeout,
            shutdown: timeout,
        },
    )
    .unwrap();
    assert_eq!(session.negotiated().sdk().unwrap().name(), "metewand-rust");
    let result = TempDir::new().unwrap();
    session
        .prepare("dataset", "c", json!({"offset":2}), json!({}), 23)
        .unwrap();
    let response = session.execute(result.path().to_str().unwrap()).unwrap();
    let catalog = metewand_core::schema::SchemaCatalog::try_new([(
        "result.json".into(),
        json!({"$schema":"https://json-schema.org/draft/2020-12/schema",
            "type":"object","properties":{"answer":{"const":8}},
            "required":["answer"],"additionalProperties":false}),
    )])
    .unwrap();
    let checked = WorkerOutputValidator::new(&catalog)
        .unwrap()
        .validate_execution_result(result.path(), &response, Path::new("result.json"))
        .unwrap();
    assert_eq!(checked.data().as_json(), &json!({"answer":8}));
    let output = session.shutdown().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(output.stdout.bytes()).contains("native worker stdout"));
    assert_eq!(output.stderr.bytes(), b"native worker stderr\n");
}

#[test]
fn data_callbacks_write_canonical_envelopes_and_require_preparation() {
    let result = TempDir::new().unwrap();
    let mut output = Vec::new();
    serve_implementation_on(
        &mut Sum::default(),
        frames(vec![
            hello("implementation"),
            prepare(),
            json!({"id":"2","method":"execute","result_dir":result.path()}),
            json!({"id":"3","method":"execute","result_dir":result.path()}),
            json!({"id":"4","method":"shutdown"}),
        ]),
        &mut output,
    )
    .unwrap();
    let replies = responses(&output);
    assert_eq!(replies[0]["sdk"]["name"], "metewand-rust");
    assert_eq!(replies[0]["capabilities"], json!(["one_shot"]));
    assert_eq!(replies[2]["result"], json!({"manifest":"result.json"}));
    assert_eq!(replies[3]["error"]["code"], "invalid_request");
    assert_eq!(
        fs::read(result.path().join("result.json")).unwrap(),
        br#"{"data":{"answer":8},"files":[],"schema_version":1}"#
    );

    let metrics_path = result.path().join("assigned.json");
    let mut output = Vec::new();
    serve_evaluator_on(
        &mut |request: &metewand_protocol::EvaluateRequest| {
            assert_eq!(request.problem_semantics(), Some(&json!({"target":0})));
            Ok(Evaluation::new(
                json!({"error":1}),
                EvaluationVerdict::NotReached,
            ))
        },
        frames(vec![
            hello("problem_evaluator"),
            json!({"id":"1","method":"evaluate","dataset_dir":"dataset",
            "problem_contract_id":"c","problem_parameters":{},
            "problem_semantics":{"target":0},"result_dir":result.path(),
            "metrics_path":metrics_path}),
            json!({"id":"2","method":"shutdown"}),
        ]),
        &mut output,
    )
    .unwrap();
    assert_eq!(responses(&output)[1]["verdict"], "not_reached");
    assert_eq!(
        fs::read(metrics_path).unwrap(),
        br#"{"data":{"error":1},"schema_version":1}"#
    );
}

#[test]
fn malformed_frames_and_unsafe_result_data_fail() {
    let mut output = Vec::new();
    assert!(
        serve_implementation_on(
            &mut Sum::default(),
            Cursor::new(b"{\"id\":\"1\",\"id\":\"2\"}\n"),
            &mut output
        )
        .is_err()
    );
    assert!(output.is_empty());
    let directory = TempDir::new().unwrap();
    for data in [json!([]), json!({"unsafe":9_007_199_254_740_992_u64})] {
        assert!(metewand_runtime::worker_sdk::write_result(directory.path(), data).is_err());
        assert!(!directory.path().join("result.json").exists());
    }
    let outside = TempDir::new().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("escape.json"),
        directory.path().join("result.json"),
    )
    .unwrap();
    assert!(metewand_runtime::worker_sdk::write_result(directory.path(), json!({})).is_err());
    assert!(!Path::new(&outside.path().join("escape.json")).exists());
}

#[test]
fn custom_result_and_metrics_writers_complete_before_their_responses() {
    use metewand_runtime::worker_sdk::{Evaluator, write_metrics, write_result};
    struct Custom;
    impl Implementation for Custom {
        type Output = Value;
        fn prepare(&mut self, _: &PrepareRequest) -> WorkerResult<()> {
            Ok(())
        }
        fn execute(&mut self) -> WorkerResult<Value> {
            Ok(json!({"answer":8}))
        }
        fn write_result(&mut self, output: Value, directory: &Path) -> WorkerResult<String> {
            let manifest = write_result(directory, output)?;
            fs::create_dir(directory.join("objects"))?;
            fs::rename(
                directory.join(manifest),
                directory.join("objects/custom.json"),
            )?;
            Ok("objects/custom.json".into())
        }
    }
    impl Evaluator for Custom {
        fn evaluate(&mut self, _: &metewand_protocol::EvaluateRequest) -> WorkerResult<Evaluation> {
            Ok(Evaluation::new(
                json!({"error":0}),
                EvaluationVerdict::Reached,
            ))
        }
        fn write_metrics(&mut self, evaluation: &Evaluation, path: &Path) -> WorkerResult<()> {
            let temporary = path.with_extension("pending");
            write_metrics(&temporary, evaluation.metrics.clone())?;
            fs::rename(temporary, path)?;
            Ok(())
        }
    }
    let directory = TempDir::new().unwrap();
    let mut output = Vec::new();
    serve_implementation_on(
        &mut Custom,
        frames(vec![
            hello("implementation"),
            prepare(),
            json!({"id":"2","method":"execute","result_dir":directory.path()}),
            json!({"id":"3","method":"shutdown"}),
        ]),
        &mut output,
    )
    .unwrap();
    assert_eq!(
        responses(&output)[2]["result"]["manifest"],
        "objects/custom.json"
    );
    assert_eq!(
        metewand_runtime::worker_sdk::read_json(&directory.path().join("objects/custom.json"))
            .unwrap()["data"],
        json!({"answer":8})
    );
    let path = directory.path().join("metrics.json");
    let mut output = Vec::new();
    serve_evaluator_on(&mut Custom, frames(vec![hello("problem_evaluator"),
        json!({"id":"1","method":"evaluate","dataset_dir":"d","problem_contract_id":"c",
            "problem_parameters":{},"problem_semantics":{},"result_dir":directory.path(),"metrics_path":path}),
        json!({"id":"2","method":"shutdown"}),
    ]), &mut output).unwrap();
    assert_eq!(responses(&output)[1]["verdict"], "reached");
    assert_eq!(
        metewand_runtime::worker_sdk::read_json(&path).unwrap()["data"],
        json!({"error":0})
    );
    assert!(!path.with_extension("pending").exists());
}
