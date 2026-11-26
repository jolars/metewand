#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

use super::support::export_fidelity;

use crate::native::buffered_worker as native;

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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let stage = TempDir::new().unwrap();
    copy(&root.join("examples/buffered-profile"), stage.path());
    copy(
        &root.join("packages/python/src/metewand"),
        &stage.path().join("metewand"),
    );
    stage
}

#[test]
fn buffered_profiles_keep_their_declared_instrumented_clock() {
    let stage = stage();
    let path = stage.path().join("metewand.yaml");
    let source = fs::read_to_string(&path)
        .unwrap()
        .replace(
            "primary-time: timed_wall_time",
            "primary-time: implementation_time",
        )
        .replace(
            "timing-scope: prepare_and_execute",
            "timing-scope: execute_only",
        );
    fs::write(path, source).unwrap();
    let output = invoke(stage.path(), &["check", "--output", "json"]);
    assert!(!output.status.success());
    let message = String::from_utf8(output.stdout).unwrap();
    assert!(
        message.contains("buffered and streaming profiles use their declared profile clocks"),
        "{message}"
    );
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
    use crate::core::public_schemas::{PublicSchema, public_schema_catalog};
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

fn adaptive_stage(initial: &str, ceiling: &str) -> TempDir {
    let stage = stage();
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_saphyr::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    manifest["experiments"].as_array_mut().unwrap().truncate(1);
    manifest["execution-policies"]["local"]["timing-scope"] = json!("execute_only");
    // A deliberately short declared watchdog must be extended for calibration.
    manifest["worker-timeouts"] = json!({"execute":"1 ms"});
    let policy = &mut manifest["observation-policies"]["accuracy"];
    policy["max-active-time"] = json!(ceiling);
    policy["adaptive-budget"] = json!({
        "initial-pilot-time":initial,"growth-factor":2,"multiplier":11,
        "minimum-time":"2 ms","fallback-time":ceiling,"watchdog-grace":"1 s"
    });
    fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let path = stage.path().join("worker.py");
    let source = fs::read_to_string(&path).unwrap()
        .replace("index = 0", "index = 0\n        trace.clock = lambda: index * (1_000_000 if trace.profile['instrumentation']['recording'] == 'direct' else 2_000_000)")
        .replace("x *= 0.5", "__import__('time').sleep(0.004)\n            x *= 0.5");
    fs::write(path, source).unwrap();
    stage
}

fn adaptive_defaults_stage() -> TempDir {
    let stage = adaptive_stage("16 ms", "100 ms");
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let profile = &mut manifest["observation-policies"]["accuracy"];
    profile.as_object_mut().unwrap().remove("max-active-time");
    profile["adaptive-budget"] = json!({});
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    stage
}

#[test]
fn adaptive_defaults_discover_a_limit_without_a_configured_ceiling() {
    let stage = adaptive_defaults_stage();
    let plan = invoke(stage.path(), &["plan", "--verbose"]);
    assert!(plan.status.success(), "{plan:?}");
    let text = String::from_utf8(plan.stdout).unwrap();
    assert!(text.contains("ceiling unbounded"), "{text}");
    let run = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let report = run_data(&run);
    assert!(run.status.success(), "{report}");
    let decision = &report["adaptive_budgets"][0];
    assert_eq!(decision["ceiling_limit_ns"], Value::Null);
    assert_eq!(decision["multiplier"], 11);
    assert_eq!(decision["minimum_limit_ns"], 1_000_000);
    assert_eq!(decision["effective_limit_ns"], 77_000_000);
    assert_eq!(decision["pilots"].as_array().unwrap().len(), 2);
    assert_eq!(decision["pilots"][0]["limit_ns"], 1_000_000_000);
    assert_eq!(report["summary"]["accepted_attempts"], 4);
    let resumed = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let resumed = run_data(&resumed);
    assert_eq!(resumed["adaptive_budgets"], report["adaptive_budgets"]);
    assert_eq!(resumed["summary"]["attempt_slots"], 0);
}

#[test]
fn unbounded_discovery_keeps_growing_without_freezing_a_fallback() {
    let stage = adaptive_defaults_stage();
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["observation-policies"]["accuracy"]["adaptive-budget"]["initial-pilot-time"] =
        json!("1 ms");
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let run = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "4",
            "--output",
            "json",
        ],
    );
    let report = run_data(&run);
    assert!(report.get("adaptive_budgets").is_none(), "{report}");
    assert!(report["attempts"].as_array().unwrap().is_empty());
    let pilots = report["calibration_attempts"].as_array().unwrap();
    assert_eq!(pilots.len(), 4);
    assert_eq!(pilots[0]["pilot"]["limit_ns"], 1_000_000);
    assert_eq!(pilots[2]["pilot"]["limit_ns"], 2_000_000);
    let resume = invoke(
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
    let resume = run_data(&resume);
    assert!(resume.get("adaptive_budgets").is_none(), "{resume}");
    assert_eq!(
        resume["calibration_attempts"][0]["pilot"]["limit_ns"],
        4_000_000
    );
}

