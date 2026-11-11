use super::support;

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::core::public_schemas::public_schema_catalog;
use crate::core::public_schemas::{PUBLIC_SCHEMAS, PublicSchema};
use serde_json::Value;
use tempfile::TempDir;

const OBJECT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object"
}
"#;

const SEMANTICS_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "preprocessing": {"type": "object", "maxProperties": 0},
    "validity": {
      "type": "object",
      "properties": {"finite": {"const": true}},
      "required": ["finite"],
      "additionalProperties": false
    },
    "one_shot_completion": {
      "type": "object",
      "properties": {"accepted": {"const": true}},
      "required": ["accepted"],
      "additionalProperties": false
    }
  },
  "required": ["validity", "one_shot_completion"],
  "additionalProperties": false
}
"#;

const CONTRACT: &str = r#"
version: 1
name: "sum"
family: "arithmetic"
parameter-schema: "schemas/parameters.json"
dataset-schemas: []
result-schema: "schemas/result.json"
metric-schema: "schemas/metrics.json"
semantics-schema: "schemas/semantics.json"
allowed-timing-scopes: ["prepare_and_execute"]
supported-budgets: ["none"]
semantics:
  preprocessing: {}
  validity:
    finite: true
  one_shot_completion:
    accepted: true
reference-cases:
  - result: "fixtures/result.json"
    expected-metrics: "fixtures/metrics.json"
"#;

const MANIFEST: &str = r#"
version: 1
name: "read-only-fixture"
problems:
  sum:
    contract: "problem.yaml"
    evaluator:
      runner: "command"
      program: "sh"
      args: ["worker.sh"]
      sources: ["worker.sh"]
      environment: "local"
implementations:
  reference:
    runner: "command"
    program: "sh"
    args: ["worker.sh"]
    sources: ["worker.sh"]
    environment: "local"
    problem-contracts: ["sum"]
    capabilities: ["one_shot"]
environments:
  local:
    kind: "local"
experiments:
  - name: "main"
    problem: "sum"
    implementations: ["reference"]
    execution-policy: "default"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 2
    seed: 7
execution-policies:
  default:
    version: 1
    timing-scope: "prepare_and_execute"
    primary-time: "timed_wall_time"
observation-policies:
  final:
    version: 1
    kind: "one_shot"
"#;

fn metewand<I, S>(current_dir: &Path, arguments: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .args(arguments)
        .current_dir(current_dir)
        .output()
        .unwrap()
}

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn repository() -> TempDir {
    let repository = TempDir::new().unwrap();
    write(repository.path(), "metewand.yaml", MANIFEST);
    write(repository.path(), "problem.yaml", CONTRACT);
    for schema in ["parameters", "result", "metrics"] {
        write(
            repository.path(),
            &format!("schemas/{schema}.json"),
            OBJECT_SCHEMA,
        );
    }
    write(
        repository.path(),
        "schemas/semantics.json",
        SEMANTICS_SCHEMA,
    );
    write(repository.path(), "fixtures/result.json", "{}\n");
    write(repository.path(), "fixtures/metrics.json", "{}\n");
    write(
        repository.path(),
        "worker.sh",
        "#!/bin/sh\ntouch worker-launched\n",
    );
    repository
}

#[derive(Debug, Eq, PartialEq)]
enum SnapshotEntry {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn files(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, SnapshotEntry>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                files.insert(relative, SnapshotEntry::Directory);
                visit(root, &path, files);
            } else if file_type.is_symlink() {
                files.insert(
                    relative,
                    SnapshotEntry::Symlink(fs::read_link(path).unwrap()),
                );
            } else {
                files.insert(relative, SnapshotEntry::File(fs::read(path).unwrap()));
            }
        }
    }

    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn candidate_id(output: &str) -> &str {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("logical candidate "))
        .and_then(|line| line.split_whitespace().next())
        .expect("plan must contain a logical candidate identity")
}

fn machine_output(output: &Output) -> Value {
    let value = serde_json::from_slice(&output.stdout).expect("stdout must contain one JSON value");
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::MachineOutput.repository_path(), &value)
        .expect("machine output must satisfy the published schema");
    value
}

