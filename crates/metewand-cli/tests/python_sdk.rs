#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

#[test]
fn python_sdk_example_publishes_two_accepted_observations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    copy_tree(&root.join("examples/python-sdk"), benchmark.path());
    copy_tree(
        &root.join("sdk/python/src/metewand"),
        &benchmark.path().join("metewand"),
    );
    assert!(!benchmark.path().join("schemas").exists());
    assert!(!benchmark.path().join("problem.yaml").exists());
    for command in [
        vec!["check", "--workers", "--output", "json"],
        vec!["plan", "--output", "json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args(command)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(benchmark.path())
        .args(["run", "--output-dir", "run", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempts = report["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    for attempt in attempts {
        assert_eq!(attempt["status"], "accepted");
        assert_eq!(attempt["result"], serde_json::json!({"answer": 8}));
        assert_eq!(attempt["metrics"], serde_json::json!({"absolute_error": 0}));
        let path = benchmark
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap());
        let record: Value =
            serde_json::from_slice(&fs::read(path.join("attempt.json")).unwrap()).unwrap();
        let execution = &record["provenance"]["data"]["execution"];
        for role in ["implementation", "evaluator"] {
            assert_eq!(execution[role]["sdk"]["name"], "metewand-python");
            assert_eq!(execution[role]["sdk"]["version"], "0.1.0");
        }
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "__pycache__" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