#[test]
fn adaptive_calibration_stops_when_every_solver_finishes_before_the_target() {
    for shared in [false, true] {
        let stage = adaptive_defaults_stage();
        let path = stage.path().join("metewand.yaml");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for name in ["observer", "direct"] {
            manifest["implementations"][name]["parameter-defaults"] = json!({"maximum_updates":0});
        }
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut args = vec![
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "4",
            "--output",
            "json",
        ];
        if shared {
            args.push("--shared");
        }
        let output = invoke(stage.path(), &args);
        assert_eq!(output.status.code(), Some(5), "{output:?}");
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        crate::core::public_schemas::public_schema_catalog()
            .unwrap()
            .validate(
                crate::core::public_schemas::PublicSchema::RunOutput.repository_path(),
                &envelope,
            )
            .unwrap();
        let diagnostic = data(&output);
        assert_eq!(
            diagnostic["code"], "adaptive_calibration_stopped",
            "{diagnostic}"
        );
        assert!(
            diagnostic["remediation"]
                .as_str()
                .unwrap()
                .contains("iteration")
        );
        let pilots = diagnostic["details"]["pilots"].as_array().unwrap();
        assert_eq!(pilots.len(), 2);
        for pilot in pilots {
            assert_eq!(pilot["status"], "censored");
            assert_eq!(pilot["reason"], "implementation_stopped");
            assert!(pilot["implementation"].is_string());
            assert!(
                stage
                    .path()
                    .join("run")
                    .join(pilot["attempt_directory"].as_str().unwrap())
                    .join("attempt.json")
                    .is_file()
            );
        }
        let before = data(&invoke(
            stage.path(),
            &["status", "--output-dir", "run", "--output", "json"],
        ));
        assert_eq!(before["summary"]["calibration_attempts"], 2);
        assert_eq!(before["summary"]["accepted_attempts"], 0);
        assert!(before.get("adaptive_budgets").is_none());
        let resumed = invoke(
            stage.path(),
            &["run", "--output-dir", "run", "--output", "json"],
        );
        assert_eq!(resumed.status.code(), Some(5));
        assert_eq!(data(&resumed), diagnostic);
        let after = data(&invoke(
            stage.path(),
            &["status", "--output-dir", "run", "--output", "json"],
        ));
        assert_eq!(after["summary"], before["summary"]);
        if shared {
            let archive = invoke(
                stage.path(),
                &[
                    "archive",
                    "create",
                    "--output-dir",
                    "run",
                    "--destination",
                    "stopped.tar.gz",
                    "--output",
                    "json",
                ],
            );
            assert!(archive.status.success(), "{archive:?}");
            let restore = invoke(
                stage.path(),
                &[
                    "archive",
                    "restore",
                    "--archive",
                    "stopped.tar.gz",
                    "--destination",
                    "restored",
                    "--output",
                    "json",
                ],
            );
            assert!(restore.status.success(), "{restore:?}");
            let restored = invoke(
                stage.path(),
                &["run", "--output-dir", "restored", "--output", "json"],
            );
            assert_eq!(restored.status.code(), Some(5), "{restored:?}");
            assert_eq!(data(&restored), diagnostic);
            let status = data(&invoke(
                stage.path(),
                &["status", "--output-dir", "restored", "--output", "json"],
            ));
            assert_eq!(status["summary"]["calibration_attempts"], 2);
            assert!(status.get("adaptive_budgets").is_none());
        }
    }
}

