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
        copy_tree(&root.join("examples/python-package"), benchmark.path());
        copy_tree(
            &root.join("packages/python/src/metewand"),
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
    copy_tree(&root.join("examples/python-package"), benchmark.path());
    copy_tree(
        &root.join("packages/python/src/metewand"),
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
    copy_tree(&root.join("examples/python-package"), benchmark.path());
    copy_tree(
        &root.join("packages/python/src/metewand"),
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
            assert_eq!(execution[role]["sdk"]["version"], env!("CARGO_PKG_VERSION"));
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

#[test]
fn applicability_checks_precede_preparation_and_preserve_exact_inputs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    copy_tree(&root.join("examples/python-package"), benchmark.path());
    copy_tree(
        &root.join("packages/python/src/metewand"),
        &benchmark.path().join("metewand"),
    );
    let manifest = benchmark.path().join("metewand.yaml");
    let source = fs::read_to_string(&manifest)
        .unwrap()
        .replace(
            "capabilities: [\"one_shot\"]",
            "capabilities: [\"one_shot\", \"applicability\"]",
        )
        .replace(
            "    measurement-repetitions: 2",
            "    problem-parameters: {offset: {grid: [2, 3]}}\n    measurement-repetitions: 2",
        )
        .replace(
            "    worker-reuse: true",
            "    worker-reuse: true\n    warmup-runs: 1",
        );
    fs::write(manifest, source).unwrap();
    let worker = benchmark.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap().replace("class Sum(OneShotImplementation):", r#"class Sum(OneShotImplementation):
    def check_applicability(self, dataset_instance, dataset_manifest, problem_contract,
                            problem_parameters, implementation_parameters):
        assert not hasattr(self, 'values'), 'check must run before prepare'
        manifest = read_json(dataset_manifest)
        assert manifest['complete'] and manifest['files'][0]['path'] == 'data.json'
        assert read_json(dataset_instance / 'data.json')['values'] == [1, 2, 3]
        excluded = problem_parameters['offset'] == 3
        return dict(outcome='not_applicable' if excluded else 'applicable',
                    code='unsupported_offset' if excluded else 'supported',
                    reason='Only offset 2 is supported.' if excluded else 'Supported configuration.',
                    details=dict(offset=problem_parameters['offset']))
"#).replace("        self.values = read_json", "        assert problem_parameters['offset'] == 2, 'excluded candidate prepared'\n        self.values = read_json");
    fs::write(worker, source).unwrap();
    let invoke = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args(args)
            .args(["--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let plan = invoke(&["plan"]);
    assert_eq!(plan["data"]["summary"]["unchecked_candidates"], 2);
    assert_eq!(plan["data"]["summary"]["known_applicable_candidates"], 0);
    for candidate in plan["data"]["experiments"][0]["candidates"]
        .as_array()
        .unwrap()
    {
        assert_eq!(candidate["applicability"]["outcome"], "unchecked");
        assert_eq!(candidate["applicability"]["code"], "check_pending");
        assert!(candidate["applicability"]["checked_inputs"].is_null());
    }
    let excluded_id = plan["data"]["experiments"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["parameters"]["problem"]["offset"] == 3)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let selected = invoke(&["run", "--output-dir", "excluded", "--select", excluded_id]);
    assert!(selected["data"]["attempts"].as_array().unwrap().is_empty());
    let excluded = invoke(&["status", "--output-dir", "excluded"]);
    assert_eq!(excluded["data"]["summary"]["attempt_slots"], 0);
    assert!(excluded["data"]["slots"].as_array().unwrap().is_empty());
    assert!(
        excluded["data"]["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let run = invoke(&["run", "--output-dir", "run"]);
    assert_eq!(run["data"]["attempts"].as_array().unwrap().len(), 2);
    let status = invoke(&["status", "--output-dir", "run"]);
    assert_eq!(status["data"]["summary"]["attempt_slots"], 2);
    assert!(
        status["data"]["slots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|slot| { slot["candidate_id"] != excluded_id })
    );
    invoke(&["export", "--output-dir", "run", "--destination", "exported"]);
    let records: Vec<Value> = fs::read_to_string(benchmark.path().join("exported/results.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let decisions: Vec<_> = records
        .iter()
        .filter(|r| r["record_type"] == "applicability")
        .collect();
    assert_eq!(decisions.len(), 2, "{records:?}");
    for decision in &decisions {
        let id = decision["id"].as_str().unwrap();
        let explanation = invoke(&["explain", id, "--output-dir", "run"]);
        let definition = &explanation["data"]["subject"]["record"]["definition"];
        let inputs = &definition["checked_inputs"];
        assert_eq!(
            inputs["dataset_manifest"]["complete"], true,
            "{explanation}"
        );
        assert_eq!(inputs["dataset_manifest"]["files"][0]["path"], "data.json");
        assert_eq!(inputs["dataset_manifest_path"], "dataset-manifest.json");
        assert_eq!(inputs["implementation_parameters"], serde_json::json!({}));
        let offset = inputs["problem_parameters"]["offset"].as_i64().unwrap();
        assert_eq!(definition["details"]["offset"], offset);
        assert_eq!(
            definition["code"],
            if offset == 3 {
                "unsupported_offset"
            } else {
                "supported"
            }
        );
        let text = explanation.to_string();
        assert!(text.contains("dataset_manifest"), "{explanation}");
        assert!(text.contains("implementation_parameters"), "{explanation}");
        assert!(text.contains("problem_contract_id"), "{explanation}");
        assert!(text.contains("dataset_instance"), "{explanation}");
    }
    assert!(decisions.iter().any(|r| r["outcome"] == "not_applicable"));
    assert!(decisions.iter().any(|r| r["outcome"] == "applicable"));
    let resumed = invoke(&["run", "--output-dir", "run", "--resume"]);
    assert!(
        resumed["data"]["attempts"].as_array().unwrap().is_empty(),
        "{resumed}"
    );
}

#[test]
fn changed_applicability_evidence_invalidates_execution_and_resume_identities() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    copy_tree(&root.join("examples/python-package"), benchmark.path());
    copy_tree(
        &root.join("packages/python/src/metewand"),
        &benchmark.path().join("metewand"),
    );
    let manifest = benchmark.path().join("metewand.yaml");
    let source = fs::read_to_string(&manifest)
        .unwrap()
        .replace(
            "capabilities: [\"one_shot\"]",
            "capabilities: [\"one_shot\", \"applicability\"]",
        )
        .replace("measurement-repetitions: 2", "measurement-repetitions: 1");
    fs::write(manifest, source).unwrap();
    // Keep the worker and declared inputs fixed while changing only its response.
    let support = benchmark.path().join("support.txt");
    fs::write(&support, "supported_a").unwrap();
    let worker = benchmark.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap().replace(
        "class Sum(OneShotImplementation):",
        &format!(
            r#"class Sum(OneShotImplementation):
    def check_applicability(self, *args):
        from pathlib import Path
        code = Path({support:?}).read_text()
        return dict(outcome='not_applicable' if code == 'excluded' else 'applicable',
                    code=code, reason='Support decision for this configuration.')
"#
        ),
    );
    fs::write(worker, source).unwrap();
    let invoke = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args(args)
            .args(["--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let run_args = ["run", "--output-dir", "run", "--cache-dir", "cache"];
    let resume_args = [
        "run",
        "--output-dir",
        "run",
        "--cache-dir",
        "cache",
        "--resume",
    ];
    let first = invoke(&run_args);
    assert_eq!(first["data"]["attempts"].as_array().unwrap().len(), 1);
    assert!(
        invoke(&resume_args)["data"]["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::write(&support, "supported_b").unwrap();
    let second = invoke(&resume_args);
    assert_eq!(second["data"]["attempts"].as_array().unwrap().len(), 1);
    let first = &first["data"]["attempts"][0];
    let second = &second["data"]["attempts"][0];
    assert_eq!(first["logical_slot_id"], second["logical_slot_id"]);
    for field in ["resolved_slot_id", "attempt_id", "observation_id"] {
        assert!(first[field].is_string(), "{first}");
        assert_ne!(first[field], second[field], "{field}");
    }
    let mut decisions = Vec::new();
    for (attempt, code) in [(first, "supported_a"), (second, "supported_b")] {
        let slot = invoke(&[
            "explain",
            attempt["resolved_slot_id"].as_str().unwrap(),
            "--output-dir",
            "run",
        ]);
        let run_id = slot["data"]["subject"]["record"]["resolved_run_id"]
            .as_str()
            .unwrap();
        let run = invoke(&["explain", run_id, "--output-dir", "run"]);
        let record = &run["data"]["subject"]["record"];
        assert_eq!(
            record["applicability_id"],
            record["definition"]["applicability"]
        );
        let decision_id = record["applicability_id"].as_str().unwrap();
        let decision = invoke(&["explain", decision_id, "--output-dir", "run"]);
        assert_eq!(
            decision["data"]["subject"]["record"]["definition"]["code"],
            code
        );
        decisions.push(decision_id.to_owned());
    }
    assert_ne!(decisions[0], decisions[1]);
    assert!(
        invoke(&resume_args)["data"]["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::write(&support, "excluded").unwrap();
    assert!(
        invoke(&resume_args)["data"]["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let status = invoke(&["status", "--output-dir", "run"]);
    assert_eq!(status["data"]["summary"]["attempt_slots"], 2);
    fs::write(&support, "supported_a").unwrap();
    assert!(
        invoke(&resume_args)["data"]["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn failed_applicability_checks_never_become_measured_attempts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (callback, expected) in [
        (
            "raise RuntimeError('support inspection failed')",
            "support inspection failed",
        ),
        (
            "(dataset_instance / 'data.json').write_text('mutated')\n        return dict(outcome='applicable', code='supported', reason='Supported.')",
            "artifact bytes or completion inventory changed",
        ),
        (
            "(dataset_instance / 'data.json').unlink()\n        return dict(outcome='not_applicable', code='unsupported', reason='Unsupported.')",
            "artifact bytes or completion inventory changed",
        ),
        (
            "(dataset_instance / 'dataset-manifest.json').write_text('{}')\n        return dict(outcome='applicable', code='supported', reason='Supported.')",
            "artifact bytes or completion inventory changed",
        ),
        (
            "(dataset_instance / 'data.json').write_text('mutated')\n        raise RuntimeError('support inspection failed')",
            "artifact bytes or completion inventory changed",
        ),
    ] {
        let benchmark = TempDir::new().unwrap();
        copy_tree(&root.join("examples/python-package"), benchmark.path());
        copy_tree(
            &root.join("packages/python/src/metewand"),
            &benchmark.path().join("metewand"),
        );
        let manifest = benchmark.path().join("metewand.yaml");
        let source = fs::read_to_string(&manifest).unwrap().replace(
            "capabilities: [\"one_shot\"]",
            "capabilities: [\"one_shot\", \"applicability\"]",
        );
        fs::write(manifest, source).unwrap();
        let worker = benchmark.path().join("worker.py");
        let source = fs::read_to_string(&worker).unwrap().replace("class Sum(OneShotImplementation):",
            &format!("class Sum(OneShotImplementation):\n    def check_applicability(self, dataset_instance, *args):\n        {callback}\n"));
        fs::write(worker, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args([
                "run",
                "--output-dir",
                "run",
                "--cache-dir",
                "cache",
                "--output",
                "json",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(expected), "{text}");
        assert!(!benchmark.path().join("run").exists());
        let cache = benchmark.path().join("cache");
        let objects = fs::read_dir(cache.join("objects"))
            .unwrap()
            .map(|entry| entry.unwrap().path().join("data"))
            .collect::<Vec<_>>();
        assert_eq!(objects.len(), 1);
        let data: Value =
            serde_json::from_slice(&fs::read(objects[0].join("data.json")).unwrap()).unwrap();
        assert_eq!(data["values"], serde_json::json!([1, 2, 3]));
        let digest = metewand_runtime::local_tree::hash_local_tree(&objects[0]).unwrap();
        let cache = metewand_runtime::artifact_store::ArtifactStore::open_existing(&cache).unwrap();
        cache.get(digest).unwrap().unwrap().verify().unwrap();
        assert!(cache.active_leases().unwrap().is_empty());
    }
}
