use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

#[allow(dead_code)]
#[path = "../../../examples/streaming-profile/main.rs"]
mod native;

#[test]
fn native_streaming_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some()
        || std::env::var_os("METEWAND_PROTOCOL_READ_HANDLE").is_some()
    {
        native::serve().unwrap();
    }
}

fn copy(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &destination.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
}
fn stage(native: bool) -> TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let stage = TempDir::new().unwrap();
    copy(&root.join("examples/streaming-profile"), stage.path());
    copy(
        &root.join("packages/python/src/metewand"),
        &stage.path().join("metewand"),
    );
    if native {
        fs::copy(
            std::env::current_exe().unwrap(),
            stage.path().join("native-worker"),
        )
        .unwrap();
        let mut manifest = fs::read_to_string(stage.path().join("native.yaml")).unwrap();
        manifest = manifest.replace(
            "    program: ./native-worker",
            &format!(
                "    program: {}\n    args: [--exact, native_streaming_entry, --nocapture]",
                serde_json::to_string(&std::env::current_exe().unwrap()).unwrap()
            ),
        );
        fs::write(stage.path().join("metewand.yaml"), manifest).unwrap();
    }
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
    let envelope: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    if envelope["data"]["command"] == "run" {
        metewand_core::public_schemas::public_schema_catalog()
            .unwrap()
            .validate(Path::new("schemas/v1/run-output.schema.json"), &envelope)
            .unwrap_or_else(|error| panic!("{error}: {envelope}"));
    }
    envelope["data"].clone()
}
fn document(path: PathBuf) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn run_first(root: &Path) -> Value {
    data(&invoke(
        root,
        &[
            "run",
            "--output-dir",
            "run",
            "--max-attempts",
            "1",
            "--output",
            "json",
        ],
    ))
}

#[test]
#[cfg(unix)]
fn streaming_control_loss_after_stop_prevents_acceptance_with_reuse() {
    let stage = stage(false);
    let manifest = stage.path().join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("    warmup-runs: 1", "    warmup-runs: 0"),
    )
    .unwrap();
    let worker = stage.path().join("worker.py");
    fs::write(
        &worker,
        fs::read_to_string(&worker).unwrap().replace(
            "if stream.checkpoint(progress, lambda: {\"x\": x}).stop:\n            return",
            "if stream.checkpoint(progress, lambda: {\"x\": x}).stop:\n            import os\n            os.chmod(os.getcwd(), 0o777)\n            return",
        ),
    )
    .unwrap();

    let report = run_first(stage.path());
    let attempt = &report["attempts"][0];
    assert_eq!(attempt["status"], "failed", "{report}");
    assert_eq!(
        attempt["diagnostic"]["code"], "control_enforcement_lost",
        "{report}"
    );
    assert_eq!(attempt["observations"].as_array().unwrap().len(), 5);
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["selected_observations"], 0);
}

#[test]
fn streaming_dataset_mutations_during_either_worker_shutdown_prevent_acceptance() {
    for role in ["implementation", "evaluator"] {
        let stage = stage(false);
        let manifest = stage.path().join("metewand.yaml");
        fs::write(
            &manifest,
            fs::read_to_string(&manifest)
                .unwrap()
                .replace("    worker-reuse: true", "    worker-reuse: false")
                .replace("    warmup-runs: 1", "    warmup-runs: 0"),
        )
        .unwrap();
        let worker = stage.path().join(if role == "implementation" {
            "worker.py"
        } else {
            "evaluator.py"
        });
        let source = fs::read_to_string(&worker).unwrap();
        let source = if role == "implementation" {
            source.replace(
                "reset=lambda state, context: None)",
                "reset=lambda state, context: None,\n                         shutdown=lambda state, context: (context.dataset_dir / 'mutated').write_text('changed'))",
            )
        } else {
            source.replace(
                "    x =",
                "    import atexit\n    atexit.register((context.dataset_dir / 'mutated').write_text, 'changed')\n    x =",
            )
        };
        fs::write(worker, source).unwrap();

        let report = run_first(stage.path());
        let attempt = &report["attempts"][0];
        assert_eq!(attempt["status"], "failed", "{role}: {report}");
        assert_eq!(
            attempt["diagnostic"]["code"], "dataset_mutation",
            "{role}: {report}"
        );
        assert!(
            attempt["diagnostic"]["message"]
                .as_str()
                .unwrap()
                .contains(&format!("{role} shutdown")),
            "{role}: {report}"
        );
        assert_eq!(attempt["observations"].as_array().unwrap().len(), 5);
        let status = data(&invoke(
            stage.path(),
            &["status", "--output-dir", "run", "--output", "json"],
        ));
        assert_eq!(status["summary"]["selected_observations"], 0);
    }
}