#[test]
fn adaptive_calibration_can_continue_when_only_one_solver_needs_more_time() {
    let stage = adaptive_stage("1 ms", "100 ms");
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["implementations"]["observer"]["parameter-defaults"] = json!({"maximum_updates":0});
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let output = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let report = run_data(&output);
    assert_eq!(report["summary"]["accepted_attempts"], 2, "{report}");
    assert_eq!(report["summary"]["censored_attempts"], 2);
    assert_eq!(
        report["adaptive_budgets"][0]["effective_limit_ns"],
        77_000_000
    );
    assert_eq!(
        report["adaptive_budgets"][0]["pilots"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
}

#[test]
fn adaptive_calibration_reports_worker_failures_without_growing_the_budget() {
    let stage = adaptive_defaults_stage();
    let path = stage.path().join("worker.py");
    let source = fs::read_to_string(&path).unwrap().replace(
        "        trace = context.observer",
        "        raise RuntimeError('fixture solver failed')\n        trace = context.observer",
    );
    fs::write(path, source).unwrap();
    let output = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "4",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let diagnostic = data(&output);
    assert_eq!(
        diagnostic["code"], "adaptive_calibration_stopped",
        "{diagnostic}"
    );
    let pilots = diagnostic["details"]["pilots"].as_array().unwrap();
    assert_eq!(pilots.len(), 2);
    for pilot in pilots {
        assert_eq!(pilot["status"], "failed");
        assert_eq!(pilot["reason"], "implementation_failure");
        assert_eq!(pilot["round"], 0);
    }
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["calibration_attempts"], 2);
    assert_eq!(status["summary"]["accepted_attempts"], 0);
    assert!(status.get("adaptive_budgets").is_none());
}

#[test]
fn adaptive_calibration_diagnoses_collection_bounds_as_distinct_from_time() {
    let stage = adaptive_defaults_stage();
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["observation-policies"]["accuracy"]["max-trace-points"] = json!(2);
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let output = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "4",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let diagnostic = data(&output);
    assert_eq!(
        diagnostic["code"], "adaptive_calibration_stopped",
        "{diagnostic}"
    );
    let pilots = diagnostic["details"]["pilots"].as_array().unwrap();
    assert_eq!(pilots.len(), 2);
    for pilot in pilots {
        assert_eq!(pilot["status"], "censored");
        assert_eq!(pilot["reason"], "trace_limit");
    }
}

#[test]
fn adaptive_budget_completes_pilots_and_excludes_them_from_measurements() {
    let stage = adaptive_stage("16 ms", "100 ms");
    let plan = invoke(stage.path(), &["plan", "--verbose"]);
    assert!(plan.status.success(), "{plan:?}");
    let text = String::from_utf8(plan.stdout).unwrap();
    for expected in [
        "scientific timeout=5 s",
        "worker execute watchdog=1000000 ns",
        "profile active limit=100 ms",
        "adaptive calibration:",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    let run = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let report = run_data(&run);
    assert!(run.status.success(), "{report}");
    let decision = &report["adaptive_budgets"][0];
    assert_eq!(
        decision["pilots"].as_array().unwrap().len(),
        2,
        "{decision}"
    );
    assert_eq!(decision["reference_time_ns"], 7_000_000, "{decision}");
    assert_eq!(decision["effective_limit_ns"], 77_000_000);
    assert_eq!(report["summary"]["accepted_attempts"], 4);
    assert_eq!(report["comparison"].as_array().unwrap().len(), 12);
    for attempt in report["attempts"].as_array().unwrap() {
        let persisted = document(
            stage
                .path()
                .join("run")
                .join(attempt["attempt_directory"].as_str().unwrap())
                .join("attempt.json"),
        );
        let context = &persisted["provenance"]["data"]["context"];
        assert_eq!(context["adaptive_budget"], *decision);
        assert_eq!(context["scientific_timeout_ns"], "1077000000");
    }
    let status = invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    );
    let status = data(&status);
    assert_eq!(status["summary"]["accepted_attempts"], 4);
    assert_eq!(status["summary"]["calibration_attempts"], 2);
    assert_eq!(status["adaptive_budgets"], report["adaptive_budgets"]);
    let explained = invoke(
        stage.path(),
        &[
            "explain",
            decision["group_id"].as_str().unwrap(),
            "--output-dir",
            "run",
            "--output",
            "json",
        ],
    );
    assert!(explained.status.success(), "{explained:?}");
    assert_eq!(data(&explained)["subject"]["record"], *decision);
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
    let analysis = document(stage.path().join("analysis/results.json"));
    assert_eq!(analysis["attempts"].as_array().unwrap().len(), 4);
    assert_eq!(analysis["adaptive_budgets"], report["adaptive_budgets"]);
    let resume = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--retry", "--output", "json"],
    );
    let resumed = run_data(&resume);
    assert!(resume.status.success(), "{resumed}");
    assert_eq!(resumed["summary"]["attempt_slots"], 0);
    assert_eq!(resumed["adaptive_budgets"], report["adaptive_budgets"]);

    let archive = invoke(
        stage.path(),
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "saved.tar.gz",
            "--output",
            "json",
        ],
    );
    assert!(archive.status.success(), "{archive:?}");
    fs::remove_dir_all(stage.path().join("run")).unwrap();
    let restored = invoke(
        stage.path(),
        &[
            "archive",
            "restore",
            "--archive",
            "saved.tar.gz",
            "--destination",
            "restored",
            "--output",
            "json",
        ],
    );
    assert!(restored.status.success(), "{restored:?}");
    let restored = invoke(
        stage.path(),
        &["run", "--output-dir", "restored", "--output", "json"],
    );
    assert!(restored.status.success(), "{restored:?}");
    assert_eq!(
        run_data(&restored)["adaptive_budgets"],
        report["adaptive_budgets"]
    );
}

