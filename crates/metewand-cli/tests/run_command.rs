#![cfg(unix)]

mod support;

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
    let updated = if path
        .extension()
        .is_some_and(|extension| extension == "yaml")
    {
        support::replace_yaml(&contents, before, after)
    } else {
        assert!(contents.contains(before), "missing {before:?} in {path:?}");
        contents.replace(before, after)
    };
    fs::write(path, updated).unwrap();
}

#[test]
fn run_publishes_each_authorized_timing_scope_with_its_accounting_boundary() {
    for (scope, cpu_scope) in [
        ("cold_end_to_end", "worker_lifetime_through_execute"),
        ("prepare_and_execute", "prepare_and_execute"),
        ("execute_only", "execute_only"),
    ] {
        let root = repository();
        replace(
            root.path(),
            "metewand.yaml",
            "timing-scope: \"prepare_and_execute\"",
            &format!("timing-scope: {scope:?}"),
        );
        replace(
            root.path(),
            "problem.yaml",
            "allowed-timing-scopes: [\"prepare_and_execute\"]",
            &format!("allowed-timing-scopes: [{scope:?}]"),
        );
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert!(output.status.success(), "{output:?}");
        let records = envelopes(&output);
        let attempt = &records.last().unwrap()["data"]["attempts"][0];
        let published = document(
            &root
                .path()
                .join("run")
                .join(attempt["observation_directory"].as_str().unwrap())
                .join("observation.json"),
        );
        let execution = &published["provenance"]["data"]["execution"];
        assert_eq!(execution["timing_scope"], scope);
        assert_eq!(execution["operation_scope"], scope == "execute_only");
        assert_eq!(
            published["provenance"]["data"]["measurements"]["cpu_time_scope"],
            cpu_scope
        );
        assert_eq!(
            published["provenance"]["data"]["context"]["timing_scope"],
            scope
        );
        assert_eq!(
            published["timing"]["timed_wall_time_ns"],
            published["provenance"]["data"]["measurements"]["timed_wall_time_ns"]["value"]
        );
        assert_eq!(execution["scientific_timeout_ns"], Value::Null);
        assert_eq!(execution["operational_timeouts_ns"]["hello"], "10000000000");
    }
}

#[test]
fn execute_only_scientific_timeout_leaves_untimed_phases_their_operational_limits() {
    let root = repository();
    replace(
        root.path(),
        "metewand.yaml",
        "timing-scope: \"prepare_and_execute\"",
        "timing-scope: \"execute_only\"\ntimeout: \"300 ms\"",
    );
    replace(
        root.path(),
        "problem.yaml",
        "allowed-timing-scopes: [\"prepare_and_execute\"]",
        "allowed-timing-scopes: [\"execute_only\"]",
    );
    for phase in ["prepare", "evaluate"] {
        replace(
            root.path(),
            "worker.py",
            &format!("elif method == {phase:?}:"),
            &format!(
                "elif method == {phase:?}:\n            import time\n            time.sleep(0.5)"
            ),
        );
    }
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(output.status.success(), "{output:?}");
    let records = envelopes(&output);
    assert_eq!(
        records.last().unwrap()["data"]["summary"]["accepted_attempts"],
        1
    );
}

