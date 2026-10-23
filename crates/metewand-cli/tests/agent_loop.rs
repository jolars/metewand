#![cfg(unix)]

use std::{collections::BTreeSet, fs, path::Path, process::Command};

use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
use metewand_runtime::publication::verify_finalized_directory;
use serde_json::Value;

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn command(root: &Path, args: &[&str], expected_code: i32) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .args(["--output", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(expected_code),
        "{args:?}: {output:?}"
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let catalog = public_schema_catalog().unwrap();
    catalog
        .validate(PublicSchema::MachineOutput.repository_path(), &document)
        .unwrap();
    let schema = match args[0] {
        "run" => Some(PublicSchema::RunOutput),
        "status" | "explain" => Some(PublicSchema::InspectionOutput),
        "export" => Some(PublicSchema::ExportOutput),
        _ => None,
    };
    if let Some(schema) = schema {
        catalog
            .validate(schema.repository_path(), &document)
            .unwrap();
    }
    document
}

#[test]
#[ignore = "Gate 2 acceptance campaign; run explicitly as documented in README.md."]
fn complete_agent_loop_preserves_identities_and_history() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset"),
        root,
    );
    let manifest_path = root.join("metewand.yaml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("measurement-repetitions: 1", "measurement-repetitions: 3");
    fs::write(&manifest_path, &manifest).unwrap();
    let worker_path = root.join("worker.py");
    let marker = root.join("fail");
    // Keep the worker source fixed so repairing the injected failure preserves identities.
    let worker = fs::read_to_string(&worker_path).unwrap().replace(
        "elif method == \"execute\":",
        &format!(
            "elif method == \"execute\":\n            if Path({:?}).exists():\n                os._exit(9)",
            marker.to_str().unwrap()
        ),
    );
    fs::write(&worker_path, &worker).unwrap();

    command(root, &["check"], 0);
    command(root, &["check", "--workers"], 0);
    let plan = command(root, &["plan"], 0);
    assert!(!root.join("run").exists());
    let slots =
        plan["data"]["experiments"][0]["candidates"][0]["specifications"][0]["attempt_slots"]
            .as_array()
            .unwrap();
    assert_eq!(slots.len(), 3);
    let selected = slots[1].as_str().unwrap();
    fs::write(&marker, "fail").unwrap();
    let failed = command(
        root,
        &[
            "run",
            "--output-dir",
            "run",
            "--select",
            selected,
            "--max-attempts",
            "1",
        ],
        5,
    );
    let failed_attempts = failed["data"]["attempts"].as_array().unwrap();
    assert_eq!(failed_attempts.len(), 1);
    let failed_attempt = &failed_attempts[0];
    assert_eq!(failed_attempt["logical_slot_id"], selected);
    let failed_id = failed_attempt["attempt_id"].as_str().unwrap();
    let failed_directory = root
        .join("run")
        .join(failed_attempt["attempt_directory"].as_str().unwrap());
    let original_failure = fs::read(failed_directory.join("attempt.json")).unwrap();
    let status = command(
        root,
        &[
            "status",
            "--output-dir",
            "run",
            "--manifest",
            "metewand.yaml",
        ],
        0,
    );
    assert_eq!(status["data"]["summary"]["retry_required_slots"], 1);
    assert_eq!(status["data"]["summary"]["pending_slots"], 2);
    let diagnostic = status["data"]["attempts"][0]["diagnostic_id"]
        .as_str()
        .unwrap();
    let explanation = command(root, &["explain", diagnostic, "--output-dir", "run"], 0);
    assert_eq!(
        explanation["data"]["subject"]["record"]["code"],
        "implementation_crash"
    );
    assert!(
        explanation["data"]["subject"]["related_ids"]
            .as_array()
            .unwrap()
            .contains(&Value::from(failed_id))
    );

    fs::remove_file(marker).unwrap();
    assert_eq!(command(root, &["plan"], 0), plan);
    let retried = command(
        root,
        &[
            "run",
            "--output-dir",
            "run",
            "--resume",
            "--retry",
            "--select",
            failed_id,
        ],
        0,
    );
    let attempts = retried["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_ne!(attempts[0]["attempt_id"], failed_id);
    assert_eq!(attempts[0]["logical_slot_id"], selected);
    assert_eq!(
        attempts[0]["resolved_slot_id"],
        failed_attempt["resolved_slot_id"]
    );
    assert_eq!(attempts[0]["status"], "accepted");
    let resumed = command(root, &["run", "--output-dir", "run", "--resume"], 0);
    assert_eq!(resumed["data"]["attempts"].as_array().unwrap().len(), 2);
    let no_op = command(root, &["run", "--output-dir", "run", "--resume"], 0);
    assert_eq!(no_op["data"]["attempts"], serde_json::json!([]));
    let status = command(
        root,
        &[
            "status",
            "--output-dir",
            "run",
            "--manifest",
            "metewand.yaml",
        ],
        0,
    );
    assert_eq!(status["data"]["summary"]["satisfied_slots"], 3);
    assert_eq!(status["data"]["summary"]["failed_attempts"], 1);
    assert_eq!(status["data"]["summary"]["accepted_attempts"], 3);
    assert_eq!(status["data"]["summary"]["retry_required_slots"], 0);
    assert_eq!(
        fs::read(failed_directory.join("attempt.json")).unwrap(),
        original_failure
    );
    verify_finalized_directory(&failed_directory).unwrap();

    command(
        root,
        &["export", "--output-dir", "run", "--destination", "table"],
        0,
    );
    let catalog = public_schema_catalog().unwrap();
    let rows: Vec<Value> = fs::read_to_string(root.join("table/results.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            let row = serde_json::from_str(line).unwrap();
            catalog
                .validate(PublicSchema::ExportRow.repository_path(), &row)
                .unwrap();
            row
        })
        .collect();
    let exported = rows
        .iter()
        .filter(|row| row["record_type"] == "attempt")
        .collect::<Vec<_>>();
    assert_eq!(exported.len(), 4);
    assert!(
        exported
            .iter()
            .any(|row| row["attempt_id"] == failed_id && row["attempt_state"] == "failed")
    );
    let expected_ids: BTreeSet<_> = status["data"]["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|attempt| attempt["id"].as_str().unwrap())
        .collect();
    let exported_ids: BTreeSet<_> = exported
        .iter()
        .map(|row| row["attempt_id"].as_str().unwrap())
        .collect();
    assert_eq!(exported_ids, expected_ids);
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), manifest);
    assert_eq!(fs::read_to_string(worker_path).unwrap(), worker);
}
