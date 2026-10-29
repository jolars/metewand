#![cfg(unix)]

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

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

fn repository() -> TempDir {
    let root = TempDir::new().unwrap();
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    copy(&examples.join("fixed-dataset"), root.path());
    root
}

fn invoke(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(["--output", "json"])
        .args(arguments)
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    metewand_core::public_schemas::public_schema_catalog()
        .unwrap()
        .validate(
            metewand_core::public_schemas::PublicSchema::MachineOutput.repository_path(),
            &value,
        )
        .unwrap();
    value["data"].clone()
}

#[test]
fn locked_preflight_failures_do_not_publish_or_change_files() {
    let root = repository();
    let output = root.path().join("run");
    let failed = invoke(
        root.path(),
        &["run", "--locked", "--output-dir", output.to_str().unwrap()],
    );
    assert!(!failed.status.success());
    let diagnostic: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(diagnostic["data"]["code"], "lockfile_unavailable");
    assert!(!output.exists());
    let failed = invoke(root.path(), &["lock"]);
    assert!(!failed.status.success());
    let diagnostic: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(diagnostic["data"]["code"], "unsupported_locked_environment");
    assert!(!root.path().join("metewand.lock").exists());
}

#[test]
#[ignore = "requires Nix and access to the pinned example's outputs"]
fn real_nix_package_and_app_locked_runs() {
    for app in [false, true] {
        let root = repository();
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/locked-nix");
        for name in ["flake.nix", "flake.lock"] {
            fs::copy(example.join(name), root.path().join(name)).unwrap();
        }
        if app {
            let nested = root.path().join("environments/nix space é");
            fs::create_dir_all(&nested).unwrap();
            for name in ["flake.nix", "flake.lock"] {
                fs::rename(root.path().join(name), nested.join(name)).unwrap();
            }
            let helper = root.path().join("environments/helper");
            fs::create_dir_all(&helper).unwrap();
            fs::write(
                helper.join("flake.nix"),
                "{ outputs = _: { marker = 1; }; }",
            )
            .unwrap();
            let flake = nested.join("flake.nix");
            fs::write(
                &flake,
                fs::read_to_string(&flake).unwrap().replace(
                    "  outputs =",
                    "  inputs.helper.url = \"path:../helper\";\n  outputs =",
                ),
            )
            .unwrap();
            let url = format!(
                "path:{}?dir=environments/nix%20space%20%C3%A9",
                root.path().display()
            );
            let native = Command::new("nix")
                .args(["flake", "lock", &url])
                .output()
                .unwrap();
            assert!(
                native.status.success(),
                "{}",
                String::from_utf8_lossy(&native.stderr)
            );
        }
        let manifest = root.path().join("metewand.yaml");
        let system = String::from_utf8(
            Command::new("nix")
                .args([
                    "eval",
                    "--raw",
                    "--impure",
                    "--expr",
                    "builtins.currentSystem",
                ])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let environment = if app {
            format!(
                "kind: nix\n    flake: environments/nix space é\n    installable: 'environments/nix space é#python3'\n    system: {system}\n    output-kind: app\n    output: python3"
            )
        } else {
            format!(
                "kind: nix\n    flake: .\n    installable: '.#python'\n    system: {system}\n    output-kind: package\n    output: out"
            )
        };
        fs::write(
            &manifest,
            fs::read_to_string(&manifest)
                .unwrap()
                .replace("kind: \"local\"", &environment),
        )
        .unwrap();
        let before = success(invoke(root.path(), &["plan"]));
        let created = success(invoke(root.path(), &["lock"]));
        assert_eq!(created["operation"], "created");
        let bytes = fs::read(root.path().join("metewand.lock")).unwrap();
        assert_eq!(
            success(invoke(root.path(), &["lock"]))["operation"],
            "unchanged"
        );
        assert_eq!(
            success(invoke(root.path(), &["lock", "--check"]))["operation"],
            "verified"
        );
        assert_eq!(before, success(invoke(root.path(), &["plan"])));
        let output = TempDir::new().unwrap();
        let run = output.path().join("run");
        let result = success(invoke(
            root.path(),
            &["run", "--locked", "--output-dir", run.to_str().unwrap()],
        ));
        assert_eq!(result["summary"]["accepted_attempts"], 1);
        let moved = TempDir::new().unwrap();
        copy(root.path(), moved.path());
        success(invoke(moved.path(), &["lock", "--check"]));
        let mut unrelated: Value = serde_json::from_slice(&bytes).unwrap();
        unrelated["environments"]["unrelated"] = unrelated["environments"]["local"].clone();
        fs::write(
            root.path().join("metewand.lock"),
            serde_json::to_vec(&unrelated).unwrap(),
        )
        .unwrap();
        let slot = result["attempts"][0]["logical_slot_id"].as_str().unwrap();
        let resumed = success(invoke(
            root.path(),
            &[
                "run",
                "--locked",
                "--output-dir",
                run.to_str().unwrap(),
                "--resume",
                "--select",
                slot,
            ],
        ));
        assert_eq!(resumed["attempts"].as_array().unwrap().len(), 0);
        fs::write(root.path().join("metewand.lock"), &bytes).unwrap();
        let export = output.path().join("export");
        success(invoke(
            root.path(),
            &[
                "export",
                "--output-dir",
                run.to_str().unwrap(),
                "--destination",
                export.to_str().unwrap(),
            ],
        ));
        let rows = fs::read_to_string(export.join("results.jsonl")).unwrap();
        let rows: Vec<Value> = rows
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(
            rows.iter()
                .any(|row| row["compatibility"]["environment_class"] == "locked")
        );
        assert_eq!(fs::read(root.path().join("metewand.lock")).unwrap(), bytes);
        fs::write(root.path().join("worker.py"), "changed source").unwrap();
        let failed = invoke(root.path(), &["lock", "--check"]);
        assert!(!failed.status.success());
        let diagnostic: Value = serde_json::from_slice(&failed.stdout).unwrap();
        assert_eq!(diagnostic["data"]["code"], "lockfile_outdated");
        assert_eq!(fs::read(root.path().join("metewand.lock")).unwrap(), bytes);
    }
}

#[test]
#[ignore = "requires Nix and access to the pinned example's outputs"]
fn real_nix_materializers_and_profile_workers() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in [
        "generated-dataset",
        "fresh-sequence",
        "buffered-profile",
        "streaming-profile",
    ] {
        let root = TempDir::new().unwrap();
        copy(&project.join("examples").join(name), root.path());
        if name != "generated-dataset" {
            copy(
                &project.join("packages/python/src/metewand"),
                &root.path().join("metewand"),
            );
        }
        for file in ["flake.nix", "flake.lock"] {
            fs::copy(
                project.join("examples/locked-nix").join(file),
                root.path().join(file),
            )
            .unwrap();
        }
        let system = String::from_utf8(
            Command::new("nix")
                .args([
                    "eval",
                    "--raw",
                    "--impure",
                    "--expr",
                    "builtins.currentSystem",
                ])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let manifest = root.path().join("metewand.yaml");
        let original = fs::read_to_string(&manifest).unwrap();
        let mut lines = Vec::new();
        let mut removed_indent = None;
        for line in original.lines() {
            let indent = line.len() - line.trim_start().len();
            if line.trim() == "uniroot:" {
                removed_indent = Some(indent);
                continue;
            }
            if removed_indent.is_some_and(|removed| indent > removed || line.trim().is_empty()) {
                continue;
            }
            removed_indent = None;
            lines.push(line.replace("[bisection, uniroot]", "[bisection]"));
            if name != "generated-dataset" && line.trim().starts_with("runner:") {
                lines.push(format!("{}environment: local", " ".repeat(indent)));
            }
        }
        let mut source = lines.join("\n");
        let environment = format!(
            "kind: nix\n    flake: .\n    installable: '.#python'\n    system: {system}\n    output-kind: package\n    output: out"
        );
        if name == "generated-dataset" {
            source = source.replace("kind: \"local\"", &environment);
        } else {
            source.push_str(&format!("\nenvironments:\n  local:\n    {environment}\n"));
        }
        fs::write(&manifest, source).unwrap();
        success(invoke(root.path(), &["lock"]));
        let output = TempDir::new().unwrap();
        let run = output.path().join("run");
        let result = success(invoke(
            root.path(),
            &["run", "--locked", "--output-dir", run.to_str().unwrap()],
        ));
        assert_eq!(result["summary"]["failed_attempts"], 0, "{name}: {result}");
        assert!(result["summary"]["accepted_attempts"].as_u64().unwrap() > 0);
        let archive = output.path().join("results.tar.gz");
        success(invoke(
            root.path(),
            &[
                "archive",
                "create",
                "--output-dir",
                run.to_str().unwrap(),
                "--destination",
                archive.to_str().unwrap(),
            ],
        ));
        success(invoke(
            root.path(),
            &["archive", "verify", "--archive", archive.to_str().unwrap()],
        ));
    }
}