#[test]
fn streaming_work_counters_remain_monotonic_across_omissions() {
    for resumed_work in [2, 100, 101] {
        let stage = stage(false);
        let manifest = stage.path().join("metewand.yaml");
        fs::write(
            &manifest,
            fs::read_to_string(&manifest)
                .unwrap()
                .replace("    warmup-runs: 1", "    warmup-runs: 0"),
        )
        .unwrap();
        let worker = stage.path().join("worker.py");
        fs::write(
            &worker,
            fs::read_to_string(&worker).unwrap().replace(
                "\"work\": {\"updates\": index}",
                &format!("\"work\": [{{\"updates\": 100}}, {{}}, {{\"updates\": {resumed_work}}}][min(index, 2)]"),
            ),
        )
        .unwrap();

        let report = run_first(stage.path());
        let attempt = &report["attempts"][0];
        let observations = attempt["observations"].as_array().unwrap();
        assert_eq!(observations[0]["profile"]["work"], json!({"updates":100}));
        assert_eq!(observations[1]["profile"]["work"], json!({}));
        if resumed_work < 100 {
            assert_eq!(attempt["status"], "failed", "{report}");
            assert_eq!(attempt["diagnostic"]["code"], "invalid_stream", "{report}");
            assert_eq!(observations.len(), 2);
        } else {
            assert_eq!(attempt["status"], "accepted", "{report}");
            assert_eq!(observations.len(), 5);
            assert_eq!(
                observations[2]["profile"]["work"],
                json!({"updates":resumed_work})
            );
        }
    }
}