#[test]
fn adaptive_reference_does_not_depend_on_serial_method_order() {
    let stage = adaptive_stage("16 ms", "100 ms");
    let first = invoke(
        stage.path(),
        &["run", "--output-dir", "first", "--output", "json"],
    );
    let first = run_data(&first);
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["experiments"][0]["implementations"]
        .as_array_mut()
        .unwrap()
        .reverse();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let second = invoke(
        stage.path(),
        &["run", "--output-dir", "second", "--output", "json"],
    );
    let second = run_data(&second);
    for field in ["group_id", "reference_time_ns", "effective_limit_ns"] {
        assert_eq!(
            first["adaptive_budgets"][0][field], second["adaptive_budgets"][0][field],
            "{field}"
        );
    }
}

#[test]
fn adaptive_target_crossings_after_the_limit_remain_censored() {
    let stage = adaptive_stage("13 ms", "13 ms");
    let run = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let report = run_data(&run);
    assert!(run.status.success(), "{report}");
    let group = &report["summary"]["profile_outcomes"]["groups"][0];
    assert_eq!(group["target"]["reached"], 2);
    assert_eq!(group["censoring_reasons"]["active_time_limit"], 2);
    assert_eq!(
        report["adaptive_budgets"][0]["reference_time_ns"], 7_000_000,
        "{report}"
    );
    assert_eq!(report["summary"]["accepted_attempts"], 2);
    assert_eq!(report["summary"]["censored_attempts"], 2);
    assert_eq!(report["summary"]["failed_attempts"], 0);
    for attempt in report["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["status"] == "censored")
    {
        assert_eq!(attempt["diagnostic"]["code"], "active_time_limit");
    }
}

#[test]
fn adaptive_reference_requires_independent_target_confirmation() {
    let stage = adaptive_stage("16 ms", "16 ms");
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for name in ["observer", "direct"] {
        manifest["implementations"][name]["parameter-defaults"] = json!({"local_bias":-1});
    }
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let run = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    assert_eq!(run.status.code(), Some(5), "{run:?}");
    let diagnostic = data(&run);
    assert_eq!(
        diagnostic["code"], "adaptive_calibration_stopped",
        "{diagnostic}"
    );
    for pilot in diagnostic["details"]["pilots"].as_array().unwrap() {
        assert_eq!(pilot["reason"], "local_target_unconfirmed");
    }
}

#[test]
fn adaptive_requirements_are_checked_without_execution() {
    for mixed_scope in [false, true] {
        let stage = adaptive_stage("16 ms", "100 ms");
        let path = stage.path().join("metewand.yaml");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if mixed_scope {
            let contract_path = stage.path().join("problem.yaml");
            let mut contract: Value =
                serde_saphyr::from_str(&fs::read_to_string(&contract_path).unwrap()).unwrap();
            contract["instrumented-scopes"]["another_scope"] =
                contract["instrumented-scopes"]["complete_solver_call"].clone();
            fs::write(contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();
            manifest["observation-policies"]["accuracy"]["implementations"]["direct"]["instrumented-scope"] =
                json!("another_scope");
        } else {
            manifest["execution-policies"]["local"]["timing-scope"] = json!("prepare_and_execute");
        }
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let check = invoke(stage.path(), &["check", "--output", "json"]);
        assert!(!check.status.success(), "{check:?}");
        let text = String::from_utf8(check.stdout).unwrap();
        assert!(
            text.contains(if mixed_scope {
                "same named instrumented scope"
            } else {
                "execute_only"
            }),
            "{text}"
        );
        assert!(!stage.path().join(".metewand").exists());
    }
}

#[test]
fn adaptive_id_selectors_preserve_the_filtered_calibration_cohort() {
    let stage = adaptive_stage("16 ms", "100 ms");
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["experiments"][0]["implementation-repetitions"] = json!(2);
    manifest["experiments"][0]["measurement-repetitions"] = json!(1);
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let plan = data(&invoke(stage.path(), &["plan", "--output", "json"]));
    let candidate = &plan["experiments"][0]["candidates"][0];
    let logical_slot = candidate["specifications"][1]["attempt_slots"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap();
    let baseline = invoke(
        stage.path(),
        &["run", "--output-dir", "baseline", "--output", "json"],
    );
    let baseline = run_data(&baseline);
    let attempt = baseline["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|attempt| attempt["logical_slot_id"] == logical_slot)
        .unwrap();
    assert_eq!(
        baseline["adaptive_budgets"][0]["effective_limit_ns"],
        77_000_000
    );
    for (index, selectors, attempts) in [
        (0, vec![logical_slot], 1),
        (1, vec![attempt["resolved_slot_id"].as_str().unwrap()], 1),
        (2, vec![candidate["id"].as_str().unwrap()], 2),
        (
            3,
            vec![logical_slot, attempt["resolved_slot_id"].as_str().unwrap()],
            1,
        ),
    ] {
        let directory = format!("selection-{index}");
        let mut args = vec!["run", "--output-dir", &directory, "--output", "json"];
        for selector in selectors {
            args.extend(["--select", selector]);
        }
        let selected = invoke(stage.path(), &args);
        let report = run_data(&selected);
        assert!(selected.status.success(), "{report}");
        assert_eq!(report["summary"]["accepted_attempts"], attempts, "{report}");
        assert_eq!(
            report["adaptive_budgets"][0]["group_id"], baseline["adaptive_budgets"][0]["group_id"],
            "{report}"
        );
        assert_eq!(
            report["adaptive_budgets"][0]["effective_limit_ns"],
            baseline["adaptive_budgets"][0]["effective_limit_ns"]
        );
        let selected_attempt = report["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attempt| attempt["logical_slot_id"] == logical_slot)
            .unwrap();
        assert_eq!(
            selected_attempt["resolved_slot_id"],
            attempt["resolved_slot_id"]
        );
    }
}

#[test]
fn adaptive_selected_attempts_remain_selectable_on_resume_and_retry() {
    let stage = adaptive_stage("2 ms", "4 ms");
    let plan = data(&invoke(stage.path(), &["plan", "--output", "json"]));
    let logical_slot =
        plan["experiments"][0]["candidates"][0]["specifications"][0]["attempt_slots"][1]
            .as_str()
            .unwrap();
    let first = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--select",
            logical_slot,
            "--output",
            "json",
        ],
    );
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let first = run_data(&first);
    assert_eq!(first["summary"]["censored_attempts"], 1, "{first}");
    let attempt = &first["attempts"][0];
    for selector in [
        attempt["attempt_id"].as_str().unwrap(),
        attempt["observations"][0]["observation_id"]
            .as_str()
            .unwrap(),
    ] {
        let resumed = invoke(
            stage.path(),
            &[
                "run",
                "--output-dir",
                "run",
                "--select",
                selector,
                "--output",
                "json",
            ],
        );
        let report = run_data(&resumed);
        assert!(resumed.status.success(), "{report}");
        assert_eq!(report["adaptive_budgets"], first["adaptive_budgets"]);
        assert_eq!(report["summary"]["attempt_slots"], 0);
        assert_eq!(
            report["summary"]["profile_outcomes"],
            first["summary"]["profile_outcomes"]
        );
        let retried = invoke(
            stage.path(),
            &[
                "run",
                "--output-dir",
                "run",
                "--select",
                selector,
                "--retry",
                "--output",
                "json",
            ],
        );
        let report = run_data(&retried);
        assert_eq!(retried.status.code(), Some(0), "{report}");
        assert_eq!(report["adaptive_budgets"], first["adaptive_budgets"]);
        assert!(report.get("calibration_attempts").is_none(), "{report}");
        assert_eq!(report["summary"]["censored_attempts"], 1, "{report}");
        assert_eq!(
            report["attempts"][0]["resolved_slot_id"],
            attempt["resolved_slot_id"]
        );
        assert_ne!(report["attempts"][0]["attempt_id"], attempt["attempt_id"]);
    }
}

#[test]
fn adaptive_cohort_changes_cannot_reuse_measurements_with_another_budget() {
    let stage = adaptive_stage("16 ms", "100 ms");
    let first = invoke(
        stage.path(),
        &[
            "run",
            "--implementation",
            "observer",
            "--output-dir",
            "run",
            "--output",
            "json",
        ],
    );
    let first = run_data(&first);
    assert_eq!(first["summary"]["accepted_attempts"], 2, "{first}");
    assert_eq!(
        first["adaptive_budgets"][0]["effective_limit_ns"],
        100_000_000
    );
    let broader = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let broader = run_data(&broader);
    assert_eq!(
        broader["adaptive_budgets"][0]["effective_limit_ns"], 77_000_000,
        "{broader}"
    );
    assert_eq!(broader["summary"]["accepted_attempts"], 4);
    assert_ne!(
        first["adaptive_budgets"][0]["group_id"],
        broader["adaptive_budgets"][0]["group_id"]
    );
    let resume = invoke(
        stage.path(),
        &[
            "run",
            "--implementation",
            "observer",
            "--output-dir",
            "run",
            "--output",
            "json",
        ],
    );
    let resume = run_data(&resume);
    assert_eq!(resume["adaptive_budgets"], first["adaptive_budgets"]);
    assert_eq!(resume["summary"]["attempt_slots"], 0);
}

#[test]
fn adaptive_pilot_rounds_resume_before_measurement_and_grow_until_verified() {
    for (initial, count) in [("4 ms", 4), ("16 ms", 2)] {
        let stage = adaptive_stage(initial, "100 ms");
        let first = invoke(
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
        let first = run_data(&first);
        assert!(first.get("adaptive_budgets").is_none(), "{first}");
        assert!(first["attempts"].as_array().unwrap().is_empty());
        assert_eq!(first["calibration_attempts"].as_array().unwrap().len(), 1);
        let next = invoke(
            stage.path(),
            &["run", "--output-dir", "run", "--output", "json"],
        );
        let report = run_data(&next);
        assert!(next.status.success(), "{report}");
        let decision = &report["adaptive_budgets"][0];
        assert_eq!(
            decision["pilots"].as_array().unwrap().len(),
            count,
            "{decision}"
        );
        assert_eq!(decision["effective_limit_ns"], 77_000_000);
        assert_eq!(decision["pilots"][0]["round"], 0);
        if count == 4 {
            assert_eq!(decision["pilots"][2]["round"], 1);
        }
        let status = data(&invoke(
            stage.path(),
            &["status", "--output-dir", "run", "--output", "json"],
        ));
        assert_eq!(status["summary"]["calibration_attempts"], count);
    }
}

#[test]
fn adaptive_no_convergence_uses_a_frozen_fallback_and_keeps_censoring() {
    use crate::core::public_schemas::PublicSchema;
    let stage = adaptive_stage("2 ms", "4 ms");
    let run = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--shared", "--output", "json"],
    );
    let report = run_data(&run);
    assert!(run.status.success(), "{report}");
    let outcomes = &report["summary"]["profile_outcomes"];
    assert_eq!(outcomes["status"], "complete");
    assert_eq!(outcomes["groups"][0]["target"]["reached"], 0);
    assert_eq!(outcomes["groups"][0]["censored"], 4);
    let no_op = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    assert!(no_op.status.success(), "{no_op:?}");
    assert_eq!(run_data(&no_op)["summary"]["profile_outcomes"], *outcomes);
    let jsonl = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "jsonl"],
    );
    assert!(jsonl.status.success(), "{jsonl:?}");
    let lines = String::from_utf8(jsonl.stdout).unwrap();
    let catalog = crate::core::public_schemas::public_schema_catalog().unwrap();
    let mut last = Value::Null;
    for line in lines.lines() {
        last = serde_json::from_str(line).unwrap();
        catalog
            .validate(PublicSchema::RunOutput.repository_path(), &last)
            .unwrap();
    }
    assert_eq!(last["data"]["summary"]["profile_outcomes"], *outcomes);
    let human = invoke(stage.path(), &["run", "--output-dir", "run"]);
    assert!(human.status.success(), "{human:?}");
    let summary = String::from_utf8(human.stdout).unwrap();
    assert!(summary.contains("Benchmarks complete"), "{summary}");
    assert!(summary.contains("active_time_limit: 4"), "{summary}");
    assert!(summary.contains("0/4 reached"), "{summary}");
    let decision = &report["adaptive_budgets"][0];
    assert_eq!(decision["fallback"], true, "{report}");
    assert_eq!(decision["reference_time_ns"], Value::Null);
    assert_eq!(decision["effective_limit_ns"], 4_000_000);
    assert_eq!(report["summary"]["censored_attempts"], 4);
    assert_eq!(report["summary"]["failed_attempts"], 0);
    let resume = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--retry", "--output", "json"],
    );
    let resumed = run_data(&resume);
    assert_eq!(resumed["adaptive_budgets"], report["adaptive_budgets"]);
    assert_eq!(resumed["summary"]["censored_attempts"], 4);
}

