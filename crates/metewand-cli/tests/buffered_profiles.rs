#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

#[path = "support/export_fidelity.rs"]
mod export_fidelity;

#[allow(dead_code)]
#[path = "../../../examples/buffered-profile/main.rs"]
mod native;

#[test]
fn native_buffered_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        native::serve().unwrap();
    }
}

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
fn stage() -> TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let stage = TempDir::new().unwrap();
    copy(&root.join("examples/buffered-profile"), stage.path());
    copy(
        &root.join("packages/python/src/metewand"),
        &stage.path().join("metewand"),
    );
    stage
}
fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn data(output: &Output) -> Value {
    serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"))["data"]
        .clone()
}
fn run_data(output: &Output) -> Value {
    use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
    let envelope: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::RunOutput.repository_path(), &envelope)
        .unwrap_or_else(|error| panic!("{error}: {envelope}"));
    envelope["data"].clone()
}
fn document(path: PathBuf) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn buffered_modes_publish_independent_snapshots_and_diagnostic_traces() {
    let stage = stage();
    let check = invoke(stage.path(), &["check", "--workers", "--output", "json"]);
    assert!(check.status.success(), "{check:?}");
    let plan = invoke(stage.path(), &["plan", "--output", "json"]);
    assert!(plan.status.success(), "{plan:?}");
    let run = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let report = run_data(&run);
    assert!(run.status.success(), "{report}");
    assert_eq!(report["summary"]["accepted_attempts"], 5, "{report}");
    let comparison = report["comparison"].as_array().unwrap();
    assert_eq!(comparison.len(), 15);
    for attempt in report["attempts"].as_array().unwrap() {
        assert_eq!(
            attempt["observations"].as_array().unwrap().len(),
            3,
            "{attempt}"
        );
        let mut previous = 0;
        for observation in attempt["observations"].as_array().unwrap() {
            let persisted = document(
                stage
                    .path()
                    .join("run")
                    .join(observation["observation_directory"].as_str().unwrap())
                    .join("observation.json"),
            );
            assert_eq!(persisted["profile"]["measurement_basis"], "instrumented");
            let row = comparison
                .iter()
                .find(|row| row["observation_id"] == observation["observation_id"])
                .unwrap();
            assert_eq!(row["metrics"], observation["metrics"]);
            assert_eq!(
                row["instrumented_time_ns"],
                persisted["profile"]["instrumented_time_ns"]
            );
            assert_eq!(
                row["instrumented_scope"],
                persisted["profile"]["instrumented_scope"]
            );
            assert_eq!(row["timed_wall_time_status"], "complete");
            let time = persisted["profile"]["instrumented_time_ns"]
                .as_u64()
                .unwrap();
            assert!(time >= previous);
            previous = time;
            assert!(
                persisted["profile"]["instrumented_total_time_ns"]
                    .as_u64()
                    .unwrap()
                    >= time
            );
            assert_eq!(persisted["attempt_id"], attempt["attempt_id"]);
        }
    }
    let status = invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    );
    assert!(status.status.success(), "{status:?}");
    assert_eq!(data(&status)["summary"]["selected_observations"], 15);
    let resume = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--resume", "--output", "json"],
    );
    assert!(resume.status.success(), "{resume:?}");
    assert_eq!(run_data(&resume)["summary"]["attempt_slots"], 0);
    for format in ["jsonl", "csv"] {
        let export = invoke(
            stage.path(),
            &[
                "export",
                "--output-dir",
                "run",
                "--destination",
                format,
                "--format",
                format,
                "--output",
                "json",
            ],
        );
        assert!(export.status.success(), "{export:?}");
    }
    let lines = fs::read_to_string(stage.path().join("jsonl/results.jsonl")).unwrap();
    let rows: Vec<Value> = lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    export_fidelity::verify(stage.path(), &rows);
    assert!(rows.iter().any(|r| r["record_type"] == "trace"
        && r["metric_value"].is_null()
        && r["provisional_metric_value"].is_number()));
    assert!(
        rows.iter()
            .any(|r| r["record_type"] == "observation" && r["metric_name"] == "/gap")
    );
}

