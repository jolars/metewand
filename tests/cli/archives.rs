#![cfg(any(unix, windows))]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

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

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_json(path: &Path, value: Value) {
    fs::write(
        path,
        crate::core::canonical::CanonicalValue::try_from(value)
            .unwrap()
            .to_canonical_bytes()
            .unwrap(),
    )
    .unwrap();
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn refresh_inventory(path: &Path) {
    let mut inventory = read_json(path);
    for file in inventory["files"].as_array_mut().unwrap() {
        let bytes = fs::read(path.parent().unwrap().join(file["path"].as_str().unwrap())).unwrap();
        file["sha256"] = serde_json::json!(sha256(&bytes));
        file["size_bytes"] = serde_json::json!(bytes.len());
    }
    write_json(path, inventory);
}

fn replace_id(value: &mut Value, old: &Value, new: &Value) {
    if value == old {
        *value = new.clone();
    } else {
        match value {
            Value::Array(values) => values.iter_mut().for_each(|v| replace_id(v, old, new)),
            Value::Object(values) => {
                values.values_mut().for_each(|v| replace_id(v, old, new));
            }
            _ => (),
        }
    }
}

fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .args(["--output", "json"])
        .output()
        .unwrap()
}

fn success(root: &Path, args: &[&str]) -> Value {
    let output = invoke(root, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let schema = match args[0] {
        "archive" => crate::core::public_schemas::PublicSchema::ArchiveOutput,
        "status" | "explain" => crate::core::public_schemas::PublicSchema::InspectionOutput,
        "export" => crate::core::public_schemas::PublicSchema::ExportOutput,
        _ => crate::core::public_schemas::PublicSchema::MachineOutput,
    };
    crate::core::public_schemas::public_schema_catalog()
        .unwrap()
        .validate(schema.repository_path(), &value)
        .unwrap();
    value["data"].clone()
}

#[test]
fn archive_rejects_scientific_payload_changes_after_inventory_updates() {
    let root = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    success(root.path(), &["run", "--output-dir", "run"]);
    success(
        root.path(),
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "results.tar.gz",
        ],
    );
    success(
        root.path(),
        &[
            "archive",
            "import",
            "--archive",
            "results.tar.gz",
            "--destination",
            "imported",
        ],
    );
    for role in ["metrics", "result"] {
        let collection = root.path().join(role);
        copy(&root.path().join("imported"), &collection);
        let mut evidence = read_json(&collection.join("evidence.json"));
        let observation = evidence["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["kind"] == "observation")
            .unwrap()["record"]
            .clone();
        let artifact = evidence["records"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|record| {
                record["kind"] == "stored-artifact"
                    && record["record"]["id"] == observation["artifact_id"]
            })
            .unwrap();
        let artifact = &mut artifact["record"];
        let directory = collection
            .join("data")
            .join(artifact["path"].as_str().unwrap());
        let payload = directory.join(if role == "metrics" {
            "metrics.json"
        } else {
            "result/result.json"
        });
        let mut document = read_json(&payload);
        document["data"] = serde_json::json!({"altered":42});
        write_json(&payload, document);
        refresh_inventory(&directory.join("manifest.json"));
        let digest = crate::runtime::local_tree::hash_local_tree(&directory).unwrap();
        let digest = crate::core::materialization::hex(digest);
        artifact["sha256"] = serde_json::json!(digest);
        artifact["manifest_sha256"] =
            serde_json::json!(sha256(&fs::read(directory.join("manifest.json")).unwrap()));
        let old_id = artifact["id"].clone();
        let new_id = serde_json::json!(format!("run-store-v1:observation-{digest}"));
        replace_id(&mut evidence, &old_id, &new_id);
        let parsed: crate::core::archive::ArchiveEvidence =
            serde_json::from_value(evidence.clone()).unwrap();
        parsed.validate().unwrap();
        write_json(&collection.join("evidence.json"), evidence);
        refresh_inventory(&collection.join("archive.json"));
        let error = crate::runtime::archive::verify_archive(
            &collection,
            crate::runtime::archive::ArchiveLimits::default(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains(&format!("observation {role}")),
            "{error}"
        );
        assert!(
            !invoke(root.path(), &["status", "--output-dir", role])
                .status
                .success()
        );
        assert!(
            !invoke(
                root.path(),
                &[
                    "export",
                    "--output-dir",
                    role,
                    "--destination",
                    "altered-export"
                ],
            )
            .status
            .success()
        );
        assert!(!root.path().join("altered-export").exists());
    }
}

#[test]
fn archive_round_trip_preserves_evidence_without_the_repository() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        &repo,
    );
    success(&repo, &["run", "--output-dir", "run"]);
    let before = success(&repo, &["status", "--output-dir", "run"]);
    let id = before["observations"][0]["id"].as_str().unwrap();
    let explanation = success(&repo, &["explain", id, "--output-dir", "run"]);
    success(
        &repo,
        &["export", "--output-dir", "run", "--destination", "original"],
    );
    fs::write(repo.join("run/index.jsonl"), "stale projection").unwrap();
    fs::write(repo.join("run/orphan"), "unregistered bytes").unwrap();
    for name in ["first.tar.gz", "second.tar.gz"] {
        success(
            &repo,
            &[
                "archive",
                "create",
                "--output-dir",
                "run",
                "--destination",
                name,
            ],
        );
    }
    assert_eq!(
        fs::read(repo.join("first.tar.gz")).unwrap(),
        fs::read(repo.join("second.tar.gz")).unwrap()
    );
    fs::rename(
        repo.join("first.tar.gz"),
        root.path().join("results.tar.gz"),
    )
    .unwrap();
    let expected = fs::read(repo.join("original/results.jsonl")).unwrap();
    fs::remove_dir_all(&repo).unwrap();
    success(
        root.path(),
        &["archive", "verify", "--archive", "results.tar.gz"],
    );
    success(
        root.path(),
        &[
            "archive",
            "import",
            "--archive",
            "results.tar.gz",
            "--destination",
            "imported",
        ],
    );
    success(root.path(), &["archive", "verify", "--archive", "imported"]);
    let after = success(root.path(), &["status", "--output-dir", "imported"]);
    let imported_explanation = success(root.path(), &["explain", id, "--output-dir", "imported"]);
    assert_eq!(
        explanation["subject"]["record"],
        imported_explanation["subject"]["record"]
    );
    for key in ["summary", "slots", "attempts", "observations", "comparison"] {
        assert_eq!(before[key], after[key], "{key}");
    }
    success(
        root.path(),
        &[
            "export",
            "--output-dir",
            "imported",
            "--destination",
            "tables",
        ],
    );
    assert_eq!(
        expected,
        fs::read(root.path().join("tables/results.jsonl")).unwrap()
    );
    assert!(!root.path().join("imported/run.sqlite3").exists());
    assert!(!root.path().join("imported/data/orphan").exists());
    assert!(
        !invoke(
            root.path(),
            &["export", "--output-dir", "imported", "--rebuild-index"]
        )
        .status
        .success()
    );
    assert!(
        !invoke(
            root.path(),
            &[
                "archive",
                "import",
                "--archive",
                "results.tar.gz",
                "--destination",
                "imported"
            ]
        )
        .status
        .success()
    );
    assert!(crate::runtime::run_store::RunStore::open(&root.path().join("imported")).is_err());
    assert!(!root.path().join("imported/run.sqlite3").exists());
    assert!(
        !invoke(
            root.path(),
            &[
                "export",
                "--output-dir",
                "imported",
                "--destination",
                "imported/tables"
            ]
        )
        .status
        .success()
    );
    assert!(!root.path().join("imported/tables").exists());
    let filtered = success(
        root.path(),
        &[
            "export",
            "--output-dir",
            "imported",
            "--id",
            id,
            "--destination",
            "filtered",
        ],
    );
    assert_eq!(
        filtered["manifest"]["observation_ids"],
        serde_json::json!([id])
    );
    let mut evidence: crate::core::archive::ArchiveEvidence =
        serde_json::from_slice(&fs::read(root.path().join("imported/evidence.json")).unwrap())
            .unwrap();
    evidence.records.retain(|record| {
        !matches!(
            record,
            crate::core::archive::ArchiveRecord::ObservationSelection(_)
        )
    });
    for record in &mut evidence.records {
        if let crate::core::archive::ArchiveRecord::Observation(observation) = record {
            observation.selected = false;
        }
    }
    assert!(
        evidence
            .validate()
            .unwrap_err()
            .to_string()
            .contains("selected observations")
    );
}

