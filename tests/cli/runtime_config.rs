#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};
use tempfile::TempDir;

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

fn repository() -> TempDir {
    let root = TempDir::new().unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    copy_tree(&project.join("examples/python-package"), root.path());
    copy_tree(
        &project.join("packages/python/src/metewand"),
        &root.path().join("metewand"),
    );
    let worker = root.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap().replace(
        "def materialize(context):",
        r#"def materialize(context):
    from pathlib import Path
    assert context.runtime["version"] == 1
    assert "cache_dir" not in context.parameters
    if "provider" in context.runtime["caches"]:
        cache = Path(context.runtime["caches"]["provider"])
        assert cache.is_absolute()
        assert (cache / "values.json").read_bytes() == b"[1,2,3]"
        (cache / "used").write_text("materialized")"#,
    );
    fs::write(worker, source).unwrap();
    fs::create_dir(root.path().join("settings")).unwrap();
    for name in ["a", "b"] {
        let cache = root.path().join(format!("downloads-{name}"));
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("values.json"), "[1,2,3]").unwrap();
        fs::write(
            root.path().join(format!("settings/{name}.yaml")),
            format!("version: 1\ncaches:\n  provider: ../downloads-{name}\n"),
        )
        .unwrap();
    }
    root
}

fn invoke(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(args)
        .args(["--output", "json"])
        .output()
        .unwrap()
}

fn document(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn run(root: &Path, destination: &str, config: Option<&str>, cache: Option<&str>) -> Value {
    let mut args = vec!["run", "--output-dir", destination, "--max-attempts", "1"];
    if let Some(config) = config {
        args.extend(["--runtime-config", config]);
    }
    if let Some(cache) = cache {
        args.extend(["--cache-dir", cache]);
    }
    let output = invoke(root, &args);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempt = &report["data"]["attempts"][0];
    assert_eq!(attempt["status"], "accepted", "{report}");
    let record = document(
        &root
            .join(destination)
            .join(attempt["attempt_directory"].as_str().unwrap())
            .join("attempt.json"),
    );
    json!({"attempt": attempt, "context": record["provenance"]["data"]["context"]})
}

#[test]
fn runtime_cache_paths_preserve_scientific_identities_seeds_and_dataset_bytes() {
    let root = repository();
    let first = run(root.path(), "run-a", Some("settings/a.yaml"), None);
    let second = run(root.path(), "run-b", Some("settings/b.yaml"), None);
    let default = run(root.path(), "run-default", None, None);
    for other in [&second, &default] {
        for key in ["logical_slot_id", "resolved_slot_id", "result", "metrics"] {
            assert_eq!(first["attempt"][key], other["attempt"][key], "{key}");
        }
        for key in ["dataset_instance", "problem_instance"] {
            assert_eq!(first["context"][key], other["context"][key], "{key}");
        }
        for key in ["derivation_id", "derivation", "output"] {
            assert_eq!(
                first["context"]["dataset_materialization"][key],
                other["context"]["dataset_materialization"][key],
                "{key}"
            );
        }
    }
    for (name, record) in [("a", first), ("b", second)] {
        let runtime = json!({"version":1,"caches":{"provider":root.path().join(format!("downloads-{name}"))}});
        assert_eq!(record["context"]["runtime"], runtime);
        assert_eq!(
            record["context"]["dataset_materialization"]["runtime"],
            runtime
        );
        assert!(root.path().join(format!("downloads-{name}/used")).exists());
    }
    assert!(default["context"].get("runtime").is_none());

    let manifest = root.path().join("metewand.yaml");
    let source = fs::read_to_string(&manifest)
        .unwrap()
        .replace("default: [1, 2, 3]", "default: [2, 3, 4]");
    fs::write(manifest, source).unwrap();
    let changed = run(root.path(), "run-changed", Some("settings/a.yaml"), None);
    assert_ne!(first_slot(&default), first_slot(&changed));
    assert_ne!(
        default["context"]["dataset_materialization"]["derivation"]["seed"],
        changed["context"]["dataset_materialization"]["derivation"]["seed"]
    );
}

fn first_slot(record: &Value) -> &Value {
    &record["attempt"]["logical_slot_id"]
}

#[test]
fn runtime_cache_changes_reuse_artifacts_and_retain_original_materialization_evidence() {
    let root = repository();
    let first = run(
        root.path(),
        "run-a",
        Some("settings/a.yaml"),
        Some("artifacts"),
    );
    let second = run(
        root.path(),
        "run-b",
        Some("settings/b.yaml"),
        Some("artifacts"),
    );
    assert_eq!(first_slot(&first), first_slot(&second));
    assert_eq!(
        first["attempt"]["resolved_slot_id"],
        second["attempt"]["resolved_slot_id"]
    );
    assert!(root.path().join("downloads-a/used").exists());
    assert!(!root.path().join("downloads-b/used").exists());
    assert_eq!(
        first["context"]["dataset_materialization"],
        second["context"]["dataset_materialization"]
    );
    assert_eq!(
        second["context"]["runtime"]["caches"]["provider"],
        root.path().join("downloads-b").to_str().unwrap()
    );
}

#[test]
fn runtime_configuration_is_explicit_strict_and_read_only_during_checks() {
    let root = repository();
    let path = root.path().join("settings/local.yaml");
    for source in [
        "version: 2\ncaches: {}\n",
        "version: 1\ncaches: {}\nparameters: {}\n",
        "version: 1\ncaches: {provider: ''}\n",
        "version: 1\ncaches: {provider: 7}\n",
        "version: 1\ncaches: {provider: first, provider: second}\n",
        "version: 1\ncaches: {'bad/name': here}\n",
    ] {
        fs::write(&path, source).unwrap();
        let output = invoke(
            root.path(),
            &["check", "--runtime-config", "settings/local.yaml"],
        );
        assert!(!output.status.success(), "{source}: {output:?}");
        let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            diagnostic["data"]["code"], "invalid_runtime_config",
            "{diagnostic}"
        );
    }
    fs::write(&path, "version: 1\ncaches: {provider: ../new-downloads}\n").unwrap();
    for args in [
        vec!["check", "--runtime-config", "settings/local.yaml"],
        vec![
            "check",
            "--workers",
            "--runtime-config",
            "settings/local.yaml",
        ],
    ] {
        let output = invoke(root.path(), &args);
        assert!(output.status.success(), "{output:?}");
        crate::core::public_schemas::public_schema_catalog()
            .unwrap()
            .validate(
                crate::core::public_schemas::PublicSchema::MachineOutput.repository_path(),
                &serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            )
            .unwrap();
    }
    assert!(!root.path().join("new-downloads").exists());
    assert!(!root.path().join(".metewand").exists());
    fs::write(
        root.path().join("metewand.local.yaml"),
        "invalid: configuration",
    )
    .unwrap();
    assert!(invoke(root.path(), &["check"]).status.success());
}

