#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

fn r_sdk_example(primary_time: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let description = fs::read_to_string(root.join("packages/r/DESCRIPTION")).unwrap();
    let version = description
        .lines()
        .find_map(|line| line.strip_prefix("Version: "))
        .unwrap();
    let benchmark = TempDir::new().unwrap();
    let package = TempDir::new().unwrap();
    copy_tree(&root.join("examples/r-package"), benchmark.path());
    copy_tree(&root.join("packages/r"), &package.path().join("r"));
    let library = benchmark.path().join("library");
    fs::create_dir(&library).unwrap();
    let output = Command::new("R")
        .args(["CMD", "INSTALL", "--preclean", "--clean"])
        .arg(format!("--library={}", library.display()))
        .arg(package.path().join("r"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    if primary_time == "implementation_time" {
        let path = benchmark.path().join("metewand.yaml");
        let source = fs::read_to_string(&path)
            .unwrap()
            .replace(
                "primary-time: \"timed_wall_time\"",
                "primary-time: implementation_time",
            )
            .replace(
                "timing-scope: \"prepare_and_execute\"",
                "timing-scope: execute_only",
            );
        fs::write(path, source).unwrap();
        let path = benchmark.path().join("problem.yaml");
        let source = fs::read_to_string(&path).unwrap().replace(
            "allowed-timing-scopes: [\"prepare_and_execute\"]",
            "allowed-timing-scopes: [\"prepare_and_execute\", \"execute_only\"]",
        );
        fs::write(path, source).unwrap();
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
    for row in report["data"]["comparison"].as_array().unwrap() {
        assert_eq!(row["primary_time"], primary_time);
        assert_eq!(row["primary_time_status"], "complete");
        assert!(row["primary_time_ns"].is_u64());
    }
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
            assert_eq!(execution[role]["sdk"]["name"], "metewand-r");
            assert_eq!(execution[role]["sdk"]["version"], version);
        }
    }
}

#[test]
fn r_sdk_example_publishes_two_accepted_observations() {
    r_sdk_example("timed_wall_time");
}

#[test]
fn r_sdk_example_can_select_implementation_primary_time() {
    r_sdk_example("implementation_time");
}

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
