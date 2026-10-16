#![cfg(unix)]

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
use metewand_runtime::publication::verify_finalized_directory;
use serde_json::Value;
use tempfile::TempDir;

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

fn repository() -> TempDir {
    let root = TempDir::new().unwrap();
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset"),
        root.path(),
    );
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

fn envelopes(output: &Output) -> Vec<Value> {
    let catalog = public_schema_catalog().unwrap();
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| {
            let value = serde_json::from_str(line).expect("stdout must contain only JSON Lines");
            catalog
                .validate(PublicSchema::MachineOutput.repository_path(), &value)
                .unwrap();
            catalog
                .validate(PublicSchema::RunOutput.repository_path(), &value)
                .unwrap();
            value
        })
        .collect()
}

fn replace(root: &Path, path: &str, before: &str, after: &str) {
    let path = root.join(path);
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains(before));
    fs::write(path, contents.replace(before, after)).unwrap();
}

#[test]
fn run_streams_events_and_publishes_a_self_describing_attempt() {
    let root = repository();
    let source = fs::read(root.path().join("worker.py")).unwrap();
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let records = envelopes(&output);
    assert_eq!(records.len(), 5);
    let events = &records[..4];
    for (sequence, event) in events.iter().enumerate() {
        assert_eq!(event["kind"], "event");
        assert_eq!(event["data"]["sequence"], sequence);
        assert_eq!(event["data"]["command"], "run");
    }
    assert_eq!(events[0]["data"]["event"], "run_started");
    assert_eq!(events[1]["data"]["event"], "attempt_started");
    assert_eq!(events[2]["data"]["event"], "attempt_finished");
    assert_eq!(events[3]["data"]["event"], "run_finished");
    let result = records.last().unwrap();
    assert_eq!(result["kind"], "result");
    assert_eq!(
        result["data"]["events"],
        Value::Array(events.iter().map(|event| event["data"].clone()).collect())
    );
    assert_eq!(result["data"]["summary"]["accepted_attempts"], 1);
    assert_eq!(result["data"]["summary"]["failed_attempts"], 0);
    let attempt = &result["data"]["attempts"][0];
    assert_eq!(attempt["status"], "accepted");
    assert_eq!(attempt["result"], serde_json::json!({"answer": 8}));
    assert_eq!(attempt["metrics"], serde_json::json!({"absolute_error": 0}));
    let attempt_dir = root
        .path()
        .join("run")
        .join(attempt["attempt_directory"].as_str().unwrap());
    let observation_dir = root
        .path()
        .join("run")
        .join(attempt["observation_directory"].as_str().unwrap());
    verify_finalized_directory(&attempt_dir).unwrap();
    verify_finalized_directory(&observation_dir).unwrap();
    assert_eq!(
        document(&attempt_dir.join("attempt.json"))["observations"][0],
        attempt["observation_id"]
    );
    assert_eq!(
        document(&observation_dir.join("observation.json"))["attempt_id"],
        attempt["attempt_id"]
    );
    assert_eq!(
        fs::read_to_string(attempt_dir.join("logs/implementation/stdout.log")).unwrap(),
        "implementation stdout\n"
    );
    assert_eq!(fs::read(root.path().join("worker.py")).unwrap(), source);
    assert!(!root.path().join("__pycache__").exists());
    assert_eq!(fs::read_dir(root.path().join("run")).unwrap().count(), 2);
}

