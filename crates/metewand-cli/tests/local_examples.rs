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

#[test]
fn compact_implementation_parameters_preserve_plans_and_apply_defaults() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    copy_tree(&root.join("examples/parameter-matrix"), benchmark.path());
    let manifest_path = benchmark.path().join("metewand.yaml");
    let compact = fs::read_to_string(&manifest_path).unwrap();
    let invoke = |command: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args([command, "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let compact_plan = invoke("plan");
    let explicit = compact.replace(
        "parameters:\n      representation:\n        type: string\n        enum: [list, packed]\n        default: list",
        "parameter-schema:\n      type: object\n      properties:\n        representation: {type: string, enum: [list, packed]}\n      required: [representation]\n      additionalProperties: false\n    parameter-defaults: {representation: list}",
    );
    assert_ne!(compact, explicit);
    fs::write(&manifest_path, explicit).unwrap();
    assert_eq!(compact_plan, invoke("plan"));

    let default_only = compact.replace(
        "    implementation-parameters:\n      reference:\n        representation: { grid: [\"list\", \"packed\"] }\n",
        "",
    );
    assert_ne!(compact, default_only);
    fs::write(&manifest_path, default_only).unwrap();
    invoke("check");
    let default_plan = invoke("plan");
    for candidate in default_plan["data"]["experiments"][0]["candidates"]
        .as_array()
        .unwrap()
    {
        assert_eq!(
            candidate["parameters"]["implementation"],
            serde_json::json!({"representation": "list"})
        );
    }
    let human = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(benchmark.path())
        .arg("plan")
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    assert!(
        String::from_utf8_lossy(&human.stdout)
            .contains("implementation parameters: {\"representation\":\"list\"}")
    );
    let run = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(benchmark.path())
        .args(["run", "--output-dir", "default-run", "--output", "json"])
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");
    let report: Value = serde_json::from_slice(&run.stdout).unwrap();
    let attempts = report["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 5);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt["status"] == "accepted")
    );
    fs::write(
        &manifest_path,
        compact.replace(
            "representation: { grid: [\"list\", \"packed\"] }",
            "representation: {value: unknown}",
        ),
    )
    .unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(benchmark.path())
        .arg("check")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    let message = String::from_utf8_lossy(&invalid.stderr);
    assert!(message.contains("/representation"), "{message}");
    assert!(!message.contains(".metewand-inline-"), "{message}");
}

#[test]
fn motivating_workflows_execute_one_shot_scientific_cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let science = Command::new("python3")
        .args(["-B", "-m", "unittest", "discover", "-s"])
        .arg(root.join("examples/workflow-smoke"))
        .args(["-p", "test_science.py"])
        .output()
        .unwrap();
    assert!(science.status.success(), "{science:?}");
    let attempts = run_example("workflow-smoke");
    assert_eq!(attempts.len(), 3);
    let expected = [
        (vec![1.14, -0.7 + 1.92 / 18.0, 0.16], 1.3),
        (vec![1.65, 1.65, 0.25], 0.0),
        (vec![-1.0, 0.5, 1.0], 0.0),
    ];
    for (attempt, (coefficients, intercept)) in attempts.iter().zip(expected) {
        let actual = attempt["result"]["coefficients"].as_array().unwrap();
        assert_eq!(actual.len(), coefficients.len());
        for (actual, expected) in actual.iter().zip(coefficients) {
            assert!((actual.as_f64().unwrap() - expected).abs() < 1e-8);
        }
        assert!((attempt["result"]["intercept"].as_f64().unwrap() - intercept).abs() < 1e-8);
    }
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
                if name == "workflow-smoke" {
                    for metric in ["objective_error", "coefficient_error", "intercept_error"] {
                        assert!(attempt["metrics"][metric].as_f64().unwrap() <= 1e-8);
                    }
                    for field in ["attempt_directory", "observation_directory"] {
                        let directory = benchmark
                            .path()
                            .join("run")
                            .join(attempt[field].as_str().unwrap());
                        assert!(directory.join("manifest.json").is_file());
                    }
                    let record: Value = serde_json::from_slice(
                        &fs::read(
                            benchmark
                                .path()
                                .join("run")
                                .join(attempt["attempt_directory"].as_str().unwrap())
                                .join("attempt.json"),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        record["provenance"]["data"]["execution"]["timing_scope"],
                        "prepare_and_execute"
                    );
                } else {
                    assert_eq!(attempt["metrics"], serde_json::json!({"absolute_error": 0}));
                }
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
