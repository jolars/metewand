#![cfg(unix)]

use crate::core::records::{ResolvedLaunchRecord, WorkerWorkingDirectory};
use crate::runtime::{
    executor::{Executor, ExecutorRequirements, LocalProcessExecutor},
    worker_process::WorkerLogLimits,
};
use std::{
    collections::BTreeMap,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    time::{Duration, Instant},
};

#[test]
fn preflight_refuses_complete_tree_containment() {
    let executor = LocalProcessExecutor::new(Duration::from_millis(30));
    assert!(
        executor
            .preflight(ExecutorRequirements {
                complete_process_tree: true
            })
            .is_err()
    );
    let capabilities = executor.preflight(ExecutorRequirements::default()).unwrap();
    assert!(capabilities.process_groups);
    assert!(capabilities.private_working_directory);
    assert!(!capabilities.complete_process_tree);
}

#[test]
fn termination_kills_stubborn_descendants_and_removes_private_directory() {
    let root = tempfile::tempdir().unwrap();
    let launch = ResolvedLaunchRecord {
        environment: "mw1-resolved-environment-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".parse().unwrap(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "trap '' INT; (trap '' INT; while :; do :; done) & echo ready; echo ready > ready; wait".into()],
        environment_variables: BTreeMap::new(),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let executor = LocalProcessExecutor::new(Duration::from_millis(30));
    let mut process = executor
        .spawn(
            &launch,
            root.path(),
            WorkerLogLimits::new(100, 100),
            ExecutorRequirements::default(),
        )
        .unwrap();
    let directory = process.private_working_directory().unwrap().to_owned();
    assert_eq!(
        directory.parent(),
        Some(root.path().canonicalize().unwrap().as_path())
    );
    assert_eq!(
        directory.metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    let ready_deadline = Instant::now();
    while !directory.join("ready").exists() {
        assert!(ready_deadline.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    executor.terminate(&mut process).unwrap();
    let output = process.wait().unwrap();
    assert_eq!(output.status.signal(), Some(libc::SIGKILL));
    assert!(start.elapsed() >= Duration::from_millis(30));
    assert_eq!(output.stdout.bytes(), b"ready\n");
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!directory.exists());
}

#[test]
fn normal_exit_cleans_up_descendants_holding_log_pipes() {
    use crate::runtime::worker_process::PosixWorkerProcess;
    use std::process::Command;

    let mut command = Command::new("/bin/sh");
    command.args(["-c", "(while :; do :; done) & echo done"]);
    let process = PosixWorkerProcess::spawn(command, WorkerLogLimits::new(100, 100)).unwrap();
    let start = Instant::now();
    let output = process.wait().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.bytes(), b"done\n");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn spawn_rechecks_requirements_before_allocating_or_launching() {
    let root = tempfile::tempdir().unwrap();
    let launch = ResolvedLaunchRecord {
        environment: "mw1-resolved-environment-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".parse().unwrap(),
        program: "/does-not-exist".into(),
        args: vec![],
        environment_variables: BTreeMap::new(),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let executor = LocalProcessExecutor::new(Duration::ZERO);
    let error = executor
        .spawn(
            &launch,
            root.path(),
            WorkerLogLimits::new(0, 0),
            ExecutorRequirements {
                complete_process_tree: true,
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        crate::runtime::executor::ExecutorError::UnsupportedCompleteProcessTree
    ));
    assert_eq!(root.path().read_dir().unwrap().count(), 0);
}

#[test]
fn cooperative_worker_exits_on_interrupt_before_forced_termination() {
    let root = tempfile::tempdir().unwrap();
    let launch = ResolvedLaunchRecord {
        environment: "mw1-resolved-environment-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".parse().unwrap(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "trap 'echo interrupted; exit 0' INT; echo ready > ready; while :; do :; done".into()],
        environment_variables: BTreeMap::new(),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let executor = LocalProcessExecutor::new(Duration::from_millis(30));
    let mut process = executor
        .spawn(
            &launch,
            root.path(),
            WorkerLogLimits::new(100, 100),
            ExecutorRequirements::default(),
        )
        .unwrap();
    let ready = process.private_working_directory().unwrap().join("ready");
    let start = Instant::now();
    while !ready.exists() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    executor.terminate(&mut process).unwrap();
    let output = process.wait().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.bytes(), b"interrupted\n");
}
