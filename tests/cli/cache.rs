#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use crate::core::{
    materialization::hex,
    public_schemas::{PublicSchema, public_schema_catalog},
};
use crate::runtime::{artifact_store::ArtifactStore, local_tree::hash_local_tree};
use serde_json::Value;

fn command(root: &Path, args: &[&str]) -> (i32, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .args(["--output", "json"])
        .output()
        .unwrap();
    let value: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    let catalog = public_schema_catalog().unwrap();
    catalog
        .validate(PublicSchema::MachineOutput.repository_path(), &value)
        .unwrap();
    if value["kind"] == "result" && args.first() == Some(&"cache") {
        catalog
            .validate(PublicSchema::CacheOutput.repository_path(), &value)
            .unwrap();
    }
    (output.status.code().unwrap(), value)
}

#[test]
fn cache_commands_use_exact_targets_and_explicit_root_and_collection_operations() {
    let root = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(&root.path().join("cache")).unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let id = format!("tree-v1-{}", hex(digest));
    drop(store.import_tree(source.path(), digest).unwrap());
    for args in [
        vec!["cache", "verify", "--cache-dir", "cache", "--id", &id],
        vec!["cache", "gc", "--cache-dir", "cache", "--dry-run"],
        vec![
            "cache",
            "root",
            "add",
            "--cache-dir",
            "cache",
            "--id",
            &id,
            "--root",
            "manual:keep",
        ],
        vec!["cache", "roots", "--cache-dir", "cache"],
    ] {
        let (code, report) = command(root.path(), &args);
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["data"]["targets"][0]["id"], id);
    }
    let (code, report) = command(root.path(), &["cache", "gc", "--cache-dir", "cache"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["data"]["targets"][0]["state"], "retained");
    assert_eq!(
        command(
            root.path(),
            &[
                "cache",
                "root",
                "remove",
                "--cache-dir",
                "cache",
                "--id",
                &id,
                "--root",
                "manual:keep"
            ]
        )
        .0,
        0
    );
    let (code, report) = command(
        root.path(),
        &["cache", "gc", "--cache-dir", "cache", "--id", &id],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["data"]["targets"][0]["state"], "deleted");
    let (code, report) = command(
        root.path(),
        &["cache", "verify", "--cache-dir", "cache", "--id", &id],
    );
    assert_eq!(code, 5, "{report}");
    assert_eq!(report["data"]["targets"][0]["state"], "missing");
}

