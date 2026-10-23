#![cfg(unix)]

use std::{collections::BTreeMap, fs, path::Path, process::Command};

use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn repository() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset"),
        root.path(),
    );
    root
}

fn command(root: &Path, args: &[&str]) -> (i32, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .args(["--output", "json"])
        .output()
        .unwrap();
    let value: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::MachineOutput.repository_path(), &value)
        .unwrap();
    if output.status.success() && args.first() == Some(&"export") {
        public_schema_catalog()
            .unwrap()
            .validate(PublicSchema::ExportOutput.repository_path(), &value)
            .unwrap();
    }
    (output.status.code().unwrap(), value)
}

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if entry.file_type().unwrap().is_dir() {
            for (path, bytes) in snapshot(&entry.path()) {
                files.insert(format!("{name}/{path}"), bytes);
            }
        } else {
            files.insert(name, fs::read(entry.path()).unwrap());
        }
    }
    files
}

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn exports_verified_history_without_the_repository_and_rebuilds_stale_indexes() {
    let root = repository();
    let (code, run) = command(root.path(), &["run", "--output-dir", "run"]);
    assert_eq!(code, 0, "{run}");
    fs::remove_file(root.path().join("metewand.yaml")).unwrap();
    fs::write(root.path().join("run/index.jsonl"), "stale and invalid").unwrap();
    let before = snapshot(&root.path().join("run"));
    for format in ["jsonl", "csv"] {
        let (code, report) = command(
            root.path(),
            &[
                "export",
                "--output-dir",
                "run",
                "--format",
                format,
                "--destination",
                format,
            ],
        );
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["data"]["records"], 3);
        let filename = format!("results.{format}");
        let bytes = fs::read(root.path().join(format).join(&filename)).unwrap();
        let manifest: Value = serde_json::from_slice(
            &fs::read(root.path().join(format).join("export.json")).unwrap(),
        )
        .unwrap();
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(manifest["files"][&filename], digest);
        assert_eq!(manifest["attempt_ids"].as_array().unwrap().len(), 1);
        assert_eq!(manifest["observation_ids"].as_array().unwrap().len(), 1);
    }
    assert_eq!(snapshot(&root.path().join("run")), before);
    let rows = lines(&root.path().join("jsonl/results.jsonl"));
    let catalog = public_schema_catalog().unwrap();
    for row in &rows {
        catalog
            .validate(PublicSchema::ExportRow.repository_path(), row)
            .unwrap();
    }
    let csv_rows = Command::new("python3").args(["-c", "import csv,json,sys; print(json.dumps(list(csv.DictReader(open(sys.argv[1], newline='')))))"]).arg(root.path().join("csv/results.csv")).output().unwrap();
    assert!(csv_rows.status.success(), "{csv_rows:?}");
    let csv_rows: Vec<Value> = serde_json::from_slice(&csv_rows.stdout).unwrap();
    assert_eq!(csv_rows.len(), rows.len());
    for (json, csv) in rows.iter().zip(&csv_rows) {
        for column in ["id", "candidate_id", "record_type", "compatibility_id"] {
            assert_eq!(json[column], csv[column]);
        }
        for column in [
            "timing",
            "compatibility",
            "provenance",
            "evidence",
            "metric_value",
        ] {
            let cell = csv[column].as_str().unwrap();
            let value: Value = if cell.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(cell).unwrap()
            };
            assert_eq!(json[column], value, "{column}");
        }
    }
    let observation = rows
        .iter()
        .find(|r| r["record_type"] == "observation")
        .unwrap();
    assert_eq!(observation["metric_name"], "/absolute_error");
    assert_eq!(observation["metric_value"], 0);
    assert_eq!(
        observation["compatibility"]["environment_class"],
        "development"
    );
    assert_eq!(
        observation["compatibility"]["timing_scope"],
        "prepare_and_execute"
    );
    assert_eq!(observation["compatibility"]["control_class"], "unsupported");
    assert_eq!(observation["selected"], true);
    for _ in 0..2 {
        let (code, report) = command(
            root.path(),
            &["export", "--output-dir", "run", "--rebuild-index"],
        );
        assert_eq!(code, 0, "{report}");
    }
    let first = fs::read(root.path().join("run/index.jsonl")).unwrap();
    let (code, report) = command(
        root.path(),
        &["export", "--output-dir", "run", "--rebuild-index"],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(
        fs::read(root.path().join("run/index.jsonl")).unwrap(),
        first
    );
    let index = lines(&root.path().join("run/index.jsonl"));
    assert_eq!(index.len(), 3);
    for record in &index {
        catalog
            .validate(PublicSchema::ExportRecord.repository_path(), record)
            .unwrap();
    }
}

