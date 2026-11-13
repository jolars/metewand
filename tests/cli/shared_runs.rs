#![cfg(any(unix, windows))]

use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn copy(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn success(root: &Path, scratch: &Path, args: &[&str]) -> Value {
    let output = invoke(root, scratch, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
}

fn invoke(root: &Path, scratch: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch)
        .args(args)
        .args(["--output", "json"])
        .output()
        .unwrap()
}

#[test]
fn shared_run_resumes_and_exports_after_all_local_state_is_lost() {
    let root = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    let manifest = root.path().join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("measurement-repetitions: 1", "measurement-repetitions: 3")
            .replace("timing-scope:", "max-concurrency: 2\n    timing-scope:"),
    )
    .unwrap();
    let local = success(
        root.path(),
        scratch.path(),
        &["run", "--output-dir", "local", "--max-attempts", "1"],
    );
    let shared = success(
        root.path(),
        scratch.path(),
        &[
            "run",
            "--shared",
            "--output-dir",
            "run",
            "--max-attempts",
            "1",
        ],
    );
    for key in ["logical_slot_id", "resolved_slot_id", "attempt_id"] {
        assert_eq!(
            local["attempts"][0][key], shared["attempts"][0][key],
            "{key}"
        );
    }
    let index = invoke(
        root.path(),
        scratch.path(),
        &[
            "export",
            "--output-dir",
            "run",
            "--rebuild-index",
            "--destination",
            "refused",
        ],
    );
    assert_eq!(index.status.code(), Some(2));
    assert!(!root.path().join("refused").exists());
    assert!(!root.path().join("run/index.jsonl").exists());
    assert!(!root.path().join("run/run.sqlite3").exists());
    let before = success(
        root.path(),
        scratch.path(),
        &["status", "--output-dir", "run"],
    );
    assert!(before["summary"]["pending_slots"].as_u64().unwrap() > 0);
    scratch.close().unwrap();
    let other_host = tempfile::tempdir().unwrap();
    let after = success(
        root.path(),
        other_host.path(),
        &["status", "--output-dir", "run"],
    );
    for key in ["summary", "slots", "attempts", "observations", "comparison"] {
        assert_eq!(before[key], after[key], "{key}");
    }
    let resumed = success(
        root.path(),
        other_host.path(),
        &["run", "--output-dir", "run"],
    );
    assert_eq!(resumed["attempts"].as_array().unwrap().len(), 2);
    let finished = success(
        root.path(),
        other_host.path(),
        &["status", "--output-dir", "run"],
    );
    assert_eq!(finished["summary"]["pending_slots"], 0);
    assert_eq!(finished["attempts"].as_array().unwrap().len(), 3);
    success(
        root.path(),
        other_host.path(),
        &["export", "--output-dir", "run", "--destination", "export"],
    );
    success(
        root.path(),
        other_host.path(),
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "run.tar.gz",
        ],
    );
    success(
        root.path(),
        other_host.path(),
        &["archive", "verify", "--archive", "run.tar.gz"],
    );
    let reused = success(
        root.path(),
        other_host.path(),
        &["run", "--output-dir", "run"],
    );
    assert!(reused["attempts"].as_array().unwrap().is_empty());
}

