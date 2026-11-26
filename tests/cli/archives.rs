#![cfg(any(unix, windows))]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    core::archive::ArchiveReport,
    runtime::archive::{ArchiveLimits, verify_archive},
};

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
fn archive_restore_rebuilds_a_resumable_run_after_losing_the_database() {
    let root = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    let manifest = root.path().join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("measurement-repetitions: 1", "measurement-repetitions: 3"),
    )
    .unwrap();
    success(
        root.path(),
        &["run", "--output-dir", "run", "--max-attempts", "1"],
    );
    let before = success(root.path(), &["status", "--output-dir", "run"]);
    assert!(before["summary"]["pending_slots"].as_u64().unwrap() > 0);
    success(
        root.path(),
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "saved.tar.gz",
        ],
    );
    let saved = fs::read(root.path().join("saved.tar.gz")).unwrap();
    fs::remove_dir_all(root.path().join("run")).unwrap();

    let report = success(
        root.path(),
        &[
            "archive",
            "restore",
            "--archive",
            "saved.tar.gz",
            "--destination",
            "restored",
        ],
    );
    assert_eq!(report["operation"], "restore");
    assert!(root.path().join("restored/run.sqlite3").is_file());
    assert!(!root.path().join("restored/archive.json").exists());
    let after = success(root.path(), &["status", "--output-dir", "restored"]);
    for key in ["summary", "slots", "attempts", "observations", "comparison"] {
        assert_eq!(before[key], after[key], "{key}");
    }
    success(
        root.path(),
        &[
            "archive",
            "create",
            "--output-dir",
            "restored",
            "--destination",
            "round-trip.tar.gz",
        ],
    );
    assert_eq!(
        saved,
        fs::read(root.path().join("round-trip.tar.gz")).unwrap()
    );
    let resumed = success(root.path(), &["run", "--output-dir", "restored"]);
    assert_eq!(resumed["attempts"].as_array().unwrap().len(), 2);
    let finished = success(root.path(), &["status", "--output-dir", "restored"]);
    assert_eq!(finished["summary"]["pending_slots"], 0);
    assert_eq!(finished["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(saved, fs::read(root.path().join("saved.tar.gz")).unwrap());
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
    success(
        &repo,
        &[
            "export",
            "--output-dir",
            "run",
            "--destination",
            "analysis",
            "--format",
            "json",
            "--require-complete",
        ],
    );
    let expected_analysis = fs::read(repo.join("analysis/results.json")).unwrap();
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
    success(
        root.path(),
        &[
            "export",
            "--output-dir",
            "imported",
            "--destination",
            "analysis",
            "--format",
            "json",
            "--require-complete",
        ],
    );
    assert_eq!(
        expected_analysis,
        fs::read(root.path().join("analysis/results.json")).unwrap()
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

#[cfg(unix)]
fn artifact_file_copies(root: &Path, name: &str) -> std::collections::BTreeMap<(u64, u64), usize> {
    use std::os::unix::fs::MetadataExt;

    fn visit(root: &Path, name: &str, copies: &mut std::collections::BTreeMap<(u64, u64), usize>) {
        for entry in fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                visit(&entry.path(), name, copies);
            } else if entry.file_name() == name {
                *copies.entry((metadata.dev(), metadata.ino())).or_default() += 1;
            }
        }
    }

    let mut copies = std::collections::BTreeMap::new();
    visit(&root.join("artifacts"), name, &mut copies);
    copies
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
    if example == "buffered-profile" {
        let worker = repo.join("worker.py");
        let source = fs::read_to_string(&worker).unwrap();
        fs::write(
            worker,
            source
                .replace(
                    r#"snapshot = lambda: {"x": x}"#,
                    r#"snapshot = lambda: trace.snapshot({"x": x}, files=[("arrays/coefficients.bin", "application/octet-stream", bytes(range(256)))])"#,
                )
                .replace("            x *= 0.5", "            if index >= 2:\n                x *= 0.5"),
        )
        .unwrap();
    }
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
    // Allow slow worker startup on shared Windows runners before testing archive fidelity.
    let manifest = repo.join("metewand.yaml");
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write(
        manifest,
        format!("{source}\nworker-timeouts:\n  hello: 30 s\n"),
    )
    .unwrap();
    if example == "fresh-sequence" {
        success(
            &repo,
            &["run", "--output-dir", "run", "--max-attempts", "1"],
        );
        assert_eq!(
            fs::read_dir(repo.join("run/fresh-memberships"))
                .unwrap()
                .count(),
            3,
            "membership is available for the two repetitions without attempts",
        );
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
    success(
        &repo,
        &[
            "archive",
            "restore",
            "--archive",
            "results.tar.gz",
            "--destination",
            "restored",
        ],
    );
    #[cfg(unix)]
    if example == "buffered-profile" {
        let live = artifact_file_copies(&repo.join("run"), "coefficients.bin");
        let restored = artifact_file_copies(&repo.join("restored"), "coefficients.bin");
        let metrics = artifact_file_copies(&repo.join("run"), "metrics.json");
        assert_eq!(metrics.values().sum::<usize>(), 15);
        assert_eq!(metrics.values().filter(|paths| **paths == 3).count(), 1);
        assert_eq!(live.values().sum::<usize>(), 30);
        assert_eq!(live.len(), 5);
        assert!(live.values().all(|paths| *paths == 6));
        assert_eq!(restored.values().copied().collect::<Vec<_>>(), [30]);
    }
    let restored = success(&repo, &["status", "--output-dir", "restored"]);
    for key in [
        "summary",
        "slots",
        "attempts",
        "observations",
        "comparison",
        "fresh_sequences",
    ] {
        assert_eq!(before[key], restored[key], "{example}: restored {key}");
    }
    let resumed = success(&repo, &["run", "--output-dir", "restored"]);
    assert!(resumed["attempts"].as_array().unwrap().is_empty());
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
    #[cfg(unix)]
    if example == "buffered-profile" {
        let imported = artifact_file_copies(&root.path().join("imported/data"), "coefficients.bin");
        assert_eq!(imported.values().copied().collect::<Vec<_>>(), [30]);
    }
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
    let membership_files = records
        .iter()
        .filter(|row| row["kind"] == "stored-artifact")
        .map(|row| &row["record"])
        .filter(|row| {
            row["path"]
                .as_str()
                .unwrap()
                .starts_with("fresh-memberships/")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        membership_files.len(),
        records
            .iter()
            .filter(|row| row["kind"] == "fresh-policy")
            .count(),
    );
    for membership in &membership_files {
        let path = root
            .path()
            .join("imported/data")
            .join(membership["path"].as_str().unwrap());
        let document = read_json(&path);
        assert_eq!(document["schema_version"], 1);
        assert_eq!(document["members"].as_array().unwrap().len(), 4);
        let edges = records.iter().filter(|row| {
            row["kind"] == "attempt-artifact"
                && row["record"]["role"] == "fresh-membership"
                && row["record"]["artifact_id"] == membership["id"]
        });
        assert!(edges.clone().count() > 1);
        for edge in edges {
            let attempt = &records
                .iter()
                .find(|row| {
                    row["kind"] == "attempt" && row["record"]["id"] == edge["record"]["attempt_id"]
                })
                .unwrap()["record"];
            let artifact = &records
                .iter()
                .find(|row| {
                    row["kind"] == "stored-artifact"
                        && row["record"]["id"] == attempt["artifact_id"]
                })
                .unwrap()["record"];
            let path = root
                .path()
                .join("imported/data")
                .join(artifact["path"].as_str().unwrap());
            let outcome = read_json(&path.join("outcome.json"));
            let fresh = &outcome["coordination"]["fresh"];
            assert!(fresh.get("members").is_none());
            assert!(fresh.get("policy").is_none());
            assert_eq!(fresh["measurement_index"], document["measurement_index"]);
            assert_eq!(fresh["resolved_run_id"], document["resolved_run_id"]);
        }
    }
    let collection = root.path().join("changed-membership");
    copy(&root.path().join("imported"), &collection);
    let mut evidence = original.clone();
    let stored = &mut evidence["records"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| {
            row["kind"] == "stored-artifact" && row["record"]["id"] == membership_files[0]["id"]
        })
        .unwrap()["record"];
    let path = collection
        .join("data")
        .join(stored["path"].as_str().unwrap());
    let mut membership = read_json(&path);
    membership["members"][0]["control"] = serde_json::json!({"iterations": 99});
    write_json(&path, membership);
    stored["sha256"] = serde_json::json!(sha256(&fs::read(path).unwrap()));
    write_json(&collection.join("evidence.json"), evidence);
    refresh_inventory(&collection.join("archive.json"));
    let error = verify_archive(&collection, ArchiveLimits::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("stored fresh membership disagrees"),
        "{error}"
    );

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
fn long_buffered_trace_round_trips_and_reports_a_malformed_last_row() {
    const TRACE_POINTS: usize = 25_001;
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    copy(&project.join("examples/buffered-profile"), root);
    copy(
        &project.join("packages/python/src/metewand"),
        &root.join("metewand"),
    );
    let manifest = root.join("metewand.yaml");
    let mut source = fs::read_to_string(&manifest).unwrap();
    let fixed = source.find("  - name: fixed\n").unwrap();
    let policies = source.find("execution-policies:\n").unwrap();
    source.replace_range(fixed..policies, "");
    fs::write(
        manifest,
        source
            .replace("[observer, direct]", "[observer]")
            .replace("measurement-repetitions: 2", "measurement-repetitions: 1")
            .replace("warmup-runs: 1", "warmup-runs: 0")
            .replace("timeout: 5 s", "timeout: 45 s")
            .replace("max-buffer-bytes: 65536", "max-buffer-bytes: 8388608")
            .replace(
                "max-trace-points: 100\n",
                &format!("max-trace-points: {TRACE_POINTS}\n"),
            )
            + "\nworker-timeouts:\n  hello: 30 s\n",
    )
    .unwrap();
    let worker = root.join("worker.py");
    let last = TRACE_POINTS - 1;
    let source = fs::read_to_string(&worker)
        .unwrap()
        .replace(
            "index = 0",
            "index = 0\n        trace.clock = lambda: index + 1",
        )
        .replace("index >= self.maximum_updates", &format!("index >= {last}"))
        .replace(
            "x *= 0.5",
            &format!("x = 0.0 if index + 1 == {last} else x"),
        );
    fs::write(worker, source).unwrap();
    success(root, &["run", "--output-dir", "run"]);

    let limited = |args: &[&str]| -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_metewand"));
        command
            .current_dir(root)
            .args(args)
            .args(["--output", "json"]);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;

            // The child has enough memory for metadata and one row. Keeping a
            // full attempt record per trace point exceeds this limit.
            // SAFETY: The child changes only its own address space limit.
            unsafe {
                command.pre_exec(|| {
                    let limit = libc::rlimit {
                        rlim_cur: 256 * 1024 * 1024,
                        rlim_max: 256 * 1024 * 1024,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        command.output().unwrap()
    };
    let report = |args: &[&str]| -> Value {
        let output = limited(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        value["data"].clone()
    };
    let archive = |args: &[&str]| -> ArchiveReport {
        let args = std::iter::once("archive")
            .chain(args.iter().copied())
            .collect::<Vec<_>>();
        serde_json::from_value(report(&args)).unwrap()
    };

    let collection = root.join("imported");
    let restored = root.join("restored");
    let created = archive(&[
        "create",
        "--output-dir",
        "run",
        "--destination",
        "results.tar.gz",
    ]);
    for args in [
        &["verify", "--archive", "results.tar.gz"][..],
        &[
            "import",
            "--archive",
            "results.tar.gz",
            "--destination",
            "imported",
        ],
        &[
            "restore",
            "--archive",
            "results.tar.gz",
            "--destination",
            "restored",
        ],
    ] {
        assert_eq!(archive(args).manifest_sha256, created.manifest_sha256);
    }

    let manifest = read_json(&collection.join("archive.json"));
    let archive_trace = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|file| file["path"].as_str())
        .find(|path| path.ends_with("/trace.jsonl"))
        .unwrap();
    let trace = archive_trace.strip_prefix("data/").unwrap();
    let recorded = fs::read(root.join("run").join(trace)).unwrap();
    assert_eq!(
        recorded.iter().filter(|&&byte| byte == b'\n').count(),
        TRACE_POINTS
    );
    assert_eq!(fs::read(collection.join(archive_trace)).unwrap(), recorded);
    assert_eq!(fs::read(restored.join(trace)).unwrap(), recorded);
    archive(&[
        "create",
        "--output-dir",
        "restored",
        "--destination",
        "restored.tar.gz",
    ]);
    assert_eq!(
        fs::read(root.join("results.tar.gz")).unwrap(),
        fs::read(root.join("restored.tar.gz")).unwrap(),
    );

    let export = |source, destination| {
        [
            "export",
            "--output-dir",
            source,
            "--format",
            "json",
            "--destination",
            destination,
            "--require-complete",
        ]
    };
    let exported = report(&export("run", "analysis"));
    assert_eq!(exported["records"], TRACE_POINTS + 3);
    assert_eq!(exported["rows"], 1);
    let attempt_id = exported["manifest"]["attempt_ids"][0].as_str().unwrap();
    let analysis = root.join("analysis/results.json");
    assert_eq!(
        read_json(&analysis)["observations"][0]["profile"]["trace_index"],
        last
    );
    let selector = format!("{attempt_id}:check:{last}");
    let mut args = export("imported", "selected").to_vec();
    args.extend(["--id", &selector]);
    let selected = report(&args);
    assert_eq!(selected["records"], exported["records"]);
    assert_eq!(selected["rows"], exported["rows"]);
    let mut manifest = exported["manifest"].clone();
    manifest["selector"] = serde_json::json!([selector]);
    assert_eq!(selected["manifest"], manifest);
    assert_eq!(
        fs::read(root.join("selected/results.json")).unwrap(),
        fs::read(analysis).unwrap()
    );
    for missing in [
        format!("{attempt_id}:check:{TRACE_POINTS}"),
        format!("{attempt_id}:check:0{last}"),
    ] {
        let mut args = export("run", "missing").to_vec();
        args.extend(["--id", &missing]);
        let output = limited(&args);
        assert!(!output.status.success(), "{output:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["data"]["code"], "export_id_not_found");
        assert!(!root.join("missing").exists());
    }

    let mut rows = recorded
        .strip_suffix(b"\n")
        .unwrap()
        .rsplitn(2, |byte| *byte == b'\n');
    let mut terminal: Value = serde_json::from_slice(rows.next().unwrap()).unwrap();
    terminal["check_index"] = Value::String("invalid".into());
    let mut malformed = rows.next().unwrap().to_vec();
    malformed.push(b'\n');
    malformed.extend(serde_json::to_vec(&terminal).unwrap());
    malformed.push(b'\n');
    fs::write(collection.join(archive_trace), malformed).unwrap();

    // Refresh the registered digests so verification reaches the malformed row.
    let (buffer, _) = trace.rsplit_once('/').unwrap();
    let directory = collection.join("data").join(buffer);
    refresh_inventory(&directory.join("buffer.json"));
    let marker = directory.join("manifest.json");
    refresh_inventory(&marker);
    let digest = crate::core::materialization::hex(
        crate::runtime::local_tree::hash_local_tree(&directory).unwrap(),
    );
    let evidence_path = collection.join("evidence.json");
    let mut evidence = read_json(&evidence_path);
    let artifact = &mut evidence["records"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["kind"] == "stored-artifact" && record["record"]["path"] == buffer)
        .unwrap()["record"];
    artifact["sha256"] = serde_json::json!(digest);
    artifact["manifest_sha256"] = serde_json::json!(sha256(&fs::read(marker).unwrap()));
    let old_id = artifact["id"].clone();
    let new_id = serde_json::json!(format!("run-store-v1:other-{digest}"));
    replace_id(&mut evidence, &old_id, &new_id);
    write_json(&evidence_path, evidence);
    refresh_inventory(&collection.join("archive.json"));

    let error = verify_archive(&collection, ArchiveLimits::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains(attempt_id), "{error}");
    assert!(
        error.contains(&format!("trace.jsonl line {TRACE_POINTS}")),
        "{error}"
    );
    assert!(error.contains("invalid type"), "{error}");
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
    success(
        &repo,
        &[
            "archive",
            "restore",
            "--archive",
            "results.tar.gz",
            "--destination",
            "restored",
        ],
    );
    let restored = success(&repo, &["status", "--output-dir", "restored"]);
    for key in [
        "summary",
        "slots",
        "attempts",
        "observations",
        "diagnostics",
    ] {
        assert_eq!(before[key], restored[key], "restored {key}");
    }
    let slot = before["attempts"][0]["slot_id"].as_str().unwrap();
    let skipped = invoke(
        &repo,
        &["run", "--output-dir", "restored", "--select", slot],
    );
    let skipped: Value = serde_json::from_slice(&skipped.stdout).unwrap();
    assert!(skipped["data"]["attempts"].as_array().unwrap().is_empty());
    let retried = invoke(
        &repo,
        &[
            "run",
            "--output-dir",
            "restored",
            "--select",
            slot,
            "--retry",
        ],
    );
    assert_eq!(retried.status.code(), Some(5));
    let history = success(&repo, &["status", "--output-dir", "restored"]);
    assert_eq!(history["attempts"].as_array().unwrap().len(), 4);
    assert_eq!(history["attempts"][3]["retry_index"], 3);
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

#[cfg(unix)]
#[test]
fn scheduler_interruption_archives_and_resumes_committed_work() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        &repo,
    );
    let gate = root.path().join("gate");
    let marker = root.path().join("worker-pid");
    let manifest = repo.join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("measurement-repetitions: 1", "measurement-repetitions: 5")
            .replace(
                "    timing-scope:",
                "    max-concurrency: 2\n    timing-scope:",
            ),
    )
    .unwrap();
    let worker = repo.join("worker.py");
    fs::write(
        &worker,
        fs::read_to_string(&worker).unwrap().replace(
            "elif method == \"execute\":",
            &format!(
                "elif method == \"execute\":\n            import os, time\n            if Path({gate:?}).exists():\n                with Path({marker:?}).open('a') as handle:\n                    print(os.getpid(), file=handle)\n                time.sleep(30)"
            ),
        ),
    )
    .unwrap();
    let first = success(
        &repo,
        &[
            "run",
            "--shared",
            "--output-dir",
            "run",
            "--max-attempts",
            "2",
        ],
    );
    fs::write(&gate, "").unwrap();
    let events = interrupt_runner(&repo, &marker, &[], 2);
    assert_eq!(
        events.matches("\"event\":\"attempt_started\"").count(),
        2,
        "{events}"
    );
    assert!(
        events.contains("\"code\":\"interrupted_attempt\""),
        "{events}"
    );
    assert!(events.contains("\"code\":\"run_interrupted\""), "{events}");
    fs::remove_file(gate).unwrap();

    let receipt = success(&repo, &["recover", "--output-dir", "run"]);
    assert_eq!(receipt["recovered_attempts"], 0);
    assert_eq!(receipt["remaining_attempts"], 0);
    success(
        &repo,
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "saved.tar.gz",
        ],
    );
    success(&repo, &["archive", "verify", "--archive", "saved.tar.gz"]);
    success(
        &repo,
        &[
            "archive",
            "restore",
            "--archive",
            "saved.tar.gz",
            "--destination",
            "restored",
        ],
    );
    let resumed = success(&repo, &["run", "--output-dir", "restored"]);
    assert_eq!(resumed["attempts"].as_array().unwrap().len(), 3);
    let final_status = success(&repo, &["status", "--output-dir", "restored"]);
    let attempts = final_status["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 7);
    assert!(
        attempts
            .iter()
            .any(|attempt| attempt["id"] == first["attempts"][0]["attempt_id"])
    );
}

#[test]
fn recovery_respects_leases_without_loading_benchmark_code() {
    use crate::runtime::run_store::{LeaseRequest, RetryAuthorization, RunStore};
    use std::time::{SystemTime, UNIX_EPOCH};

    let root = tempfile::tempdir().unwrap();
    let repo = root.path();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        repo,
    );
    let manifest = repo.join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("measurement-repetitions: 1", "measurement-repetitions: 3"),
    )
    .unwrap();
    for (home, shared) in [("local", false), ("shared", true)] {
        let mut args = vec!["run", "--output-dir", home, "--max-attempts", "1"];
        if shared {
            args.push("--shared");
        }
        success(repo, &args);
        let status = success(repo, &["status", "--output-dir", home]);
        let slots: Vec<_> = status["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["attempt_ids"].as_array().is_some_and(Vec::is_empty))
            .filter_map(|slot| slot["resolved_slot_id"].as_str())
            .collect();
        assert_eq!(slots.len(), 2);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let mut store = RunStore::open(&repo.join(home)).unwrap();
        for (slot, started, expires) in [
            (slots[0], now - 60_000_000_000, now - 30_000_000_000),
            (slots[1], now, now + 600_000_000_000),
        ] {
            store
                .reserve(
                    slot.parse().unwrap(),
                    RetryAuthorization::FirstAttempt,
                    LeaseRequest {
                        id: format!("test:{slot}"),
                        owner_token: format!("test:{slot}"),
                        expires_at_ns: expires,
                    },
                    started,
                )
                .unwrap();
        }
        drop(store);
        let moved_manifest = repo.join("manifest-away.yaml");
        fs::rename(&manifest, &moved_manifest).unwrap();
        for recovered in [1, 0] {
            let result = invoke(repo, &["recover", "--output-dir", home]);
            assert_eq!(result.status.code(), Some(4), "{result:?}");
            let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(receipt["data"]["recovered_attempts"], recovered);
            assert_eq!(receipt["data"]["remaining_attempts"], 1);
        }
        let mut store = RunStore::open(&repo.join(home)).unwrap();
        store.recover(|| now + 601_000_000_000).unwrap();
        drop(store);
        let settled = success(repo, &["recover", "--output-dir", home]);
        assert_eq!(settled["remaining_attempts"], 0);
        success(
            repo,
            &[
                "archive",
                "create",
                "--output-dir",
                home,
                "--destination",
                &format!("{home}.tar.gz"),
            ],
        );
        fs::rename(moved_manifest, &manifest).unwrap();
    }
    assert!(
        !invoke(repo, &["recover", "--output-dir", "missing"])
            .status
            .success()
    );
    assert!(!repo.join("missing").exists());
    success(
        repo,
        &[
            "archive",
            "import",
            "--archive",
            "local.tar.gz",
            "--destination",
            "collection",
        ],
    );
    assert!(
        !invoke(repo, &["recover", "--output-dir", "collection"])
            .status
            .success()
    );
}

#[cfg(unix)]
pub(super) fn interrupt_runner(repo: &Path, marker: &Path, args: &[&str], lanes: usize) -> String {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut runner = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(repo)
        .args(["run", "--output-dir", "run", "--output", "jsonl"])
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let worker_pids = loop {
        let pids: Vec<i32> = fs::read_to_string(marker)
            .unwrap_or_default()
            .lines()
            .filter_map(|pid| pid.parse().ok())
            .collect();
        if pids.len() == lanes {
            break pids;
        }
        if Instant::now() >= deadline || runner.try_wait().unwrap().is_some() {
            let _ = runner.kill();
            panic!(
                "workers did not enter execution: {:?}",
                runner.wait_with_output()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // SAFETY: These are the PIDs of the coordinator and worker started by the fixture.
    assert_eq!(unsafe { libc::kill(runner.id() as i32, libc::SIGUSR1) }, 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while runner.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = runner.kill();
            panic!(
                "runner did not settle promptly: {:?}",
                runner.wait_with_output()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let stopped = runner.wait_with_output().unwrap();
    assert_eq!(stopped.status.code(), Some(5), "{stopped:?}");
    for pid in worker_pids {
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
    let events = String::from_utf8(stopped.stdout).unwrap();
    assert!(events.contains("\"code\":\"run_interrupted\""), "{events}");
    events
}

#[cfg(unix)]
#[test]
fn interrupted_calibration_and_streaming_checkpoints_survive_archives() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    copy(&project.join("examples/streaming-profile"), &repo);
    copy(
        &project.join("packages/python/src/metewand"),
        &repo.join("metewand"),
    );
    let gate = root.path().join("gate");
    let marker = root.path().join("worker-pid");
    let manifest = repo.join("metewand.yaml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("warmup-runs: 1", "warmup-runs: 0")
            .replace("timeout: 5 s", "timeout: 30 s")
            .replacen(
                "max-active-time: 2 s",
                "adaptive-budget: {initial-pilot-time: 20 s, minimum-time: 20 s}",
                1,
            ),
    )
    .unwrap();
    let worker = repo.join("worker.py");
    fs::write(
        &worker,
        fs::read_to_string(&worker).unwrap().replace(
            "    stream = context.stream",
            "    from pathlib import Path\n    import os, time\n    stream = context.stream",
        ).replace(
            "        x *= 0.5",
            &format!("        if index == 2 and Path({gate:?}).exists():\n            Path({marker:?}).write_text(str(os.getpid()))\n            time.sleep(30)\n        x *= 0.5"),
        ),
    ).unwrap();
    for calibration in [true, false] {
        fs::write(&gate, "").unwrap();
        interrupt_runner(&repo, &marker, &["target"], 1);
        fs::remove_file(&gate).unwrap();
        fs::remove_file(&marker).unwrap();
        let before = success(&repo, &["status", "--output-dir", "run"]);
        assert!(
            !before["observations"].as_array().unwrap().is_empty(),
            "{before}"
        );
        let archive = if calibration {
            "pilot.tar.gz"
        } else {
            "measured.tar.gz"
        };
        success(
            &repo,
            &[
                "archive",
                "create",
                "--output-dir",
                "run",
                "--destination",
                archive,
            ],
        );
        success(
            &repo,
            &[
                "archive",
                "restore",
                "--archive",
                archive,
                "--destination",
                "restored",
            ],
        );
        let after = success(&repo, &["status", "--output-dir", "restored"]);
        for key in ["attempts", "observations", "adaptive_budgets"] {
            assert_eq!(after[key], before[key], "{key}");
        }
        if calibration {
            assert!(before["adaptive_budgets"].is_null());
            let report = success(
                &repo,
                &[
                    "run",
                    "target",
                    "--output-dir",
                    "restored",
                    "--max-attempts",
                    "1",
                ],
            );
            assert_eq!(report["calibration_attempts"].as_array().unwrap().len(), 1);
            assert_eq!(report["adaptive_budgets"].as_array().unwrap().len(), 1);
            fs::remove_dir_all(repo.join("run")).unwrap();
            fs::rename(repo.join("restored"), repo.join("run")).unwrap();
        } else {
            let report = success(&repo, &["run", "target", "--output-dir", "restored"]);
            assert!(report["calibration_attempts"].is_null());
            assert_eq!(report["adaptive_budgets"], before["adaptive_budgets"]);
            assert_eq!(report["attempts"].as_array().unwrap().len(), 2);
        }
    }
}

#[test]
fn invalid_recovery_targets_are_untouched_and_identified() {
    let root = tempfile::tempdir().unwrap();
    for (name, marker) in [("sqlite", "run.sqlite3"), ("shared", "journal")] {
        let home = root.path().join(name);
        fs::create_dir(&home).unwrap();
        if marker == "journal" {
            fs::create_dir(home.join(marker)).unwrap();
        } else {
            fs::write(home.join(marker), []).unwrap();
        }
        let output = invoke(root.path(), &["recover", "--output-dir", name]);
        assert!(!output.status.success());
        let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(diagnostic["data"]["code"], "run_recovery_failed");
        assert_eq!(diagnostic["data"]["details"]["command"], "recover");
        let entries: Vec<_> = fs::read_dir(home)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, [marker]);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scheduler_interruption_settles_a_batch_behind_a_full_event_pipe() {
    use std::{
        io::Write,
        process::Stdio,
        time::{Duration, Instant},
    };

    let root = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    let (reader, writer) = rustix::pipe::pipe().unwrap();
    let capacity = rustix::pipe::fcntl_getpipe_size(&writer).unwrap();
    let mut writer = fs::File::from(writer);
    // Leave space for run_started, then force the first attempt_started write
    // to wait with its slot already reserved.
    let first = b"{\"schema_version\":1,\"kind\":\"event\",\"data\":{\"command\":\"run\",\"sequence\":0,\"event\":\"run_started\",\"attempt_slots\":1}}\n";
    writer
        .write_all(&vec![b' '; capacity - first.len()])
        .unwrap();
    let mut runner = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root.path())
        .args(["run", "--output-dir", "run", "--output", "jsonl"])
        .stdout(Stdio::from(writer))
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = invoke(root.path(), &["status", "--output-dir", "run"]);
        if let Ok(value) = serde_json::from_slice::<Value>(&status.stdout)
            && value["data"]["summary"]["active_attempts"] == 1
        {
            break;
        }
        if Instant::now() >= deadline || runner.try_wait().unwrap().is_some() {
            let _ = runner.kill();
            let _ = runner.wait();
            panic!("runner did not reserve a slot behind the full pipe: {status:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: The runner is the child process whose open attempt was observed.
    assert_eq!(unsafe { libc::kill(runner.id() as i32, libc::SIGUSR1) }, 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = runner.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = runner.kill();
            let _ = runner.wait();
            panic!("runner did not finish with a full event pipe");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(reader);
    assert_eq!(status.code(), Some(5));
    success(
        root.path(),
        &[
            "archive",
            "create",
            "--output-dir",
            "run",
            "--destination",
            "stopped.tar.gz",
        ],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn scheduler_interruption_exits_with_blocked_progress_or_final_report() {
    use std::{
        io::Write,
        process::Stdio,
        time::{Duration, Instant},
    };

    let root = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    for (home, format) in [("progress", "human"), ("report", "json")] {
        let (reader, writer) = rustix::pipe::pipe().unwrap();
        let capacity = rustix::pipe::fcntl_getpipe_size(&writer).unwrap();
        let mut writer = fs::File::from(writer);
        writer.write_all(&vec![b' '; capacity]).unwrap();
        let flags = rustix::fs::fcntl_getfl(&writer).unwrap();
        let blocked = Stdio::from(writer.try_clone().unwrap());
        let mut command = Command::new(env!("CARGO_BIN_EXE_metewand"));
        command
            .current_dir(root.path())
            .args(["run", "--output-dir", home, "--output", format]);
        if format == "human" {
            command.stderr(blocked).stdout(Stdio::null());
        } else {
            command.stdout(blocked).stderr(Stdio::piped());
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = invoke(root.path(), &["status", "--output-dir", home]);
            if let Ok(value) = serde_json::from_slice::<Value>(&status.stdout)
                && value["data"]["summary"]["accepted_attempts"] == 1
            {
                break;
            }
            if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{format} runner did not commit its attempt: {status:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: This is the child process whose committed attempt was observed.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGUSR1) }, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{format} runner did not exit with blocked output");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.code(), Some(5), "{format}: {status}");
        if format == "json" {
            let output = child.wait_with_output().unwrap();
            let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
            assert_eq!(diagnostic["data"]["code"], "run_interrupted");
        }
        assert_eq!(rustix::fs::fcntl_getfl(&writer).unwrap(), flags);
        drop(reader);
    }
}