#[test]
fn runtime_caches_must_be_separate_from_sources_output_and_artifact_cache() {
    for cache in [
        "..",
        "../worker.py",
        "../metewand",
        "../run",
        "../artifacts/nested",
    ] {
        let root = repository();
        fs::create_dir(root.path().join("artifacts")).unwrap();
        fs::write(
            root.path().join("settings/local.yaml"),
            format!("version: 1\ncaches: {{provider: {cache}}}\n"),
        )
        .unwrap();
        let output = invoke(
            root.path(),
            &[
                "run",
                "--runtime-config",
                "settings/local.yaml",
                "--output-dir",
                "run",
                "--cache-dir",
                "artifacts",
            ],
        );
        assert!(!output.status.success(), "{cache}: {output:?}");
        assert!(!root.path().join("run").exists());
        assert_eq!(
            fs::read_dir(root.path().join("artifacts")).unwrap().count(),
            0
        );
    }
}

#[test]
fn explicit_runtime_bindings_require_materializer_support_before_construction() {
    let root = TempDir::new().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/generated-dataset"),
        root.path(),
    );
    fs::write(
        root.path().join("runtime.yaml"),
        "version: 1\ncaches: {provider: downloads}\n",
    )
    .unwrap();
    for args in [
        vec!["check", "--workers", "--runtime-config", "runtime.yaml"],
        vec![
            "run",
            "--output-dir",
            "run",
            "--runtime-config",
            "runtime.yaml",
        ],
    ] {
        let output = invoke(root.path(), &args);
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("RuntimeConfig"),
            "{output:?}"
        );
        assert!(!root.path().join("run").exists());
        assert!(!root.path().join("downloads").exists());
    }
}