fn profile_round_trip(example: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let repo = root.path().join("repository");
    copy(&project.join("examples").join(example), &repo);
    copy(
        &project.join("packages/python/src/metewand"),
        &repo.join("metewand"),
    );
    if example == "fresh-sequence" {
        let path = repo.join("metewand.yaml");
        let source = fs::read_to_string(&path).unwrap();
        let start = source.find("  uniroot:\n").unwrap();
        let end = source[start..].find("experiments:\n").unwrap() + start;
        let mut source = format!("{}{}", &source[..start], &source[end..]);
        let start = source.find("      uniroot:\n").unwrap();
        let end = source[start..].find("  four_points:\n").unwrap() + start;
        source.replace_range(start..end, "");
        fs::write(path, source.replace("[bisection, uniroot]", "[bisection]")).unwrap();
    }
    success(&repo, &["run", "--output-dir", "run"]);
    let before = success(&repo, &["status", "--output-dir", "run"]);
    let mut expected = Vec::new();
    for format in ["jsonl", "csv"] {
        success(
            &repo,
            &[
                "export",
                "--output-dir",
                "run",
                "--format",
                format,
                "--destination",
                format,
            ],
        );
        expected.push(fs::read(repo.join(format).join(format!("results.{format}"))).unwrap());
    }
    success(
        &repo,
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "results.tar.gz",
        ],
    );
    fs::rename(
        repo.join("results.tar.gz"),
        root.path().join("results.tar.gz"),
    )
    .unwrap();
    fs::remove_dir_all(repo).unwrap();
    success(
        root.path(),
        &[
            "archive",
            "import",
            "--archive",
            "results.tar.gz",
            "--destination",
            "imported",
        ],
    );
    let after = success(root.path(), &["status", "--output-dir", "imported"]);
    for key in [
        "summary",
        "slots",
        "attempts",
        "observations",
        "comparison",
        "fresh_sequences",
    ] {
        assert_eq!(before[key], after[key], "{example}: {key}");
    }
    for (format, expected) in ["jsonl", "csv"].iter().zip(expected) {
        success(
            root.path(),
            &[
                "export",
                "--output-dir",
                "imported",
                "--format",
                format,
                "--destination",
                format,
            ],
        );
        assert_eq!(
            expected,
            fs::read(root.path().join(format).join(format!("results.{format}"))).unwrap()
        );
    }
    if example == "streaming-profile" {
        let collection = root.path().join("imported");
        let path = collection.join("evidence.json");
        let mut evidence: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let id = after["observations"][0]["id"].clone();
        evidence["records"]
            .as_array_mut()
            .unwrap()
            .retain(|record| {
                !(record["kind"] == "observation" && record["record"]["id"] == id
                    || record["kind"] == "observation-selection"
                        && record["record"]["observation_id"] == id)
            });
        let bytes = crate::core::canonical::CanonicalValue::try_from(evidence)
            .unwrap()
            .to_canonical_bytes()
            .unwrap();
        fs::write(path, &bytes).unwrap();
        let path = collection.join("archive.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let entry = manifest["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|file| file["path"] == "evidence.json")
            .unwrap();
        entry["size_bytes"] = serde_json::json!(bytes.len());
        entry["sha256"] = serde_json::json!(
            sha2::Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        fs::write(
            path,
            crate::core::canonical::CanonicalValue::try_from(manifest)
                .unwrap()
                .to_canonical_bytes()
                .unwrap(),
        )
        .unwrap();
        let output = invoke(root.path(), &["archive", "verify", "--archive", "imported"]);
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("observation list")
        );
    }
    root
}

