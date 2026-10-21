#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

#[test]
fn dataset_free_example_evaluates_each_problem_configuration() {
    let attempts = run_example("dataset-free");
    let mut answers: Vec<_> = attempts
        .iter()
        .map(|a| a["result"]["answer"].as_i64().unwrap())
        .collect();
    answers.sort();
    assert_eq!(answers, [6, 55]);
}

#[test]
fn parameter_matrix_example_executes_grids_and_coupled_cases() {
    let attempts = run_example("parameter-matrix");
    let mut answers: Vec<_> = attempts
        .iter()
        .map(|a| a["result"]["answer"].as_i64().unwrap())
        .collect();
    answers.sort();
    assert_eq!(answers, [6, 6, 8, 8, 10, 10, 12, 12, 21, 21]);
}

fn run_example(name: &str) -> Vec<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    copy_tree(&root.join("examples").join(name), benchmark.path());
    for args in [
        vec!["check", "--workers", "--output", "json"],
        vec!["run", "--output-dir", "run", "--output", "json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args(&args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{name} {args:?}: {output:?}");
        if args[0] == "run" {
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            let attempts = report["data"]["attempts"].as_array().unwrap().clone();
            for attempt in &attempts {
                assert_eq!(attempt["status"], "accepted");
                assert_eq!(attempt["metrics"], serde_json::json!({"absolute_error": 0}));
            }
            return attempts;
        }
    }
    unreachable!()
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
