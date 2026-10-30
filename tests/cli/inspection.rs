#![cfg(unix)]

use std::{collections::BTreeMap, fs, path::Path, process::Command};

use crate::core::public_schemas::{PublicSchema, public_schema_catalog};
use serde_json::Value;
use tempfile::TempDir;

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = target.join(entry.file_name());
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
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    let path = root.path().join("metewand.yaml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        manifest.replace("measurement-repetitions: 1", "measurement-repetitions: 3"),
    )
    .unwrap();
    root
}

fn command(root: &Path, arguments: &[&str]) -> (i32, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(arguments)
        .args(["--output", "json"])
        .output()
        .unwrap();
    let value: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    let catalog = public_schema_catalog().unwrap();
    catalog
        .validate(PublicSchema::MachineOutput.repository_path(), &value)
        .unwrap();
    if output.status.success() && matches!(arguments.first(), Some(&"status" | &"explain")) {
        catalog
            .validate(PublicSchema::InspectionOutput.repository_path(), &value)
            .unwrap();
    }
    (output.status.code().unwrap(), value)
}

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if entry.file_type().unwrap().is_dir() {
            for (path, bytes) in snapshot(&entry.path()) {
                result.insert(format!("{name}/{path}"), bytes);
            }
        } else {
            result.insert(name, fs::read(entry.path()).unwrap());
        }
    }
    result
}