#[test]
fn selecting_a_capture_or_candidate_keeps_complete_buffered_attempts() {
    let stage = stage();
    let plan = data(&invoke(stage.path(), &["plan", "--output", "json"]));
    let candidate = &plan["experiments"][0]["candidates"][0];
    for (selection, directory, attempts) in [
        (
            &candidate["specifications"][0]["observation_slots"][1],
            "capture",
            1,
        ),
        (&candidate["id"], "candidate", 2),
    ] {
        let run = invoke(
            stage.path(),
            &[
                "run",
                "--output-dir",
                directory,
                "--select",
                selection.as_str().unwrap(),
                "--output",
                "json",
            ],
        );
        let report = run_data(&run);
        assert!(run.status.success(), "{run:?}");
        assert_eq!(report["summary"]["accepted_attempts"], attempts, "{report}");
        assert!(
            report["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|attempt| { attempt["observations"].as_array().unwrap().len() == 3 })
        );
    }
}

#[test]
fn local_target_disagreement_censors_and_contract_agreement_fails() {
    for agreement in [false, true] {
        let stage = stage();
        let manifest = stage.path().join("metewand.yaml");
        let source = fs::read_to_string(&manifest)
            .unwrap()
            .replace(
                "local_bias: { type: number, default: 0 }",
                "local_bias: { type: number, default: -1 }",
            )
            .replace("warmup-runs: 1", "warmup-runs: 0");
        fs::write(manifest, source).unwrap();
        if agreement {
            let path = stage.path().join("problem.yaml");
            let source = fs::read_to_string(&path).unwrap().replace(
                "    clock-origin:",
                "    metric-agreement: { gap: { atol: 0, rtol: 0 } }\n    clock-origin:",
            );
            fs::write(path, source).unwrap();
        }
        let run = invoke(
            stage.path(),
            &[
                "run",
                "--output-dir",
                "run",
                "--max-attempts",
                "1",
                "--output",
                "json",
            ],
        );
        let report = run_data(&run);
        assert!(!run.status.success(), "{report}");
        let attempt = &report["attempts"][0];
        assert_eq!(
            attempt["status"],
            if agreement { "failed" } else { "censored" },
            "{report}"
        );
        assert_eq!(attempt["observations"].as_array().unwrap().len(), 1);
        assert_eq!(attempt["observations"][0]["metrics"], json!({"gap":1}));
        assert_eq!(
            attempt["diagnostic"]["code"],
            if agreement {
                "metric_disagreement"
            } else {
                "local_target_unconfirmed"
            }
        );
    }
}

#[test]
fn native_loop_returns_the_same_buffer_without_python_callbacks() {
    let stage = stage();
    let path = stage.path().join("metewand.yaml");
    let source=fs::read_to_string(&path).unwrap().replace("runner: python\n    entrypoint: worker.py\n    sources: [worker.py, \"metewand/*.py\"]",&format!("runner: command\n    program: {}\n    args: [--exact, native_buffered_entry, --nocapture]",serde_json::to_string(&std::env::current_exe().unwrap()).unwrap()));
    fs::write(path, source).unwrap();
    let run = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "1",
            "--output",
            "json",
        ],
    );
    let report = run_data(&run);
    assert!(run.status.success(), "{report}");
    assert_eq!(
        report["attempts"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn a_written_but_unreturned_buffer_produces_no_observations() {
    let stage = stage();
    let manifest = stage.path().join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("warmup-runs: 1", "warmup-runs: 0"),
    )
    .unwrap();
    let worker = stage.path().join("worker.py");
    fs::write(&worker,fs::read_to_string(&worker).unwrap().replace("        trace.finish(progress, measure, snapshot)","        trace.finish(progress, measure, snapshot)\n        trace.write()\n        raise RuntimeError('complete files without a returned buffer')")).unwrap();
    let run = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "1",
            "--output",
            "json",
        ],
    );
    let report = run_data(&run);
    assert!(!run.status.success(), "{report}");
    assert_eq!(
        report["attempts"][0]["diagnostic"]["code"],
        "implementation_failure"
    );
    assert!(report["attempts"][0].get("observations").is_none());
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["selected_observations"], 0);
    assert!(status["observations"].as_array().unwrap().is_empty());
}

#[test]
fn evaluation_failure_preserves_a_prefix_and_retry_selects_one_new_trajectory() {
    let stage = stage();
    let manifest = stage.path().join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("warmup-runs: 1", "warmup-runs: 0"),
    )
    .unwrap();
    // This external latch injects an evaluator fault without changing the
    // checked source, scientific inputs, or resolved slot between retries.
    let latch = stage.path().join("injected-failure");
    fs::write(&latch, "").unwrap();
    let evaluator = stage.path().join("evaluator.py");
    let injection = format!(
        "    from pathlib import Path\n    latch = Path({})\n    if 0 < abs(x) < 0.5 and latch.exists():\n        latch.unlink()\n        raise RuntimeError('injected evaluation failure')\n    return Evaluation",
        serde_json::to_string(&latch).unwrap()
    );
    fs::write(
        &evaluator,
        fs::read_to_string(&evaluator)
            .unwrap()
            .replace("    return Evaluation", &injection),
    )
    .unwrap();
    let run = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "1",
            "--output",
            "json",
        ],
    );
    let first = run_data(&run);
    assert!(!run.status.success(), "{first}");
    assert_eq!(
        first["attempts"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{first}"
    );
    let first_id = first["attempts"][0]["attempt_id"].clone();
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["selected_observations"], 0);
    let retry = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--resume",
            "--retry",
            "--max-attempts",
            "1",
            "--output",
            "json",
        ],
    );
    let second = run_data(&retry);
    assert!(retry.status.success(), "{second}");
    assert_ne!(second["attempts"][0]["attempt_id"], first_id);
    assert_eq!(
        second["attempts"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["selected_observations"], 3);
    assert_eq!(status["observations"].as_array().unwrap().len(), 4);
    for observation in second["attempts"][0]["observations"].as_array().unwrap() {
        let persisted = document(
            stage
                .path()
                .join("run")
                .join(observation["observation_directory"].as_str().unwrap())
                .join("observation.json"),
        );
        assert_eq!(persisted["attempt_id"], second["attempts"][0]["attempt_id"]);
    }
}

#[test]
fn selected_bindings_and_numeric_targets_have_precise_dependencies() {
    let stage = stage();
    let path = stage.path().join("metewand.yaml");
    let source = fs::read_to_string(&path).unwrap();
    let first = data(&invoke(stage.path(), &["plan", "--output", "json"]));
    fs::write(
        &path,
        source.replace(
            "instrumentation: { recording: direct }",
            "instrumentation: { recording: observer }",
        ),
    )
    .unwrap();
    let second = data(&invoke(stage.path(), &["plan", "--output", "json"]));
    assert_eq!(
        first["experiments"][0]["candidates"][0]["id"],
        second["experiments"][0]["candidates"][0]["id"]
    );
    assert_ne!(
        first["experiments"][0]["candidates"][1]["id"],
        second["experiments"][0]["candidates"][1]["id"]
    );
    fs::write(&path, source.replace("metric: gap", "metric: missing")).unwrap();
    let rejected = invoke(stage.path(), &["check", "--output", "json"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8(rejected.stdout)
            .unwrap()
            .contains("numeric property")
    );
}
