#![cfg(windows)]

use std::{
    collections::BTreeMap,
    env, fs,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use metewand_core::records::{ResolvedLaunchRecord, WorkerWorkingDirectory};
use metewand_runtime::{
    executor::{Executor, ExecutorRequirements, LocalProcessExecutor},
    worker_process::{WorkerLogLimits, WorkerProcess},
};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

const FIXTURE_MODE: &str = "METEWAND_WINDOWS_PROCESS_FIXTURE";

#[test]
#[allow(
    clippy::zombie_processes,
    reason = "The fixture tests job cleanup after the parent exits without waiting."
)]
fn fixture_worker() {
    match env::var(FIXTURE_MODE).as_deref() {
        Ok("descendant") => {
            fs::write("descendant.pending", std::process::id().to_string()).unwrap();
            fs::rename("descendant.pending", "descendant.ready").unwrap();
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        Ok(mode @ ("wait-parent" | "exit-parent")) => {
            let child = fixture_command("descendant").spawn().unwrap();
            await_file("descendant.ready");
            println!("descendant {}", child.id());
            if mode == "wait-parent" {
                loop {
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
        _ => {}
    }
}

fn fixture_command(mode: &str) -> Command {
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args(["--exact", "fixture_worker", "--nocapture"])
        .env(FIXTURE_MODE, mode);
    command
}

fn await_file(path: impl AsRef<std::path::Path>) {
    let start = Instant::now();
    while !path.as_ref().exists() {
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(5));
    }
}

fn launch(mode: &str) -> ResolvedLaunchRecord {
    ResolvedLaunchRecord {
        environment: "mw1-resolved-environment-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".parse().unwrap(),
        program: env::current_exe().unwrap().to_str().unwrap().to_owned(),
        args: vec!["--exact".into(), "fixture_worker".into(), "--nocapture".into()],
        environment_variables: BTreeMap::from([(FIXTURE_MODE.to_owned(), mode.to_owned())]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    }
}

#[test]
fn windows_jobs_provide_complete_process_tree_containment() {
    let capabilities = LocalProcessExecutor::new(Duration::ZERO)
        .preflight(ExecutorRequirements {
            complete_process_tree: true,
        })
        .unwrap();
    assert!(capabilities.complete_process_tree);
    assert!(capabilities.private_working_directory);
    assert!(!capabilities.process_groups);
}

#[test]
fn forced_termination_kills_descendants_and_releases_private_directory() {
    let root = tempfile::tempdir().unwrap();
    let executor = LocalProcessExecutor::new(Duration::from_secs(30));
    let mut worker = executor
        .spawn(
            &launch("wait-parent"),
            root.path(),
            WorkerLogLimits::new(4096, 4096),
            ExecutorRequirements {
                complete_process_tree: true,
            },
        )
        .unwrap();
    let private = worker.private_working_directory().unwrap().to_owned();
    let marker = private.join("descendant.ready");
    await_file(&marker);
    let pid = fs::read_to_string(marker).unwrap().parse().unwrap();
    // SAFETY: The test requests observation access and takes ownership once.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!process.is_null());
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
    let start = Instant::now();
    executor.terminate(&mut worker).unwrap();
    let output = worker.wait().unwrap();
    assert!(!output.status.success());
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), 1000) },
        WAIT_OBJECT_0
    );
    assert!(!private.exists());
}

#[test]
fn normal_exit_terminates_descendants_holding_standard_streams() {
    let root = tempfile::tempdir().unwrap();
    let mut command = fixture_command("exit-parent");
    command.current_dir(root.path());
    let worker = WorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    let start = Instant::now();
    let output = worker.wait().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(output.stdout.bytes()).contains("descendant"));
    assert!(start.elapsed() < Duration::from_secs(10));
}