#[test]
fn status_reports_pending_slots_and_explains_every_planned_identity_without_writes() {
    let root = repository();
    let before = snapshot(root.path());
    let (code, status) = command(root.path(), &["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["data"]["summary"]["pending_slots"], 3);
    let (_, plan) = command(root.path(), &["plan"]);
    fn ids(value: &Value, output: &mut Vec<String>) {
        match value {
            Value::String(id) if id.starts_with("mw1-") => output.push(id.clone()),
            Value::Array(values) => values.iter().for_each(|value| ids(value, output)),
            Value::Object(values) => values.values().for_each(|value| ids(value, output)),
            _ => (),
        }
    }
    let mut identities = Vec::new();
    ids(&plan["data"], &mut identities);
    identities.sort();
    identities.dedup();
    for id in identities {
        let (code, explanation) = command(root.path(), &["explain", &id]);
        assert_eq!(code, 0, "{id}: {explanation}");
        assert_eq!(explanation["data"]["subject"]["id"], id);
        if id.starts_with("mw1-source-bundle-") {
            assert!(
                !explanation["data"]["subject"]["record"]["value"]["entries"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn status_preserves_history_and_explains_committed_artifacts_without_the_repository() {
    let root = repository();
    let (code, report) = command(
        root.path(),
        &["run", "--output-dir", "run", "--max-attempts", "1"],
    );
    assert_eq!(code, 0, "{report}");
    let before = snapshot(&root.path().join("run"));
    let (code, status) = command(
        root.path(),
        &[
            "status",
            "--output-dir",
            "run",
            "--manifest",
            "metewand.yaml",
        ],
    );
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["data"]["summary"]["satisfied_slots"], 1);
    assert_eq!(status["data"]["summary"]["pending_slots"], 2);
    fs::remove_file(root.path().join("metewand.yaml")).unwrap();
    let attempt = &report["data"]["attempts"][0];
    for id in [
        attempt["attempt_id"].as_str().unwrap(),
        attempt["resolved_slot_id"].as_str().unwrap(),
        attempt["observation_id"].as_str().unwrap(),
    ] {
        let (code, explanation) = command(root.path(), &["explain", id, "--output-dir", "run"]);
        assert_eq!(code, 0, "{explanation}");
        assert_eq!(explanation["data"]["subject"]["id"], id);
    }
    let (code, status) = command(root.path(), &["status", "--output-dir", "run"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["data"]["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot(&root.path().join("run")), before);
}

#[test]
fn invalid_inspection_requests_fail_without_initializing_a_store() {
    let root = repository();
    let before = snapshot(root.path());
    for (arguments, expected) in [
        (vec!["explain"], 2),
        (vec!["explain", "bad-id"], 2),
        (
            vec![
                "explain",
                "mw1-attempt-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ],
            4,
        ),
        (vec!["status", "--output-dir", "missing"], 4),
        (vec!["status", "--output-dir", "."], 4),
    ] {
        let (code, value) = command(root.path(), &arguments);
        assert_eq!(code, expected, "{arguments:?}: {value}");
        assert_eq!(value["kind"], "diagnostic");
    }
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn failures_and_retries_keep_distinct_attempts_and_explainable_diagnostics() {
    let root = repository();
    let worker = root.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap();
    let marker = root.path().join("fail");
    fs::write(&marker, "fail").unwrap();
    fs::write(&worker, source.replace("elif method == \"execute\":", &format!("elif method == \"execute\":\n            if Path({:?}).exists():\n                os._exit(9)", marker.to_str().unwrap()))).unwrap();
    let (code, first) = command(
        root.path(),
        &["run", "--output-dir", "run", "--max-attempts", "1"],
    );
    assert_eq!(code, 5, "{first}");
    let (code, status) = command(root.path(), &["status", "--output-dir", "run"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["data"]["summary"]["failed_attempts"], 1);
    assert_eq!(status["data"]["summary"]["retry_required_slots"], 1);
    assert_eq!(status["data"]["summary"]["pending_slots"], 2);
    let diagnostic = status["data"]["attempts"][0]["diagnostic_id"]
        .as_str()
        .unwrap();
    assert!(diagnostic.starts_with("mw1-diagnostic-"));
    let (_, explanation) = command(root.path(), &["explain", diagnostic, "--output-dir", "run"]);
    assert_eq!(
        explanation["data"]["subject"]["record"]["code"],
        "implementation_crash"
    );
    let failed_id = first["data"]["attempts"][0]["attempt_id"].as_str().unwrap();
    assert!(
        explanation["data"]["subject"]["related_ids"]
            .as_array()
            .unwrap()
            .contains(&Value::from(failed_id))
    );
    fs::remove_file(marker).unwrap();
    let (code, retry) = command(
        root.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--resume",
            "--retry",
            "--select",
            failed_id,
        ],
    );
    assert_eq!(code, 0, "{retry}");
    let (_, status) = command(root.path(), &["status", "--output-dir", "run"]);
    assert_eq!(status["data"]["summary"]["failed_attempts"], 1);
    assert_eq!(status["data"]["summary"]["accepted_attempts"], 1);
    assert_eq!(status["data"]["summary"]["satisfied_slots"], 1);
    assert_eq!(status["data"]["summary"]["retry_required_slots"], 0);
    assert_eq!(status["data"]["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(status["data"]["attempts"][1]["retry_index"], 1);
}

#[test]
fn artifact_ids_are_exact_and_corruption_is_reported_without_quarantine() {
    let root = repository();
    let (code, report) = command(
        root.path(),
        &["run", "--output-dir", "run", "--max-attempts", "1"],
    );
    assert_eq!(code, 0, "{report}");
    let observation_path = report["data"]["attempts"][0]["observation_directory"]
        .as_str()
        .unwrap();
    let observation: Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("run")
                .join(observation_path)
                .join("observation.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for role in ["result", "metrics"] {
        let id = observation[role]["id"].as_str().unwrap();
        let (code, explanation) = command(root.path(), &["explain", id, "--output-dir", "run"]);
        assert_eq!(code, 0, "{explanation}");
        assert_eq!(explanation["data"]["subject"]["kind"], "artifact");
        assert_eq!(
            explanation["data"]["subject"]["record"]["document"]["schema_version"],
            1
        );
        assert!(
            !explanation["data"]["subject"]["paths"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    let metrics = root
        .path()
        .join("run")
        .join(observation_path)
        .join("metrics.json");
    fs::write(metrics, "corrupted").unwrap();
    let before = snapshot(&root.path().join("run"));
    let (code, error) = command(root.path(), &["status", "--output-dir", "run"]);
    assert_eq!(code, 3, "{error}");
    assert_eq!(error["data"]["code"], "invalid_inspection_evidence");
    assert_eq!(snapshot(&root.path().join("run")), before);
}

#[test]
fn changed_plans_retain_history_and_mark_old_slots_explicitly() {
    let root = repository();
    let (code, run) = command(
        root.path(),
        &["run", "--output-dir", "run", "--max-attempts", "1"],
    );
    assert_eq!(code, 0, "{run}");
    let manifest = root.path().join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("seed: 7", "seed: 8"),
    )
    .unwrap();
    let (_, status) = command(
        root.path(),
        &[
            "status",
            "--output-dir",
            "run",
            "--manifest",
            "metewand.yaml",
        ],
    );
    let slots = status["data"]["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 6);
    assert_eq!(
        slots
            .iter()
            .filter(|slot| slot["in_current_plan"] == true)
            .count(),
        3
    );
    assert_eq!(
        slots
            .iter()
            .filter(|slot| slot["in_current_plan"] == false)
            .count(),
        3
    );
    assert_eq!(status["data"]["summary"]["accepted_attempts"], 1);
    assert_eq!(status["data"]["summary"]["pending_slots"], 5);
    let id = run["data"]["attempts"][0]["attempt_id"].as_str().unwrap();
    let (code, explanation) = command(
        root.path(),
        &[
            "explain",
            id,
            "--output-dir",
            "run",
            "--manifest",
            "metewand.yaml",
        ],
    );
    assert_eq!(code, 0, "{explanation}");
    let resolved = explanation["data"]["subject"]["record"]["document"]["provenance"]["data"]["context"]["resolved_attempt_slot"]["specification"].as_str().unwrap();
    let (_, run_explanation) = command(root.path(), &["explain", resolved, "--output-dir", "run"]);
    assert_eq!(
        run_explanation["data"]["subject"]["kind"],
        "resolved-specification"
    );
    assert!(run_explanation["data"]["subject"]["record"]["definition"].is_object());
}

#[test]
fn json_lines_and_human_output_render_the_same_status_and_explanation() {
    let root = repository();
    let (_, expected) = command(root.path(), &["status"]);
    let id = expected["data"]["slots"][0]["logical_slot_id"]
        .as_str()
        .unwrap();
    for arguments in [vec!["status"], vec!["explain", id]] {
        let (_, expected) = command(root.path(), &arguments);
        let jsonl = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(root.path())
            .args(&arguments)
            .args(["--output", "jsonl"])
            .output()
            .unwrap();
        assert!(jsonl.status.success(), "{jsonl:?}");
        assert!(jsonl.stderr.is_empty());
        assert_eq!(
            jsonl.stdout.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&jsonl.stdout).unwrap(),
            expected
        );
        let human = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(root.path())
            .args(&arguments)
            .output()
            .unwrap();
        assert!(human.status.success(), "{human:?}");
        assert!(human.stderr.is_empty());
        assert!(String::from_utf8(human.stdout).unwrap().contains(id));
    }
}
