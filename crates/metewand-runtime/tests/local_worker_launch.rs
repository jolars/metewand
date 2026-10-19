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
    File::create("created-file").unwrap();
    fs::create_dir("created-directory").unwrap();
    serde_json::to_writer(
        &mut write_pipe,
        &json!({
            "cwd": env::current_dir().unwrap(),
            "environment": environment,
            "file_mode": fs::metadata("created-file").unwrap().permissions().mode() & 0o777,
            "directory_mode": fs::metadata("created-directory").unwrap().permissions().mode() & 0o777,
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
    assert_eq!(response["cwd"], json!(working_directory));
    assert_eq!(response["file_mode"], 0o600);
    assert_eq!(response["directory_mode"], 0o700);
    let environment = response["environment"].as_object().unwrap();
    let mut expected = metewand_runtime::worker_environment::worker_environment_template();
    for value in expected.values_mut() {
        *value = value.replace("{worker}", working_directory.to_str().unwrap());
    }
    expected.extend(launch.environment_variables.clone());
    assert_eq!(response["environment"], json!(expected));
    assert_eq!(environment["LC_ALL"], "C");
    assert_eq!(environment["LANG"], "C");
    assert_eq!(environment["TZ"], "UTC");
    assert_eq!(environment[FIXTURE_MODE], "1");
    assert_eq!(environment["METEWAND_ALLOWED"], "present");
    for variable in [
        "HOME",
        "TMPDIR",
        "TMP",
        "TEMP",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "PYTHONUSERBASE",
        "PIP_CACHE_DIR",
        "UV_CACHE_DIR",
        "R_USER",
        "R_LIBS_USER",
        "JULIA_DEPOT_PATH",
    ] {
        let path = Path::new(environment[variable].as_str().unwrap());
        assert!(path.starts_with(&working_directory), "{variable}: {path:?}");
        assert!(path.is_dir(), "{variable}: {path:?}");
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    }

    let output = worker.wait().unwrap();
    assert!(output.status.success());
    let provenance = output.environment.unwrap();
    assert_eq!(provenance.environment_variables, expected);
    assert_eq!(provenance.working_directory, working_directory);
    assert_eq!(provenance.umask, "0077");
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
