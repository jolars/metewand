#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

#[test]
fn callback_tracebacks_survive_failed_attempts_and_remain_explainable() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (trigger, code) in [
        ("self.values = read_json", "implementation_failure"),
        ("self.answer = sum", "implementation_failure"),
        ("values = read_json", "evaluator_failure"),
    ] {
        let benchmark = TempDir::new().unwrap();
        copy_tree(&root.join("examples/python-sdk"), benchmark.path());
        copy_tree(
            &root.join("sdk/python/src/metewand"),
            &benchmark.path().join("metewand"),
        );
        let worker = benchmark.path().join("worker.py");
        let source = fs::read_to_string(&worker).unwrap();
        let source = source
            .lines()
            .map(|line| {
                if line.trim_start().starts_with(trigger) {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    format!("{indent}raise RuntimeError('scientific callback broke')")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&worker, source).unwrap();
        let invoke = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_metewand"))
                .current_dir(benchmark.path())
                .args(args)
                .output()
                .unwrap()
        };
        let output = invoke(&[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "1",
            "--output",
            "json",
        ]);
        assert!(!output.status.success(), "{output:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let attempt = &report["data"]["attempts"][0];
        assert_eq!(attempt["diagnostic"]["code"], code, "{report}");
        let attempt_path = benchmark
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap())
            .join("attempt.json");
        let persisted: Value = serde_json::from_slice(&fs::read(attempt_path).unwrap()).unwrap();
        let message = persisted["outcome"]["diagnostic"]["message"]
            .as_str()
            .unwrap();
        assert!(message.contains("Traceback"), "{message}");
        assert!(
            message.contains("RuntimeError: scientific callback broke"),
            "{message}"
        );
        assert!(message.contains("worker.py"), "{message}");
        fs::remove_file(worker).unwrap();
        let status = invoke(&["status", "--output-dir", "run", "--output", "json"]);
        assert!(status.status.success(), "{status:?}");
        let status: Value = serde_json::from_slice(&status.stdout).unwrap();
        let id = status["data"]["attempts"][0]["diagnostic_id"]
            .as_str()
            .unwrap();
        for format in ["json", "human"] {
            let explanation = invoke(&["explain", id, "--output-dir", "run", "--output", format]);
            assert!(explanation.status.success(), "{explanation:?}");
            let text = String::from_utf8(explanation.stdout).unwrap();
            assert!(
                text.contains("RuntimeError: scientific callback broke"),
                "{text}"
            );
            assert!(!text.contains("see the worker log"), "{text}");
        }
    }
}

#[test]
fn declared_completion_tolerance_controls_independent_evaluation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    copy_tree(&root.join("examples/python-sdk"), benchmark.path());
    copy_tree(
        &root.join("sdk/python/src/metewand"),
        &benchmark.path().join("metewand"),
    );
    let worker = benchmark.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap();
    fs::write(
        &worker,
        source.replace(
            "self.answer = sum(self.values) + self.offset",
            "self.answer = sum(self.values) + self.offset + 1",
        ),
    )
    .unwrap();
    let manifest = benchmark.path().join("metewand.yaml");
    let original = fs::read_to_string(&manifest).unwrap();
    for (tolerance, status) in [(0, "failed"), (1, "accepted")] {
        fs::write(
            &manifest,
            original.replace(
                "one_shot_completion: { absolute_error: 0 }",
                &format!("one_shot_completion: {{ absolute_error: {tolerance} }}"),
            ),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args([
                "run",
                "--output-dir",
                &format!("run-{tolerance}"),
                "--output",
                "json",
            ])
            .output()
            .unwrap();
        let report: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
        assert_eq!(output.status.success(), tolerance == 1, "{report}");
        for attempt in report["data"]["attempts"].as_array().unwrap() {
            assert_eq!(attempt["status"], status, "{attempt}");
            assert_eq!(attempt["metrics"], serde_json::json!({"absolute_error": 1}));
        }
    }
}

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
            .args(&command)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        if command[0] == "plan" {
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            let candidates = report["data"]["experiments"][0]["candidates"]
                .as_array()
                .unwrap();
            assert_eq!(candidates.len(), 1);
            assert_eq!(
                candidates[0]["parameters"],
                serde_json::json!({
                    "dataset": {"values": [1, 2, 3]}, "problem": {"offset": 2}, "implementation": {}
                })
            );
            assert_eq!(candidates[0]["specifications"].as_array().unwrap().len(), 1);
            assert_eq!(
                candidates[0]["specifications"][0]["attempt_slots"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
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
