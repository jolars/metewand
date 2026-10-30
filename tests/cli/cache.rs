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
