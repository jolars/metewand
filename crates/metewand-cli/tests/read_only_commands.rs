mod support;

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use metewand_core::public_schemas::public_schema_catalog;
use metewand_core::public_schemas::{PUBLIC_SCHEMAS, PublicSchema};
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
fn plan_identity_changes_when_a_transitive_worker_source_changes() {
    let repository = repository();
    let before = metewand(repository.path(), ["plan"]);
    assert!(before.status.success(), "{}", stderr(&before));

    write(
        repository.path(),
        "worker.sh",
        "#!/bin/sh\ntouch worker-launched-v2\n",
    );
    let after = metewand(repository.path(), ["plan"]);
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
    assert!(stdout(&output).contains("repository schemas: 5"));
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
    let baseline = metewand(repository.path(), ["plan"]);
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
    let changed_schema = metewand(repository.path(), ["plan"]);
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
    let changed_fixture = metewand(repository.path(), ["plan"]);
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

    let output = metewand(repository.path(), ["plan"]);

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
            include_str!("../../../fixtures/planning/v1/plan.json")
        );
    }
}