#[test]
fn run_streams_events_and_publishes_a_self_describing_attempt() {
    let root = repository();
    let manifest_path = root.path().join("metewand.yaml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("      environment: \"local\"\n", "")
        .replace("    environment: \"local\"\n", "")
        .replace("environments:\n  local:\n    kind: \"local\"\n", "")
        .replace("    version: 1\n", "")
        .replace("    implementation-repetitions: 1\n", "")
        .replace("    measurement-repetitions: 1\n", "");
    fs::write(manifest_path, manifest).unwrap();
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
    let provenance = document(&attempt_dir.join("attempt.json"));
    let observation = document(&observation_dir.join("observation.json"));
    let measured = &provenance["provenance"]["data"]["measurements"];
    assert_eq!(*measured, observation["provenance"]["data"]["measurements"]);
    assert_eq!(measured["version"], 2);
    assert_eq!(
        measured["materialization_time_ns"]["reason"],
        "provided_dataset"
    );
    for phase in [
        "worker_startup_time_ns",
        "evaluator_startup_time_ns",
        "prepare_time_ns",
        "execute_request_time_ns",
        "timed_wall_time_ns",
        "evaluation_time_ns",
    ] {
        assert_eq!(measured[phase]["status"], "complete");
        assert!(measured[phase]["value"].as_u64().unwrap() > 0);
    }
    assert_eq!(
        measured["timed_wall_time_ns"]["value"],
        observation["timing"]["timed_wall_time_ns"]
    );
    assert_eq!(
        measured["evaluation_time_ns"]["value"],
        observation["timing"]["evaluation_time_ns"]
    );
    assert!(observation["timing"]["cpu_time_ns"].is_null());
    assert!(observation["timing"]["peak_rss_bytes"].is_null());
    for role in ["implementation", "evaluator"] {
        let realized = &provenance["provenance"]["data"]["execution"][role]["worker_environment"];
        public_schema_catalog()
            .unwrap()
            .validate(PublicSchema::WorkerEnvironment.repository_path(), realized)
            .unwrap();
        assert_eq!(realized["environment_variables"]["LC_ALL"], "C");
        assert_eq!(
            realized["environment_variables"]["HOME"],
            format!("{}/home", realized["working_directory"].as_str().unwrap())
        );
        assert_eq!(realized["umask"], "0077");
        let normalized = &provenance["provenance"]["data"]["context"][role];
        assert!(
            !normalized
                .to_string()
                .contains(realized["working_directory"].as_str().unwrap())
        );
    }
    let enforcement = serde_json::json!({
        "view": "copy", "enforcement": "best_effort", "verification": "each_worker_operation",
    });
    assert_eq!(
        provenance["provenance"]["data"]["context"]["dataset_immutability"],
        enforcement
    );
    assert_eq!(
        provenance["provenance"]["data"]["execution"]["dataset_immutability"],
        enforcement
    );
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
    assert!(root.path().join("run/run.sqlite3").is_file());
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
        "metewand.yaml",
        "version: 1\ntiming-scope",
        "version: 1\nenforcement: \"required\"\ntiming-scope",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        envelopes(&output)[0]["data"]["causes"][0]["code"],
        "required_control_unsupported"
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
    let record = document(&path.join("attempt.json"));
    let measured = &record["provenance"]["data"]["measurements"];
    assert_eq!(measured["worker_startup_time_ns"]["status"], "complete");
    assert_eq!(measured["prepare_time_ns"]["status"], "complete");
    assert_eq!(measured["execute_request_time_ns"]["status"], "partial");
    assert_eq!(measured["timed_wall_time_ns"]["status"], "partial");
    assert_eq!(measured["evaluation_time_ns"]["reason"], "not_started");
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
        "metewand.yaml",
        "measurement-repetitions: 1",
        "measurement-repetitions: 2",
    );
    replace(
        root.path(),
        "metewand.yaml",
        "implementation-repetitions: 1",
        "implementation-repetitions: 2",
    );
    replace(
        root.path(),
        "metewand.yaml",
        "seed: 7",
        "seed: 7\nproblem-parameters:\n  offset: { grid: [2, 3] }",
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
fn randomized_runs_interleave_comparable_implementations_and_record_the_schedule() {
    use std::collections::BTreeSet;

    let mut orders = Vec::new();
    for _ in 0..2 {
        let root = reuse_repository();
        let path = root.path().join("metewand.yaml");
        let manifest = fs::read_to_string(&path).unwrap();
        let implementation = manifest
            .split_once("  reference:")
            .unwrap()
            .1
            .split_once("environments:")
            .unwrap()
            .0;
        let manifest = manifest
            .replace("implementations:\n", &format!("implementations:\n  alternative:{implementation}"))
            .replace("datasets:\n", "datasets:\n  other:\n    output-schema: \"schemas/dataset.json\"\n    sources: [\"data.json\"]\n");
        fs::write(&path, manifest).unwrap();
        for (before, after) in [
            (
                "implementations: [\"reference\"]",
                "implementations: [\"reference\", \"alternative\"]",
            ),
            (
                "datasets: [\"numbers\"]",
                "datasets: [\"numbers\", \"other\"]",
            ),
            ("measurement-repetitions: 3", "measurement-repetitions: 2"),
            (
                "version: 1\nworker-reuse",
                "version: 1\nrun-order: \"randomized\"\nworker-reuse",
            ),
        ] {
            replace(root.path(), "metewand.yaml", before, after);
        }
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert!(output.status.success(), "{output:?}");
        let records = envelopes(&output);
        let report = records.last().unwrap();
        assert_eq!(report["data"]["summary"]["accepted_attempts"], 8);
        let attempts = attempt_documents(root.path(), report);
        let mut blocks = BTreeSet::new();
        let mut previous_priority = None;
        for pair in attempts.as_chunks::<2>().0 {
            let contexts = pair
                .iter()
                .map(|attempt| &attempt["provenance"]["data"]["context"])
                .collect::<Vec<_>>();
            let first = &contexts[0]["scheduling"];
            let second = &contexts[1]["scheduling"];
            assert_eq!(first["run_order"], "randomized");
            assert_eq!(first["policy_version"], 1);
            assert_eq!(first["comparison_block"], second["comparison_block"]);
            assert!(blocks.insert(first["comparison_block"].to_string()));
            let priority = first["block_priority"].as_str().unwrap();
            if let Some(previous) = previous_priority {
                assert!(previous < priority);
            }
            previous_priority = Some(priority);
            assert!(first["slot_priority"].as_str() < second["slot_priority"].as_str());
            assert_eq!(
                contexts[0]["implementation_seed"],
                contexts[1]["implementation_seed"]
            );
            assert_ne!(
                contexts[0]["logical_candidate_id"],
                contexts[1]["logical_candidate_id"]
            );
            for context in contexts {
                assert_eq!(
                    context["scheduling"]["comparison_block"]["dataset_instance"],
                    context["problem_instance"]["dataset"]
                );
                assert_eq!(
                    context["scheduling"]["comparison_block"]["problem_configuration"],
                    context["problem_instance"]["configuration"]
                );
            }
        }
        assert_eq!(blocks.len(), 4);
        let mut previous_specification = None;
        for attempt in &attempts {
            let data = &attempt["provenance"]["data"];
            let warmups = data["context"]["worker_reuse"]["warmups_on_this_attempt"]
                .as_array()
                .unwrap();
            let specification = &data["context"]["resolved_specification"]["logical_specification"];
            assert_eq!(
                warmups.len(),
                if previous_specification == Some(specification) {
                    0
                } else {
                    2
                }
            );
            for (index, warmup) in warmups.iter().enumerate() {
                assert_eq!(warmup["index"], index);
            }
            assert!(warmups.iter().all(|warmup| warmup["status"] == "validated"));
            previous_specification = Some(specification);
        }
        let order = report["data"]["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|attempt| attempt["resolved_slot_id"].clone())
            .collect::<Vec<_>>();
        let started = report["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event"] == "attempt_started")
            .map(|event| event["resolved_slot_id"].clone())
            .collect::<Vec<_>>();
        assert_eq!(started, order);
        orders.push(order);
    }
    assert_eq!(orders[0], orders[1]);
}

fn scheduling_contract_repository() -> TempDir {
    let root = repository();
    let path = root.path().join("metewand.yaml");
    let manifest = fs::read_to_string(&path).unwrap();
    let implementation = manifest
        .split_once("  reference:")
        .unwrap()
        .1
        .split_once("environments:")
        .unwrap()
        .0;
    fs::write(
        &path,
        manifest.replace(
            "implementations:\n",
            &format!("implementations:\n  alternative:{implementation}"),
        ),
    )
    .unwrap();
    for (before, after) in [
        (
            "  reference:",
            "  reference:\n    parameter-schema: \"schemas/implementation-parameters.json\"\n    parameter-defaults: { method: 0 }",
        ),
        (
            "implementations: [\"reference\"]",
            "implementations: [\"reference\", \"alternative\"]",
        ),
        ("measurement-repetitions: 1", "measurement-repetitions: 2"),
        (
            "version: 1\ntiming-scope",
            "version: 1\nrun-order: \"randomized\"\ntiming-scope",
        ),
    ] {
        replace(root.path(), "metewand.yaml", before, after);
    }
    fs::write(
        root.path().join("schemas/implementation-parameters.json"),
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object",
            "properties":{"method":{"type":"integer"}},"required":["method"],
            "additionalProperties":false}"#,
    )
    .unwrap();
    replace(
        root.path(),
        "worker.py",
        "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]",
        "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]\n            prepared_seed = request[\"implementation_seed\"]",
    );
    replace(
        root.path(),
        "worker.py",
        "statistics={},",
        "statistics={\"received_seed\": prepared_seed},",
    );
    root
}

