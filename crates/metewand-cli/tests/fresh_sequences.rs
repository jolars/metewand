#![cfg(unix)]
#[path = "support/export_fidelity.rs"]
mod export_fidelity;

use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

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
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn stage(r: bool) -> TempDir {
    let stage = TempDir::new().unwrap();
    copy(&root().join("examples/fresh-sequence"), stage.path());
    copy(
        &root().join("sdk/python/src/metewand"),
        &stage.path().join("metewand"),
    );
    if r {
        let package = TempDir::new().unwrap();
        copy(&root().join("sdk/r"), &package.path().join("r"));
        let library = stage.path().join("library");
        fs::create_dir(&library).unwrap();
        let output = Command::new("R")
            .args(["CMD", "INSTALL", "--preclean", "--clean"])
            .arg(format!("--library={}", library.display()))
            .arg(package.path().join("r"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    } else {
        edit(stage.path(), "metewand.yaml", |mut s| {
            let start = s.find("  uniroot:\n").unwrap();
            let end = s[start..].find("experiments:\n").unwrap() + start;
            s.replace_range(start..end, "");
            let start = s.find("      uniroot:\n").unwrap();
            let end = s[start..].find("  four_points:\n").unwrap() + start;
            s.replace_range(start..end, "");
            s.replace("[bisection, uniroot]", "[bisection]")
        });
    }
    stage
}
fn edit(root: &Path, file: &str, f: impl FnOnce(String) -> String) {
    let path = root.join(file);
    fs::write(&path, f(fs::read_to_string(&path).unwrap())).unwrap();
}
fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn data(output: &Output) -> Value {
    let value: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    let schema = match value["data"]["command"].as_str() {
        Some("run") => "schemas/v1/run-output.schema.json",
        Some("status" | "explain") => "schemas/v1/inspection-output.schema.json",
        Some("export") => "schemas/v1/export-output.schema.json",
        _ => "schemas/v1/machine-output.schema.json",
    };
    metewand_core::public_schemas::public_schema_catalog()
        .unwrap()
        .validate(Path::new(schema), &value)
        .unwrap_or_else(|error| panic!("{error}\n{value}"));
    value["data"].clone()
}
fn run(root: &Path, args: &[&str]) -> (Output, Value) {
    let mut all = vec!["run", "--output-dir", "run", "--output", "json"];
    all.extend(args);
    let output = invoke(root, &all);
    let report = data(&output);
    (output, report)
}
fn fixed_only(root: &Path) {
    edit(root, "metewand.yaml", |mut s| {
        let start = s.find("  - name: target\n").unwrap();
        let end = s.find("  - name: fixed\n").unwrap();
        s.replace_range(start..end, "");
        s.replace("run-order: randomized", "run-order: sequential")
    });
}

#[test]
fn fresh_points_stop_on_independent_metrics_and_export_skipped_slots() {
    let stage = stage(false);
    edit(stage.path(), "metewand.yaml", |s| {
        s.replace("threshold: 0.000001", "threshold: 0.1")
    });
    let check = invoke(stage.path(), &["check", "--workers", "--output", "json"]);
    assert!(check.status.success(), "{check:?}");
    data(&check);
    let plan = data(&invoke(stage.path(), &["plan", "--output", "json"]));
    assert_eq!(
        plan["experiments"][0]["candidates"][0]["specifications"][0]["fresh_observations"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let (output, report) = run(stage.path(), &[]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 8);
    assert!(
        report["fresh_sequences"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "accepted")
    );
    let mut below = false;
    for attempt in report["attempts"].as_array().unwrap() {
        assert_eq!(attempt["status"], "accepted");
        let record: Value = serde_json::from_slice(
            &fs::read(
                stage
                    .path()
                    .join("run")
                    .join(attempt["observation_directory"].as_str().unwrap())
                    .join("observation.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record["profile"]["measurement_basis"], "executor_observed");
        below |= record["validity"]["completion"] == "not_reached";
        assert!(record["timing"]["timed_wall_time_ns"].as_u64().unwrap() > 0);
    }
    assert!(below);
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["skipped_target_reached_slots"], 4);
    assert_eq!(status["summary"]["selected_observations"], 8);
    let skipped = status["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["state"] == "skipped_target_reached")
        .unwrap();
    let explain = invoke(
        stage.path(),
        &[
            "explain",
            skipped["resolved_slot_id"].as_str().unwrap(),
            "--output-dir",
            "run",
            "--output",
            "json",
        ],
    );
    assert!(explain.status.success(), "{explain:?}");
    data(&explain);
    let (output, resume) = run(stage.path(), &["--resume", "--retry"]);
    assert!(output.status.success(), "{output:?}");
    assert!(resume["attempts"].as_array().unwrap().is_empty());
    fs::remove_file(stage.path().join("metewand.yaml")).unwrap();
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
        data(&export);
    }
    let rows = fs::read_to_string(stage.path().join("jsonl/results.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    export_fidelity::verify(stage.path(), &rows);
    for row in &rows {
        metewand_core::public_schemas::public_schema_catalog()
            .unwrap()
            .validate(Path::new("schemas/v1/export-row.schema.json"), row)
            .unwrap();
    }
    let skipped = rows
        .iter()
        .filter(|r| r["record_type"] == "slot")
        .collect::<Vec<_>>();
    assert_eq!(skipped.len(), 4);
    assert!(
        skipped
            .iter()
            .all(|r| r["metric_value"].is_null() && r["attempt_id"].is_null())
    );
    assert!(rows.iter().any(|r| r["record_type"] == "observation"
        && r["metric_name"] == "/residual"
        && r["profile"]["observation_control"].is_object()));
}

#[test]
fn selecting_one_point_does_not_execute_its_prefix() {
    let stage = stage(false);
    fixed_only(stage.path());
    let plan = data(&invoke(stage.path(), &["plan", "--output", "json"]));
    let id = plan["experiments"][0]["candidates"][0]["specifications"][0]["observation_slots"][2]
        .as_str()
        .unwrap();
    let (output, report) = run(stage.path(), &["--select", id]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(report["fresh_sequences"][0]["status"], "partial");
    assert_eq!(
        report["attempts"][0]["result"],
        json!({"x":1.4142074584960938})
    );
    let (output, report) = run(stage.path(), &["--resume"]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(report["fresh_sequences"][0]["status"], "accepted");
}

#[test]
fn failed_point_does_not_stop_later_controls_and_retry_preserves_budget() {
    let stage = stage(false);
    fixed_only(stage.path());
    let marker = stage.path().join("failed-once");
    edit(stage.path(), "worker.py", |s| {
        s.replace("        self.control = observation_control",&format!("        from pathlib import Path\n        marker = Path({:?})\n        if observation_control['iterations'] == 4 and not marker.exists():\n            marker.write_text('failed')\n            raise RuntimeError('one failed point')\n        self.control = observation_control",marker.to_str().unwrap()))
    });
    let (output, report) = run(stage.path(), &[]);
    assert!(!output.status.success());
    assert_eq!(report["attempts"].as_array().unwrap().len(), 4);
    assert_eq!(report["summary"]["failed_attempts"], 1);
    assert_eq!(report["fresh_sequences"][0]["accepted_points"], 3);
    let consumed = report["fresh_sequences"][0]["consumed_time_ns"]
        .as_u64()
        .unwrap();
    let (_, report) = run(stage.path(), &["--resume"]);
    assert!(report["attempts"].as_array().unwrap().is_empty());
    let (output, report) = run(stage.path(), &["--resume", "--retry"]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(report["fresh_sequences"][0]["status"], "accepted");
    assert!(
        report["fresh_sequences"][0]["consumed_time_ns"]
            .as_u64()
            .unwrap()
            > consumed
    );
}

#[test]
fn active_time_bound_censors_and_resume_does_not_reset_it() {
    let stage = stage(false);
    fixed_only(stage.path());
    edit(stage.path(), "metewand.yaml", |s| {
        s.replace("max-active-time: 10 s", "max-active-time: 20 ms")
    });
    edit(stage.path(), "worker.py", |s| {
        s.replace(
            "        self.calls += 1",
            "        import time\n        time.sleep(0.05)\n        self.calls += 1",
        )
    });
    let (output, report) = run(stage.path(), &[]);
    assert!(!output.status.success(), "{report}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(report["attempts"][0]["status"], "censored");
    assert_eq!(report["fresh_sequences"][0]["consumed_time_ns"], 20_000_000);
    let (_, resume) = run(stage.path(), &["--resume", "--retry"]);
    assert!(resume["attempts"].as_array().unwrap().is_empty());
    assert_eq!(resume["fresh_sequences"][0]["consumed_time_ns"], 20_000_000);
}

#[test]
fn python_and_r_black_box_adapters_share_the_evaluator() {
    let stage = stage(true);
    let check = invoke(stage.path(), &["check", "--workers", "--output", "json"]);
    assert!(check.status.success(), "{check:?}");
    let (output, report) = run(stage.path(), &[]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert_eq!(report["fresh_sequences"].as_array().unwrap().len(), 5);
    assert!(
        report["fresh_sequences"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "accepted")
    );
}

#[test]
fn raw_workers_must_echo_the_selected_control() {
    for mode in ["valid", "float_echo", "mismatch"] {
        let stage = stage(false);
        fixed_only(stage.path());
        fs::copy(
            root().join("fixtures/worker-protocol/v1/fresh_worker.py"),
            stage.path().join("raw.py"),
        )
        .unwrap();
        edit(stage.path(), "metewand.yaml", |s| {
            s.replace("    runner: python\n    entrypoint: worker.py\n    sources: [worker.py, solver.py, \"metewand/*.py\"]",&format!("    runner: command\n    program: python3\n    args: [\"{{sources}}/raw.py\", {mode}]\n    sources: [raw.py]"))
        });
        let (output, report) = run(stage.path(), &[]);
        assert_eq!(
            output.status.success(),
            mode != "mismatch",
            "{output:?}\n{report}"
        );
        if mode != "mismatch" {
            assert_eq!(report["fresh_sequences"][0]["status"], "accepted");
        } else {
            assert_eq!(report["summary"]["failed_attempts"], 4);
            assert!(
                report["attempts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|a| a["observation_id"].is_null())
            );
        }
    }
}

#[test]
fn max_attempts_counts_dispatches_after_target_skips() {
    let stage = stage(false);
    edit(stage.path(), "metewand.yaml", |s| {
        s.replace("threshold: 0.000001", "threshold: 0.1")
    });
    let (output, report) = run(stage.path(), &["--max-attempts", "5"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["summary"]["attempt_slots"], 5);
    let (output, report) = run(stage.path(), &["--resume"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["summary"]["attempt_slots"], 3);
}

#[test]
fn one_policy_can_mix_fresh_and_buffered_delivery() {
    let stage = stage(false);
    fs::write(
        stage.path().join("buffered.py"),
        r#"from metewand import BufferedImplementation, serve_implementation
class Solver(BufferedImplementation):
    def prepare(self, *args): pass
    def reset(self): pass
    def execute_buffered(self, budget, trace):
        progress = {'check_index':0,'work':{}}
        trace.observe(progress, lambda:{'residual':0.04}, lambda:{'x':1.4})
        trace.finish(progress, lambda:{'residual':0.04}, lambda:{'x':1.4})
serve_implementation(Solver())
"#,
    )
    .unwrap();
    edit(stage.path(), "metewand.yaml", |s| {
        s.replace("experiments:\n",r#"  buffered:
    runner: python
    entrypoint: buffered.py
    sources: [buffered.py, "metewand/*.py"]
    problem-contracts: [square_root]
    capabilities: [buffered_profile]
experiments:
"#).replace("implementations: [bisection]","implementations: [bisection, buffered]").replace("threshold: 0.000001","threshold: 0.1").replace("    max-active-time: 10 s\n    implementations:\n", "    max-active-time: 10 s\n    max-trace-points: 10\n    max-buffer-bytes: 65536\n    terminal-reserve-bytes: 8192\n    implementations:\n").replace("      bisection: &bisection",r#"      buffered: &buffered
        delivery: buffered_profile
        measurement-basis: instrumented
        instrumented-scope: solver
        instrumentation-schema: { type: object }
        instrumentation: {}
        check-schedule: { every: 1 }
      bisection: &bisection"#).replace("      bisection: *bisection","      buffered: *buffered\n      bisection: *bisection")
    });
    // The fixed-count experiment is unrelated to this mixed target comparison.
    edit(stage.path(), "metewand.yaml", |mut s| {
        let start = s.find("  - name: fixed\n").unwrap();
        let end = s.find("execution-policies:\n").unwrap();
        s.replace_range(start..end, "");
        s
    });
    edit(stage.path(), "problem.yaml", |s| {
        s.replace("supported-budgets:","instrumented-scopes:\n  solver:\n    clock-origin: Before solving.\n    included-work: Solving and snapshots.\n    excluded-preparation: None.\nsupported-budgets:")
    });
    let (output, report) = run(stage.path(), &[]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert_eq!(report["fresh_sequences"].as_array().unwrap().len(), 2);
    assert!(
        report["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["observations"].as_array().is_some_and(|o| !o.is_empty()))
    );
}

#[test]
fn execution_timeout_allows_later_controls_but_target_blocks_old_retries() {
    let stage = stage(false);
    edit(stage.path(), "metewand.yaml", |mut s| {
        let start = s.find("  - name: fixed\n").unwrap();
        let end = s.find("execution-policies:\n").unwrap();
        s.replace_range(start..end, "");
        s.replace("threshold: 0.000001", "threshold: 0.1")
            .replace("timeout: 5 s", "timeout: 200 ms")
            .replace("warmup-runs: 1", "warmup-runs: 0")
    });
    edit(stage.path(), "worker.py", |s| {
        s.replace("        self.calls += 1","        if observation_control['iterations'] == 1:\n            import time\n            time.sleep(0.4)\n        self.calls += 1")
    });
    let (output, report) = run(stage.path(), &[]);
    assert!(!output.status.success(), "{report}");
    assert_eq!(report["summary"]["failed_attempts"], 2);
    assert_eq!(report["summary"]["accepted_attempts"], 2);
    assert!(
        report["fresh_sequences"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "accepted")
    );
    let (output, report) = run(stage.path(), &["--resume", "--retry"]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert!(report["attempts"].as_array().unwrap().is_empty());
    assert_eq!(report["events"][0]["attempt_slots"], 0);
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["failed_attempts"], 2);
    assert_eq!(status["summary"]["skipped_target_reached_slots"], 6);
}

#[test]
fn concurrent_invocations_leave_owned_repetitions_for_their_owner() {
    let stage = stage(false);
    fixed_only(stage.path());
    edit(stage.path(), "metewand.yaml", |s| {
        s.replace("warmup-runs: 1", "warmup-runs: 0")
    });
    let marker = stage.path().join("point-started");
    let release = stage.path().join("release-point");
    edit(stage.path(), "worker.py", |s| {
        s.replace("        self.calls += 1",&format!("        if observation_control['iterations'] == 1:\n            from pathlib import Path\n            import time\n            Path({:?}).write_text('ready')\n            while not Path({:?}).exists():\n                time.sleep(0.01)\n        self.calls += 1",marker.to_str().unwrap(),release.to_str().unwrap()))
    });
    let first = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(stage.path())
        .args([
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "1",
            "--output",
            "json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(marker.exists(), "the first point did not start");
    let (output, report) = run(stage.path(), &["--resume"]);
    fs::write(release, "continue").unwrap();
    let first = first.wait_with_output().unwrap();
    assert!(first.status.success(), "{first:?}");
    assert!(output.status.success(), "{output:?}\n{report}");
    assert!(report["attempts"].as_array().unwrap().is_empty());
    assert_eq!(report["fresh_sequences"][0]["status"], "partial");
    let (output, report) = run(stage.path(), &["--resume"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(report["fresh_sequences"][0]["status"], "accepted");
}