#[test]
fn fresh_sequence_archive_preserves_controls_budgets_and_target_skips() {
    let root = profile_round_trip("fresh-sequence");
    let original = read_json(&root.path().join("imported/evidence.json"));
    let records = original["records"].as_array().unwrap();
    let budget = &records
        .iter()
        .find(|record| {
            record["kind"] == "fresh-budget"
                && record["record"]["charged_ns"]
                    .as_u64()
                    .is_some_and(|n| n > 0)
        })
        .unwrap()["record"];
    let charged = budget["charged_ns"].as_u64().unwrap();
    let reserved = budget["reserved_ns"].as_u64().unwrap();
    let attempt = &records
        .iter()
        .find(|record| {
            record["kind"] == "attempt" && record["record"]["id"] == budget["attempt_id"]
        })
        .unwrap()["record"];
    let member = &records
        .iter()
        .find(|record| {
            record["kind"] == "fresh-member" && record["record"]["slot_id"] == attempt["slot_id"]
        })
        .unwrap()["record"];
    for change in ["charge", "reservation", "unknown", "missing"] {
        let collection = root.path().join(change);
        copy(&root.path().join("imported"), &collection);
        let mut evidence = original.clone();
        let records = evidence["records"].as_array_mut().unwrap();
        let changed_budget = records
            .iter_mut()
            .find(|record| {
                record["kind"] == "fresh-budget"
                    && record["record"]["attempt_id"] == budget["attempt_id"]
            })
            .unwrap();
        match change {
            "charge" => changed_budget["record"]["charged_ns"] = serde_json::json!(0),
            "reservation" => {
                changed_budget["record"]["reserved_ns"] = serde_json::json!(reserved + 1)
            }
            "unknown" => changed_budget["record"]["charged_ns"] = Value::Null,
            "missing" => records.retain(|record| {
                record["kind"] != "fresh-budget"
                    || record["record"]["attempt_id"] != budget["attempt_id"]
            }),
            _ => unreachable!(),
        }
        let progress = &mut records
            .iter_mut()
            .find(|record| {
                record["kind"] == "fresh-sequence"
                    && record["record"]["resolved_specification_id"] == member["resolved_run_id"]
                    && record["record"]["measurement_index"] == member["measurement_index"]
            })
            .unwrap()["record"];
        let consumed = progress["consumed_time_ns"].as_u64().unwrap();
        if change == "charge" || change == "missing" {
            progress["consumed_time_ns"] = serde_json::json!(consumed - charged);
        } else if change == "unknown" {
            progress["consumed_time_ns"] = serde_json::json!(consumed - charged + reserved);
            progress["conservative_time_ns"] = serde_json::json!(reserved);
        }
        let parsed: crate::core::archive::ArchiveEvidence =
            serde_json::from_value(evidence.clone()).unwrap();
        parsed.validate().unwrap();
        write_json(&collection.join("evidence.json"), evidence);
        refresh_inventory(&collection.join("archive.json"));
        let error = crate::runtime::archive::verify_archive(
            &collection,
            crate::runtime::archive::ArchiveLimits::default(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("fresh budget"),
            "{change}: {error}"
        );
        assert!(
            !invoke(root.path(), &["status", "--output-dir", change])
                .status
                .success()
        );
    }
}

#[test]
fn buffered_archive_preserves_trace_rows_and_instrumented_coordinates() {
    profile_round_trip("buffered-profile");
}

#[test]
fn streaming_archive_preserves_checkpoint_evidence() {
    profile_round_trip("streaming-profile");
}

#[test]
fn archive_preserves_failed_attempts_retries_and_pending_work() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        &repo,
    );
    let manifest = repo.join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("measurement-repetitions: 1", "measurement-repetitions: 3"),
    )
    .unwrap();
    let worker = repo.join("worker.py");
    let original = fs::read_to_string(&worker).unwrap();
    fs::write(
        &worker,
        original.replace("prepared is not None", "prepared is None"),
    )
    .unwrap();
    assert_eq!(
        invoke(
            &repo,
            &["run", "--output-dir", "run", "--max-attempts", "1"]
        )
        .status
        .code(),
        Some(5)
    );
    assert_eq!(
        invoke(
            &repo,
            &[
                "run",
                "--output-dir",
                "run",
                "--resume",
                "--retry",
                "--max-attempts",
                "1"
            ]
        )
        .status
        .code(),
        Some(5)
    );
    let history = success(&repo, &["status", "--output-dir", "run"]);
    {
        use crate::runtime::run_store::{
            AttemptPhase, LeaseRequest, RetryAuthorization, RunStore, TerminalOutcome,
        };
        let slot = history["attempts"][0]["slot_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let now = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
        .unwrap();
        let mut store = RunStore::open(&repo.join("run")).unwrap();
        let owned = store
            .reserve(
                slot,
                RetryAuthorization::Next { retry: true },
                LeaseRequest {
                    id: "archive-test".into(),
                    owner_token: "owner".into(),
                    expires_at_ns: now + 30_000_000_000,
                },
                now,
            )
            .unwrap();
        let owned = store
            .advance(&owned, AttemptPhase::Finalizing, now + 1)
            .unwrap();
        store
            .publish_finalization(
                &owned,
                TerminalOutcome::Superseded,
                &[],
                |_| Ok(()),
                || now + 2,
            )
            .unwrap();
    }
    let before = success(&repo, &["status", "--output-dir", "run"]);
    assert_eq!(before["summary"]["failed_attempts"], 2);
    assert_eq!(before["summary"]["superseded_attempts"], 1);
    assert_eq!(before["summary"]["pending_slots"], 2);
    success(
        &repo,
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "results.tar.gz",
        ],
    );
    success(
        &repo,
        &[
            "archive",
            "import",
            "--archive",
            "results.tar.gz",
            "--destination",
            "imported",
        ],
    );
    let after = success(&repo, &["status", "--output-dir", "imported"]);
    for key in [
        "summary",
        "slots",
        "attempts",
        "observations",
        "diagnostics",
    ] {
        assert_eq!(before[key], after[key], "{key}");
    }
    let output = invoke(
        &repo,
        &[
            "run",
            "--manifest",
            "metewand.yaml",
            "--output-dir",
            "imported",
            "--resume",
        ],
    );
    assert!(!output.status.success());
    assert!(!repo.join("imported/run.sqlite3").exists());
}