fn scheduled_slot_snapshots(root: &Path, expected_count: usize) -> Vec<(String, Value)> {
    use std::collections::{BTreeMap, BTreeSet};

    use metewand_core::seed::derive_implementation_seed;

    let output = metewand(root, &["plan", "--output", "json"]);
    assert!(output.status.success(), "{output:?}");
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["data"]["summary"]["attempt_slots"], expected_count);
    assert_eq!(plan["data"]["summary"]["observation_slots"], expected_count);
    let mut planned = BTreeMap::new();
    for experiment in plan["data"]["experiments"].as_array().unwrap() {
        let seed = match experiment["name"].as_str().unwrap() {
            "main" => 7,
            "unrelated" => 8,
            name => panic!("unexpected experiment: {name}"),
        };
        for candidate in experiment["candidates"].as_array().unwrap() {
            for (repetition, specification) in candidate["specifications"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                let seed = derive_implementation_seed(
                    seed,
                    candidate["dataset_configuration"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                    candidate["problem_configuration"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                    repetition as u64,
                )
                .unwrap();
                let slots = specification["attempt_slots"].as_array().unwrap();
                let observations = specification["observation_slots"].as_array().unwrap();
                assert_eq!(slots.len(), observations.len());
                for (slot, observation) in slots.iter().zip(observations) {
                    assert!(
                        planned
                            .insert(
                                slot.as_str().unwrap().to_owned(),
                                (seed.value, observation.as_str().unwrap().to_owned()),
                            )
                            .is_none()
                    );
                }
            }
        }
    }
    assert_eq!(planned.len(), expected_count);

    let output = metewand(root, &["run", "--output-dir", "run", "--output", "jsonl"]);
    assert!(output.status.success(), "{output:?}");
    let records = envelopes(&output);
    let report = records.last().unwrap();
    assert_eq!(report["data"]["summary"]["attempt_slots"], expected_count);
    assert_eq!(
        report["data"]["summary"]["accepted_attempts"],
        expected_count
    );
    assert_eq!(report["data"]["summary"]["failed_attempts"], 0);
    let attempts = report["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), expected_count);
    let actual = attempts
        .iter()
        .map(|attempt| attempt["logical_slot_id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, planned.keys().map(String::as_str).collect());
    let observations = attempts
        .iter()
        .map(|attempt| attempt["observation_id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(observations.len(), expected_count);
    let started = records
        .iter()
        .filter(|record| record["data"]["event"] == "attempt_started")
        .map(|record| record["data"]["logical_slot_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        started,
        attempts
            .iter()
            .map(|attempt| attempt["logical_slot_id"].clone())
            .collect::<Vec<_>>()
    );

    attempt_documents(root, report)
        .iter()
        .map(|attempt| {
            let data = &attempt["provenance"]["data"];
            let context = &data["context"];
            let slot = context["logical_slot_id"].as_str().unwrap();
            let (seed, observation) = &planned[slot];
            assert_eq!(context["implementation_seed"], *seed);
            assert_eq!(data["implementation_statistics"]["received_seed"], *seed);
            assert_eq!(
                context["resolved_observation_slot"]["logical_slot"],
                observation.as_str()
            );
            (
                slot.to_owned(),
                serde_json::json!({
                    "seed": seed,
                    "scheduling": context["scheduling"],
                    "dataset_instance": context["dataset_instance"],
                    "problem_instance": context["problem_instance"],
                    "specification": context["resolved_specification"],
                    "attempt_slot": context["resolved_attempt_slot"],
                    "observation_slot": context["resolved_observation_slot"],
                }),
            )
        })
        .collect()
}

#[test]
fn unrelated_slots_preserve_existing_relative_execution_order_and_seeds() {
    use std::collections::BTreeSet;

    let root = scheduling_contract_repository();
    let baseline = scheduled_slot_snapshots(root.path(), 4);
    let existing = baseline.iter().map(|(id, _)| id).collect::<BTreeSet<_>>();
    let root = scheduling_contract_repository();
    replace(
        root.path(),
        "metewand.yaml",
        "experiments:",
        r#"experiments:
  - name: "unrelated"
    problem: "sum"
    datasets: ["numbers"]
    implementations: ["alternative", "reference"]
    execution-policy: "local"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 2
    seed: 8
"#,
    );
    let retained = scheduled_slot_snapshots(root.path(), 8)
        .into_iter()
        .filter(|(id, _)| existing.contains(id))
        .collect::<Vec<_>>();
    assert_eq!(retained, baseline);
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
fn human_run_output_reports_metrics_and_scientific_failure_reasons() {
    for reached in [true, false] {
        let root = repository();
        if !reached {
            replace(
                root.path(),
                "worker.py",
                "\"reached\" if error == 0",
                "\"not_reached\" if error == 0",
            );
        }
        let output = metewand(root.path(), &["run", "--output-dir", "run"]);
        assert_eq!(output.status.code(), Some(if reached { 0 } else { 5 }));
        let human = String::from_utf8(output.stdout).unwrap();
        assert!(human.contains("Method | Rep"), "{human}");
        assert!(human.contains("reference | 1:1 | 0 |"), "{human}");
        assert!(human.contains("{\"absolute_error\":0}"), "{human}");
        assert!(human.contains("timing: prepare_and_execute"), "{human}");
        if reached {
            assert!(!human.contains("failure ["), "{human}");
        } else {
            assert!(
                human.contains("failure [completion_target_not_reached]"),
                "{human}"
            );
            assert!(
                human.contains("The valid result did not reach the problem's completion target."),
                "{human}"
            );
        }
    }
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
            "scientific_timeout",
            Some("500 ms"),
        ),
    ];
    for (before, after, code, timeout) in cases {
        let root = repository();
        replace(root.path(), "worker.py", before, after);
        if let Some(timeout) = timeout {
            replace(
                root.path(),
                "metewand.yaml",
                "version: 1\ntiming-scope",
                &format!("version: 1\ntimeout: {timeout:?}\ntiming-scope"),
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
        let row = &records.last().unwrap()["data"]["comparison"][0];
        assert_eq!(row["status"], "failed");
        assert_eq!(row["diagnostic"]["code"], code);
        assert!(row["metrics"].is_null());
        assert!(row["completion"].is_null());
        if code == "scientific_timeout" {
            assert_eq!(row["timed_wall_time_status"], "partial");
            assert!(row["timed_wall_time_ns"].as_u64().unwrap() > 0);
        }
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
        "metewand.yaml",
        "measurement-repetitions: 1",
        "measurement-repetitions: 2",
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
        assert_eq!(attempt["diagnostic"]["code"], "dataset_mutation");
        assert!(attempt["observation_id"].is_null());
        let failed = document(
            &root
                .path()
                .join("run")
                .join(attempt["attempt_directory"].as_str().unwrap())
                .join("attempt.json"),
        );
        assert_eq!(
            failed["provenance"]["data"]["context"]["dataset_immutability"]["enforcement"],
            "best_effort"
        );
    }
    assert_eq!(fs::read(root.path().join("data.json")).unwrap(), original);
}

#[test]
fn a_private_view_mutation_does_not_poison_the_next_attempt() {
    let root = repository();
    let first_attempt = root.path().join("first-attempt");
    let views = root.path().join("views");
    replace(
        root.path(),
        "metewand.yaml",
        "measurement-repetitions: 1",
        "measurement-repetitions: 2",
    );
    replace(
        root.path(),
        "worker.py",
        "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]",
        &format!(
            "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]\n            with open({views:?}, 'a') as views: views.write(request['dataset_dir'] + '\\n')\n            marker = Path({first_attempt:?})\n            if not marker.exists():\n                marker.touch()\n                path = Path(request['dataset_dir']) / 'data.json'\n                path.write_bytes(path.read_bytes() + b'\\n')"
        ),
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let records = envelopes(&output);
    let report = &records.last().unwrap()["data"];
    assert_eq!(report["summary"]["failed_attempts"], 1);
    assert_eq!(report["summary"]["accepted_attempts"], 1);
    assert_eq!(
        report["attempts"][0]["diagnostic"]["code"],
        "dataset_mutation"
    );
    assert!(report["attempts"][0]["observation_id"].is_null());
    assert_eq!(
        report["attempts"][1]["result"],
        serde_json::json!({"answer": 8})
    );
    let views = fs::read_to_string(views).unwrap();
    let paths = views.lines().collect::<Vec<_>>();
    assert_eq!(paths.len(), 2);
    assert_ne!(paths[0], paths[1]);
    assert!(paths.iter().all(|path| !Path::new(path).exists()));
}

#[test]
fn prepare_and_execute_timing_includes_integrity_checks_between_operations() {
    let root = repository();
    let cache_lock = root.path().join("cache/.lock");
    replace(
        root.path(),
        "worker.py",
        "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]",
        &format!(
            "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]\n            import fcntl, threading, time\n            cache_lock = open({cache_lock:?}, 'r+')\n            fcntl.flock(cache_lock, fcntl.LOCK_EX)\n            def release():\n                time.sleep(0.25)\n                fcntl.flock(cache_lock, fcntl.LOCK_UN)\n                cache_lock.close()\n            threading.Thread(target=release).start()"
        ),
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
            "jsonl",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let records = envelopes(&output);
    let attempt = &records.last().unwrap()["data"]["attempts"][0];
    let observation = document(
        &root
            .path()
            .join("run")
            .join(attempt["observation_directory"].as_str().unwrap())
            .join("observation.json"),
    );
    assert!(
        observation["timing"]["timed_wall_time_ns"]
            .as_u64()
            .unwrap()
            >= 200_000_000
    );
    let attempt = document(
        &root
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap())
            .join("attempt.json"),
    );
    assert_eq!(
        attempt["provenance"]["data"]["execution"]["timing_dataset_integrity"],
        "included_between_prepare_and_execute"
    );
}

#[test]
fn dataset_mutations_stop_at_each_operation_before_later_restoration() {
    for (role, phase, next_phase) in [
        ("implementation", "prepare", "implementation.execute"),
        ("implementation", "execute", "problem_evaluator.evaluate"),
        ("problem_evaluator", "evaluate", "implementation.shutdown"),
        ("implementation", "shutdown", "problem_evaluator.shutdown"),
        ("problem_evaluator", "shutdown", "finished"),
    ] {
        let root = repository();
        let operations = root.path().join("operations");
        replace(
            root.path(),
            "worker.py",
            "    prepared = None\n    for line",
            "    prepared = None\n    dataset_path = None\n    original_dataset = None\n    for line",
        );
        replace(
            root.path(),
            "worker.py",
            "method = request[\"method\"]",
            &format!(
                "method = request[\"method\"]\n        with open({operations:?}, 'a') as events: events.write(role + '.' + method + '\\n')\n        if 'dataset_dir' in request:\n            dataset_path = Path(request['dataset_dir']) / 'data.json'\n            original_dataset = dataset_path.read_bytes()\n        if dataset_path and method != {phase:?}:\n            dataset_path.write_bytes(original_dataset)"
            ),
        );
        replace(
            root.path(),
            "worker.py",
            "writer.write(json.dumps(response",
            &format!(
                "if role == {role:?} and method == {phase:?}:\n            dataset_path.write_bytes(original_dataset + b'\\n')\n        writer.write(json.dumps(response"
            ),
        );
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert_eq!(output.status.code(), Some(5), "{role}.{phase}: {output:?}");
        let records = envelopes(&output);
        let attempt = &records.last().unwrap()["data"]["attempts"][0];
        assert_eq!(
            attempt["diagnostic"]["code"], "dataset_mutation",
            "{role}.{phase}"
        );
        assert!(
            attempt["diagnostic"]["message"]
                .as_str()
                .unwrap()
                .contains(phase)
        );
        assert!(attempt["observation_id"].is_null());
        assert!(!root.path().join("run/observations").exists());
        let operations = fs::read_to_string(operations).unwrap();
        assert!(operations.contains(&format!("{role}.{phase}")));
        assert!(!operations.contains(next_phase), "{operations}");
        assert_eq!(
            fs::read(root.path().join("data.json")).unwrap(),
            fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../examples/fixed-dataset/data.json")
            )
            .unwrap()
        );
    }
}

#[test]
fn canonical_dataset_mutations_quarantine_the_object_before_another_attempt() {
    let root = repository();
    let cache_parent = tempfile::tempdir().unwrap();
    let cache = cache_parent.path().join("cache");
    let initial = metewand(
        root.path(),
        &[
            "run",
            "--output-dir",
            "initial",
            "--cache-dir",
            cache.to_str().unwrap(),
            "--output",
            "jsonl",
        ],
    );
    assert!(initial.status.success(), "{initial:?}");
    let object = fs::read_dir(cache.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let operations = root.path().join("operations");
    replace(
        root.path(),
        "metewand.yaml",
        "measurement-repetitions: 1",
        "measurement-repetitions: 2",
    );
    replace(
        root.path(),
        "worker.py",
        "method = request[\"method\"]",
        &format!(
            "method = request[\"method\"]\n        with open({operations:?}, 'a') as events: events.write(role + '.' + method + '\\n')"
        ),
    );
    replace(
        root.path(),
        "worker.py",
        "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]",
        &format!(
            "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]\n            Path({:?}).write_bytes(b'changed canonical bytes')",
            object.join("data/data.json")
        ),
    );
    let output = metewand(
        root.path(),
        &[
            "run",
            "--output-dir",
            "mutated",
            "--cache-dir",
            cache.to_str().unwrap(),
            "--output",
            "jsonl",
        ],
    );
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let records = envelopes(&output);
    let attempts = records.last().unwrap()["data"]["attempts"]
        .as_array()
        .unwrap();
    assert_eq!(attempts.len(), 2);
    for attempt in attempts {
        assert_eq!(attempt["diagnostic"]["code"], "dataset_mutation");
        assert!(attempt["observation_id"].is_null());
    }
    let operations = fs::read_to_string(operations).unwrap();
    assert_eq!(operations.matches("implementation.prepare").count(), 1);
    assert!(!operations.contains("implementation.execute"));
    assert!(!object.exists());
    let quarantined = fs::read_dir(cache.join("quarantine"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read(quarantined.join("object/data/data.json")).unwrap(),
        b"changed canonical bytes"
    );
    assert!(!root.path().join("mutated/observations").exists());
}

#[test]
fn dataset_free_runs_receive_a_single_empty_unit_dataset() {
    let root = repository();
    replace(root.path(), "metewand.yaml", "datasets: [\"numbers\"]", "");
    replace(
        root.path(),
        "problem.yaml",
        "dataset-schemas: [\"schemas/dataset.json\"]",
        "dataset-schemas: []",
    );
    replace(
        root.path(),
        "problem.yaml",
        "  - dataset: \"data.json\"\n    result:",
        "  - result:",
    );
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
        "metewand.yaml",
        "implementations: [\"reference\"]",
        "implementations: [\"reference\", \"later\"]",
    );
    let manifest = fs::read_to_string(root.path().join("metewand.yaml")).unwrap();
    let manifest = manifest.replace("implementations:\n", "implementations:\n  later:\n    runner: \"command\"\n    program: \"python3\"\n    args: [\"worker.py\", \"implementation\"]\n    sources: [\"worker.py\"]\n    environment: \"local\"\n    problem-contracts: [\"sum\"]\n    capabilities: [\"one_shot\"]\n    protocol-transport: \"stdio\"\n");
    fs::write(root.path().join("metewand.yaml"), manifest).unwrap();
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
            root.path().join("metewand.yaml").to_str().unwrap(),
            "--output",
            "jsonl",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(working.path().join("run/run.sqlite3").is_file());
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
                "metewand.yaml",
                "runner: \"command\"\nprogram: \"python3\"",
                "runner: \"python\"\nentrypoint: \"worker.py\"",
            );
        } else {
            replace(
                root.path(),
                "metewand.yaml",
                "program: \"python3\"",
                "program: \"./worker.py\"",
            );
            let path = root.path().join("worker.py");
            let source = fs::read_to_string(&path).unwrap();
            fs::write(&path, format!("#!/usr/bin/env python3\n{source}")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        replace(
            root.path(),
            "metewand.yaml",
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
fn symlinked_venv_packages_are_available_to_all_local_python_launches() {
    use std::os::unix::fs::PermissionsExt;

    for runner in ["python", "command", "shebang"] {
        let root = repository();
        let venv = root.path().join(".venv");
        let output = Command::new("python3")
            .args(["-m", "venv", "--without-pip", "--symlinks"])
            .arg(&venv)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = Command::new(venv.join("bin/python3"))
            .args([
                "-c",
                "import sysconfig; print(sysconfig.get_path('purelib'))",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let packages = String::from_utf8(output.stdout).unwrap();
        fs::write(
            Path::new(packages.trim()).join("venv_probe.py"),
            "VALUE = 42\n",
        )
        .unwrap();
        let worker = root.path().join("worker.py");
        let source = fs::read_to_string(&worker).unwrap();
        let prefix = if runner == "shebang" {
            "#!/usr/bin/env python3\n"
        } else {
            ""
        };
        fs::write(
            &worker,
            format!("{prefix}import venv_probe\nassert venv_probe.VALUE == 42\n{source}"),
        )
        .unwrap();
        if runner == "python" {
            replace(
                root.path(),
                "metewand.yaml",
                "runner: \"command\"\nprogram: \"python3\"",
                "runner: \"python\"\nentrypoint: \"worker.py\"",
            );
        } else if runner == "shebang" {
            replace(
                root.path(),
                "metewand.yaml",
                "program: \"python3\"",
                "program: \"./worker.py\"",
            );
            fs::set_permissions(&worker, fs::Permissions::from_mode(0o755)).unwrap();
        }
        if runner != "command" {
            replace(
                root.path(),
                "metewand.yaml",
                "\"{sources}/worker.py\", ",
                "",
            );
        }
        let path = std::env::join_paths(std::iter::once(venv.join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(root.path())
            .env("PATH", path)
            .args(["run", "--output-dir", "run", "--output", "jsonl"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{runner}: {output:?}");
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
        "metewand.yaml",
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
        "metewand.yaml",
        "program: \"python3\"",
        &format!("program: {relative:?}"),
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
        "metewand.yaml",
        "measurement-repetitions: 1",
        "measurement-repetitions: 2",
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
        let measured = &provenance["provenance"]["data"]["measurements"]["materialization_time_ns"];
        assert_eq!(measured["status"], "complete");
        assert_eq!(*measured, materialization["materialization_time_ns"]);
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
    assert!(
        derivations
            .iter()
            .all(|derivation| derivation == &derivations[0])
    );
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
        "metewand.yaml",
        "measurement-repetitions: 1",
        "measurement-repetitions: 2",
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
        assert_eq!(report["data"]["summary"]["accepted_attempts"], 4);
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
            let measured = &record["provenance"]["data"]["measurements"]["materialization_time_ns"];
            if destination == "first" {
                assert_eq!(measured["status"], "complete");
            } else {
                assert_eq!(
                    measured,
                    &serde_json::json!({"status": "unavailable", "reason": "cache_hit"})
                );
            }
        }
    }
    assert_eq!(fs::read_to_string(&builds).unwrap(), "1");
    assert!(
        artifacts
            .iter()
            .all(|a| a == &artifacts[0] && a.is_string())
    );
    let view_paths = fs::read_to_string(views).unwrap();
    let view_paths = view_paths
        .lines()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(view_paths.len(), 8);
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
    assert!(!object.exists());
    let quarantined = fs::read_dir(root.path().join("cache/quarantine"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read(quarantined.join("object/data/data.json")).unwrap(),
        b"changed canonical data"
    );
    let rebuilt = metewand(
        root.path(),
        &[
            "run",
            "--output-dir",
            "rebuilt",
            "--cache-dir",
            "cache",
            "--output",
            "json",
        ],
    );
    assert!(rebuilt.status.success(), "{rebuilt:?}");
    assert_eq!(fs::read_to_string(builds).unwrap(), "11");
    assert!(object.join("manifest.json").is_file());
    assert_eq!(
        fs::read_dir(root.path().join("cache/quarantine"))
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
}

#[test]
fn cache_preflight_refuses_source_overlap_and_unsupported_policies_without_creating_a_cache() {
    let root = repository();
    replace(
        root.path(),
        "metewand.yaml",
        "timing-scope: \"prepare_and_execute\"",
        "timing-scope: \"prepare_and_execute\"\nenforcement: \"required\"",
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
        "metewand.yaml",
        "enforcement: \"required\"",
        "enforcement: \"best_effort\"",
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
        "metewand.yaml",
        "sources: [\"data.json\"]",
        "sources: [\"data\"]",
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
        "metewand.yaml",
        "parameter-defaults: { values: [1, 2, 3] }",
        &format!(
            "parameter-defaults: {{ values: [1, 2, 3] }}\nsource: {{ url: {url:?}, sha256: \"4cf6ac7faa18ca1751f7b12a1391efef4e57a497c909157394ec9072d29f0641\" }}"
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

fn reuse_repository() -> TempDir {
    let root = repository();
    replace(
        root.path(),
        "metewand.yaml",
        "version: 1\ntiming-scope",
        "version: 1\nworker-reuse: true\nwarmup-runs: 2\ntiming-scope",
    );
    replace(
        root.path(),
        "metewand.yaml",
        "measurement-repetitions: 1",
        "measurement-repetitions: 3",
    );
    replace(
        root.path(),
        "worker.py",
        "    prepared = None\n    for line",
        "    prepared = None\n    executions = 0\n    for line",
    );
    replace(
        root.path(),
        "worker.py",
        "        elif method == \"prepare\":",
        r#"        elif method == "prepare":
            assert not Path("dirty").exists()
            assert not Path(os.environ["XDG_CACHE_HOME"]).joinpath("dirty").exists()"#,
    );
    replace(
        root.path(),
        "worker.py",
        "            write_json(\n                Path(request[\"result_dir\"])",
        r#"            executions += 1
            Path("dirty").write_text("slot state")
            Path(os.environ["XDG_CACHE_HOME"]).joinpath("dirty").write_text("cache state")
            write_json(
                Path(request["result_dir"])"#,
    );
    replace(
        root.path(),
        "worker.py",
        "statistics={},",
        "statistics={\"pid\": os.getpid(), \"executions\": executions},",
    );
    root
}

fn attempt_documents(root: &Path, report: &Value) -> Vec<Value> {
    report["data"]["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|attempt| {
            let directory = root
                .join("run")
                .join(attempt["attempt_directory"].as_str().unwrap());
            verify_finalized_directory(&directory).unwrap();
            document(&directory.join("attempt.json"))
        })
        .collect()
}

#[test]
fn concurrent_lanes_own_separate_reusable_workers_and_warmups() {
    let root = reuse_repository();
    replace(
        root.path(),
        "metewand.yaml",
        "warmup-runs: 2",
        "warmup-runs: 2\nmax-concurrency: 2\nrun-order: \"randomized\"",
    );
    replace(
        root.path(),
        "metewand.yaml",
        "measurement-repetitions: 3",
        "measurement-repetitions: 5",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(output.status.success(), "{output:?}");
    let records = envelopes(&output);
    let report = records.last().unwrap();
    assert_eq!(report["data"]["summary"]["accepted_attempts"], 5);
    let attempts = attempt_documents(root.path(), report);
    let pids = attempts
        .iter()
        .map(|attempt| {
            attempt["provenance"]["data"]["implementation_statistics"]["pid"]
                .as_u64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_ne!(pids[0], pids[1]);
    assert_eq!(pids, [pids[0], pids[1], pids[0], pids[1], pids[0]]);
    for (index, attempt) in attempts.iter().enumerate() {
        let data = &attempt["provenance"]["data"];
        assert_eq!(
            data["implementation_statistics"]["executions"],
            index / 2 + 3
        );
        assert_eq!(
            data["context"]["worker_reuse"]["warmups_on_this_attempt"]
                .as_array()
                .unwrap()
                .len(),
            if index < 2 { 2 } else { 0 }
        );
    }
}

#[test]
fn concurrency_batches_stop_at_experiment_and_policy_boundaries() {
    let root = reuse_repository();
    replace(
        root.path(),
        "metewand.yaml",
        "warmup-runs: 2",
        "warmup-runs: 2\nmax-concurrency: 2",
    );
    let path = root.path().join("metewand.yaml");
    let manifest = fs::read_to_string(&path).unwrap();
    let manifest = manifest.replace(
        "execution-policies:",
        r#"  - name: "serial"
    problem: "sum"
    datasets: ["numbers"]
    implementations: ["reference"]
    execution-policy: "serial"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 2
    seed: 8
execution-policies:
  serial:
    version: 1
    worker-reuse: true
    warmup-runs: 2
    timing-scope: "prepare_and_execute"
    primary-time: "timed_wall_time"
"#,
    );
    fs::write(path, manifest).unwrap();
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(output.status.success(), "{output:?}");
    let records = envelopes(&output);
    let report = records.last().unwrap();
    assert_eq!(report["data"]["summary"]["accepted_attempts"], 5);
    let attempts = attempt_documents(root.path(), report);
    for (index, attempt) in attempts.iter().enumerate() {
        let concurrency = &attempt["provenance"]["data"]["context"]["concurrency"];
        assert_eq!(
            concurrency["max_concurrency"],
            if index < 3 { 2 } else { 1 }
        );
        assert_eq!(concurrency["batch_index"], [0, 0, 1, 0, 1][index]);
        assert_eq!(concurrency["batch_size"], if index < 2 { 2 } else { 1 });
    }
    let events = report["data"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        [
            "run_started",
            "attempt_started",
            "attempt_started",
            "attempt_finished",
            "attempt_finished",
            "attempt_started",
            "attempt_finished",
            "attempt_started",
            "attempt_finished",
            "attempt_started",
            "attempt_finished",
            "run_finished"
        ]
    );
    let statistics = attempts
        .iter()
        .map(|attempt| &attempt["provenance"]["data"]["implementation_statistics"])
        .collect::<Vec<_>>();
    assert_eq!(statistics[0]["pid"], statistics[2]["pid"]);
    assert_ne!(statistics[0]["pid"], statistics[3]["pid"]);
    assert_ne!(statistics[1]["pid"], statistics[3]["pid"]);
    assert_eq!(statistics[3]["pid"], statistics[4]["pid"]);
    assert_eq!(statistics[3]["executions"], 3);
    assert_eq!(statistics[4]["executions"], 4);
}

#[test]
fn reused_workers_warm_once_reset_and_clear_private_caches_between_slots() {
    let root = reuse_repository();
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
    let report = records.last().unwrap();
    assert_eq!(report["data"]["summary"]["accepted_attempts"], 3);
    let attempts = attempt_documents(root.path(), report);
    let pid = &attempts[0]["provenance"]["data"]["implementation_statistics"]["pid"];
    for (index, attempt) in attempts.iter().enumerate() {
        let data = &attempt["provenance"]["data"];
        assert_eq!(&data["implementation_statistics"]["pid"], pid);
        assert_eq!(data["implementation_statistics"]["executions"], index + 3);
        let warmups = data["context"]["worker_reuse"]["warmups_on_this_attempt"]
            .as_array()
            .unwrap();
        assert_eq!(warmups.len(), if index == 0 { 2 } else { 0 });
        for warmup in warmups {
            assert_eq!(warmup["status"], "validated");
        }
        assert_eq!(data["logs"]["implementation"]["complete"], false);
    }
}

#[test]
fn worker_crash_repeats_warmups_before_the_next_measurement() {
    for failure_at in [1, 3] {
        let root = reuse_repository();
        let marker = serde_json::to_string(&root.path().join("crashed").to_str().unwrap()).unwrap();
        replace(
            root.path(),
            "worker.py",
            "            executions += 1",
            &format!(
                r#"            executions += 1
            marker = Path({marker})
            if executions == {failure_at} and not marker.exists():
                marker.write_text("crashed")
                os._exit(9)"#
            ),
        );
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert_eq!(
            output.status.code(),
            Some(5),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records = envelopes(&output);
        let report = records.last().unwrap();
        assert_eq!(report["data"]["summary"]["failed_attempts"], 1);
        assert_eq!(report["data"]["summary"]["accepted_attempts"], 2);
        let attempts = attempt_documents(root.path(), report);
        for (index, attempt) in attempts.iter().enumerate() {
            let data = &attempt["provenance"]["data"];
            let warmups = data["context"]["worker_reuse"]["warmups_on_this_attempt"]
                .as_array()
                .unwrap();
            assert_eq!(
                warmups.len(),
                if index == 0 && failure_at == 1 {
                    1
                } else if index < 2 {
                    2
                } else {
                    0
                }
            );
            if index == 0 && failure_at == 1 {
                assert_eq!(warmups[0]["status"], "failed");
            }
        }
        assert_eq!(
            attempts[1]["provenance"]["data"]["implementation_statistics"]["executions"],
            3
        );
        assert_eq!(
            attempts[2]["provenance"]["data"]["implementation_statistics"]["executions"],
            4
        );
    }
}

#[test]
fn reset_failure_discards_the_worker_and_the_replacement_warms_again() {
    let root = reuse_repository();
    let marker =
        serde_json::to_string(&root.path().join("reset-failed").to_str().unwrap()).unwrap();
    replace(
        root.path(),
        "worker.py",
        "        elif method == \"reset\":",
        &format!(
            r#"        elif method == "reset":
            marker = Path({marker})
            if executions == 3 and not marker.exists():
                marker.write_text("failed")
                response.update(ok=False, error={{"code": "operation_failed", "message": "reset failed", "details": {{}}}})"#
        ),
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(
        output.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = envelopes(&output);
    let report = records.last().unwrap();
    assert_eq!(report["data"]["summary"]["failed_attempts"], 1);
    assert_eq!(report["data"]["summary"]["accepted_attempts"], 2);
    let attempts = attempt_documents(root.path(), report);
    let first = &attempts[0]["provenance"]["data"];
    let last = &attempts[2]["provenance"]["data"];
    assert_ne!(
        first["implementation_statistics"]["pid"],
        last["implementation_statistics"]["pid"]
    );
    assert_eq!(last["implementation_statistics"]["executions"], 3);
    assert_eq!(
        last["context"]["worker_reuse"]["warmups_on_this_attempt"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        attempts[1]["outcome"]["diagnostic"]["code"],
        "implementation_failure"
    );
}

#[test]
fn timeout_and_protocol_failures_replace_workers_and_repeat_warmups() {
    for (fault, code) in [
        ("timeout", "scientific_timeout"),
        ("protocol", "protocol_error"),
    ] {
        let root = reuse_repository();
        let marker = serde_json::to_string(&root.path().join("faulted").to_str().unwrap()).unwrap();
        let action = if fault == "timeout" {
            replace(
                root.path(),
                "metewand.yaml",
                "warmup-runs: 2",
                "warmup-runs: 2\ntimeout: \"3 s\"",
            );
            "import time; time.sleep(5)"
        } else {
            "response[\"id\"] = \"wrong-id\""
        };
        replace(
            root.path(),
            "worker.py",
            "            executions += 1",
            &format!(
                r#"            executions += 1
            marker = Path({marker})
            if executions == 3 and not marker.exists():
                marker.write_text("faulted")
                {action}"#
            ),
        );
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert_eq!(
            output.status.code(),
            Some(5),
            "{fault}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records = envelopes(&output);
        let report = records.last().unwrap();
        assert_eq!(report["data"]["summary"]["failed_attempts"], 1);
        assert_eq!(report["data"]["summary"]["accepted_attempts"], 2);
        let attempts = attempt_documents(root.path(), report);
        assert_eq!(attempts[0]["outcome"]["diagnostic"]["code"], code);
        let replacement = &attempts[1]["provenance"]["data"];
        assert_eq!(replacement["implementation_statistics"]["executions"], 3);
        assert_eq!(
            replacement["context"]["worker_reuse"]["warmups_on_this_attempt"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn every_implementation_repetition_runs_its_own_planned_warmups() {
    let root = reuse_repository();
    replace(
        root.path(),
        "metewand.yaml",
        "implementation-repetitions: 1",
        "implementation-repetitions: 2",
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
    let report = records.last().unwrap();
    assert_eq!(report["data"]["summary"]["accepted_attempts"], 6);
    let attempts = attempt_documents(root.path(), report);
    for (index, attempt) in attempts.iter().enumerate() {
        let data = &attempt["provenance"]["data"];
        assert_eq!(
            data["implementation_statistics"]["executions"],
            index + 3 + if index >= 3 { 2 } else { 0 }
        );
        assert_eq!(
            data["context"]["worker_reuse"]["warmups_on_this_attempt"]
                .as_array()
                .unwrap()
                .len(),
            if index % 3 == 0 { 2 } else { 0 }
        );
    }
    assert_eq!(
        attempts[0]["provenance"]["data"]["implementation_statistics"]["pid"],
        attempts[3]["provenance"]["data"]["implementation_statistics"]["pid"]
    );
}

#[test]
fn best_effort_runs_publish_every_requested_resource_control() {
    let root = repository();
    replace(
        root.path(),
        "metewand.yaml",
        "version: 1\ntiming-scope",
        "version: 1\ncpus: 2\nthreads: 4\nmemory: \"8 GiB\"\nnetwork: true\ntiming-scope",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(output.status.success(), "{output:?}");
    let records = envelopes(&output);
    let attempt = &records.last().unwrap()["data"]["attempts"][0];
    let record = document(
        &root
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap())
            .join("attempt.json"),
    );
    let data = &record["provenance"]["data"];
    let controls = data["context"]["controls"].as_array().unwrap();
    assert_eq!(controls.len(), 9);
    assert_eq!(data["execution"]["controls"], data["context"]["controls"]);
    for (name, value, status) in [
        ("cpus", serde_json::json!(2), "unsupported"),
        ("threads", serde_json::json!(4), "unsupported"),
        ("memory", serde_json::json!("8 GiB"), "unsupported"),
        ("network", serde_json::json!(true), "enforced"),
        ("process_tree", serde_json::json!(true), "best_effort"),
    ] {
        let control = controls
            .iter()
            .find(|control| control["name"] == name)
            .unwrap();
        assert_eq!(control["requested"], value);
        assert_eq!(control["status"], status);
        assert_eq!(control["required"], false);
        assert_eq!(control["scope"], "worker_lifetime");
    }
}

#[test]
fn control_loss_publishes_a_failed_attempt_with_observed_evidence() {
    for phase in ["hello", "prepare", "execute", "evaluate", "shutdown"] {
        let root = repository();
        let branch = if matches!(phase, "hello" | "shutdown") {
            "if"
        } else {
            "elif"
        };
        replace(
            root.path(),
            "worker.py",
            &format!("{branch} method == {phase:?}:"),
            &format!("{branch} method == {phase:?}:\n            os.chmod('.', 0o777)"),
        );
        let output = metewand(
            root.path(),
            &["run", "--output-dir", "run", "--output", "jsonl"],
        );
        assert_eq!(output.status.code(), Some(5), "{phase}: {output:?}");
        let records = envelopes(&output);
        let attempt = &records.last().unwrap()["data"]["attempts"][0];
        assert_eq!(attempt["diagnostic"]["code"], "control_enforcement_lost");
        assert!(attempt["observation_id"].is_null());
        let directory = root
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap());
        verify_finalized_directory(&directory).unwrap();
        let record = document(&directory.join("attempt.json"));
        let context = &record["provenance"]["data"]["context"];
        assert_eq!(
            context["control_failure"]["control"],
            "private_working_directory"
        );
        assert_eq!(
            context["controls"]
                .as_array()
                .unwrap()
                .iter()
                .find(|control| control["name"] == "private_working_directory")
                .unwrap()["status"],
            "unsupported"
        );
        assert!(!root.path().join("run/observations").exists());
    }
}

#[test]
fn reset_control_loss_replaces_the_worker_and_repeats_warmups() {
    let root = reuse_repository();
    replace(
        root.path(),
        "worker.py",
        "elif method == \"reset\":",
        "elif method == \"reset\":\n            if executions == 3:\n                os.chmod('.', 0o777)",
    );
    let output = metewand(
        root.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let records = envelopes(&output);
    let attempts = attempt_documents(root.path(), records.last().unwrap());
    assert_eq!(attempts[0]["outcome"]["kind"], "accepted");
    assert_eq!(
        attempts[1]["outcome"]["diagnostic"]["code"],
        "control_enforcement_lost"
    );
    assert_eq!(attempts[2]["outcome"]["kind"], "accepted");
    assert_eq!(
        attempts[2]["provenance"]["data"]["context"]["worker_reuse"]["warmups_on_this_attempt"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_ne!(
        attempts[0]["provenance"]["data"]["implementation_statistics"]["pid"],
        attempts[2]["provenance"]["data"]["implementation_statistics"]["pid"]
    );
}