#[test]
fn schema_lists_stable_entry_points_and_prints_exact_documents() {
    let working_directory = TempDir::new().unwrap();
    let list = metewand(working_directory.path(), ["schema"]);

    assert!(list.status.success(), "{}", stderr(&list));
    assert!(list.stderr.is_empty());
    let list = stdout(&list);
    for schema in PUBLIC_SCHEMAS {
        assert!(list.contains(schema.slug()));
        assert!(list.contains(schema.id()));
    }

    let manifest = metewand(working_directory.path(), ["schema", "manifest"]);
    assert!(manifest.status.success(), "{}", stderr(&manifest));
    assert!(manifest.stderr.is_empty());
    assert_eq!(stdout(&manifest), PublicSchema::Manifest.source());
}

#[test]
fn check_validates_the_repository_without_launching_workers_or_writing() {
    let repository = repository();
    let before = files(repository.path());

    let output = metewand(repository.path(), ["check"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("read-only-fixture"));
    assert!(stdout(&output).contains("valid"));
    assert!(stdout(&output).contains("source bundles: 1"));
    assert_eq!(files(repository.path()), before);
    assert!(!repository.path().join("worker-launched").exists());
}

#[test]
fn positional_experiments_match_explicit_selection_without_side_effects() {
    let repository = repository();
    let manifest = MANIFEST.replace("name: \"main\"", "name: gaussian-regression").replace(
        "execution-policies:\n",
        "  - name: logistic-regression\n    problem: sum\n    implementations: [reference]\n    seed: 11\nexecution-policies:\n",
    );
    write(
        repository.path(),
        "metewand.yaml",
        &format!("{manifest}\ndefault-experiment: logistic-regression\n"),
    );
    let before = files(repository.path());

    for command in ["check", "plan"] {
        let explicit = metewand(
            repository.path(),
            [
                command,
                "--experiment",
                "gaussian-regression",
                "--output",
                "json",
            ],
        );
        assert!(explicit.status.success(), "{explicit:?}");
        for arguments in [
            vec![command, "gaussian-regression"],
            vec![
                command,
                "--manifest",
                "metewand.yaml",
                "gaussian-regression",
            ],
            vec![
                command,
                "gaussian-regression",
                "--manifest",
                "metewand.yaml",
            ],
        ] {
            let output = metewand(
                repository.path(),
                arguments.into_iter().chain(["--output", "json"]),
            );
            assert!(output.status.success(), "{output:?}");
            assert_eq!(output.stdout, explicit.stdout);
        }
        let explicit_union = metewand(
            repository.path(),
            [
                command,
                "--experiment",
                "logistic-regression",
                "--experiment",
                "gaussian-regression",
                "--output",
                "json",
            ],
        );
        let mixed = metewand(
            repository.path(),
            [
                command,
                "logistic-regression",
                "--manifest",
                "metewand.yaml",
                "--experiment",
                "gaussian-regression",
                "logistic-regression",
                "--output",
                "json",
            ],
        );
        assert!(mixed.status.success(), "{mixed:?}");
        assert_eq!(mixed.stdout, explicit_union.stdout);
        if command == "plan" {
            let plan = machine_output(&mixed);
            assert_eq!(
                plan["data"]["experiments"][0]["name"],
                "gaussian-regression"
            );
            assert_eq!(
                plan["data"]["experiments"][1]["name"],
                "logistic-regression"
            );
        }
    }
    assert_eq!(files(repository.path()), before);
}

#[test]
fn invalid_positional_experiments_fail_before_execution_or_writes() {
    let repository = repository();
    let before = files(repository.path());
    for command in ["check", "plan", "run"] {
        let explicit = metewand(
            repository.path(),
            [command, "--experiment", "missing", "--output", "json"],
        );
        let positional = metewand(repository.path(), [command, "missing", "--output", "json"]);
        assert!(!positional.status.success(), "{positional:?}");
        assert_eq!(positional.status.code(), explicit.status.code());
        assert_eq!(positional.stdout, explicit.stdout);
        for (name, flag, code) in [
            ("", None, "invalid_experiment_name"),
            ("main", Some("--typo"), "unexpected_argument"),
        ] {
            let arguments = [command, name]
                .into_iter()
                .chain(flag)
                .chain(["--output", "json"]);
            let output = metewand(repository.path(), arguments);
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            assert_eq!(machine_output(&output)["data"]["code"], code);
        }
    }
    assert_eq!(files(repository.path()), before);
}

#[test]
fn invalid_name_filters_fail_before_execution_or_writes() {
    let repository = repository();
    let before = files(repository.path());
    for command in ["check", "plan", "run"] {
        for flag in ["--implementation", "--dataset", "--problem"] {
            for value in [None, Some(""), Some("--workers"), Some("typo")] {
                let output = metewand(
                    repository.path(),
                    [command, "--output", "json", flag].into_iter().chain(value),
                );
                assert_eq!(output.status.code(), Some(2), "{output:?}");
                let diagnostic = machine_output(&output);
                if value == Some("typo") {
                    assert!(
                        diagnostic["data"]["message"]
                            .as_str()
                            .unwrap()
                            .contains("is not declared"),
                        "{diagnostic}"
                    );
                } else {
                    assert_eq!(diagnostic["data"]["code"], "missing_filter_name");
                }
            }
        }
    }
    assert_eq!(files(repository.path()), before);
}

#[test]
fn help_advertises_positional_experiment_selection() {
    let directory = TempDir::new().unwrap();
    for command in [None, Some("check"), Some("plan"), Some("run")] {
        let output = metewand(directory.path(), command.into_iter().chain(["--help"]));
        assert!(output.status.success(), "{output:?}");
        assert!(stdout(&output).contains("[EXPERIMENT]..."));
        assert!(stdout(&output).contains("--experiment"));
        for flag in ["--implementation", "--dataset", "--problem"] {
            assert!(stdout(&output).contains(flag), "{output:?}");
        }
    }
}

#[test]
fn plan_identity_changes_when_a_transitive_worker_source_changes() {
    let repository = repository();
    let before = metewand(repository.path(), ["plan", "--verbose"]);
    assert!(before.status.success(), "{}", stderr(&before));

    write(
        repository.path(),
        "worker.sh",
        "#!/bin/sh\ntouch worker-launched-v2\n",
    );
    let after = metewand(repository.path(), ["plan", "--verbose"]);
    assert!(after.status.success(), "{}", stderr(&after));

    assert_ne!(
        candidate_id(&stdout(&before)),
        candidate_id(&stdout(&after))
    );
    assert!(!repository.path().join("worker-launched").exists());
    assert!(!repository.path().join("worker-launched-v2").exists());
}

#[test]
fn check_loads_relative_schema_references_without_network_access() {
    let repository = repository();
    write(
        repository.path(),
        "schemas/parameters.json",
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$ref": "definitions.json#/$defs/parameters"
}
"#,
    );
    write(
        repository.path(),
        "schemas/definitions.json",
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$defs": {"parameters": {"type": "object"}}
}
"#,
    );

    let output = metewand(repository.path(), ["check"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("repository schemas: 6"));
}