#[test]
fn failures_and_invalid_results_remain_visible_beside_successful_retries() {
    let root = repository();
    let worker = root.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap();
    let marker = root.path().join("bad");
    fs::write(&marker, "bad").unwrap();
    fs::write(
        &worker,
        source.replace(
            "\"answer\": prepared",
            &format!(
                "\"answer\": prepared + (1 if Path({:?}).exists() else 0)",
                marker.to_str().unwrap()
            ),
        ),
    )
    .unwrap();
    let (code, failed) = command(root.path(), &["run", "--output-dir", "run"]);
    assert_eq!(code, 5, "{failed}");
    fs::remove_file(marker).unwrap();
    let (code, retried) = command(
        root.path(),
        &["run", "--output-dir", "run", "--resume", "--retry"],
    );
    assert_eq!(code, 0, "{retried}");
    let (code, report) = command(
        root.path(),
        &["export", "--output-dir", "run", "--destination", "table"],
    );
    assert_eq!(code, 0, "{report}");
    let rows = lines(&root.path().join("table/results.jsonl"));
    let attempts = rows
        .iter()
        .filter(|r| r["record_type"] == "attempt")
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().any(|r| r["outcome"] == "invalid"
        && r["attempt_state"] == "failed"
        && r["retry_index"] == 0
        && r["compatibility"]["control_evidence"] == "declared"
        && r["compatibility"]["control_class"] == "unknown"));
    assert!(
        attempts
            .iter()
            .any(|r| r["outcome"] == "accepted" && r["retry_index"] == 1)
    );
    let before = snapshot(root.path());
    for args in [
        vec!["--destination", "table"],
        vec!["--destination", "missing/parent"],
        vec!["--destination", "new", "--id", "unknown"],
    ] {
        let mut command_args = vec!["export", "--output-dir", "run"];
        command_args.extend(args);
        assert_ne!(command(root.path(), &command_args).0, 0);
        assert_eq!(snapshot(root.path()), before);
    }
}

#[test]
fn malformed_requests_and_empty_runs_do_not_initialize_or_recover_sources() {
    let root = tempfile::tempdir().unwrap();
    for (args, expected) in [
        (vec!["export"], 2),
        (vec!["export", "--output-dir", "missing"], 2),
        (
            vec![
                "export",
                "--output-dir",
                "missing",
                "--destination",
                "table",
            ],
            4,
        ),
        (
            vec!["export", "--output-dir", "missing", "--rebuild-index"],
            4,
        ),
        (
            vec![
                "export",
                "--output-dir",
                "missing",
                "--destination",
                "table",
                "--format",
                "parquet",
            ],
            2,
        ),
        (
            vec![
                "export",
                "--output-dir",
                "missing",
                "--rebuild-index",
                "--rebuild-index",
            ],
            2,
        ),
    ] {
        let (code, report) = command(root.path(), &args);
        assert_eq!(code, expected, "{report}");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
    fs::create_dir(root.path().join("run")).unwrap();
    drop(metewand_runtime::run_store::RunStore::open(&root.path().join("run")).unwrap());
    let before = snapshot(&root.path().join("run"));
    for format in ["jsonl", "csv"] {
        let (code, report) = command(
            root.path(),
            &[
                "export",
                "--output-dir",
                "run",
                "--destination",
                format,
                "--format",
                format,
            ],
        );
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["data"]["records"], 0);
        assert_eq!(report["data"]["rows"], 0);
    }
    assert!(
        fs::read(root.path().join("jsonl/results.jsonl"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("csv/results.csv"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(snapshot(&root.path().join("run")), before);
}