#[test]
fn python_and_native_streams_publish_evaluator_owned_curves_and_exports() {
    for native in [false, true] {
        let stage = stage(native);
        for args in [
            &["check", "--workers", "--output", "json"][..],
            &["plan", "--output", "json"][..],
        ] {
            let output = invoke(stage.path(), args);
            assert!(output.status.success(), "{output:?}");
        }
        let output = invoke(
            stage.path(),
            &["run", "--output-dir", "run", "--output", "json"],
        );
        let report = data(&output);
        assert!(output.status.success(), "{report}: {output:?}");
        assert_eq!(report["summary"]["accepted_attempts"], 3, "{report}");
        for attempt in report["attempts"].as_array().unwrap() {
            let observations = attempt["observations"].as_array().unwrap();
            assert!(matches!(observations.len(), 3 | 5));
            let mut previous = 0;
            for (sequence, observation) in observations.iter().enumerate() {
                let persisted = document(
                    stage
                        .path()
                        .join("run")
                        .join(observation["observation_directory"].as_str().unwrap())
                        .join("observation.json"),
                );
                assert_eq!(persisted["profile"]["delivery"], "streaming_profile");
                assert_eq!(persisted["profile"]["checkpoint_sequence"], sequence);
                assert_eq!(persisted["attempt_id"], attempt["attempt_id"]);
                let active = persisted["profile"]["active_time_ns"].as_u64().unwrap();
                assert!(active >= previous);
                previous = active;
                assert_eq!(persisted["timing"]["timed_wall_time_ns"], active);
                assert!(
                    persisted["profile"]["elapsed_wall_time_ns"]
                        .as_u64()
                        .unwrap()
                        >= active
                );
                assert_eq!(
                    persisted["provenance"]["data"]["measurements"]["version"],
                    3
                );
            }
        }
        let resume = data(&invoke(
            stage.path(),
            &["run", "--output-dir", "run", "--resume", "--output", "json"],
        ));
        assert_eq!(resume["summary"]["attempt_slots"], 0);
        let status = data(&invoke(
            stage.path(),
            &["status", "--output-dir", "run", "--output", "json"],
        ));
        assert_eq!(status["summary"]["selected_observations"], 13, "{status}");
        for format in ["jsonl", "csv"] {
            let output = invoke(
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
            assert!(output.status.success(), "{output:?}");
        }
        let lines = fs::read_to_string(stage.path().join("jsonl/results.jsonl")).unwrap();
        let rows: Vec<Value> = lines
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert!(
            rows.iter()
                .any(|r| r["profile"]["delivery"] == "streaming_profile"
                    && r["compatibility"]["profile"]["measurement_basis"] == "executor_observed")
        );
    }
}

#[test]
fn failed_stream_preserves_checkpoints_and_retry_selects_one_attempt() {
    let stage = stage(false);
    let manifest = fs::read_to_string(stage.path().join("metewand.yaml"))
        .unwrap()
        .replace("    worker-reuse: true", "    worker-reuse: false")
        .replace("    warmup-runs: 1", "    warmup-runs: 0");
    fs::write(stage.path().join("metewand.yaml"), manifest).unwrap();
    let worker = stage.path().join("worker.py");
    let marker = serde_json::to_string(&stage.path().join("failed-once")).unwrap();
    let code = fs::read_to_string(&worker).unwrap().replace("        progress =",&format!("        from pathlib import Path\n        marker = Path({marker})\n        if index == 2 and not marker.exists():\n            marker.write_text('failed')\n            raise RuntimeError('deliberate failure after publication')\n        progress ="));
    fs::write(worker, code).unwrap();
    let output = invoke(
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
    let first = data(&output);
    assert_eq!(first["summary"]["failed_attempts"], 1, "{first}");
    assert_eq!(
        first["attempts"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["selected_observations"], 0);
    let partial = invoke(
        stage.path(),
        &[
            "export",
            "--output-dir",
            "run",
            "--destination",
            "partial",
            "--format",
            "jsonl",
            "--output",
            "json",
        ],
    );
    assert!(partial.status.success(), "{partial:?}");
    let rows: Vec<Value> = fs::read_to_string(stage.path().join("partial/results.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let points: Vec<_> = rows
        .iter()
        .filter(|row| row["record_type"] == "observation")
        .collect();
    assert_eq!(points.len(), 2);
    assert!(
        points.iter().all(
            |row| row["selected"] == false && row["profile"]["delivery"] == "streaming_profile"
        )
    );
    let slot = first["attempts"][0]["resolved_slot_id"].as_str().unwrap();
    let output = invoke(
        stage.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--resume",
            "--retry",
            "--select",
            slot,
            "--output",
            "json",
        ],
    );
    let retry = data(&output);
    assert!(output.status.success(), "{retry}: {output:?}");
    assert_eq!(retry["summary"]["accepted_attempts"], 1, "{retry}");
    assert_eq!(
        retry["attempts"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let status = data(&invoke(
        stage.path(),
        &["status", "--output-dir", "run", "--output", "json"],
    ));
    assert_eq!(status["summary"]["selected_observations"], 5, "{status}");
    assert_ne!(
        first["attempts"][0]["attempt_id"],
        retry["attempts"][0]["attempt_id"]
    );
    assert_eq!(
        retry["attempts"][0]["observations"][0]["result"],
        json!({"x":1})
    );
}

#[test]
fn python_and_native_streams_share_the_finalized_checkpoint_byte_limit() {
    for native in [false, true] {
        for limit in [45, 46, 50] {
            let stage = stage(native);
            let manifest = stage.path().join("metewand.yaml");
            fs::write(
                &manifest,
                fs::read_to_string(&manifest)
                    .unwrap()
                    .replace("    warmup-runs: 1", "    warmup-runs: 0")
                    .replace("    threshold: 0.01", "    threshold: 1")
                    .replace(
                        "record-thresholds: [0.5, 0.05, 0.01]",
                        "record-thresholds: [1]",
                    )
                    .replace(
                        "max-checkpoint-bytes: 65536",
                        &format!("max-checkpoint-bytes: {limit}"),
                    ),
            )
            .unwrap();
            let report = run_first(stage.path());
            let attempt = &report["attempts"][0];
            if limit < 46 {
                assert_eq!(attempt["status"], "failed", "native={native}: {report}");
                assert_eq!(attempt["observations"].as_array().map_or(0, Vec::len), 0);
            } else {
                assert_eq!(attempt["status"], "accepted", "native={native}: {report}");
                assert_eq!(attempt["observations"].as_array().unwrap().len(), 1);
                assert_eq!(attempt["observations"][0]["result"], json!({"x":1}));
            }
        }
    }
}

#[test]
fn streaming_byte_limit_includes_finalized_artifact_metadata() {
    let stage = stage(false);
    let manifest = stage.path().join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("    warmup-runs: 1", "    warmup-runs: 0")
            .replace("max-checkpoint-bytes: 65536", "max-checkpoint-bytes: 150"),
    )
    .unwrap();
    let worker = stage.path().join("worker.py");
    fs::write(
        &worker,
        fs::read_to_string(&worker)
            .unwrap()
            .replace(
                "from metewand import serve_implementation",
                "from metewand import Artifact, Result, serve_implementation",
            )
            .replace(
                "        if stream.checkpoint(progress, lambda: {\"x\": x}).stop:",
                "        def snapshot():\n            (stream.output_dir / 'payload.txt').write_text('payload')\n            return Result({'x': x}, [Artifact('payload.txt', 'text/plain')])\n        if stream.checkpoint(progress, snapshot).stop:",
            ),
    )
    .unwrap();
    let report = run_first(stage.path());
    let attempt = &report["attempts"][0];
    assert_eq!(attempt["status"], "failed", "{report}");
    assert_eq!(attempt["diagnostic"]["code"], "invalid_stream", "{report}");
    assert!(
        attempt["diagnostic"]["message"]
            .as_str()
            .unwrap()
            .contains("checkpoint exceeds max-checkpoint-bytes"),
        "{report}"
    );
    assert_eq!(attempt["observations"].as_array().map_or(0, Vec::len), 0);
}

#[test]
fn streaming_bounds_preserve_partial_curves_and_distinguish_failures() {
    for case in ["early", "observations", "bytes", "time", "prepare"] {
        let stage = stage(false);
        let manifest = stage.path().join("metewand.yaml");
        let mut text = fs::read_to_string(&manifest)
            .unwrap()
            .replace("    worker-reuse: true", "    worker-reuse: false")
            .replace("    warmup-runs: 1", "    warmup-runs: 0");
        match case {
            "prepare" => {
                text = text.replace(
                    "timing-scope: prepare_and_execute",
                    "timing-scope: execute_only",
                );
                let worker = stage.path().join("worker.py");
                fs::write(
                    &worker,
                    fs::read_to_string(&worker).unwrap().replace(
                        "    return context.parameters[\"maximum_updates\"]",
                        "    raise RuntimeError('prepare failed')",
                    ),
                )
                .unwrap();
            }
            "early" => text = text.replace("default: 20", "default: 0"),
            "observations" => text = text.replace("max-observations: 8", "max-observations: 3"),
            "bytes" => {
                text = text.replace("max-checkpoint-bytes: 65536", "max-checkpoint-bytes: 1")
            }
            "time" => {
                text = text.replace("max-active-time: 2 s", "max-active-time: 1 s");
                let worker = stage.path().join("worker.py");
                fs::write(
                    &worker,
                    fs::read_to_string(&worker).unwrap().replace(
                        "        x *= 0.5",
                        "        import time\n        time.sleep(2)\n        x *= 0.5",
                    ),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        fs::write(&manifest, text).unwrap();
        let output = invoke(
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
        let report = data(&output);
        let attempt = &report["attempts"][0];
        let (status, reason, count) = match case {
            "prepare" => ("failed", "implementation_failure", 0),
            "early" => ("censored", "implementation_stopped", 1),
            "observations" => ("censored", "observation_limit", 3),
            "bytes" => ("failed", "resource_limit", 0),
            "time" => ("censored", "active_time_limit", 1),
            _ => unreachable!(),
        };
        assert_eq!(attempt["status"], status, "{case}: {report}");
        assert_eq!(attempt["diagnostic"]["code"], reason, "{case}: {report}");
        assert_eq!(
            attempt["observations"].as_array().map_or(0, Vec::len),
            count,
            "{case}: {report}"
        );
        let persisted = document(
            stage
                .path()
                .join("run")
                .join(attempt["attempt_directory"].as_str().unwrap())
                .join("attempt.json"),
        );
        assert_eq!(
            persisted["provenance"]["data"]["measurements"]["timed_wall_time_ns"]["status"],
            if case == "prepare" {
                "unavailable"
            } else if case == "time" || case == "bytes" {
                "partial"
            } else {
                "complete"
            }
        );
        if case == "prepare" {
            assert_eq!(
                persisted["provenance"]["data"]["measurements"]["cpu_time_ns"]["status"],
                "unavailable"
            );
        }
    }
}

#[test]
fn all_streaming_envelopes_exclude_long_evaluation_pauses() {
    for scope in ["prepare_and_execute", "execute_only", "cold_end_to_end"] {
        let stage = stage(false);
        let manifest = stage.path().join("metewand.yaml");
        let text = fs::read_to_string(&manifest)
            .unwrap()
            .replace(
                "timing-scope: prepare_and_execute",
                &format!("timing-scope: {scope}"),
            )
            .replace("    worker-reuse: true", "    worker-reuse: false")
            .replace("    warmup-runs: 1", "    warmup-runs: 0")
            .replace("timeout: 5 s", "timeout: 1 s");
        fs::write(manifest, text).unwrap();
        let evaluator = stage.path().join("evaluator.py");
        fs::write(
            &evaluator,
            fs::read_to_string(&evaluator)
                .unwrap()
                .replace("    x =", "    import time\n    time.sleep(0.25)\n    x ="),
        )
        .unwrap();
        let output = invoke(
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
        let report = data(&output);
        assert_eq!(
            report["summary"]["accepted_attempts"], 1,
            "{scope}: {report}"
        );
        let points = report["attempts"][0]["observations"].as_array().unwrap();
        let last = points.last().unwrap();
        assert!(last["profile"]["evaluation_time_ns"].as_u64().unwrap() >= 1_250_000_000);
        assert!(last["profile"]["active_time_ns"].as_u64().unwrap() < 1_000_000_000);
        let record = document(
            stage
                .path()
                .join("run")
                .join(last["observation_directory"].as_str().unwrap())
                .join("observation.json"),
        );
        assert_eq!(
            record["provenance"]["data"]["execution"]["timing_scope"],
            scope
        );
    }
}

#[test]
fn one_policy_can_compare_streaming_and_fresh_deliveries() {
    let stage = stage(false);
    let manifest = stage.path().join("metewand.yaml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    let start = text.find("implementations:\n").unwrap() + "implementations:\n".len();
    let end = text.find("experiments:").unwrap();
    let fresh = text[start..end].replace("  halving:", "  fresh:").replace(
        "capabilities: [streaming_profile]",
        "capabilities: [fresh_sequence]",
    );
    text.insert_str(end, &fresh);
    let start = text.find("  - name: fixed").unwrap();
    let end = text.find("execution-policies:").unwrap();
    text.replace_range(start..end, "");
    text = text
        .replace(
            "implementations: [halving]",
            "implementations: [halving, fresh]",
        )
        .replace("warmup-runs: 1", "warmup-runs: 0");
    text=text.replace("  three_checks:","      fresh:\n        delivery: fresh_sequence\n        control-schema:\n          type: object\n          properties: {iterations: {type: integer}}\n          required: [iterations]\n          additionalProperties: false\n        schedule:\n          kind: explicit\n          controls: [{iterations: 0}, {iterations: 1}, {iterations: 2}, {iterations: 3}, {iterations: 4}]\n  three_checks:");
    fs::write(manifest, text).unwrap();
    let worker = stage.path().join("worker.py");
    fs::write(&worker,fs::read_to_string(&worker).unwrap().replace("serve_implementation(prepare=prepare, execute_stream=execute_stream,","serve_implementation(prepare=prepare, execute=lambda state, context: {\"x\": 0.5 ** context.observation_control[\"iterations\"]}, execute_stream=execute_stream,")).unwrap();
    let output = invoke(
        stage.path(),
        &["run", "--output-dir", "run", "--output", "json"],
    );
    let report = data(&output);
    assert!(output.status.success(), "{report}: {output:?}");
    assert_eq!(report["summary"]["accepted_attempts"], 12, "{report}");
    let comparison = report["comparison"].as_array().unwrap();
    assert_eq!(comparison.len(), 20);
    assert!(
        comparison
            .iter()
            .all(|row| row["measurement_index"].as_u64().unwrap() <= 1)
    );
}