#[test]
fn plan_identity_tracks_transitive_schema_and_contract_fixture_content() {
    let repository = repository();
    write(
        repository.path(),
        "schemas/parameters.json",
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$ref": "definitions.json#/$defs/parameters"
}
"#,
    );
    write(
        repository.path(),
        "schemas/definitions.json",
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$defs": {"parameters": {"type": "object"}}
}
"#,
    );
    let baseline = metewand(repository.path(), ["plan", "--verbose"]);
    assert!(baseline.status.success(), "{}", stderr(&baseline));
    let baseline_id = candidate_id(&stdout(&baseline)).to_owned();

    write(
        repository.path(),
        "schemas/definitions.json",
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$defs": {"parameters": {"type": "object", "maxProperties": 0}}
}
"#,
    );
    let changed_schema = metewand(repository.path(), ["plan", "--verbose"]);
    assert!(
        changed_schema.status.success(),
        "{}",
        stderr(&changed_schema)
    );
    let changed_schema_id = candidate_id(&stdout(&changed_schema)).to_owned();
    assert_ne!(baseline_id, changed_schema_id);

    write(
        repository.path(),
        "fixtures/result.json",
        "{\"answer\": 1}\n",
    );
    let changed_fixture = metewand(repository.path(), ["plan", "--verbose"]);
    assert!(
        changed_fixture.status.success(),
        "{}",
        stderr(&changed_fixture)
    );
    assert_ne!(changed_schema_id, candidate_id(&stdout(&changed_fixture)));
}

#[test]
fn check_reports_invalid_repository_schemas() {
    let repository = repository();
    write(
        repository.path(),
        "schemas/parameters.json",
        r#"{"type":"object"}"#,
    );

    let output = metewand(repository.path(), ["check"]);

    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("parameters.json"));
    assert!(stderr(&output).contains("$schema"));
}