fn validate_journal(root: &Path) {
    let catalog = crate::core::public_schemas::public_schema_catalog().unwrap();
    for entry in fs::read_dir(root.join("run/journal")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().to_str().unwrap().starts_with('.') {
            continue;
        }
        let record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        catalog
            .validate(
                crate::core::public_schemas::PublicSchema::DurableRecord.repository_path(),
                &record,
            )
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
}

#[test]
fn shared_profiles_preserve_observations_selections_and_budgets_across_invocations() {
    for example in ["fresh-sequence", "buffered-profile", "streaming-profile"] {
        let root = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join(example),
            root.path(),
        );
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/python/src/metewand"),
            &root.path().join("metewand"),
        );
        if example == "fresh-sequence" {
            copy(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/r"),
                &root.path().join("library/metewand"),
            );
        }
        success(
            root.path(),
            scratch.path(),
            &[
                "run",
                "--shared",
                "--output-dir",
                "run",
                "--experiment",
                "fixed",
                "--max-attempts",
                "1",
            ],
        );
        let before = success(
            root.path(),
            scratch.path(),
            &["status", "--output-dir", "run"],
        );
        assert!(
            !before["observations"].as_array().unwrap().is_empty(),
            "{example}: {before}"
        );
        scratch.close().unwrap();
        let other_host = tempfile::tempdir().unwrap();
        let after = success(
            root.path(),
            other_host.path(),
            &["status", "--output-dir", "run"],
        );
        for key in [
            "summary",
            "slots",
            "attempts",
            "observations",
            "fresh_sequences",
        ] {
            assert_eq!(before[key], after[key], "{example}: {key}");
        }
        success(
            root.path(),
            other_host.path(),
            &["run", "--output-dir", "run", "--experiment", "fixed"],
        );
        let complete = success(
            root.path(),
            other_host.path(),
            &["status", "--output-dir", "run"],
        );
        assert_eq!(
            complete["summary"]["pending_slots"], 0,
            "{example}: {complete}"
        );
        let reused = success(
            root.path(),
            other_host.path(),
            &["run", "--output-dir", "run", "--experiment", "fixed"],
        );
        assert!(reused["attempts"].as_array().unwrap().is_empty());
        success(
            root.path(),
            other_host.path(),
            &[
                "archive",
                "create",
                "--output-dir",
                "run",
                "--destination",
                "saved.tar.gz",
            ],
        );
        success(
            root.path(),
            other_host.path(),
            &["archive", "verify", "--archive", "saved.tar.gz"],
        );
        validate_journal(root.path());
    }
}

#[test]
fn shared_failures_require_explicit_retry_after_scratch_loss() {
    let root = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    let worker = root.path().join("worker.py");
    let marker = root.path().join("fail");
    fs::write(&marker, b"fail").unwrap();
    let source = fs::read_to_string(&worker).unwrap().replace("elif method == \"execute\":", &format!("elif method == \"execute\":\n            if Path({}).exists():\n                os._exit(9)", serde_json::to_string(&marker).unwrap()));
    fs::write(worker, source).unwrap();
    let failed = invoke(
        root.path(),
        scratch.path(),
        &["run", "--shared", "--output-dir", "run"],
    );
    assert_eq!(failed.status.code(), Some(5), "{failed:?}");
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap();
    let attempt = &failed["data"]["attempts"][0];
    let original = fs::read(
        root.path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap())
            .join("attempt.json"),
    )
    .unwrap();
    scratch.close().unwrap();
    fs::remove_file(marker).unwrap();
    let other_host = tempfile::tempdir().unwrap();
    let skipped = invoke(
        root.path(),
        other_host.path(),
        &["run", "--output-dir", "run"],
    );
    let skipped: Value = serde_json::from_slice(&skipped.stdout).unwrap();
    assert!(skipped["data"]["attempts"].as_array().unwrap().is_empty());
    let retried = success(
        root.path(),
        other_host.path(),
        &["run", "--output-dir", "run", "--retry"],
    );
    assert_ne!(attempt["attempt_id"], retried["attempts"][0]["attempt_id"]);
    let status = success(
        root.path(),
        other_host.path(),
        &["status", "--output-dir", "run"],
    );
    assert_eq!(status["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(
        original,
        fs::read(
            root.path()
                .join("run")
                .join(attempt["attempt_directory"].as_str().unwrap())
                .join("attempt.json")
        )
        .unwrap()
    );
    let journal: String = fs::read_dir(root.path().join("run/journal"))
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert!(journal.contains("explicit_retry"));
    validate_journal(root.path());
}
