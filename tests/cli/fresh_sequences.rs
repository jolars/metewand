#![cfg(unix)]
use super::support::export_fidelity;

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
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}
fn stage(r: bool) -> TempDir {
    let stage = TempDir::new().unwrap();
    copy(&root().join("examples/fresh-sequence"), stage.path());
    copy(
        &root().join("packages/python/src/metewand"),
        &stage.path().join("metewand"),
    );
    if r {
        let package = TempDir::new().unwrap();
        copy(&root().join("packages/r"), &package.path().join("r"));
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
    crate::core::public_schemas::public_schema_catalog()
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
fn attempt_document(root: &Path, attempt: &Value) -> Value {
    let directory = attempt["attempt_directory"].as_str().unwrap();
    serde_json::from_slice(
        &fs::read(root.join("run").join(directory).join("attempt.json")).unwrap(),
    )
    .unwrap()
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
fn adaptive_fresh_pilots_resume_and_use_cumulative_executor_time() {
    let stage = stage(false);
    let root = stage.path();
    edit(root, "metewand.yaml", |s| {
        s.replacen(
            "    max-active-time: 10 s",
            "    adaptive-budget: {initial-pilot-time: 2 s}",
            1,
        )
        .replace(
            "timing-scope: prepare_and_execute",
            "timing-scope: execute_only",
        )
        .replace(
            "primary-time: timed_wall_time",
            "primary-time: implementation_time",
        )
    });
    let plan = data(&invoke(
        root,
        &["plan", "--experiment", "target", "--output", "json"],
    ));
    let point = plan["experiments"][0]["candidates"][0]["specifications"][0]["observation_slots"]
        [2]
    .as_str()
    .unwrap();
    let (output, bounded) = run(
        root,
        &[
            "--shared",
            "--experiment",
            "target",
            "--select",
            point,
            "--max-attempts",
            "1",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(bounded["calibration_attempts"].as_array().unwrap().len(), 1);
    assert_eq!(bounded["summary"]["profile_outcomes"]["status"], "paused");
    let record = attempt_document(root, &bounded["calibration_attempts"][0]["attempt"]);
    let context = &record["provenance"]["data"]["context"];
    assert!(context["calibration"].is_object());
    assert!(context.get("concurrency").is_none());
    assert!(bounded["adaptive_budgets"].is_null());
    assert!(
        bounded["fresh_sequences"]
            .as_array()
            .is_none_or(Vec::is_empty)
    );
    assert!(bounded["attempts"].as_array().unwrap().is_empty());
    let (output, run_report) = run(root, &["--experiment", "target"]);
    assert!(output.status.success(), "{run_report}");
    let decision = &run_report["adaptive_budgets"][0];
    assert_eq!(decision["schema_version"], 2);
    assert_eq!(decision["timing_basis"], "cumulative_timed_wall_time");
    assert!(decision["instrumented_scope"].is_null());
    let new_pilots = run_report["calibration_attempts"].as_array().unwrap();
    assert_eq!(new_pilots.len(), 3);
    let timed_wall_sum: u64 = bounded["calibration_attempts"]
        .as_array()
        .unwrap()
        .iter()
        .chain(new_pilots)
        .map(|pilot| {
            let directory = pilot["attempt"]["observation_directory"].as_str().unwrap();
            let observation: Value = serde_json::from_slice(
                &fs::read(root.join("run").join(directory).join("observation.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                observation["profile"]["measurement_basis"],
                "worker_reported"
            );
            observation["timing"]["timed_wall_time_ns"]
                .as_u64()
                .unwrap()
        })
        .sum();
    assert_eq!(decision["reference_time_ns"], timed_wall_sum);
    assert_eq!(
        decision["reference_observation_id"],
        new_pilots[2]["attempt"]["observation_id"]
    );
    assert_eq!(run_report["fresh_sequences"].as_array().unwrap().len(), 2);
    let output = invoke(root, &["status", "--output-dir", "run", "--output", "json"]);
    assert!(output.status.success(), "{output:?}");
    let status = data(&output);
    assert_eq!(status["fresh_sequences"], run_report["fresh_sequences"]);
    let export = invoke(
        root,
        &[
            "export",
            "--output-dir",
            "run",
            "--format",
            "json",
            "--destination",
            "analysis",
        ],
    );
    assert!(export.status.success(), "{export:?}");
    let analysis: Value =
        serde_json::from_slice(&fs::read(root.join("analysis/results.json")).unwrap()).unwrap();
    assert_eq!(analysis["profiles"].as_array().unwrap().len(), 2);
    assert_eq!(analysis["adaptive_budgets"][0], *decision);
    for args in [
        [
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "saved.tar.gz",
        ],
        [
            "archive",
            "restore",
            "--archive",
            "saved.tar.gz",
            "--destination",
            "restored",
        ],
    ] {
        let output = invoke(root, &args);
        assert!(output.status.success(), "{output:?}");
    }
    let replay = invoke(
        root,
        &[
            "run",
            "--output-dir",
            "restored",
            "--experiment",
            "target",
            "--retry",
            "--output",
            "json",
        ],
    );
    assert!(replay.status.success(), "{replay:?}");
    let restored = data(&replay);
    assert_eq!(restored["adaptive_budgets"], run_report["adaptive_budgets"]);
    assert!(restored["calibration_attempts"].is_null());
}

#[test]
fn adaptive_fresh_continues_after_a_failed_fit() {
    let stage = stage(false);
    let root = stage.path();
    edit(root, "metewand.yaml", |s| {
        s.replacen(
            "    max-active-time: 10 s",
            "    adaptive-budget: {initial-pilot-time: 2 s}",
            1,
        )
        .replace("warmup-runs: 1", "warmup-runs: 0")
    });
    edit(root, "worker.py", |s| {
        s.replace(
            "        return {\"x\": bisect",
            "        if observation_control[\"iterations\"] == 1:\n            raise RuntimeError(\"first fit fails\")\n        return {\"x\": bisect",
        )
    });
    let (_, report) = run(root, &["--experiment", "target"]);
    let pilots = report["calibration_attempts"].as_array().unwrap();
    assert_eq!(pilots.len(), 4, "{report}");
    assert_eq!(pilots[0]["attempt"]["status"], "failed");
    let timed_wall = |pilot: &Value| {
        let attempt = attempt_document(root, &pilot["attempt"]);
        attempt["provenance"]["data"]["measurements"]["timed_wall_time_ns"]["value"]
            .as_u64()
            .unwrap_or_else(|| panic!("{attempt}"))
    };
    assert!(timed_wall(&pilots[0]) > 0);
    let reference = &report["adaptive_budgets"][0];
    assert_eq!(
        reference["reference_observation_id"],
        pilots[3]["attempt"]["observation_id"]
    );
    let elapsed: u64 = pilots.iter().map(timed_wall).sum();
    assert_eq!(reference["reference_time_ns"], elapsed);
}

#[test]
fn adaptive_fresh_grows_only_for_time_limited_fits() {
    for time_limited in [false, true] {
        let stage = stage(false);
        let root = stage.path();
        edit(root, "metewand.yaml", |s| {
            let parameters = if time_limited {
                "    max-active-time: 16 ms\n    adaptive-budget: {initial-pilot-time: 8 ms, fallback-time: 12 ms}"
            } else {
                "    adaptive-budget: {initial-pilot-time: 2 s}"
            };
            s.replacen("    max-active-time: 10 s", parameters, 1)
                .replace("threshold: 0.000001", "threshold: 0.0")
                .replace("warmup-runs: 1", "warmup-runs: 0")
        });
        if time_limited {
            edit(root, "worker.py", |s| {
                s.replace(
                    "        return {\"x\": bisect",
                    "        import time\n        time.sleep(0.1)\n        return {\"x\": bisect",
                )
            });
        }
        let output = invoke(
            root,
            &[
                "run",
                "--output-dir",
                "run",
                "--experiment",
                "target",
                "--output",
                "json",
            ],
        );
        let report = data(&output);
        assert_eq!(output.status.success(), time_limited, "{report}");
        if time_limited {
            assert_eq!(report["adaptive_budgets"][0]["fallback"], true);
            assert_eq!(
                report["adaptive_budgets"][0]["effective_limit_ns"],
                12_000_000
            );
            assert_eq!(
                report["adaptive_budgets"][0]["pilots"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(
                report["adaptive_budgets"][0]["pilots"][1]["reason"],
                "active_time_limit"
            );
        } else {
            assert!(
                report.to_string().contains("adaptive_calibration_stopped"),
                "{report}"
            );
            assert!(
                report.to_string().contains("schedule_exhausted"),
                "{report}"
            );
        }
    }
}

#[test]
fn fresh_points_can_select_implementation_time_without_changing_budget_accounting() {
    let stage = stage(false);
    fixed_only(stage.path());
    edit(stage.path(), "metewand.yaml", |s| {
        s.replace(
            "primary-time: timed_wall_time",
            "primary-time: implementation_time",
        )
        .replace(
            "timing-scope: prepare_and_execute",
            "timing-scope: execute_only",
        )
    });
    let (output, report) = run(stage.path(), &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["summary"]["accepted_attempts"], 4);
    let mut wall_sum = 0;
    for row in report["comparison"].as_array().unwrap() {
        assert_eq!(row["primary_time"], "implementation_time");
        assert_eq!(row["primary_time_status"], "complete");
        assert!(row["primary_time_ns"].is_u64());
        assert_eq!(row["profile"]["measurement_basis"], "worker_reported");
        wall_sum += row["timed_wall_time_ns"].as_u64().unwrap();
    }
    assert_eq!(report["fresh_sequences"][0]["consumed_time_ns"], wall_sum);
    let explicit_plan = invoke(stage.path(), &["plan", "--output", "json"]);
    assert!(explicit_plan.status.success(), "{explicit_plan:?}");
    edit(stage.path(), "metewand.yaml", |source| {
        let mut document: Value = serde_saphyr::from_str(&source).unwrap();
        for policy in document["observation-policies"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            policy.as_object_mut().unwrap().remove("max-observations");
        }
        serde_json::to_string_pretty(&document).unwrap()
    });
    let inferred_plan = invoke(stage.path(), &["plan", "--output", "json"]);
    assert!(inferred_plan.status.success(), "{inferred_plan:?}");
    assert_eq!(data(&inferred_plan), data(&explicit_plan));
    let (output, resumed) = run(stage.path(), &["--resume"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(resumed["summary"]["attempt_slots"], 0);
    assert_eq!(resumed["fresh_sequences"], report["fresh_sequences"]);
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
    let export = invoke(
        stage.path(),
        &[
            "export",
            "--output-dir",
            "run",
            "--destination",
            "analysis",
            "--format",
            "json",
            "--require-complete",
            "--output",
            "json",
        ],
    );
    assert!(export.status.success(), "{export:?}");
    let analysis: Value =
        serde_json::from_slice(&fs::read(stage.path().join("analysis/results.json")).unwrap())
            .unwrap();
    assert_eq!(analysis["observations"].as_array().unwrap().len(), 8);
    assert_eq!(analysis["profiles"].as_array().unwrap().len(), 3);
    assert!(
        analysis["observations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["completion"] == "not_reached")
    );
    assert_eq!(analysis["completeness"]["complete"], true);
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
        crate::core::public_schemas::public_schema_catalog()
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
        report["summary"]["profile_outcomes"]["groups"][0]["partial"],
        1
    );
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
    let (output, report) = run(stage.path(), &["--resume"]);
    assert_eq!(output.status.code(), Some(5), "{report}");
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
    assert!(output.status.success(), "{report}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(report["attempts"][0]["status"], "censored");
    assert_eq!(report["fresh_sequences"][0]["consumed_time_ns"], 20_000_000);
    let archived = invoke(
        stage.path(),
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "saved.tar.gz",
        ],
    );
    assert!(archived.status.success(), "{archived:?}");
    fs::remove_dir_all(stage.path().join("run")).unwrap();
    let restored = invoke(
        stage.path(),
        &[
            "archive",
            "restore",
            "--archive",
            "saved.tar.gz",
            "--destination",
            "run",
        ],
    );
    assert!(restored.status.success(), "{restored:?}");
    let (output, resume) = run(stage.path(), &["--resume", "--retry"]);
    assert!(output.status.success(), "{resume}");
    assert_eq!(
        resume["summary"]["profile_outcomes"],
        report["summary"]["profile_outcomes"]
    );
    assert!(resume["attempts"].as_array().unwrap().is_empty());
    assert_eq!(resume["fresh_sequences"][0]["consumed_time_ns"], 20_000_000);
    let export = invoke(
        stage.path(),
        &[
            "export",
            "--output-dir",
            "run",
            "--destination",
            "analysis",
            "--format",
            "json",
            "--require-complete",
            "--output",
            "json",
        ],
    );
    assert_eq!(export.status.code(), Some(5), "{export:?}");
    assert!(!stage.path().join("analysis").exists());
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
            s.replace("    runner: python\n    protocol-transport: socket\n    entrypoint: worker.py\n    sources: [worker.py, solver.py, \"metewand/*.py\"]",&format!("    runner: command\n    program: python3\n    args: [\"{{sources}}/raw.py\", {mode}]\n    sources: [raw.py]"))
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
    assert_eq!(report["summary"]["profile_outcomes"]["status"], "paused");
    let (output, report) = run(stage.path(), &["--resume"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["summary"]["attempt_slots"], 3);
}

fn mixed_delivery(restore_subset: bool) {
    let stage = stage(false);
    fs::write(
        stage.path().join("buffered.py"),
        r#"from metewand import serve_implementation
class Solver:
    def prepare(self, context): return self
    def reset(self, state, context): pass
    def execute_buffered(self, state, context):
        trace = context.observer
        progress = {'check_index':0,'work':{}}
        trace.observe(progress, lambda:{'residual':0.04}, lambda:{'x':1.4})
        trace.finish(progress, lambda:{'residual':0.04}, lambda:{'x':1.4})
worker = Solver()
serve_implementation(prepare=worker.prepare, execute_buffered=worker.execute_buffered, reset=worker.reset)
"#,
    )
    .unwrap();
    edit(stage.path(), "metewand.yaml", |s| {
        s.replace("experiments:\n",r#"  buffered:
    runner: python
    entrypoint: buffered.py
    protocol-transport: socket
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
    let saved_observations = if restore_subset {
        let directory = stage.path().join("run");
        fs::create_dir(&directory).unwrap();
        drop(crate::runtime::run_store::RunStore::open_fresh(&directory).unwrap());
        let (output, report) = run(stage.path(), &["--implementation", "buffered"]);
        assert!(output.status.success(), "{output:?}\n{report}");
        let status = data(&invoke(
            stage.path(),
            &["status", "--output-dir", "run", "--output", "json"],
        ));
        let archived = invoke(
            stage.path(),
            &[
                "archive",
                "create",
                "--output-dir",
                "run",
                "--destination",
                "subset.tar.gz",
            ],
        );
        assert!(archived.status.success(), "{archived:?}");
        fs::remove_dir_all(directory).unwrap();
        let restored = invoke(
            stage.path(),
            &[
                "archive",
                "restore",
                "--archive",
                "subset.tar.gz",
                "--destination",
                "run",
            ],
        );
        assert!(restored.status.success(), "{restored:?}");
        status["observations"].as_array().unwrap().clone()
    } else {
        vec![]
    };
    let (output, report) = run(stage.path(), &[]);
    assert!(output.status.success(), "{output:?}\n{report}");
    assert_eq!(report["fresh_sequences"].as_array().unwrap().len(), 2);
    if restore_subset {
        assert!(!saved_observations.is_empty());
        assert!(report["summary"]["accepted_attempts"].as_u64().unwrap() > 0);
        assert!(
            report["fresh_sequences"]
                .as_array()
                .unwrap()
                .iter()
                .all(|sequence| sequence["status"] == "accepted")
        );
        let status = data(&invoke(
            stage.path(),
            &["status", "--output-dir", "run", "--output", "json"],
        ));
        let observations = status["observations"].as_array().unwrap();
        assert!(
            saved_observations
                .iter()
                .all(|observation| observations.contains(observation))
        );
    } else {
        assert!(
            report["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["observations"].as_array().is_some_and(|o| !o.is_empty()))
        );
    }
}

#[test]
fn one_policy_can_mix_fresh_and_buffered_delivery() {
    mixed_delivery(false);
}

#[test]
fn archive_restore_can_continue_a_subset_with_another_profile_delivery() {
    mixed_delivery(true);
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
    let outcomes = &report["summary"]["profile_outcomes"];
    assert_eq!(outcomes["groups"][0]["target"]["reached"], 2);
    assert_eq!(outcomes["groups"][0]["failed"], 2);
    let (output, report) = run(stage.path(), &["--resume", "--retry"]);
    assert_eq!(output.status.code(), Some(5), "{output:?}\n{report}");
    assert_eq!(report["summary"]["profile_outcomes"], *outcomes);
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
fn human_profile_summary_uses_current_experiment_and_saved_outcomes() {
    let stage = stage(false);
    let root = stage.path();
    edit(root, "metewand.yaml", |source| {
        source.replace("threshold: 0.000001", "threshold: 0.00000000000000000001")
    });
    let (output, report) = run(root, &["--experiment", "target"]);
    assert!(output.status.success(), "{report}");
    assert_eq!(
        report["summary"]["profile_outcomes"]["groups"][0]["censored"],
        2
    );

    for _ in 0..2 {
        let output = invoke(
            root,
            &["run", "--output-dir", "run", "--experiment", "fixed"],
        );
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("Benchmarks complete"), "{text}");
        assert!(text.contains("Profiles: 1 accepted, 0 censored"), "{text}");
        assert!(!text.contains("2 censored"), "{text}");
        assert!(!text.contains("Target attainment"), "{text}");
    }
}

#[test]
fn concurrent_invocations_leave_owned_repetitions_for_their_owner() {
    let stage = stage(false);
    fixed_only(stage.path());
    edit(stage.path(), "metewand.yaml", |s| {
        // This barrier tests ownership, so let the operational deadline bound
        // the competing invocation's startup instead of the example's budgets.
        s.replace("warmup-runs: 1", "warmup-runs: 0")
            .replace("    timeout: 5 s\n", "")
            .replace("max-active-time: 10 s", "max-active-time: 1 min")
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
    assert_eq!(output.status.code(), Some(5), "{output:?}\n{report}");
    assert_eq!(
        report["summary"]["profile_outcomes"]["status"],
        "incomplete"
    );
    assert!(report["attempts"].as_array().unwrap().is_empty());
    assert_eq!(report["fresh_sequences"][0]["status"], "partial");
    let (output, report) = run(stage.path(), &["--resume"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(report["fresh_sequences"][0]["status"], "accepted");
}

#[test]
fn interrupted_fresh_work_preserves_archives_and_scientific_budget() {
    let stage = stage(false);
    let repo = stage.path();
    let signals = TempDir::new().unwrap();
    let gate = signals.path().join("gate");
    let marker = signals.path().join("pid");
    edit(repo, "metewand.yaml", |source| {
        source
            .replacen(
                "    max-active-time: 10 s",
                "    adaptive-budget: {initial-pilot-time: 20 s}",
                1,
            )
            .replace("warmup-runs: 1", "warmup-runs: 0")
            .replace(
                "timing-scope: prepare_and_execute",
                "timing-scope: execute_only",
            )
            .replace("timeout: 5 s", "timeout: 30 s")
    });
    edit(repo, "worker.py", |source| {
        source.replace("        observation_control =", &format!(
            "        import time\n        from pathlib import Path\n        if Path({gate:?}).exists():\n            Path({marker:?}).write_text(str(os.getpid()))\n            time.sleep(30)\n        observation_control ="
        ))
    });
    fs::write(&gate, "").unwrap();
    super::archives::interrupt_runner(repo, &marker, &["--shared", "target"], 1);
    fs::remove_file(&gate).unwrap();
    for args in [
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "pilot.tar.gz",
        ][..],
        &[
            "archive",
            "restore",
            "--archive",
            "pilot.tar.gz",
            "--destination",
            "restored",
        ],
        &["run", "target", "--output-dir", "restored"],
    ] {
        let output = invoke(repo, args);
        assert!(output.status.success(), "{args:?}: {output:?}");
    }

    fs::rename(repo.join("run"), repo.join("pilot-run")).unwrap();
    fs::write(&gate, "").unwrap();
    fs::remove_file(&marker).unwrap();
    super::archives::interrupt_runner(repo, &marker, &["fixed"], 1);
    fs::remove_file(&gate).unwrap();
    let status = invoke(repo, &["status", "--output-dir", "run", "--output", "json"]);
    assert!(status.status.success(), "{status:?}");
    let status = data(&status);
    let document = fs::read_dir(repo.join("run/artifacts"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("attempt.json"))
        .find(|path| path.is_file())
        .expect("interrupted attempt document");
    let record: Value = serde_json::from_slice(&fs::read(document).unwrap()).unwrap();
    let measurements = &record["provenance"]["data"]["measurements"];
    let timed = measurements["timed_wall_time_ns"]["value"]
        .as_u64()
        .unwrap();
    let request = measurements["execute_request_time_ns"]["value"]
        .as_u64()
        .unwrap();
    assert_eq!(measurements["timed_wall_time_ns"]["status"], "partial");
    // The request includes the executor's 100 ms termination grace period.
    // The scientific interval ends when the stop is observed.
    assert!(
        request.saturating_sub(timed) >= 80_000_000,
        "{measurements}"
    );
    assert_eq!(status["fresh_sequences"][0]["consumed_time_ns"], timed);
}