#[test]
fn partial_streaming_evidence_survives_a_failed_trajectory() {
    let root = tempfile::tempdir().unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let repo = root.path().join("repository");
    copy(&project.join("examples/streaming-profile"), &repo);
    copy(
        &project.join("packages/python/src/metewand"),
        &repo.join("metewand"),
    );
    let manifest = repo.join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("warmup-runs: 1", "warmup-runs: 0"),
    )
    .unwrap();
    let worker = repo.join("worker.py");
    fs::write(&worker, fs::read_to_string(&worker).unwrap().replace("        x *= 0.5", "        if index == 1:\n            raise RuntimeError('failure after publication')\n        x *= 0.5")).unwrap();
    assert_eq!(
        invoke(
            &repo,
            &["run", "--output-dir", "run", "--max-attempts", "1"]
        )
        .status
        .code(),
        Some(5)
    );
    let before = success(&repo, &["status", "--output-dir", "run"]);
    assert!(!before["observations"].as_array().unwrap().is_empty());
    success(
        &repo,
        &["export", "--output-dir", "run", "--destination", "original"],
    );
    let expected = fs::read(repo.join("original/results.jsonl")).unwrap();
    success(
        &repo,
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "results.tar.gz",
        ],
    );
    success(
        &repo,
        &[
            "archive",
            "import",
            "--archive",
            "results.tar.gz",
            "--destination",
            "imported",
        ],
    );
    let after = success(&repo, &["status", "--output-dir", "imported"]);
    assert_eq!(before["observations"], after["observations"]);
    success(
        &repo,
        &[
            "export",
            "--output-dir",
            "imported",
            "--destination",
            "tables",
        ],
    );
    assert_eq!(
        expected,
        fs::read(repo.join("tables/results.jsonl")).unwrap()
    );
}