#[test]
fn plan_reports_identified_slots_with_declared_capabilities_and_no_side_effects() {
    let repository = repository();
    let before = files(repository.path());

    let output = metewand(repository.path(), ["plan", "--verbose"]);

    assert!(output.status.success(), "{}", stderr(&output));
    let output_text = stdout(&output);
    assert!(output_text.contains("read-only-fixture"));
    assert!(output_text.contains("1 logical candidate"));
    assert!(output_text.contains("1 logical specification"));
    assert!(output_text.contains("2 observation slots"));
    assert!(output_text.contains("2 attempt slots"));
    assert!(output_text.contains("declared"));
    assert!(output_text.contains("mw1-logical-candidate-"));
    assert!(output_text.contains("mw1-logical-specification-"));
    assert!(output_text.contains("mw1-logical-observation-slot-"));
    assert!(output_text.contains("mw1-logical-attempt-slot-"));
    assert_eq!(files(repository.path()), before);
    assert!(!repository.path().join("worker-launched").exists());
}

#[test]
fn check_and_plan_do_not_acquire_selected_remote_sources_or_launch_materializers() {
    use std::{
        io::Write,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread,
        time::Duration,
    };

    let repository = repository();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let manifest = support::replace_yaml(
        MANIFEST,
        "problem: \"sum\"\n",
        "problem: \"sum\"\ndatasets: [\"remote\"]\n",
    );
    write(
        repository.path(),
        "metewand.yaml",
        &format!(
            "{manifest}\ndatasets:\n  remote:\n    output-schema: \"schemas/dataset.json\"\n    runner: \"command\"\n    program: \"sh\"\n    args: [\"worker.sh\"]\n    sources: [\"worker.sh\"]\n    environment: \"local\"\n    source:\n      url: \"http://{}/data\"\n      sha256: \"{}\"\n",
            listener.local_addr().unwrap(),
            "0".repeat(64),
        ),
    );
    write(repository.path(), "schemas/dataset.json", OBJECT_SCHEMA);
    write(repository.path(), "fixtures/dataset/input", "reference\n");
    write(
        repository.path(),
        "problem.yaml",
        &CONTRACT
            .replace(
                "dataset-schemas: []",
                "dataset-schemas: [\"schemas/dataset.json\"]",
            )
            .replace(
                "reference-cases:\n  - result:",
                "reference-cases:\n  - dataset: \"fixtures/dataset\"\n    result:",
            ),
    );
    let before = files(repository.path());

    let stop = AtomicBool::new(false);
    let requests = AtomicUsize::new(0);
    let outputs = thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        requests.fetch_add(1, Ordering::Relaxed);
                        let _ = stream.write_all(
                            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("source request probe failed: {error}"),
                }
            }
        });
        let outputs = ["check", "plan", "status"]
            .map(|command| metewand(repository.path(), [command, "--output", "json"]));
        stop.store(true, Ordering::Relaxed);
        outputs
    });
    for output in outputs {
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(files(repository.path()), before);
        assert!(!repository.path().join("worker-launched").exists());
    }
    assert_eq!(requests.load(Ordering::Relaxed), 0);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn check_and_plan_accept_an_explicit_manifest_path() {
    let repository = repository();
    let parent = repository.path().parent().unwrap();
    let manifest = repository.path().join("metewand.yaml");

    for command in ["check", "plan", "status"] {
        let output = metewand(parent, [command, "--manifest", manifest.to_str().unwrap()]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
}

#[test]
fn every_command_has_versioned_json_output() {
    let repository = repository();

    for arguments in [
        vec!["--help", "--output", "json"],
        vec!["--version", "--output", "json"],
        vec!["schema", "--output", "json"],
        vec!["schema", "manifest", "--output", "json"],
        vec!["check", "--output", "json"],
        vec!["plan", "--output", "json"],
    ] {
        let output = metewand(repository.path(), arguments);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(output.stderr.is_empty());
        let envelope = machine_output(&output);
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["kind"], "result");
        assert!(envelope["data"]["command"].is_string());
    }
}

#[test]
fn jsonl_output_is_one_schema_conforming_record_per_line() {
    let repository = repository();

    let output = metewand(repository.path(), ["--output", "jsonl", "plan"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stderr.is_empty());
    let stdout = stdout(&output);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let envelope: Value = serde_json::from_str(lines[0]).unwrap();
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::MachineOutput.repository_path(), &envelope)
        .unwrap();
    assert_eq!(envelope["data"]["command"], "plan");
    assert_eq!(envelope["data"]["summary"]["logical_candidates"], 1);
    assert_eq!(envelope["data"]["summary"]["attempt_slots"], 2);
}

#[test]
fn machine_failures_have_stable_codes_spans_causes_and_stream_separation() {
    let repository = repository();
    write(repository.path(), "metewand.yaml", "version: 1\nname: []\n");

    let output = metewand(repository.path(), ["check", "--output", "json"]);

    assert_eq!(output.status.code(), Some(3));
    let envelope = machine_output(&output);
    assert_eq!(envelope["kind"], "diagnostic");
    assert_eq!(envelope["data"]["code"], "repository_check_failed");
    assert_eq!(envelope["data"]["causes"][0]["code"], "invalid_manifest");
    assert_eq!(
        envelope["data"]["causes"][0]["source_span"]["path"],
        "metewand.yaml"
    );
    assert_eq!(envelope["data"]["causes"][0]["source_span"]["line"], 2);
    assert!(envelope["data"]["causes"][0]["remediation"].is_string());
    assert!(stderr(&output).contains("error[repository_check_failed]"));
    assert!(stderr(&output).contains("metewand.yaml:2:"));
}

#[test]
fn usage_and_input_failures_have_distinct_stable_exit_codes() {
    let repository = repository();

    let usage = metewand(repository.path(), ["unknown", "--output", "json"]);
    assert_eq!(usage.status.code(), Some(2));
    let usage = machine_output(&usage);
    assert_eq!(usage["data"]["code"], "unknown_command");

    let input = metewand(
        repository.path(),
        ["check", "--manifest", "missing.yaml", "--output", "json"],
    );
    assert_eq!(input.status.code(), Some(4));
    let input = machine_output(&input);
    assert_eq!(input["data"]["code"], "repository_check_failed");
    assert_eq!(input["data"]["causes"][0]["code"], "manifest_access_failed");
}

#[test]
fn complete_plan_json_matches_the_version_one_snapshot() {
    for _ in 0..2 {
        let repository = repository();
        let output = metewand(repository.path(), ["plan", "--output", "json"]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(
            stdout(&output),
            include_str!("../../fixtures/planning/v1/plan.json")
        );
    }
}

#[test]
fn plan_exposes_resolved_local_environment_and_one_shot_defaults() {
    let repository = repository();
    let explicit = metewand(repository.path(), ["plan", "--output", "json"]);
    assert!(explicit.status.success(), "{}", stderr(&explicit));
    let implicit = MANIFEST
        .replace("      environment: \"local\"\n", "")
        .replace("    environment: \"local\"\n", "")
        .replace("environments:\n  local:\n    kind: \"local\"\n", "")
        .replace("    version: 1\n", "")
        .replace("    implementation-repetitions: 1\n", "");
    write(repository.path(), "metewand.yaml", &implicit);
    let before = files(repository.path());
    let output = metewand(repository.path(), ["plan", "--output", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), stdout(&explicit));
    let plan = machine_output(&output);
    assert_eq!(
        plan["data"]["requirements"]["controls"][0],
        serde_json::json!({
            "experiment": "main", "cpus": null, "threads": null, "memory": null,
            "network": false, "worker_reuse": false, "warmup_runs": 0, "timeout": null,
            "timing_scope": "prepare_and_execute", "primary_time": "timed_wall_time",
            "run_order": "sequential", "enforcement": "best_effort"
        })
    );
    assert_eq!(
        plan["data"]["requirements"]["environments"][0]["name"],
        "local"
    );
    assert_eq!(plan["data"]["experiments"][0]["max_concurrency"], 1);
    assert_eq!(
        plan["data"]["experiments"][0]["candidates"][0]["parameters"]["implementation"],
        serde_json::json!({})
    );
    assert_eq!(files(repository.path()), before);
}

#[test]
fn plan_default_is_bounded_and_verbose_preserves_complete_machine_output() {
    let repository = repository();
    write(
        repository.path(),
        "metewand.yaml",
        &MANIFEST.replace("measurement-repetitions: 2", "measurement-repetitions: 200"),
    );
    let before = files(repository.path());
    let output = metewand(repository.path(), ["plan"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.lines().count() < 30, "{text}");
    for expected in [
        "read-only-fixture",
        "main",
        "sum",
        "reference",
        "200 observation slots",
        "200 attempt slots",
        "--verbose",
        "--output json",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("mw1-"));
    let verbose = metewand(repository.path(), ["plan", "--verbose"]);
    assert!(verbose.status.success(), "{}", stderr(&verbose));
    assert!(stdout(&verbose).contains("mw1-logical-attempt-slot-"));
    let json = metewand(repository.path(), ["plan", "--output", "json"]);
    let detailed_json = metewand(repository.path(), ["plan", "--verbose", "--output", "json"]);
    assert_eq!(machine_output(&json), machine_output(&detailed_json));
    assert_eq!(files(repository.path()), before);
}
