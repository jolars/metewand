use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::{Command, Output},
};

use metewand_core::{
    manifest::parse_manifest,
    public_schemas::{PublicSchema, public_schema_catalog},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const MANIFEST: &str = "version = 1\nname = \"benchmark\"\n";

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metewand"));
    command.current_dir(root);
    command
}

fn metewand(root: &Path, arguments: &[&str]) -> Output {
    command(root).args(arguments).output().unwrap()
}

fn envelope(output: &Output, success: bool) -> Value {
    assert_eq!(output.status.success(), success, "{output:?}");
    let value = serde_json::from_slice(&output.stdout).unwrap();
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::MachineOutput.repository_path(), &value)
        .unwrap();
    value
}

#[test]
fn creates_only_a_minimal_versioned_manifest_that_checks_and_plans() {
    let root = TempDir::new().unwrap();
    let result = metewand(root.path(), &["init", "--output", "jsonl"]);
    assert!(result.stderr.is_empty(), "{result:?}");
    assert_eq!(
        String::from_utf8(result.stdout.clone()).unwrap(),
        "{\"schema_version\":1,\"kind\":\"result\",\"data\":{\"command\":\"init\",\"dry_run\":false,\"manifest\":\"./metewand.toml\",\"paths\":[{\"path\":\"./metewand.toml\",\"kind\":\"file\"}]}}\n"
    );
    envelope(&result, true);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    let source = fs::read_to_string(root.path().join("metewand.toml")).unwrap();
    assert_eq!(source, MANIFEST);
    let manifest = parse_manifest(Path::new("metewand.toml"), &source).unwrap();
    assert!(manifest.datasets.is_empty());
    assert!(manifest.problems.is_empty());
    assert!(manifest.implementations.is_empty());
    assert!(manifest.environments.is_empty());
    assert!(manifest.experiments.is_empty());
    assert!(manifest.execution_policies.is_empty());
    assert!(manifest.observation_policies.is_empty());
    for subcommand in ["check", "plan"] {
        envelope(
            &metewand(root.path(), &[subcommand, "--output", "json"]),
            true,
        );
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn dry_run_reports_every_missing_path_without_creating_anything() {
    let root = TempDir::new().unwrap();
    let arguments = [
        "init",
        "new/nested benchmark",
        "--dry-run",
        "--output",
        "json",
    ];
    let first = metewand(root.path(), &arguments);
    let second = metewand(root.path(), &arguments);
    assert_eq!(first.stdout, second.stdout);
    let preview = envelope(&first, true);
    assert_eq!(preview["data"]["dry_run"], true);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let result = envelope(
        &metewand(
            root.path(),
            &["init", "new/nested benchmark", "--output", "json"],
        ),
        true,
    );
    assert_eq!(result["data"]["paths"], preview["data"]["paths"]);
    assert_eq!(
        result["data"]["paths"],
        json!([
            {"path": "new", "kind": "directory"},
            {"path": "new/nested benchmark", "kind": "directory"},
            {"path": "new/nested benchmark/metewand.toml", "kind": "file"}
        ])
    );
    assert_eq!(
        fs::read_to_string(root.path().join("new/nested benchmark/metewand.toml")).unwrap(),
        MANIFEST
    );
    assert_eq!(
        fs::read_dir(root.path().join("new/nested benchmark"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn existing_directories_and_unrelated_files_are_preserved() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("keep.txt"), "keep\n").unwrap();
    let preview = metewand(root.path(), &["init", "--dry-run"]);
    assert!(preview.status.success(), "{preview:?}");
    assert_eq!(
        String::from_utf8(preview.stdout).unwrap(),
        "Would create file: ./metewand.toml\n"
    );
    let result = metewand(root.path(), &["init"]);
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "Created file: ./metewand.toml\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("keep.txt")).unwrap(),
        "keep\n"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn refuses_existing_manifest_files_and_directories_including_dry_run() {
    for directory in [false, true] {
        let root = TempDir::new().unwrap();
        let target = root.path().join("metewand.toml");
        if directory {
            fs::create_dir(&target).unwrap();
        } else {
            fs::write(&target, "do not replace\n").unwrap();
        }
        for flag in [None, Some("--dry-run")] {
            let mut arguments = vec!["init", "--output", "json"];
            arguments.extend(flag);
            let output = metewand(root.path(), &arguments);
            assert_eq!(output.status.code(), Some(74));
            let diagnostic = envelope(&output, false);
            assert_eq!(diagnostic["data"]["code"], "init_path_exists");
            assert_eq!(diagnostic["data"]["details"]["created_paths"], json!([]));
        }
        if !directory {
            assert_eq!(fs::read_to_string(&target).unwrap(), "do not replace\n");
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn rejects_invalid_arguments_and_blocked_parent_paths_without_writes() {
    let root = TempDir::new().unwrap();
    for arguments in [
        vec!["init", ""],
        vec!["init", "one", "two"],
        vec!["init", "--dry-run", "--dry-run"],
        vec!["init", "--template", "python"],
        vec!["init", "--manifest", "other.toml"],
    ] {
        let mut arguments = arguments;
        arguments.extend(["--output", "json"]);
        let output = metewand(root.path(), &arguments);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        envelope(&output, false);
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    fs::write(root.path().join("blocked"), "keep\n").unwrap();
    for path in ["blocked", "blocked/child"] {
        for dry_run in [false, true] {
            let mut arguments = vec!["init", path, "--output", "json"];
            if dry_run {
                arguments.push("--dry-run");
            }
            let diagnostic = envelope(&metewand(root.path(), &arguments), false);
            assert_eq!(diagnostic["data"]["details"]["created_paths"], json!([]));
        }
    }
    assert_eq!(
        fs::read_to_string(root.path().join("blocked")).unwrap(),
        "keep\n"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn never_follows_existing_or_dangling_manifest_symlinks() {
    use std::os::unix::fs::symlink;
    for dangling in [false, true] {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target");
        if !dangling {
            fs::write(&target, "keep\n").unwrap();
        }
        symlink(&target, root.path().join("metewand.toml")).unwrap();
        for arguments in [
            vec!["init", "--output", "json"],
            vec!["init", "--dry-run", "--output", "json"],
        ] {
            let diagnostic = envelope(&metewand(root.path(), &arguments), false);
            assert_eq!(diagnostic["data"]["code"], "init_path_exists");
        }
        assert!(root.path().join("metewand.toml").is_symlink());
        if dangling {
            assert!(!target.exists());
        } else {
            assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
        }
    }
}

#[test]
fn concurrent_initializers_publish_only_one_manifest() {
    let root = TempDir::new().unwrap();
    let mut children = (0..8)
        .map(|_| {
            command(root.path())
                .args(["init", "--output", "json"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let results = children
        .drain(..)
        .map(|child| child.wait_with_output().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|output| output.status.success())
            .count(),
        1
    );
    for result in &results {
        envelope(result, result.status.success());
    }
    assert_eq!(
        fs::read_to_string(root.path().join("metewand.toml")).unwrap(),
        MANIFEST
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn accepts_absolute_paths_and_option_like_paths_after_the_separator() {
    let root = TempDir::new().unwrap();
    for path in [
        root.path().join("absolute"),
        "--benchmark".into(),
        "--output".into(),
    ] {
        let output = command(root.path())
            .args([
                OsStr::new("init"),
                OsStr::new("--output"),
                OsStr::new("json"),
                OsStr::new("--"),
                path.as_os_str(),
            ])
            .output()
            .unwrap();
        let report = envelope(&output, true);
        assert_eq!(
            report["data"]["manifest"],
            json!(path.join("metewand.toml"))
        );
        assert_eq!(
            fs::read_to_string(root.path().join(&path).join("metewand.toml")).unwrap(),
            MANIFEST
        );
    }
}

#[test]
fn parent_components_resolve_without_creating_or_overwriting_existing_entries() {
    let root = TempDir::new().unwrap();
    let path = "new/child/../child/../../destination";
    let preview = envelope(
        &metewand(
            root.path(),
            &["init", path, "--dry-run", "--output", "json"],
        ),
        true,
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    assert_eq!(preview["data"]["paths"].as_array().unwrap().len(), 4);
    let result = envelope(
        &metewand(root.path(), &["init", path, "--output", "json"]),
        true,
    );
    assert_eq!(result["data"]["paths"], preview["data"]["paths"]);
    assert_eq!(
        fs::read_to_string(root.path().join("destination/metewand.toml")).unwrap(),
        MANIFEST
    );
    let conflict = envelope(
        &metewand(
            root.path(),
            &[
                "init",
                "missing/../destination",
                "--dry-run",
                "--output",
                "json",
            ],
        ),
        false,
    );
    assert_eq!(conflict["data"]["code"], "init_path_exists");
    assert!(!root.path().join("missing").exists());
}

#[cfg(unix)]
#[test]
fn rejects_non_utf8_paths_and_dangling_directory_links_before_writing() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let root = TempDir::new().unwrap();
    let output = command(root.path())
        .args([
            OsStr::new("init"),
            OsStr::from_bytes(b"invalid-\xff"),
            OsStr::new("--output"),
            OsStr::new("json"),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        envelope(&output, false)["data"]["code"],
        "invalid_init_path"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    symlink("missing", root.path().join("dangling")).unwrap();
    for path in ["dangling", "dangling/child"] {
        let result = envelope(
            &metewand(root.path(), &["init", path, "--output", "json"]),
            false,
        );
        assert_eq!(result["data"]["code"], "init_path_exists");
        assert_eq!(result["data"]["details"]["created_paths"], json!([]));
    }
    assert!(!root.path().join("missing").exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn help_documents_init_and_creates_nothing() {
    let root = TempDir::new().unwrap();
    for arguments in [vec!["--help"], vec!["init", "--help"], vec!["init", "-h"]] {
        let output = metewand(root.path(), &arguments);
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("init [PATH] [--dry-run]")
        );
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn resolves_parent_components_through_existing_directory_links() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("actual/child")).unwrap();
    symlink("actual/child", root.path().join("alias")).unwrap();
    let result = metewand(root.path(), &["init", "alias/..", "--output", "json"]);
    envelope(&result, true);
    assert_eq!(
        fs::read_to_string(root.path().join("actual/metewand.toml")).unwrap(),
        MANIFEST
    );
    assert!(!root.path().join("metewand.toml").exists());
    let conflict = metewand(
        root.path(),
        &["init", "alias/..", "--dry-run", "--output", "json"],
    );
    assert_eq!(
        envelope(&conflict, false)["data"]["code"],
        "init_path_exists"
    );
}