#[test]
fn archive_cli_validates_limits_options_and_all_output_formats() {
    let root = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/v1/fixtures/archive-executable-v1");
    copy(&fixture, &root.path().join("collection"));
    for format in ["human", "json", "jsonl"] {
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(root.path())
            .args([
                "archive",
                "verify",
                "--archive",
                "collection",
                "--output",
                format,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        if format != "human" {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            crate::core::public_schemas::public_schema_catalog()
                .unwrap()
                .validate(
                    crate::core::public_schemas::PublicSchema::ArchiveOutput.repository_path(),
                    &value,
                )
                .unwrap();
        }
    }
    let limited = invoke(
        root.path(),
        &[
            "archive",
            "verify",
            "--archive",
            "collection",
            "--max-entries",
            "1",
        ],
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&limited.stdout).unwrap()["data"]["code"],
        "archive_limit_exceeded"
    );
    success(
        root.path(),
        &[
            "archive",
            "verify",
            "--archive",
            "collection",
            "--max-entries",
            "10",
            "--max-bytes",
            "10000",
        ],
    );
    for args in [
        vec!["archive", "create"],
        vec![
            "archive",
            "verify",
            "--archive",
            "collection",
            "--max-bytes",
            "0",
        ],
        vec![
            "archive",
            "verify",
            "--archive",
            "collection",
            "--destination",
            "unexpected",
        ],
        vec![
            "archive",
            "verify",
            "--archive",
            "collection",
            "--archive",
            "collection",
        ],
    ] {
        assert_eq!(invoke(root.path(), &args).status.code(), Some(2));
    }
}