#[test]
fn profile_summary_uses_current_resolved_slots() {
    let stage = stage();
    let root = stage.path();
    let args = [
        "run",
        "--output-dir",
        "run",
        "--max-attempts",
        "1",
        "--output",
        "json",
    ];
    let first = invoke(root, &args);
    assert!(first.status.success(), "{first:?}");
    let first = run_data(&first);

    let path = root.join("worker.py");
    let source = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        format!("{source}\n# Change the resolved worker identity.\n"),
    )
    .unwrap();
    let second = invoke(root, &args);
    assert!(second.status.success(), "{second:?}");
    let second = run_data(&second);
    assert_ne!(
        first["attempts"][0]["resolved_slot_id"],
        second["attempts"][0]["resolved_slot_id"]
    );
    assert_eq!(
        second["summary"]["profile_outcomes"],
        first["summary"]["profile_outcomes"]
    );
    let status = data(&invoke(
        root,
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert!(
        status["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|attempt| attempt["id"] == first["attempts"][0]["attempt_id"])
    );
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
    let analysis = document(stage.path().join("analysis/results.json"));
    assert_eq!(analysis["observations"].as_array().unwrap().len(), 15);
    assert_eq!(analysis["profiles"].as_array().unwrap().len(), 5);
    assert_eq!(
        analysis["observations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["measurement_index"].as_u64().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        [0, 1].into_iter().collect()
    );
    assert!(
        analysis["observations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["instrumented_time_ns"].is_u64())
    );
    let path = stage.path().join("metewand.yaml");
    let mut manifest: Value = serde_saphyr::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    for policy in manifest["observation-policies"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        let binding = policy["implementations"]
            .as_object_mut()
            .unwrap()
            .remove("observer")
            .unwrap();
        policy["default-binding"] = binding;
    }
    fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let default_plan = invoke(stage.path(), &["plan", "--output", "json"]);
    assert!(default_plan.status.success(), "{default_plan:?}");
    assert_eq!(data(&default_plan), data(&plan));
    let resume = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--resume", "--output", "json"],
    );
    assert!(resume.status.success(), "{resume:?}");
    assert_eq!(run_data(&resume)["summary"]["attempt_slots"], 0);
    assert_eq!(
        run_data(&resume)["summary"]["profile_outcomes"],
        report["summary"]["profile_outcomes"]
    );
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
        assert_eq!(run.status.success(), !agreement, "{report}");
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
    let source = fs::read_to_string(&path).unwrap();
    let worker = "runner: python\n    protocol-transport: socket\n    entrypoint: worker.py\n    sources: [worker.py, \"metewand/*.py\"]";
    assert_eq!(
        source.matches(worker).count(),
        2,
        "expected both buffered worker definitions to match"
    );
    let executable = serde_json::to_string(&std::env::current_exe().unwrap()).unwrap();
    let source = source.replace(
        worker,
        &format!("runner: command\n    program: {executable}\n    args: [--exact, cli_tests::buffered_profiles::native_buffered_entry, --nocapture]"),
    );
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