#[test]
fn invalid_and_missing_cache_arguments_do_not_initialize_a_store() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["cache", "init", "--cache-dir", "missing"],
        vec![
            "cache",
            "init",
            "--shared",
            "--dry-run",
            "--cache-dir",
            "missing",
        ],
        vec!["cache", "gc", "--shared", "--cache-dir", "missing"],
        vec![
            "cache",
            "verify",
            "--cache-dir",
            "missing",
            "--id",
            "../anything",
        ],
        vec!["cache", "verify", "--cache-dir", "missing"],
        vec!["cache", "gc", "--cache-dir", "missing", "--unknown"],
        vec![
            "cache",
            "root",
            "remove",
            "--cache-dir",
            "missing",
            "--root",
            "run:test",
        ],
    ] {
        assert_ne!(command(root.path(), &args).0, 0);
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn failed_run_roots_remain_after_the_output_directory_is_removed() {
    let root = tempfile::tempdir().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fixed-dataset"),
        root.path(),
    );
    let path = root.path().join("worker.py");
    let source = fs::read_to_string(&path).unwrap();
    assert!(source.contains("\"answer\": prepared"));
    fs::write(
        path,
        source.replace("\"answer\": prepared", "\"answer\": prepared + 1"),
    )
    .unwrap();
    let (code, report) = command(
        root.path(),
        &["run", "--output-dir", "failed", "--cache-dir", "cache"],
    );
    assert_eq!(code, 5, "{report}");
    fs::remove_dir_all(root.path().join("failed")).unwrap();
    let (code, report) = command(root.path(), &["cache", "gc", "--cache-dir", "cache"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["data"]["targets"][0]["state"], "retained");
    assert_eq!(
        report["data"]["targets"][0]["roots"][0],
        format!(
            "run:{}",
            root.path().canonicalize().unwrap().join("failed").display()
        )
    );
}

#[test]
fn durable_run_roots_survive_stale_indexes_and_collection_preserves_reconstruction_mappings() {
    let root = tempfile::tempdir().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/generated-dataset"),
        root.path(),
    );
    for output in ["first", "second"] {
        let (code, run) = command(
            root.path(),
            &["run", "--output-dir", output, "--cache-dir", "cache"],
        );
        assert_eq!(code, 0, "{run}");
    }
    let (code, roots) = command(root.path(), &["cache", "roots", "--cache-dir", "cache"]);
    assert_eq!(code, 0, "{roots}");
    let target = &roots["data"]["targets"][0];
    let id = target["id"].as_str().unwrap();
    assert_eq!(target["roots"].as_array().unwrap().len(), 2);
    fs::write(root.path().join("cache/index.jsonl"), "stale").unwrap();
    fs::write(root.path().join("first/index.jsonl"), "stale").unwrap();
    let derivation = fs::read_dir(root.path().join("cache/derivations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mapping = fs::read(derivation.join("dataset.json")).unwrap();
    for (index, name) in ["first", "second"].iter().enumerate() {
        let root_name = format!(
            "run:{}",
            root.path().canonicalize().unwrap().join(name).display()
        );
        let (code, report) = command(
            root.path(),
            &[
                "cache",
                "root",
                "remove",
                "--cache-dir",
                "cache",
                "--id",
                id,
                "--root",
                &root_name,
            ],
        );
        assert_eq!(code, 0, "{report}");
        let (code, report) = command(
            root.path(),
            &["cache", "gc", "--cache-dir", "cache", "--id", id],
        );
        assert_eq!(code, 0, "{report}");
        assert_eq!(
            report["data"]["targets"][0]["state"],
            if index == 0 { "retained" } else { "deleted" }
        );
    }
    assert_eq!(fs::read(derivation.join("dataset.json")).unwrap(), mapping);
    let (code, run) = command(
        root.path(),
        &["run", "--output-dir", "third", "--cache-dir", "cache"],
    );
    assert_eq!(code, 0, "{run}");
    let (_, roots) = command(root.path(), &["cache", "roots", "--cache-dir", "cache"]);
    assert_eq!(roots["data"]["targets"][0]["id"], id);
    assert_eq!(
        roots["data"]["targets"][0]["roots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(fs::read(derivation.join("dataset.json")).unwrap(), mapping);
}

#[test]
fn shared_cache_reuses_generated_data_and_keeps_roots_after_run_relocation() {
    let root = tempfile::tempdir().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/generated-dataset"),
        root.path(),
    );
    let materializations = root.path().join("materializations");
    let worker = root.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap();
    let trace = serde_json::to_string(materializations.to_str().unwrap()).unwrap();
    fs::write(&worker, source.replace(
        "assert role == \"dataset_materializer\"",
        &format!("assert role == \"dataset_materializer\"\n            with open({trace}, \"ab\") as trace: trace.write(b'x')"),
    )).unwrap();
    let (code, init) = command(
        root.path(),
        &["cache", "init", "--shared", "--cache-dir", "cache"],
    );
    assert_eq!(code, 0, "{init}");
    assert_eq!(init["data"]["operation"], "init");
    let (code, local) = command(
        root.path(),
        &["run", "--output-dir", "local", "--cache-dir", "local-cache"],
    );
    assert_eq!(code, 0, "{local}");
    for output in ["first", "second"] {
        let (code, run) = command(
            root.path(),
            &[
                "run",
                "--shared",
                "--output-dir",
                output,
                "--cache-dir",
                "cache",
            ],
        );
        assert_eq!(code, 0, "{run}");
        for (local, shared) in local["data"]["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .zip(run["data"]["attempts"].as_array().unwrap())
        {
            for key in ["logical_slot_id", "resolved_slot_id", "attempt_id"] {
                assert_eq!(local[key], shared[key], "{key}");
            }
        }
    }
    assert_eq!(fs::read(&materializations).unwrap(), b"xx");
    let (_, roots) = command(root.path(), &["cache", "roots", "--cache-dir", "cache"]);
    let id = roots["data"]["targets"][0]["id"].as_str().unwrap();
    let owners = roots["data"]["targets"][0]["roots"].as_array().unwrap();
    assert_eq!(owners.len(), 2);
    assert!(
        owners
            .iter()
            .all(|owner| owner.as_str().unwrap().starts_with("run-v1:"))
    );
    let owner: Value =
        serde_json::from_slice(&fs::read(root.path().join("first/.cache-owner.json")).unwrap())
            .unwrap();
    let owner = owner["root"].as_str().unwrap();
    fs::rename(root.path().join("first"), root.path().join("moved-run")).unwrap();
    fs::rename(root.path().join("cache"), root.path().join("moved-cache")).unwrap();
    let (code, run) = command(
        root.path(),
        &[
            "run",
            "--output-dir",
            "moved-run",
            "--cache-dir",
            "moved-cache",
        ],
    );
    assert_eq!(code, 0, "{run}");
    let (_, relocated) = command(
        root.path(),
        &["cache", "roots", "--cache-dir", "moved-cache"],
    );
    assert_eq!(relocated["data"]["targets"], roots["data"]["targets"]);
    fs::remove_dir_all(root.path().join("moved-run")).unwrap();
    let (_, gc) = command(root.path(), &["cache", "gc", "--cache-dir", "moved-cache"]);
    assert_eq!(gc["data"]["targets"][0]["state"], "retained");
    assert!(owners.iter().any(|name| name == owner));
    for name in owners {
        let (code, report) = command(
            root.path(),
            &[
                "cache",
                "root",
                "remove",
                "--cache-dir",
                "moved-cache",
                "--id",
                id,
                "--root",
                name.as_str().unwrap(),
            ],
        );
        assert_eq!(code, 0, "{report}");
    }
    let (_, gc) = command(
        root.path(),
        &["cache", "gc", "--cache-dir", "moved-cache", "--id", id],
    );
    assert_eq!(gc["data"]["targets"][0]["state"], "deleted");
    assert!(!root.path().join("moved-cache/cache.sqlite3").exists());
    // The retained derivation mapping permits reconstruction after explicit GC.
    let (code, run) = command(
        root.path(),
        &["run", "--output-dir", "third", "--cache-dir", "moved-cache"],
    );
    assert_eq!(code, 0, "{run}");
    let (_, roots) = command(
        root.path(),
        &["cache", "roots", "--cache-dir", "moved-cache"],
    );
    assert_eq!(roots["data"]["targets"][0]["id"], id);
    assert_eq!(fs::read(&materializations).unwrap(), b"xxx");
}
