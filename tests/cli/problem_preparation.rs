use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

fn repository() -> TempDir {
    let root = TempDir::new().unwrap();
    let venv = root.path().join("venv");
    let output = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args(["-m", "venv", "--without-pip"])
        .arg(&venv)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let python = venv.join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python3"
    });
    let output = Command::new(python)
        .args([
            "-c",
            "import sysconfig; print(sysconfig.get_path('purelib'))",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let package =
        std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).join("metewand");
    fs::create_dir_all(&package).unwrap();
    for file in ["__init__.py", "__main__.py"] {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("packages/python/src/metewand")
                .join(file),
            package.join(file),
        )
        .unwrap();
    }
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/problem-preparation");
    for file in ["metewand.yaml", "functions.py", "preparation.py"] {
        fs::copy(example.join(file), root.path().join(file)).unwrap();
    }
    let trace = serde_json::to_string(&root.path().join("trace.jsonl")).unwrap();
    for (file, callbacks) in [
        (
            "functions.py",
            vec!["generate", "solve", "solve_loop", "evaluate"],
        ),
        ("preparation.py", vec!["prepare_path"]),
    ] {
        let path = root.path().join(file);
        let mut source = fs::read_to_string(&path).unwrap();
        source.push_str(&format!(r#"
import functools
import json
import time
def trace_callback(callback):
    @functools.wraps(callback)
    def traced(*args):
        context = args[-1]
        started_ns = time.monotonic_ns()
        if callback.__name__ == "prepare_path":
            time.sleep(0.1)
        value = callback(*args)
        with open({trace}, "a") as output:
            output.write(json.dumps({{"callback": callback.__name__, "parameters": context.problem_parameters,
                "problem_dir": str(getattr(context, "problem_dir", "")), "started_ns": started_ns, "finished_ns": time.monotonic_ns(),
                "value": value}}) + "\n")
        return value
    return traced
"#));
        for callback in callbacks {
            source.push_str(&format!("{callback} = trace_callback({callback})\n"));
        }
        fs::write(path, source).unwrap();
    }
    root
}

fn invoke(root: &Path, arguments: &[&str]) -> (bool, Value) {
    let path = std::env::join_paths(
        std::iter::once(root.join(if cfg!(windows) {
            "venv/Scripts"
        } else {
            "venv/bin"
        }))
        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .env("PATH", path)
        .args(arguments)
        .args(["--output", "json"])
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {output:?}"));
    (output.status.success(), value)
}

fn trace(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("trace.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn worker_checks_prepare_reference_inputs_without_a_run_store() {
    let root = repository();
    let path = root.path().join("metewand.yaml");
    let source = fs::read_to_string(&path).unwrap()
        .replace("path_length: {type: integer, grid: [2, 4]}", "path_length: {type: integer, default: 2}")
        .replace("    result:", "    reference-cases:\n      - dataset: {values: [0, 1, 2, 3, 4]}\n        result: {total: 75}\n        expected-metrics: {error: 0}\n    result:");
    fs::write(path, source).unwrap();
    let (success, report) = invoke(root.path(), &["check", "--workers"]);
    assert!(success, "{report}");
    assert_eq!(report["data"]["reference_cases"][0]["metrics"]["error"], 0);
    let events = trace(root.path());
    assert_eq!(
        events
            .iter()
            .filter(|event| event["callback"] == "prepare_path")
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| event["callback"] == "generate" || event["callback"] == "solve")
    );
    assert!(!root.path().join("run").exists());
    assert!(!root.path().join("cache").exists());
}

#[test]
fn problem_preparation_is_shared_cached_and_bound_to_problem_settings() {
    let root = repository();
    for command in ["check", "plan"] {
        let (success, value) = invoke(root.path(), &[command]);
        assert!(success, "{value}");
        assert!(!root.path().join("trace.jsonl").exists());
    }
    let mut original_dataset = None;
    for (index, expected_preparations) in [(0, 2), (1, 2), (2, 4), (3, 6)] {
        if index == 2 {
            let path = root.path().join("metewand.yaml");
            let source = fs::read_to_string(&path).unwrap();
            fs::write(path, source.replace("default: 0.5", "default: 0.25")).unwrap();
        }
        if index == 3 {
            let path = root.path().join("preparation.py");
            let source = fs::read_to_string(&path).unwrap();
            fs::write(path, format!("{source}\n")).unwrap();
        }
        let output = format!("run-{index}");
        let started = std::time::Instant::now();
        let (success, report) = invoke(
            root.path(),
            &["run", "--output-dir", &output, "--cache-dir", "cache"],
        );
        let elapsed = started.elapsed();
        assert!(success, "{report}");
        let attempts = report["data"]["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 8);
        let events = trace(root.path());
        assert_eq!(
            events
                .iter()
                .filter(|event| event["callback"] == "generate")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event["callback"] == "prepare_path")
                .count(),
            expected_preparations
        );
        let mut measured = 0;
        let mut preparations = std::collections::BTreeMap::new();
        for attempt in attempts {
            assert_eq!(attempt["status"], "accepted");
            assert_eq!(attempt["metrics"]["error"], 0);
            let record: Value = serde_json::from_slice(
                &fs::read(
                    root.path()
                        .join(&output)
                        .join(attempt["attempt_directory"].as_str().unwrap())
                        .join("attempt.json"),
                )
                .unwrap(),
            )
            .unwrap();
            let context = &record["provenance"]["data"]["context"];
            measured += record["provenance"]["data"]["measurements"]["timed_wall_time_ns"]["value"]
                .as_u64()
                .unwrap();
            let dataset = context["dataset_instance"].clone();
            assert_eq!(
                original_dataset.get_or_insert_with(|| dataset.clone()),
                &dataset
            );
            assert!(context["problem_instance"]["preparation"].is_string());
            let preparation = &context["problem_preparation"];
            assert!(
                preparation["derivation_id"]
                    .as_str()
                    .unwrap()
                    .starts_with("mw1-problem-preparation-")
            );
            assert_eq!(
                preparation["derivation"]["problem"],
                context["problem_instance"]["configuration"]
            );
            let problem = context["problem_instance"]["configuration"].to_string();
            let artifact = context["problem_instance"]["preparation"].to_string();
            assert_eq!(
                preparations
                    .entry(problem)
                    .or_insert_with(|| artifact.clone()),
                &artifact
            );
            if index == 1 {
                assert_eq!(
                    context["problem_preparation_time_ns"]["reason"],
                    "cache_hit"
                );
            }
        }
        assert_eq!(preparations.len(), 2);
        if index == 0 {
            let shared: u64 = events
                .iter()
                .filter(|event| event["callback"] == "prepare_path")
                .map(|event| {
                    event["finished_ns"].as_u64().unwrap() - event["started_ns"].as_u64().unwrap()
                })
                .sum();
            assert!(elapsed >= std::time::Duration::from_nanos(measured + shared));
        }
    }
    let events = trace(root.path());
    let prepared = events
        .iter()
        .filter(|event| event["callback"] == "prepare_path")
        .collect::<Vec<_>>();
    assert_eq!(prepared[0]["value"]["alpha"].as_array().unwrap().len(), 2);
    assert_eq!(prepared[1]["value"]["alpha"].as_array().unwrap().len(), 3);
    assert!(
        events
            .iter()
            .filter(|event| event["callback"] == "evaluate")
            .all(|event| !event["problem_dir"].as_str().unwrap().is_empty())
    );
}

#[test]
fn problem_preparation_outputs_must_satisfy_the_declared_schema() {
    let root = repository();
    let path = root.path().join("metewand.yaml");
    let source = fs::read_to_string(&path).unwrap().replace("      function: preparation.py:prepare_path", "      function: preparation.py:prepare_path\n      output-schema: {type: object, required: [missing]}");
    fs::write(path, source).unwrap();
    let (success, report) = invoke(
        root.path(),
        &["run", "--output-dir", "run", "--cache-dir", "cache"],
    );
    assert!(!success, "{report}");
    assert!(!root.path().join("run").exists());
    assert!(
        !fs::read_dir(root.path().join("cache/derivations"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("mw1-problem-preparation-"))
    );
}

#[test]
fn implementations_cannot_mutate_shared_problem_inputs() {
    let root = repository();
    let path = root.path().join("functions.py");
    let source = fs::read_to_string(&path).unwrap().replace(
        "    grid = read_json(context.problem_dir / \"data.json\")[\"alpha\"]",
        "    (context.problem_dir / 'data.json').write_text('{}')\n    grid = []",
    );
    fs::write(path, source).unwrap();
    let (success, report) = invoke(
        root.path(),
        &["run", "--output-dir", "run", "--cache-dir", "cache"],
    );
    assert!(!success, "{report}");
    for attempt in report["data"]["attempts"].as_array().unwrap() {
        assert_eq!(attempt["diagnostic"]["code"], "dataset_mutation");
        assert!(attempt["observation_id"].is_null());
    }
}

#[test]
fn problem_preparation_rejects_mutation_before_attempts() {
    let root = repository();
    let path = root.path().join("preparation.py");
    let source = fs::read_to_string(&path).unwrap().replace(
        "    parameters = context.problem_parameters",
        "    (context.dataset_dir / 'data.json').write_text('{}')\n    parameters = context.problem_parameters",
    );
    fs::write(path, source).unwrap();
    let (success, report) = invoke(
        root.path(),
        &["run", "--output-dir", "run", "--cache-dir", "cache"],
    );
    assert!(!success, "{report}");
    assert!(!root.path().join("run").exists());
    assert!(
        !trace(root.path())
            .iter()
            .any(|event| event["callback"] == "solve")
    );
}
