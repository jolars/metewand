use std::{fs, path::Path, process::Command};

use crate::core::public_schemas::{PublicSchema, public_schema_catalog};
use crate::runtime::build_evidence::executable_evidence;
use serde_json::Value;

fn git(root: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn checkout_observations_record_git_state_without_requiring_git_repositories() {
    use crate::runtime::build_evidence::repository_observation;
    let root = tempfile::tempdir().unwrap();
    assert_eq!(repository_observation(root.path())["status"], "unavailable");
    git(root.path(), &["init", "-q"]);
    fs::write(root.path().join("source.txt"), "initial").unwrap();
    git(root.path(), &["add", "source.txt"]);
    git(
        root.path(),
        &[
            "-c",
            "user.name=Run Test",
            "-c",
            "user.email=run@example.invalid",
            "commit",
            "-qm",
            "initial",
        ],
    );
    let clean = repository_observation(root.path());
    assert_eq!(clean["dirty"], false);
    assert_eq!(clean["commit"].as_str().unwrap().len(), 40);
    fs::write(root.path().join("source.txt"), "changed").unwrap();
    let dirty = repository_observation(root.path());
    assert_eq!(dirty["commit"], clean["commit"]);
    assert_eq!(dirty["dirty"], true);
}

#[test]
fn build_receipts_capture_inputs_and_bind_the_exact_artifact() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    fs::write(root.path().join(".gitignore"), "build/\n").unwrap();
    fs::write(root.path().join("source.txt"), "source before build\n").unwrap();
    fs::write(root.path().join("Cargo.lock"), "locked inputs\n").unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/build_evidence.py");
    let build = Command::new("python3")
        .current_dir(root.path())
        .arg(script)
        .args(["--executable", "build/worker", "--source", "worker=.", "--compiler", "python3", "--profile", "release", "--features", "native", "--lockfile", "Cargo.lock", "--", "python3", "-c", "from pathlib import Path; Path('build').mkdir(); Path('build/worker').write_bytes(b'compiled worker')"])
        .output().unwrap();
    assert!(build.status.success(), "{build:?}");
    let receipt: Value = serde_json::from_slice(
        &fs::read(root.path().join("build/worker.metewand-build.json")).unwrap(),
    )
    .unwrap();
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::BuildEvidence.repository_path(), &receipt)
        .unwrap();
    assert_eq!(receipt["schema_version"], 1);
    assert_eq!(receipt["capture"], "build_wrapper");
    assert_eq!(receipt["build"]["profile"], "release");
    assert_eq!(receipt["build"]["features"], serde_json::json!(["native"]));
    assert!(
        receipt["build"]["compiler"]["version"]
            .as_str()
            .unwrap()
            .contains("Python")
    );
    assert_eq!(receipt["build"]["sources"]["worker"]["dirty"], true);
    assert!(receipt["build"]["sources"]["worker"]["tree_sha256"].is_string());
    assert!(receipt["build"]["lockfiles"]["Cargo.lock"].is_string());
    assert!(
        receipt["build"]["flags"]
            .as_object()
            .unwrap()
            .contains_key("RUSTFLAGS")
    );
    let evidence = executable_evidence(&root.path().join("build/worker")).unwrap();
    assert_eq!(evidence["build"]["status"], "available");
    assert_eq!(evidence["build"]["evidence"], receipt);
    fs::write(root.path().join("build/worker"), "different executable").unwrap();
    let evidence = executable_evidence(&root.path().join("build/worker")).unwrap();
    assert_eq!(evidence["build"]["status"], "unavailable");
    assert_eq!(evidence["build"]["reason"], "executable_hash_mismatch");
    assert!(evidence["build"].get("evidence").is_none());
}

#[test]
fn failed_or_input_changing_builds_never_publish_a_receipt() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    fs::write(root.path().join(".gitignore"), "build/\n").unwrap();
    fs::write(root.path().join("source.txt"), "original").unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/build_evidence.py");
    for code in [
        "raise SystemExit(7)",
        "from pathlib import Path; Path('source.txt').write_text('changed'); Path('build/worker').write_bytes(b'worker')",
    ] {
        let build = Command::new("python3")
            .current_dir(root.path())
            .arg(&script)
            .args([
                "--executable",
                "build/worker",
                "--source",
                "worker=.",
                "--compiler",
                "python3",
                "--profile",
                "release",
                "--",
                "python3",
                "-c",
                code,
            ])
            .output()
            .unwrap();
        assert!(!build.status.success(), "{build:?}");
        assert!(
            !root
                .path()
                .join("build/worker.metewand-build.json")
                .exists()
        );
    }
    let evidence = executable_evidence(&root.path().join("build/worker")).unwrap();
    assert_eq!(evidence["build"]["reason"], "not_recorded_at_build");
    fs::write(root.path().join("build/worker.metewand-build.json"), "{}").unwrap();
    assert_eq!(
        executable_evidence(&root.path().join("build/worker")).unwrap()["build"]["reason"],
        "invalid_receipt"
    );
    let receipt_path = root.path().join("build/worker.metewand-build.json");
    fs::remove_file(&receipt_path).unwrap();
    fs::create_dir(&receipt_path).unwrap();
    assert_eq!(
        executable_evidence(&root.path().join("build/worker")).unwrap()["build"]["reason"],
        "unreadable_receipt"
    );
    fs::remove_dir(&receipt_path).unwrap();
    let oversized = fs::File::create(&receipt_path).unwrap();
    oversized.set_len(4 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        executable_evidence(&root.path().join("build/worker")).unwrap()["build"]["reason"],
        "receipt_too_large"
    );
}

#[test]
fn source_state_hashes_submodule_worktrees_as_well_as_gitlinks() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    let child = root.path().join("dependency");
    fs::create_dir(&child).unwrap();
    git(&child, &["init", "-q"]);
    fs::write(child.join("source.txt"), "original").unwrap();
    git(&child, &["add", "source.txt"]);
    git(
        &child,
        &[
            "-c",
            "user.name=Build Test",
            "-c",
            "user.email=build@example.invalid",
            "commit",
            "-qm",
            "initial",
        ],
    );
    git(root.path(), &["add", "dependency"]);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/build_evidence.py");
    let state = || {
        let output = Command::new("python3").args(["-c", "import json,runpy,sys; m=runpy.run_path(sys.argv[1]); print(json.dumps(m['source_state'](sys.argv[2])))"])
            .arg(&script).arg(root.path()).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let before = state();
    assert!(before["submodules"]["dependency"]["expected_commit"].is_string());
    fs::write(child.join("source.txt"), "changed").unwrap();
    let after = state();
    assert_ne!(before["tree_sha256"], after["tree_sha256"]);
    assert_eq!(after["submodules"]["dependency"]["source"]["dirty"], true);
}
