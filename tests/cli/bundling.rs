use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn bundle(root: &Path, destination: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(["--output", "json", "bundle", "--destination"])
        .arg(destination)
        .args([
            "--base-image",
            "python@sha256:af8a5aa14780025dbfbcfbc81bd8a91ebd87711dd5b0f48ed71090949dd7cb1d",
        ])
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn bundles_declared_inputs_without_changing_identities() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("benchmark space é");
    fs::create_dir(&root).unwrap();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/dataset-free");
    fn copy(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &dst.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
            }
        }
    }
    copy(&example, &root);
    let schema = fs::read(root.join("schemas/result.json")).unwrap();
    fs::write(root.join("schemas/shared.json"), schema).unwrap();
    fs::write(
        root.join("schemas/result.json"),
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"shared.json"}"#,
    )
    .unwrap();
    fs::write(root.join("unrelated.secret"), "excluded").unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    let destination = temp.path().join("context");
    let dry = bundle(&root, &destination, &["--dry-run", "--include", "empty"]);
    assert!(dry.status.success(), "{dry:?}");
    assert!(!destination.exists());
    let output = bundle(&root, &destination, &["--include", "empty"]);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["data"]["bundle"],
        serde_json::from_slice::<Value>(&dry.stdout).unwrap()["data"]["bundle"]
    );
    assert!(!destination.join("benchmark/unrelated.secret").exists());
    assert!(destination.join("benchmark/empty").is_dir());
    let before = crate::runtime::repository::check_repository(&root.join("metewand.yaml")).unwrap();
    let after =
        crate::runtime::repository::check_repository(&destination.join("benchmark/metewand.yaml"))
            .unwrap();
    assert_eq!(before.manifest_hash, after.manifest_hash);
    assert!(
        before
            .resources
            .records
            .keys()
            .eq(after.resources.records.keys())
    );
    assert!(!bundle(&root, &destination, &[]).status.success());
    let relocated = temp.path().join("second");
    assert!(
        bundle(&root, &relocated, &["--include", "empty"])
            .status
            .success()
    );
    assert_eq!(
        fs::read(destination.join("bundle.json")).unwrap(),
        fs::read(relocated.join("bundle.json")).unwrap()
    );
    let catalog = crate::core::public_schemas::public_schema_catalog().unwrap();
    catalog
        .validate(
            crate::core::public_schemas::PublicSchema::BundleOutput.repository_path(),
            &report["data"],
        )
        .unwrap();
}

#[test]
fn rejects_remote_inputs_and_nonlocal_environments_before_writing() {
    let temp = tempfile::tempdir().unwrap();
    for example in ["locked-oci", "locked-uv"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(example);
        let destination = temp.path().join(example);
        assert!(!bundle(&root, &destination, &[]).status.success());
        assert!(!destination.exists());
    }
}

#[test]
fn bundles_inline_schemas_and_rejects_unsafe_options() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    fs::create_dir(&root).unwrap();
    for file in [
        "metewand.yaml",
        "benchmark.py",
        "install.sh",
        "requirements.txt",
    ] {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples/bundled-benchmark")
                .join(file),
            root.join(file),
        )
        .unwrap();
    }
    let dest = temp.path().join("context");
    let result = bundle(
        &root,
        &dest,
        &["--recipe", "install.sh", "--include", "requirements.txt"],
    );
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        fs::read(root.join("metewand.yaml")).unwrap(),
        fs::read(dest.join("benchmark/metewand.yaml")).unwrap()
    );
    assert!(
        fs::read_to_string(dest.join("Dockerfile"))
            .unwrap()
            .contains("/bin/sh")
    );
    for args in [
        vec!["--runner-version", "^0.5"],
        vec!["--recipe", "../escape"],
        vec!["--include", "missing*"],
    ] {
        assert!(
            !bundle(&root, &temp.path().join("invalid"), &args)
                .status
                .success()
        );
        assert!(!temp.path().join("invalid").exists());
    }
    assert!(!bundle(&root, &root.join("context"), &[]).status.success());
    #[cfg(unix)]
    {
        use std::os::unix::{fs::PermissionsExt, fs::symlink};
        fs::set_permissions(root.join("benchmark.py"), fs::Permissions::from_mode(0o755)).unwrap();
        let exec = temp.path().join("executable");
        assert!(bundle(&root, &exec, &[]).status.success());
        assert_ne!(
            fs::metadata(exec.join("benchmark/benchmark.py"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        symlink("benchmark.py", root.join("link")).unwrap();
        assert!(
            !bundle(&root, &temp.path().join("symlink"), &["--include", "link"])
                .status
                .success()
        );
    }
    let manifest = root.join("metewand.yaml");
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, source.replace("function: benchmark.py:generate", &format!("function: benchmark.py:generate\n    source: {{url: 'https://example.org/data', sha256: '{}' }}", "0".repeat(64)))).unwrap();
    let remote = bundle(&root, &temp.path().join("remote"), &[]);
    assert!(!remote.status.success());
    assert!(String::from_utf8_lossy(&remote.stdout).contains("local dataset inputs"));
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "requires Docker, registry access, and a released runner build"]
fn docker_context_runs_offline_and_resumes_without_checkout() {
    fn docker(args: &[&str]) -> std::process::Output {
        let output = Command::new("docker").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    struct Image(String);
    impl Drop for Image {
        fn drop(&mut self) {
            let _ = Command::new("docker")
                .args(["image", "rm", "--force", &self.0])
                .output();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    fs::create_dir(&root).unwrap();
    for file in [
        "metewand.yaml",
        "benchmark.py",
        "install.sh",
        "requirements.txt",
    ] {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples/bundled-benchmark")
                .join(file),
            root.join(file),
        )
        .unwrap();
    }
    let context = temp.path().join("context");
    let generated = bundle(
        &root,
        &context,
        &[
            "--runner-version",
            "0.5.0",
            "--recipe",
            "install.sh",
            "--include",
            "requirements.txt",
        ],
    );
    assert!(generated.status.success(), "{generated:?}");
    fs::remove_dir_all(root).unwrap();
    let image = Image(format!("metewand-bundle-test-{}", std::process::id()));
    docker(&["build", "--tag", &image.0, context.to_str().unwrap()]);
    let output = temp.path().join("output");
    fs::create_dir(&output).unwrap();
    let mount = format!("type=bind,src={},dst=/output", output.display());
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let user = format!("{uid}:{gid}");
    docker(&[
        "run",
        "--rm",
        "--network",
        "none",
        "--user",
        &user,
        "--mount",
        &mount,
        &image.0,
    ]);
    assert!(output.join("run").is_dir());
    docker(&[
        "run",
        "--rm",
        "--network",
        "none",
        "--user",
        &user,
        "--mount",
        &mount,
        &image.0,
        "--resume",
    ]);
    let status = docker(&[
        "run",
        "--rm",
        "--network",
        "none",
        "--user",
        &user,
        "--entrypoint",
        "/usr/local/bin/metewand",
        "--mount",
        &mount,
        &image.0,
        "--output",
        "json",
        "status",
        "--output-dir",
        "/output/run",
    ]);
    let report: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(report["data"]["summary"]["accepted_attempts"], 2);
}
