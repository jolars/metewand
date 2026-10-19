#![cfg(unix)]

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

const PREPARATION_VECTOR: &str =
    include_str!("../../../examples/generated-dataset/fixtures/preparation.json");

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

fn replace(root: &Path, path: &str, before: &str, after: &str) {
    let path = root.join(path);
    let source = fs::read_to_string(&path).unwrap();
    assert!(source.contains(before), "missing {before:?} in {path:?}");
    fs::write(path, source.replace(before, after)).unwrap();
}

fn repository() -> TempDir {
    let root = TempDir::new().unwrap();
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/generated-dataset"),
        root.path(),
    );
    let trace_path =
        serde_json::to_string(root.path().join("trace.jsonl").to_str().unwrap()).unwrap();
    let source = fs::read_to_string(root.path().join("worker.py")).unwrap();
    let source = format!("import time\nTRACE_PATH = {trace_path}\n{source}");
    let source = source.replace(
        "method = request[\"method\"]",
        r#"method = request["method"]
        started_ns = time.monotonic_ns()
        input_hash = None
        if "dataset_dir" in request:
            input_hash = hashlib.sha256((Path(request["dataset_dir"]) / "data.json").read_bytes()).hexdigest()
        if method == "materialize":
            time.sleep(0.25)
        elif method == "prepare":
            time.sleep(0.05)"#,
    );
    let source = source.replace(
        "writer.write(json.dumps(response",
        r#"with open(TRACE_PATH, "a") as trace:
            trace.write(json.dumps({
                "role": role, "pid": os.getpid(), "request": request, "input_sha256": input_hash,
                "prepared_type": type(prepared).__name__,
                "prepared_value": prepared.hex() if isinstance(prepared, bytes) else prepared,
                "started_ns": started_ns, "finished_ns": time.monotonic_ns(),
            }) + "\n")
        writer.write(json.dumps(response"#,
    );
    fs::write(root.path().join("worker.py"), source).unwrap();
    root
}

fn metewand(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn document(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn trace(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("trace.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn duration(event: &Value) -> Duration {
    Duration::from_nanos(
        event["finished_ns"].as_u64().unwrap() - event["started_ns"].as_u64().unwrap(),
    )
}

#[test]
fn shared_materialization_and_private_conversions_have_distinct_timing_boundaries() {
    let root = repository();
    let vector: Value = serde_json::from_str(PREPARATION_VECTOR).unwrap();
    replace(
        root.path(),
        "metewand.toml",
        "measurement_repetitions = 1",
        "measurement_repetitions = 2",
    );
    let started = Instant::now();
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let elapsed = started.elapsed();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempts = report["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 4);
    let trace = trace(root.path());
    let constructions: Vec<_> = trace
        .iter()
        .filter(|event| event["request"]["method"] == "materialize")
        .collect();
    assert_eq!(constructions.len(), 1);
    let construction = constructions[0];
    let request = &construction["request"];
    assert_eq!(request["dataset_parameters"], vector["dataset_parameters"]);
    assert!(request.get("implementation_parameters").is_none());
    assert!(request.get("implementation_seed").is_none());
    assert!(request.get("problem_parameters").is_none());
    assert!(request.get("problem_contract_id").is_none());

    let mut dataset_instances = BTreeSet::new();
    let mut problem_instances = BTreeSet::new();
    let mut derivations = BTreeSet::new();
    let mut formats = BTreeSet::new();
    let mut views = BTreeSet::new();
    let mut input_hashes = BTreeSet::new();
    let mut measured = Duration::ZERO;
    for attempt in attempts {
        assert_eq!(attempt["status"], "accepted");
        assert_eq!(attempt["result"], vector["result"]);
        assert_eq!(attempt["metrics"], vector["metrics"]);
        let directory = root
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap());
        let record = document(&directory.join("attempt.json"));
        let context = &record["provenance"]["data"]["context"];
        let execution = &record["provenance"]["data"]["execution"];
        let measurements = &record["provenance"]["data"]["measurements"];
        assert_eq!(
            measurements["materialization_time_ns"]["status"],
            "complete"
        );
        assert!(
            Duration::from_nanos(
                measurements["materialization_time_ns"]["value"]
                    .as_u64()
                    .unwrap()
            ) >= duration(construction)
        );
        assert!(context["dataset_instance"]["artifact"].is_string());
        assert!(context["problem_instance"]["dataset"].is_string());
        assert_eq!(
            request["dataset_seed"],
            context["dataset_materialization"]["derivation"]["seed"]["value"]
        );
        dataset_instances.insert(context["dataset_instance"].to_string());
        problem_instances.insert(context["problem_instance"].to_string());
        derivations.insert(
            context["dataset_materialization"]["derivation_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        let view = execution["dataset_directory"].as_str().unwrap();
        assert!(views.insert(view.to_owned()));
        let prepare = trace
            .iter()
            .find(|event| {
                event["request"]["method"] == "prepare" && event["request"]["dataset_dir"] == view
            })
            .unwrap();
        assert!(
            prepare["started_ns"].as_u64().unwrap() > construction["finished_ns"].as_u64().unwrap()
        );
        assert_eq!(
            prepare["request"]["problem_contract_id"],
            execution["problem_contract_id"]
        );
        let format = prepare["prepared_type"].as_str().unwrap();
        assert_eq!(prepare["prepared_value"], vector["prepared_values"][format]);
        let execute = trace
            .iter()
            .find(|event| event["request"]["method"] == "execute" && event["pid"] == prepare["pid"])
            .unwrap();
        let evaluation = trace
            .iter()
            .find(|event| {
                event["request"]["method"] == "evaluate" && event["request"]["dataset_dir"] == view
            })
            .unwrap();
        assert_eq!(evaluation["input_sha256"], prepare["input_sha256"]);
        formats.insert(prepare["prepared_type"].as_str().unwrap().to_owned());
        input_hashes.insert(prepare["input_sha256"].as_str().unwrap().to_owned());
        let observation = document(
            &root
                .path()
                .join("run")
                .join(attempt["observation_directory"].as_str().unwrap())
                .join("observation.json"),
        );
        let timed = Duration::from_nanos(
            observation["timing"]["timed_wall_time_ns"]
                .as_u64()
                .unwrap(),
        );
        for (field, event) in [
            ("prepare_time_ns", prepare),
            ("execute_request_time_ns", execute),
            ("evaluation_time_ns", evaluation),
        ] {
            assert_eq!(measurements[field]["status"], "complete");
            assert!(
                Duration::from_nanos(measurements[field]["value"].as_u64().unwrap())
                    >= duration(event)
            );
        }
        assert_eq!(
            measurements["timed_wall_time_ns"]["value"],
            observation["timing"]["timed_wall_time_ns"]
        );
        assert!(
            timed >= duration(prepare) + duration(execute),
            "conversion and result writing must remain inside prepare_and_execute"
        );
        measured += timed;
        assert_eq!(execution["timing_scope"], "prepare_and_execute");
    }
    assert_eq!(dataset_instances.len(), 1);
    assert_eq!(problem_instances.len(), 1);
    assert_eq!(derivations.len(), 1);
    assert_eq!(input_hashes.len(), 1);
    assert_eq!(
        formats,
        BTreeSet::from(["bytes".to_owned(), "list".to_owned()])
    );
    assert!(
        elapsed >= measured + duration(construction),
        "shared construction must remain outside every measurement"
    );
    assert_eq!(
        trace
            .iter()
            .filter(|event| event["request"]["method"] == "prepare")
            .count(),
        4
    );
}

#[test]
fn implementation_conversion_cannot_rewrite_the_dataset_view() {
    let root = repository();
    replace(
        root.path(),
        "worker.py",
        "elif method == \"prepare\":",
        "elif method == \"prepare\":\n            (Path(request[\"dataset_dir\"]) / \"implementation.bin\").write_bytes(b\"converted\")",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    assert_eq!(output.status.code(), Some(5));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempts = report["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    for attempt in attempts {
        assert_eq!(attempt["diagnostic"]["code"], "dataset_mutation");
        assert!(attempt["observation_id"].is_null());
    }
    assert!(
        !trace(root.path())
            .iter()
            .any(|event| event["request"]["method"] == "execute")
    );
}

#[test]
fn contract_permissions_change_identity_without_moving_the_selected_timing_boundary() {
    let root = repository();
    let mut contracts = BTreeSet::new();
    let mut derivations = BTreeSet::new();
    for destination in ["initial", "permitted"] {
        if destination == "permitted" {
            replace(
                root.path(),
                "problem.toml",
                "allowed_outside_timed_scope = []",
                "allowed_outside_timed_scope = [\"integer_packing\"]",
            );
        }
        let output = metewand(
            root.path(),
            &[
                "run",
                "--output-dir",
                destination,
                "--cache-dir",
                "cache",
                "--output",
                "json",
            ],
        );
        assert!(output.status.success(), "{output:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["data"]["summary"]["accepted_attempts"], 2);
        let trace = trace(root.path());
        for attempt in report["data"]["attempts"].as_array().unwrap() {
            let record = document(
                &root
                    .path()
                    .join(destination)
                    .join(attempt["attempt_directory"].as_str().unwrap())
                    .join("attempt.json"),
            );
            let provenance = &record["provenance"]["data"];
            contracts.insert(provenance["execution"]["problem_contract_id"].to_string());
            derivations.insert(
                provenance["context"]["dataset_materialization"]["derivation_id"].to_string(),
            );
            let prepare = trace
                .iter()
                .find(|event| {
                    event["request"]["method"] == "prepare"
                        && event["request"]["dataset_dir"]
                            == provenance["execution"]["dataset_directory"]
                })
                .unwrap();
            let observation = document(
                &root
                    .path()
                    .join(destination)
                    .join(attempt["observation_directory"].as_str().unwrap())
                    .join("observation.json"),
            );
            assert!(
                Duration::from_nanos(
                    observation["timing"]["timed_wall_time_ns"]
                        .as_u64()
                        .unwrap()
                ) >= duration(prepare)
            );
        }
    }
    assert_eq!(
        contracts.len(),
        2,
        "preparation rules belong to contract identity"
    );
    assert_eq!(
        derivations.len(),
        1,
        "preparation permissions do not change shared construction"
    );
    assert_eq!(
        trace(root.path())
            .iter()
            .filter(|event| event["request"]["method"] == "materialize")
            .count(),
        1
    );
}

#[test]
fn preparation_cannot_bypass_dataset_schemas_or_contract_timing_scopes() {
    for incompatible_schema in [false, true] {
        let root = repository();
        let expected_message = if incompatible_schema {
            fs::copy(
                root.path().join("schemas/dataset.json"),
                root.path().join("schemas/other.json"),
            )
            .unwrap();
            replace(
                root.path(),
                "metewand.toml",
                "output_schema = \"schemas/dataset.json\"",
                "output_schema = \"schemas/other.json\"",
            );
            "does not accept"
        } else {
            replace(
                root.path(),
                "metewand.toml",
                "timing_scope = \"prepare_and_execute\"",
                "timing_scope = \"execute_only\"",
            );
            "does not allow"
        };
        for command in ["check", "plan", "run"] {
            let mut args = vec![command, "--output", "json"];
            if command == "run" {
                args.extend(["--output-dir", "run", "--cache-dir", "cache"]);
            }
            let output = metewand(root.path(), &args);
            assert!(
                !output.status.success(),
                "{command} must reject incompatible selection"
            );
            let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(diagnostic["kind"], "diagnostic");
            assert!(
                diagnostic.to_string().contains(expected_message),
                "{diagnostic}"
            );
            assert!(!root.path().join("trace.jsonl").exists());
            assert!(!root.path().join("run").exists());
            assert!(!root.path().join("cache").exists());
        }
    }
}

#[test]
fn a_contract_permission_does_not_enable_an_unimplemented_timing_scope() {
    let root = repository();
    replace(
        root.path(),
        "metewand.toml",
        "timing_scope = \"prepare_and_execute\"",
        "timing_scope = \"execute_only\"",
    );
    replace(
        root.path(),
        "problem.toml",
        "allowed_timing_scopes = [\"prepare_and_execute\"]",
        "allowed_timing_scopes = [\"prepare_and_execute\", \"execute_only\"]",
    );
    for command in ["check", "plan"] {
        let output = metewand(root.path(), &[command]);
        assert!(output.status.success(), "{output:?}");
    }
    let output = metewand(
        root.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--cache-dir",
            "cache",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diagnostic["data"]["code"], "unsupported_operation");
    assert!(!root.path().join("trace.jsonl").exists());
    assert!(!root.path().join("run").exists());
    assert!(!root.path().join("cache").exists());
}
