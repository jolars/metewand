//! Apptainer CLI coverage with an isolated command shim and an opt-in real SIF.
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn cli(root: &Path, bin: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metewand"));
    command
        .current_dir(root)
        .args(["--output", "json"])
        .args(args);
    if let Some(bin) = bin {
        let mut paths = vec![bin.to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command.env("PATH", std::env::join_paths(paths).unwrap());
    }
    command.output().unwrap()
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
}
fn fixture(root: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/locked-apptainer");
    for name in ["metewand.yaml", "benchmark.py"] {
        fs::copy(source.join(name), root.join(name)).unwrap();
    }
}

#[test]
fn sif_lock_checks_are_read_only_and_changed_images_fail_before_publication() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("benchmark space é");
    fs::create_dir(&root).unwrap();
    fixture(&root);
    fs::create_dir(root.join("images")).unwrap();
    fs::write(root.join("images/worker.sif"), "image").unwrap();
    let bin = temporary.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return,
    };
    let shim = bin.join("apptainer");
    fs::write(&shim, format!("#!/bin/sh\ncase \"$1 $2\" in\n'--version ') echo 'apptainer version 1.3.0';;\n'sif header') echo 'Primary Architecture: {arch}';;\n'sif list') echo '1 |FS (Squashfs/*System/{arch})';;\n*) exit 99;;\nesac\n")).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    success(cli(&root, Some(&bin), &["check"]));
    let plan = success(cli(&root, Some(&bin), &["plan"]));
    success(cli(&root, Some(&bin), &["lock"]));
    let bytes = fs::read(root.join("metewand.lock")).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["schema_version"],
        4
    );
    success(cli(&root, Some(&bin), &["lock", "--check"]));
    assert_eq!(bytes, fs::read(root.join("metewand.lock")).unwrap());
    assert_eq!(plan, success(cli(&root, Some(&bin), &["plan"])));
    fs::write(root.join("images/worker.sif"), "changed image").unwrap();
    assert!(
        !cli(&root, Some(&bin), &["lock", "--check"])
            .status
            .success()
    );
    let output = temporary.path().join("run");
    assert!(
        !cli(
            &root,
            Some(&bin),
            &["run", "--locked", "--output-dir", output.to_str().unwrap()]
        )
        .status
        .success()
    );
    assert!(!output.exists());
    fs::remove_file(&shim).unwrap();
    assert!(
        !cli(&root, Some(&bin), &["lock", "--check"])
            .status
            .success()
    );
}

#[test]
#[ignore = "requires Linux Apptainer 1.3+ and METEWAND_TEST_SIF containing Python 3"]
fn local_sif_workers_run_offline_and_resume() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("benchmark space é");
    fs::create_dir(&root).unwrap();
    fixture(&root);
    fs::create_dir(root.join("images")).unwrap();
    fs::copy(
        std::env::var_os("METEWAND_TEST_SIF").expect("set METEWAND_TEST_SIF"),
        root.join("images/worker.sif"),
    )
    .unwrap();
    success(cli(&root, None, &["lock"]));
    success(cli(&root, None, &["lock", "--check"]));
    let output = temporary.path().join("run");
    let output = output.to_str().unwrap();
    let first = success(cli(
        &root,
        None,
        &[
            "run",
            "--locked",
            "--output-dir",
            output,
            "--max-attempts",
            "1",
        ],
    ));
    assert_eq!(first["summary"]["accepted_attempts"], 1);
    let resumed = success(cli(
        &root,
        None,
        &["run", "--locked", "--output-dir", output, "--resume"],
    ));
    assert_eq!(resumed["summary"]["accepted_attempts"], 2);
    fn inspect(directory: &Path) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                inspect(&entry.path());
            } else if entry.file_name() == "observation.json" {
                let observation: Value =
                    serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
                assert!(observation["timing"]["cpu_time_ns"].is_null());
                assert!(observation["timing"]["peak_rss_bytes"].is_null());
                for role in ["implementation", "evaluator"] {
                    let environment =
                        &observation["provenance"]["data"]["execution"][role]["worker_environment"];
                    assert_eq!(environment["version"], 3);
                    assert!(environment["umask"].is_null());
                    assert!(
                        environment["environment_variables"]
                            .get("METEWAND_APPTAINER_IMAGE")
                            .is_none()
                    );
                }
            }
        }
    }
    inspect(Path::new(output));
}

#[test]
#[ignore = "requires Linux Apptainer 1.3+ and METEWAND_TEST_SIF containing Python 3"]
fn sif_timeouts_terminate_descendants_that_create_new_sessions() {
    use crate::{
        core::records::{RecordId, ResolvedLaunchRecord, WorkerWorkingDirectory},
        runtime::{
            executor::{Executor, LocalProcessExecutor},
            local_worker::launch_trusted_local_worker,
            worker_process::WorkerLogLimits,
        },
    };
    use std::{collections::BTreeMap, time::Duration};
    let root = tempfile::tempdir().unwrap();
    let image =
        fs::canonicalize(std::env::var_os("METEWAND_TEST_SIF").expect("set METEWAND_TEST_SIF"))
            .unwrap();
    let marker = root.path().join("descendant");
    let script = "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import pathlib,sys,time; p=pathlib.Path(sys.argv[1]);\\nwhile True: p.write_text(str(time.monotonic_ns())); time.sleep(0.01)',sys.argv[1]],start_new_session=True); time.sleep(60)";
    let mut launch = ResolvedLaunchRecord {
        environment: RecordId::from_digest([0; 32]),
        program: "python3".into(),
        args: vec!["-c".into(), script.into(), marker.to_str().unwrap().into()],
        environment_variables: BTreeMap::from([
            (
                "METEWAND_APPTAINER_IMAGE".into(),
                image.to_str().unwrap().into(),
            ),
            ("METEWAND_PROTOCOL_TRANSPORT".into(), "socket".into()),
        ]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let mut process =
        launch_trusted_local_worker(&launch, root.path(), WorkerLogLimits::new(4096, 4096))
            .unwrap();
    let start = std::time::Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "descendant failed to start");
    LocalProcessExecutor::new(Duration::from_millis(100))
        .terminate(&mut process)
        .unwrap();
    process.wait().unwrap();
    let before = fs::read(&marker).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(before, fs::read(&marker).unwrap());
    launch.program = "/missing-metewand-worker".into();
    launch.args.clear();
    let process =
        launch_trusted_local_worker(&launch, root.path(), WorkerLogLimits::new(4096, 4096))
            .unwrap();
    assert!(!process.wait().unwrap().status.success());
    launch.program = "python3".into();
    launch.args = vec!["-c".into(), "import time; time.sleep(30)".into()];
    let process =
        launch_trusted_local_worker(&launch, root.path(), WorkerLogLimits::new(4096, 4096))
            .unwrap();
    assert!(
        crate::runtime::worker_session::WorkerSession::connect(
            process,
            crate::protocol::WorkerRole::Implementation,
            format!("mw1-resolved-worker-{}", "0".repeat(64))
                .parse()
                .unwrap(),
            &[],
            crate::runtime::worker_session::PhaseTimeouts {
                hello: Duration::from_millis(250),
                ..Default::default()
            }
        )
        .is_err()
    );
}
