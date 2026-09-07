#![cfg(unix)]

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    os::{fd::FromRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use metewand_core::{
    identity::IdentityKind,
    records::{RecordId, ResolvedEnvironmentRecord, ResolvedLaunchRecord, WorkerWorkingDirectory},
};
use metewand_protocol::{WorkerIdentity, WorkerRole};
use metewand_runtime::{
    local_worker::launch_trusted_local_worker,
    worker_process::{PROTOCOL_READ_FD_ENV, PROTOCOL_WRITE_FD_ENV, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerSession},
};
use serde_json::json;
use tempfile::TempDir;

const FIXTURE_MODE: &str = "METEWAND_TEST_LOCAL_WORKER";
const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn fixture_worker() {
    if env::var_os(FIXTURE_MODE).is_none() {
        return;
    }

    let read_fd = protocol_fd(PROTOCOL_READ_FD_ENV);
    let write_fd = protocol_fd(PROTOCOL_WRITE_FD_ENV);
    // SAFETY: The launcher grants the fixture ownership of each inherited
    // protocol descriptor, and this fixture creates exactly one owner for each.
    let read_pipe = unsafe { File::from_raw_fd(read_fd) };
    // SAFETY: See the ownership argument above.
    let mut write_pipe = unsafe { File::from_raw_fd(write_fd) };
    let mut request = String::new();
    BufReader::new(read_pipe).read_line(&mut request).unwrap();
    assert_eq!(request, "inspect\n");

    let mut environment = env::vars().collect::<BTreeMap<_, _>>();
    environment.remove(PROTOCOL_READ_FD_ENV);
    environment.remove(PROTOCOL_WRITE_FD_ENV);
    serde_json::to_writer(
        &mut write_pipe,
        &json!({
            "cwd": env::current_dir().unwrap(),
            "environment": environment,
        }),
    )
    .unwrap();
    write_pipe.write_all(b"\n").unwrap();
    write_pipe.flush().unwrap();
}

#[test]
fn trusted_local_launch_uses_a_private_directory_and_only_allowlisted_environment() {
    let private_root = TempDir::new().unwrap();
    let launch = ResolvedLaunchRecord {
        environment: record_id::<ResolvedEnvironmentRecord>(1),
        program: env::current_exe().unwrap().to_str().unwrap().to_owned(),
        args: vec![
            "--exact".to_owned(),
            "fixture_worker".to_owned(),
            "--nocapture".to_owned(),
        ],
        environment_variables: BTreeMap::from([
            (FIXTURE_MODE.to_owned(), "1".to_owned()),
            ("METEWAND_ALLOWED".to_owned(), "present".to_owned()),
        ]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };

    let mut worker = launch_trusted_local_worker(
        &launch,
        private_root.path(),
        WorkerLogLimits::new(4096, 4096),
    )
    .unwrap();
    let working_directory = worker.private_working_directory().unwrap().to_owned();

    assert_eq!(working_directory.parent(), Some(private_root.path()));
    assert_ne!(working_directory, private_root.path());
    assert_eq!(
        fs::metadata(&working_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    worker.protocol_writer().write_all(b"inspect\n").unwrap();
    worker.protocol_writer().flush().unwrap();
    let response = worker.protocol_reader().read_frame().unwrap().unwrap();
    assert_eq!(
        response,
        json!({
            "cwd": working_directory,
            "environment": {
                (FIXTURE_MODE): "1",
                "METEWAND_ALLOWED": "present",
            },
        })
    );

    let output = worker.wait().unwrap();
    assert!(output.status.success());
    assert!(!working_directory.exists());
}

#[test]
fn trusted_local_launch_runs_a_raw_fixture_worker() {
    let private_root = TempDir::new().unwrap();
    let launch = ResolvedLaunchRecord {
        environment: record_id::<ResolvedEnvironmentRecord>(1),
        program: fixture_path("dataset-materializer")
            .to_str()
            .unwrap()
            .to_owned(),
        args: Vec::new(),
        environment_variables: BTreeMap::from([("PATH".to_owned(), env::var("PATH").unwrap())]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let process = launch_trusted_local_worker(
        &launch,
        private_root.path(),
        WorkerLogLimits::new(4096, 4096),
    )
    .unwrap();

    let session = WorkerSession::connect(
        process,
        WorkerRole::DatasetMaterializer,
        WorkerIdentity::from_str(WORKER_ID).unwrap(),
        &[],
        phase_timeouts(),
    )
    .unwrap();
    let output = session.shutdown().unwrap();

    assert!(output.status.success());
}

fn protocol_fd(name: &str) -> i32 {
    env::var(name).unwrap().parse().unwrap()
}

fn record_id<T: IdentityKind>(byte: u8) -> RecordId<T> {
    RecordId::from_digest([byte; 32])
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/worker-protocol/v1")
        .join(name)
}

fn phase_timeouts() -> PhaseTimeouts {
    PhaseTimeouts {
        hello: Duration::from_secs(2),
        materialize: Duration::from_secs(2),
        prepare: Duration::from_secs(2),
        execute: Duration::from_secs(2),
        reset: Duration::from_secs(2),
        evaluate: Duration::from_secs(2),
        shutdown: Duration::from_secs(2),
    }
}