#[test]
fn json_is_one_document_and_relocated_inputs_keep_resolved_ids() {
    let first = repository();
    let second = repository();
    fs::create_dir(second.path().join("implementation")).unwrap();
    fs::write(second.path().join("python3"), "unrelated file").unwrap();
    let mut slot_ids = Vec::new();
    for root in [&first, &second] {
        let output = metewand(
            root.path(),
            &["--output", "json", "run", "--output-dir", "results"],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        public_schema_catalog()
            .unwrap()
            .validate(PublicSchema::RunOutput.repository_path(), &result)
            .unwrap();
        assert_eq!(result["data"]["events"].as_array().unwrap().len(), 4);
        slot_ids.push(result["data"]["attempts"][0]["resolved_slot_id"].clone());
    }
    assert_eq!(slot_ids[0], slot_ids[1]);
}

#[test]
fn run_rejects_unsupported_policies_before_writing_output() {
    let root = repository();
    replace(
        root.path(),
        "metewand.toml",
        "version = 1\ntiming_scope",
        "version = 1\nenforcement = \"required\"\ntiming_scope",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        envelopes(&output)[0]["data"]["code"],
        "unsupported_operation"
    );
    assert!(!root.path().join("run").exists());
}

#[test]
fn run_refuses_existing_output_and_requires_an_explicit_destination() {
    let root = repository();
    let output = metewand(root.path(), &["run", "--output", "jsonl"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        envelopes(&output)[0]["data"]["code"],
        "missing_output_directory"
    );
    fs::create_dir(root.path().join("run")).unwrap();
    fs::write(root.path().join("run/keep"), "original").unwrap();
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(74));
    assert_eq!(envelopes(&output)[0]["data"]["code"], "run_setup_failed");
    assert_eq!(
        fs::read_to_string(root.path().join("run/keep")).unwrap(),
        "original"
    );
}

#[test]
fn worker_crashes_remain_typed_failed_attempts_and_exit_unsuccessfully() {
    let root = repository();
    replace(
        root.path(),
        "worker.py",
        "elif method == \"execute\":",
        "elif method == \"execute\":\n            os._exit(17)",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(5));
    let records = envelopes(&output);
    let result = &records.last().unwrap()["data"];
    assert_eq!(result["summary"]["failed_attempts"], 1);
    let attempt = &result["attempts"][0];
    assert_eq!(attempt["status"], "failed");
    assert_eq!(attempt["diagnostic"]["code"], "implementation_crash");
    assert!(attempt["observation_id"].is_null());
    let path = root
        .path()
        .join("run")
        .join(attempt["attempt_directory"].as_str().unwrap());
    verify_finalized_directory(&path).unwrap();
    assert_eq!(
        document(&path.join("attempt.json"))["outcome"]["diagnostic"]["code"],
        "implementation_crash"
    );
    assert!(!root.path().join("run/observations").exists());
}

#[test]
fn jsonl_flushes_events_before_a_worker_finishes() {
    let root = repository();
    replace(
        root.path(),
        "worker.py",
        "elif method == \"execute\":",
        "elif method == \"execute\":\n            import time\n            time.sleep(0.3)",
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root.path())
        .args(["run", "--output-dir", "run", "--output", "jsonl"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["data"]["event"],
        "run_started"
    );
    assert!(child.try_wait().unwrap().is_none());
    let mut remainder = String::new();
    std::io::Read::read_to_string(&mut reader, &mut remainder).unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn grids_and_repetitions_execute_in_the_same_order_as_the_logical_plan() {
    let root = repository();
    replace(
        root.path(),
        "metewand.toml",
        "measurement_repetitions = 1",
        "measurement_repetitions = 2",
    );
    replace(
        root.path(),
        "metewand.toml",
        "implementation_repetitions = 1",
        "implementation_repetitions = 2",
    );
    replace(
        root.path(),
        "metewand.toml",
        "seed = 7",
        "seed = 7\n\n[experiments.problem_parameters]\noffset = { grid = [2, 3] }",
    );
    let plan = metewand(root.path(), &["plan", "--output", "json"]);
    assert!(plan.status.success());
    let plan: Value = serde_json::from_slice(&plan.stdout).unwrap();
    let expected = plan["data"]["experiments"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|candidate| candidate["specifications"].as_array().unwrap())
        .flat_map(|specification| specification["attempt_slots"].as_array().unwrap())
        .cloned()
        .collect::<Vec<_>>();
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = envelopes(&output);
    let result = &records.last().unwrap()["data"];
    assert_eq!(result["summary"]["accepted_attempts"], 8);
    assert_eq!(result["summary"]["failed_attempts"], 0);
    assert_eq!(
        result["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|attempt| attempt["logical_slot_id"].clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(result["events"].as_array().unwrap().len(), 18);
    for (index, event) in result["events"].as_array().unwrap().iter().enumerate() {
        assert_eq!(event["sequence"], index);
    }
}

#[test]
fn scientific_completion_failure_keeps_its_valid_observation() {
    let root = repository();
    replace(
        root.path(),
        "worker.py",
        "\"reached\" if error == 0",
        "\"not_reached\" if error == 0",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(5));
    let records = envelopes(&output);
    let attempt = &records.last().unwrap()["data"]["attempts"][0];
    assert_eq!(
        attempt["diagnostic"]["code"],
        "completion_target_not_reached"
    );
    let observation = root
        .path()
        .join("run")
        .join(attempt["observation_directory"].as_str().unwrap());
    verify_finalized_directory(&observation).unwrap();
    assert_eq!(
        document(&observation.join("observation.json"))["validity"]["completion"],
        "not_reached"
    );
}

#[test]
fn technical_failures_keep_distinct_codes_and_do_not_publish_observations() {
    let cases = [
        (
            "prepared = sum(values)",
            "prepared = 1 + sum(values)",
            "invalid_result",
            None,
        ),
        (
            "\"data\": {\"answer\": prepared}",
            "\"data\": {\"answer\": \"invalid\"}",
            "invalid_result",
            None,
        ),
        (
            "\"data\": {\"absolute_error\": error}",
            "\"data\": {\"absolute_error\": \"invalid\"}",
            "evaluator_failure",
            None,
        ),
        (
            "elif method == \"execute\":",
            "elif method == \"execute\":\n            writer.write(b'{\"id\":}\\n')\n            writer.flush()\n            return",
            "protocol_error",
            None,
        ),
        (
            "elif method == \"execute\":",
            "elif method == \"execute\":\n            import time\n            time.sleep(2)",
            "phase_timeout",
            Some("500 ms"),
        ),
    ];
    for (before, after, code, timeout) in cases {
        let root = repository();
        replace(root.path(), "worker.py", before, after);
        if let Some(timeout) = timeout {
            replace(
                root.path(),
                "metewand.toml",
                "version = 1\ntiming_scope",
                &format!("version = 1\ntimeout = {timeout:?}\ntiming_scope"),
            );
        }
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert_eq!(
            output.status.code(),
            Some(5),
            "{code}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records = envelopes(&output);
        let attempt = &records.last().unwrap()["data"]["attempts"][0];
        assert_eq!(attempt["diagnostic"]["code"], code);
        assert!(attempt["observation_id"].is_null());
        verify_finalized_directory(
            &root
                .path()
                .join("run")
                .join(attempt["attempt_directory"].as_str().unwrap()),
        )
        .unwrap();
        assert!(!root.path().join("run/observations").exists());
    }
}

#[test]
fn input_mutation_fails_the_attempt_and_preserves_repository_files() {
    let root = repository();
    let original = fs::read(root.path().join("data.json")).unwrap();
    replace(
        root.path(),
        "worker.py",
        "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]",
        "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]\n            path = Path(request[\"dataset_dir\"]) / \"data.json\"\n            path.write_bytes(path.read_bytes() + b'\\n')",
    );
    replace(
        root.path(),
        "metewand.toml",
        "measurement_repetitions = 1",
        "measurement_repetitions = 2",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(5));
    let records = envelopes(&output);
    let report = &records.last().unwrap()["data"];
    assert_eq!(report["summary"]["failed_attempts"], 2);
    for attempt in report["attempts"].as_array().unwrap() {
        assert_eq!(attempt["diagnostic"]["code"], "input_changed");
        assert!(attempt["observation_id"].is_null());
    }
    assert_eq!(fs::read(root.path().join("data.json")).unwrap(), original);
}

#[test]
fn dataset_free_runs_receive_a_single_empty_unit_dataset() {
    let root = repository();
    replace(root.path(), "metewand.toml", "datasets = [\"numbers\"]", "");
    replace(
        root.path(),
        "problem.toml",
        "dataset_schemas = [\"schemas/dataset.json\"]",
        "dataset_schemas = []",
    );
    replace(root.path(), "problem.toml", "dataset = \"data.json\"\n", "");
    replace(
        root.path(),
        "worker.py",
        "values = read_json(Path(request[\"dataset_dir\"]) / \"data.json\")[\"values\"]",
        "assert list(Path(request[\"dataset_dir\"]).iterdir()) == []\n            values = []",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = envelopes(&output);
    assert_eq!(
        records.last().unwrap()["data"]["attempts"][0]["result"],
        serde_json::json!({"answer": 2})
    );
}

#[test]
fn every_selected_worker_is_preflighted_before_any_launch() {
    let root = repository();
    replace(
        root.path(),
        "metewand.toml",
        "implementations = [\"reference\"]",
        "implementations = [\"reference\", \"later\"]",
    );
    let mut manifest = fs::read_to_string(root.path().join("metewand.toml")).unwrap();
    manifest.push_str("\n[implementations.later]\nrunner = \"command\"\nprogram = \"python3\"\nargs = [\"worker.py\", \"implementation\"]\nsources = [\"worker.py\"]\nenvironment = \"local\"\nproblem_contracts = [\"sum\"]\ncapabilities = [\"one_shot\"]\nprotocol_transport = \"stdio\"\n");
    fs::write(root.path().join("metewand.toml"), manifest).unwrap();
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(2));
    let records = envelopes(&output);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["data"]["code"], "unsupported_operation");
    assert!(!root.path().join("run").exists());
}

#[test]
fn manifest_paths_are_relative_to_the_invocation_and_inputs_to_the_manifest() {
    let root = repository();
    let working = TempDir::new().unwrap();
    let output = metewand(
        working.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--manifest",
            root.path().join("metewand.toml").to_str().unwrap(),
            "--output",
            "jsonl",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(working.path().join("run/attempts").is_dir());
    assert!(!root.path().join("run").exists());
}

#[test]
fn interpreted_entrypoints_and_repository_shebangs_use_declared_sources() {
    use std::os::unix::fs::PermissionsExt;

    for interpreted in [true, false] {
        let root = repository();
        if interpreted {
            replace(
                root.path(),
                "metewand.toml",
                "runner = \"command\"\nprogram = \"python3\"",
                "runner = \"python\"\nentrypoint = \"worker.py\"",
            );
        } else {
            replace(
                root.path(),
                "metewand.toml",
                "program = \"python3\"",
                "program = \"./worker.py\"",
            );
            let path = root.path().join("worker.py");
            let source = fs::read_to_string(&path).unwrap();
            fs::write(&path, format!("#!/usr/bin/env python3\n{source}")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        replace(
            root.path(),
            "metewand.toml",
            "\"{sources}/worker.py\", ",
            "",
        );
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            envelopes(&output).last().unwrap()["data"]["summary"]["accepted_attempts"],
            1
        );
    }
}

#[test]
fn source_arguments_and_repository_programs_cannot_escape_their_roots() {
    let root = repository();
    replace(
        root.path(),
        "metewand.toml",
        "{sources}/worker.py",
        "{sources}/../worker.py",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        envelopes(&output)[0]["data"]["code"],
        "unsupported_operation"
    );
    assert!(!root.path().join("run").exists());

    let root = repository();
    let outside = TempDir::new_in(root.path().parent().unwrap()).unwrap();
    let external = outside.path().join("worker");
    fs::write(&external, "#!/usr/bin/env python3\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&external, fs::Permissions::from_mode(0o755)).unwrap();
    let relative = format!(
        "../{}/worker",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    replace(
        root.path(),
        "metewand.toml",
        "program = \"python3\"",
        &format!("program = {relative:?}"),
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        envelopes(&output)[0]["data"]["code"],
        "unsupported_operation"
    );
    assert!(!root.path().join("run").exists());
}

#[test]
fn generated_datasets_record_derivation_and_share_materialization() {
    let root = TempDir::new().unwrap();
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/generated-dataset"),
        root.path(),
    );
    replace(
        root.path(),
        "metewand.toml",
        "measurement_repetitions = 1",
        "measurement_repetitions = 2",
    );
    let marker = root.path().join("materialization-count");
    replace(
        root.path(),
        "worker.py",
        "assert role == \"dataset_materializer\"",
        &format!(
            "assert role == \"dataset_materializer\"\n            with open({:?}, 'a') as count: count.write('1')",
            marker.to_str().unwrap()
        ),
    );
    let manifest = fs::read_to_string(root.path().join("metewand.toml")).unwrap();
    let start = manifest.find("[implementations.reference]").unwrap();
    let end = manifest[start..].find("[environments.local]").unwrap() + start;
    let alias = manifest[start..end].replace("implementations.reference", "implementations.second");
    fs::write(
        root.path().join("metewand.toml"),
        format!(
            "{}\n{alias}",
            manifest.replace(
                "implementations = [\"reference\"]",
                "implementations = [\"reference\", \"second\"]"
            )
        ),
    )
    .unwrap();
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempts = report["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 4);
    assert_eq!(fs::read_to_string(marker).unwrap(), "1");
    let mut derivations = Vec::new();
    for attempt in attempts {
        let path = root
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap());
        let provenance = document(&path.join("attempt.json"));
        let materialization =
            &provenance["provenance"]["data"]["context"]["dataset_materialization"];
        assert!(
            materialization["derivation_id"]
                .as_str()
                .unwrap()
                .starts_with("mw1-dataset-derivation-")
        );
        assert_eq!(
            materialization["output"]["sha256"].as_str().unwrap().len(),
            64
        );
        derivations.push(materialization["derivation_id"].clone());
    }
    assert_eq!(derivations[0], derivations[1]);
}

#[test]
fn persistent_cache_reuses_constructions_and_assigns_a_private_view_to_each_attempt() {
    let root = TempDir::new().unwrap();
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/generated-dataset"),
        root.path(),
    );
    replace(
        root.path(),
        "metewand.toml",
        "measurement_repetitions = 1",
        "measurement_repetitions = 2",
    );
    let builds = root.path().join("build-count");
    let views = root.path().join("dataset-views");
    replace(
        root.path(),
        "worker.py",
        "assert role == \"dataset_materializer\"",
        &format!(
            "assert role == \"dataset_materializer\"\n            with open({:?}, 'a') as count: count.write('1')",
            builds.to_str().unwrap()
        ),
    );
    replace(
        root.path(),
        "worker.py",
        "assert role == \"implementation\"\n            values =",
        &format!(
            "assert role == \"implementation\"\n            with open({:?}, 'a') as views: views.write(request['dataset_dir'] + '\\n')\n            values =",
            views.to_str().unwrap()
        ),
    );
    let mut artifacts = Vec::new();
    for destination in ["first", "second"] {
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
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["data"]["summary"]["accepted_attempts"], 2);
        for attempt in report["data"]["attempts"].as_array().unwrap() {
            let record = document(
                &root
                    .path()
                    .join(destination)
                    .join(attempt["attempt_directory"].as_str().unwrap())
                    .join("attempt.json"),
            );
            artifacts.push(
                record["provenance"]["data"]["context"]["dataset_instance"]["artifact"].clone(),
            );
        }
    }
    assert_eq!(fs::read_to_string(builds).unwrap(), "1");
    assert!(
        artifacts
            .iter()
            .all(|a| a == &artifacts[0] && a.is_string())
    );
    let view_paths = fs::read_to_string(views).unwrap();
    let view_paths = view_paths
        .lines()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(view_paths.len(), 4);
    for path in view_paths {
        assert!(!path.contains("/cache/"));
        assert!(!Path::new(path).exists());
    }
    assert_eq!(
        fs::read_dir(root.path().join("cache/objects"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(root.path().join("cache/derivations"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(root.path().join("cache/leases"))
            .unwrap()
            .count(),
        0
    );

    let object = fs::read_dir(root.path().join("cache/objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(object.join("data/data.json"), b"changed canonical data").unwrap();
    let output = metewand(
        root.path(),
        &[
            "run",
            "--output-dir",
            "third",
            "--cache-dir",
            "cache",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(74));
    let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diagnostic["data"]["code"], "artifact_cache_failed");
    assert!(!root.path().join("third").exists());
    assert_eq!(
        fs::read(object.join("data/data.json")).unwrap(),
        b"changed canonical data"
    );
}

#[test]
fn cache_preflight_refuses_source_overlap_and_unsupported_policies_without_creating_a_cache() {
    let root = repository();
    replace(
        root.path(),
        "metewand.toml",
        "timing_scope = \"prepare_and_execute\"",
        "timing_scope = \"prepare_and_execute\"\nenforcement = \"required\"",
    );
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
    assert!(!output.status.success());
    assert!(!root.path().join("cache").exists());
    assert!(!root.path().join("run").exists());
    replace(
        root.path(),
        "metewand.toml",
        "enforcement = \"required\"",
        "enforcement = \"best_effort\"",
    );
    let output = metewand(
        root.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--cache-dir",
            "run",
            "--output",
            "json",
        ],
    );
    assert!(!output.status.success());
    assert!(!root.path().join("run").exists());

    fs::create_dir(root.path().join("data")).unwrap();
    fs::copy(
        root.path().join("data.json"),
        root.path().join("data/input.json"),
    )
    .unwrap();
    replace(
        root.path(),
        "metewand.toml",
        "sources = [\"data.json\"]",
        "sources = [\"data\"]",
    );
    let output = metewand(
        root.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--cache-dir",
            "data/cache",
            "--output",
            "json",
        ],
    );
    assert!(!output.status.success());
    assert!(!root.path().join("data/cache").exists());
    assert!(!root.path().join("run").exists());
}

#[test]
fn pinned_transform_downloads_only_during_run() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    let root = TempDir::new().unwrap();
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/generated-dataset"),
        root.path(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/data", listener.local_addr().unwrap());
    replace(
        root.path(),
        "metewand.toml",
        "parameter_defaults = { values = [1, 2, 3] }",
        &format!(
            "parameter_defaults = {{ values = [1, 2, 3] }}\nsource = {{ url = {url:?}, sha256 = \"4cf6ac7faa18ca1751f7b12a1391efef4e57a497c909157394ec9072d29f0641\" }}"
        ),
    );
    for command in ["check", "plan"] {
        let output = metewand(root.path(), &[command]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    listener.set_nonblocking(false).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        let body = br#"{"values":[4,5]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempt = &report["data"]["attempts"][0];
    let path = root
        .path()
        .join("run")
        .join(attempt["attempt_directory"].as_str().unwrap());
    let record = document(&path.join("attempt.json"));
    let source = &record["provenance"]["data"]["context"]["dataset_materialization"]["derivation"]
        ["sources"][0];
    assert_eq!(source["selection"]["url"], url);
    assert_eq!(
        source["selection"]["sha256"],
        "4cf6ac7faa18ca1751f7b12a1391efef4e57a497c909157394ec9072d29f0641"
    );
}

#[test]
fn materialization_failure_reports_a_diagnostic_before_attempts_exist() {
    let root = TempDir::new().unwrap();
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/generated-dataset"),
        root.path(),
    );
    replace(
        root.path(),
        "worker.py",
        "elif method == \"materialize\":",
        "elif method == \"materialize\":\n            os._exit(17)",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    assert_eq!(output.status.code(), Some(5));
    let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diagnostic["kind"], "diagnostic");
    assert_eq!(diagnostic["data"]["code"], "dataset_materialization_failed");
    assert!(!root.path().join("run").exists());
}
