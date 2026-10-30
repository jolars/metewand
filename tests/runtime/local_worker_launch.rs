use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::Path,
};

#[cfg(windows)]
use std::os::windows::io::FromRawHandle;
#[cfg(unix)]
use std::{
    os::{fd::FromRawFd, unix::fs::PermissionsExt},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

use crate::core::{
    identity::IdentityKind,
    records::{RecordId, ResolvedEnvironmentRecord, ResolvedLaunchRecord, WorkerWorkingDirectory},
};
#[cfg(unix)]
use crate::protocol::{WorkerIdentity, WorkerRole};
#[cfg(unix)]
use crate::runtime::worker_session::{PhaseTimeouts, WorkerSession};
use crate::runtime::{
    local_worker::{LocalWorkerLaunchError, launch_trusted_local_worker},
    worker_environment::WorkerEnvironmentError,
    worker_process::{PROTOCOL_READ_FD_ENV, PROTOCOL_WRITE_FD_ENV, WorkerLogLimits},
};
use serde_json::json;
use tempfile::TempDir;

const FIXTURE_MODE: &str = "METEWAND_TEST_LOCAL_WORKER";
#[cfg(unix)]
const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn fixture_worker() {
    if env::var_os(FIXTURE_MODE).is_none() {
        return;
    }

    let (read_pipe, mut write_pipe) = protocol_files();
    let mut request = String::new();
    BufReader::new(read_pipe).read_line(&mut request).unwrap();
    assert_eq!(request, "inspect\n");

    let mut environment = env::vars().collect::<BTreeMap<_, _>>();
    environment.remove(PROTOCOL_READ_FD_ENV);
    environment.remove(PROTOCOL_WRITE_FD_ENV);
    environment.remove("METEWAND_PROTOCOL_READ_HANDLE");
    environment.remove("METEWAND_PROTOCOL_WRITE_HANDLE");
    File::create("created-file").unwrap();
    fs::create_dir("created-directory").unwrap();
    serde_json::to_writer(
        &mut write_pipe,
        &json!({
            "cwd": env::current_dir().unwrap(),
            "environment": environment,
            "file_mode": file_mode(Path::new("created-file")),
            "directory_mode": file_mode(Path::new("created-directory")),
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
            "runtime_tests::local_worker_launch::fixture_worker".to_owned(),
            "--nocapture".to_owned(),
        ],
        environment_variables: BTreeMap::from([
            (FIXTURE_MODE.to_owned(), "1".to_owned()),
            ("METEWAND_ALLOWED".to_owned(), "present".to_owned()),
            (
                "EXTRA_CACHE".to_owned(),
                "{worker}/cache/custom path/café".to_owned(),
            ),
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

    let canonical_root = fs::canonicalize(private_root.path()).unwrap();
    assert_eq!(working_directory.parent(), Some(canonical_root.as_path()));
    assert_ne!(working_directory, private_root.path());
    #[cfg(unix)]
    assert_eq!(file_mode(&working_directory), Some(0o700));

    worker.protocol_writer().write_all(b"inspect\n").unwrap();
    worker.protocol_writer().flush().unwrap();
    let response = worker.protocol_reader().read_frame().unwrap().unwrap();
    assert_eq!(
        fs::canonicalize(response["cwd"].as_str().unwrap()).unwrap(),
        working_directory,
    );
    #[cfg(unix)]
    {
        assert_eq!(response["file_mode"], 0o600);
        assert_eq!(response["directory_mode"], 0o700);
    }
    let environment = response["environment"].as_object().unwrap();
    let mut expected = crate::runtime::worker_environment::resolve_worker_environment(
        &launch.environment_variables,
    )
    .unwrap();
    for value in expected.values_mut() {
        if let Some(relative) = value.strip_prefix("{worker}/") {
            let mut path = working_directory.clone();
            for component in relative.split('/') {
                path.push(component);
            }
            *value = path.to_str().unwrap().to_owned();
        }
    }
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
        "EXTRA_CACHE",
    ] {
        let path = Path::new(environment[variable].as_str().unwrap());
        assert!(path.starts_with(&working_directory), "{variable}: {path:?}");
        assert!(path.is_dir(), "{variable}: {path:?}");
        #[cfg(unix)]
        assert_eq!(file_mode(path), Some(0o700));
    }

    let output = worker.wait().unwrap();
    assert!(output.status.success());
    let provenance = output.environment.unwrap();
    assert_eq!(provenance.environment_variables, expected);
    assert_eq!(provenance.working_directory, working_directory);
    #[cfg(unix)]
    assert_eq!(provenance.umask.as_deref(), Some("0077"));
    #[cfg(windows)]
    {
        assert_eq!(provenance.version, 2);
        assert_eq!(provenance.umask, None);
        assert_eq!(
            provenance.directory_access.as_deref(),
            Some("current_user_and_system")
        );
    }
    assert!(!working_directory.exists());
}

#[test]
fn native_traversal_in_worker_directory_templates_fails_before_allocation() {
    let storage = TempDir::new().unwrap();
    let private_root = storage.path().join("private");
    fs::create_dir(&private_root).unwrap();
    let launch = ResolvedLaunchRecord {
        environment: record_id::<ResolvedEnvironmentRecord>(1),
        program: storage
            .path()
            .join("missing-worker")
            .to_str()
            .unwrap()
            .to_owned(),
        args: Vec::new(),
        environment_variables: BTreeMap::from([(
            "EXTRA_CACHE".to_owned(),
            r"{worker}/..\..\escaped".to_owned(),
        )]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };

    let error = launch_trusted_local_worker(&launch, &private_root, WorkerLogLimits::new(0, 0))
        .unwrap_err();
    assert!(
        matches!(
            &error,
            LocalWorkerLaunchError::Environment(WorkerEnvironmentError::InvalidPath)
        ),
        "{error:?}"
    );
    assert_eq!(fs::read_dir(&private_root).unwrap().count(), 0);
    assert_eq!(fs::read_dir(storage.path()).unwrap().count(), 1);
    assert!(!storage.path().join("escaped").exists());
}

#[test]
#[cfg(unix)]
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

#[cfg(unix)]
fn protocol_fd(name: &str) -> i32 {
    env::var(name).unwrap().parse().unwrap()
}

fn record_id<T: IdentityKind>(byte: u8) -> RecordId<T> {
    RecordId::from_digest([byte; 32])
}

#[cfg(unix)]
fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/worker-protocol/v1")
        .join(name)
}

#[cfg(unix)]
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

fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        Some(fs::metadata(path).unwrap().permissions().mode() & 0o777)
    }
    #[cfg(windows)]
    {
        let _ = path;
        None
    }
}

#[cfg(unix)]
fn protocol_files() -> (File, File) {
    // SAFETY: The fixture takes ownership of each inherited descriptor once.
    unsafe {
        (
            File::from_raw_fd(protocol_fd(PROTOCOL_READ_FD_ENV)),
            File::from_raw_fd(protocol_fd(PROTOCOL_WRITE_FD_ENV)),
        )
    }
}

#[cfg(windows)]
fn protocol_files() -> (File, File) {
    let handle = |name| env::var(name).unwrap().parse::<usize>().unwrap() as *mut _;
    // SAFETY: The fixture takes ownership of each distinct inherited handle once.
    unsafe {
        (
            File::from_raw_handle(handle("METEWAND_PROTOCOL_READ_HANDLE")),
            File::from_raw_handle(handle("METEWAND_PROTOCOL_WRITE_HANDLE")),
        )
    }
}
