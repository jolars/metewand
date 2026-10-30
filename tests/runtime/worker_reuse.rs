#![cfg(any(unix, windows))]

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
use std::os::{
    fd::FromRawFd,
    unix::fs::{PermissionsExt, symlink},
};

use crate::core::{
    identity::record_id,
    manifest::{Enforcement, PrimaryTime, RunOrder, TimingScope},
    records::{
        ExecutionPolicyRecord, RecordId, ResolvedLaunchRecord, ResolvedWorkerRecord,
        WorkerWorkingDirectory,
    },
};
use crate::protocol::{Capability, WorkerRole};
use crate::runtime::{
    local_worker::launch_trusted_local_worker,
    worker_process::WorkerLogLimits,
    worker_reuse::{ReusableWorker, WorkerReuseError, WorkerReuseKey},
    worker_session::{PhaseTimeouts, WorkerSession, WorkerSessionError},
};
use serde_json::{Value, json};

const MODE: &str = "METEWAND_REUSE_FIXTURE";
const ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn fixture_worker() {
    let Some(mode) = env::var_os(MODE) else {
        return;
    };
    let (mut reader, mut writer) = protocol_streams();
    let mut executed = 0;
    let mut prepared = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            return;
        }
        let request: Value = serde_json::from_str(&line).unwrap();
        let mut response = json!({"id": request["id"], "ok": true});
        match request["method"].as_str().unwrap() {
            "hello" => {
                response["protocol"] = json!(1);
                response["worker_id"] = json!(ID);
                response["sdk"] = Value::Null;
                response["capabilities"] = json!(["one_shot"]);
            }
            "prepare" => {
                assert!(!prepared);
                assert!(!Path::new("dirty").exists());
                assert!(!Path::new("cache/dirty").exists());
                assert!(Path::new("cache/pip").is_dir());
                if let Some(path) = env::var_os("CUSTOM_PRIVATE") {
                    assert!(Path::new(&path).is_dir());
                    assert!(!Path::new(&path).join("dirty").exists());
                    assert!(fs::symlink_metadata("outside-link").is_err());
                    #[cfg(unix)]
                    assert_eq!(
                        fs::metadata(".").unwrap().permissions().mode() & 0o777,
                        0o700
                    );
                }
                prepared = true;
            }
            "execute" => {
                assert!(prepared);
                if mode == "crash" {
                    return;
                }
                fs::write("dirty", b"slot").unwrap();
                fs::write("cache/dirty", b"cache").unwrap();
                if let Some(path) = env::var_os("CUSTOM_PRIVATE") {
                    fs::write(Path::new(&path).join("dirty"), b"private").unwrap();
                    let outside = env::var_os("OUTSIDE").unwrap();
                    directory_link(Path::new(&outside), Path::new("outside-link"));
                    directory_link(
                        Path::new(&outside),
                        &Path::new("cache").join("outside-link"),
                    );
                    #[cfg(unix)]
                    fs::set_permissions("cache", fs::Permissions::from_mode(0o777)).unwrap();
                }
                executed += 1;
                response["result"] = json!({"manifest": "result.json"});
                response["implementation_time_ns"] = Value::Null;
                response["statistics"] = json!({"pid": std::process::id(), "executed": executed});
            }
            "reset" => {
                if mode == "reset-failure" {
                    response = json!({"id": request["id"], "ok": false,
                        "error": {"code": "operation_failed", "message": "reset failed", "details": {}}});
                }
                prepared = false;
            }
            _ => panic!("unexpected request"),
        }
        serde_json::to_writer(&mut writer, &response).unwrap();
        writer.write_all(b"\n").unwrap();
        writer.flush().unwrap();
    }
}

#[cfg(unix)]
fn protocol_streams() -> (BufReader<File>, File) {
    let fd = |name| env::var(name).unwrap().parse().unwrap();
    // SAFETY: The launcher transfers these two descriptors to the worker, and
    // each descriptor has exactly one owner here.
    let reader = BufReader::new(unsafe { File::from_raw_fd(fd("METEWAND_PROTOCOL_READ_FD")) });
    // SAFETY: The write descriptor is distinct from the read descriptor.
    let writer = unsafe { File::from_raw_fd(fd("METEWAND_PROTOCOL_WRITE_FD")) };
    (reader, writer)
}

#[cfg(windows)]
fn protocol_streams() -> (BufReader<File>, File) {
    let handle = |name| env::var(name).unwrap().parse::<usize>().unwrap() as *mut _;
    // SAFETY: The fixture takes ownership of each inherited endpoint once.
    let reader =
        BufReader::new(unsafe { File::from_raw_handle(handle("METEWAND_PROTOCOL_READ_HANDLE")) });
    // SAFETY: The write endpoint is distinct from the read endpoint.
    let writer = unsafe { File::from_raw_handle(handle("METEWAND_PROTOCOL_WRITE_HANDLE")) };
    (reader, writer)
}

#[cfg(unix)]
fn directory_link(target: &Path, link: &Path) {
    symlink(target, link).unwrap();
}

#[cfg(windows)]
fn directory_link(target: &Path, link: &Path) {
    // Junctions exercise directory-link cleanup without requiring symlink privileges.
    let output = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction creation failed: {output:?}"
    );
}

fn launch_record(mode: &str) -> ResolvedLaunchRecord {
    ResolvedLaunchRecord {
        environment: RecordId::from_digest([1; 32]),
        program: env::current_exe().unwrap().to_str().unwrap().into(),
        args: vec![
            "--exact".into(),
            "runtime_tests::worker_reuse::fixture_worker".into(),
            "--nocapture".into(),
        ],
        environment_variables: BTreeMap::from([(MODE.into(), mode.into())]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    }
}

fn key(launch: &ResolvedLaunchRecord, reuse: bool, warmups: u64) -> WorkerReuseKey {
    WorkerReuseKey {
        selected_capabilities: vec![crate::protocol::Capability::OneShot],
        role: WorkerRole::Implementation,
        definition: RecordId::from_digest([2; 32]),
        resolved: ResolvedWorkerRecord {
            source_bundle: None,
            environment: launch.environment,
            launch: record_id(launch).unwrap(),
        },
        policy: ExecutionPolicyRecord {
            max_concurrency: 1,
            name: serde_json::from_value(json!("default")).unwrap(),
            cpus: None,
            threads: None,
            memory: None,
            network: false,
            worker_reuse: reuse,
            warmup_runs: warmups,
            timeout: None,
            timing_scope: TimingScope::PrepareAndExecute,
            primary_time: PrimaryTime::TimedWallTime,
            run_order: RunOrder::Sequential,
            enforcement: Enforcement::BestEffort,
        },
    }
}

fn connect(
    launch: &ResolvedLaunchRecord,
    root: &Path,
) -> Result<WorkerSession, WorkerSessionError> {
    let process =
        launch_trusted_local_worker(launch, root, WorkerLogLimits::new(4096, 4096)).unwrap();
    WorkerSession::connect(
        process,
        WorkerRole::Implementation,
        ID.parse().unwrap(),
        &[Capability::OneShot],
        PhaseTimeouts::default(),
    )
}

fn slot(session: &mut WorkerSession) -> Result<Value, WorkerSessionError> {
    session.prepare(
        "/dataset",
        "mw1-problem-contract-test",
        json!({}),
        json!({}),
        1,
    )?;
    Ok(session.execute("/result")?.statistics().clone())
}

#[test]
fn reuses_process_resets_directories_and_warms_only_once() {
    let root = tempfile::tempdir().unwrap();
    let launch = launch_record("normal");
    let key = key(&launch, true, 2);
    let mut owner = ReusableWorker::default();
    let mut warmups = vec![];
    let first = owner
        .run_slot(
            key.clone(),
            || connect(&launch, root.path()),
            |session, index| {
                warmups.push(index);
                slot(session).map(|_| ())
            },
            slot,
        )
        .unwrap();
    assert_eq!(warmups, [0, 1]);
    assert_eq!(first["executed"], 3);
    let next = owner
        .run_slot(
            key,
            || panic!("must reuse"),
            |_, _| panic!("already warmed"),
            slot,
        )
        .unwrap();
    assert_eq!(first["pid"], next["pid"]);
    assert_eq!(next["executed"], 4);
    owner.discard();
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn fresh_policy_drops_every_process() {
    let root = tempfile::tempdir().unwrap();
    let launch = launch_record("normal");
    let mut owner = ReusableWorker::default();
    for _ in 0..2 {
        let result = owner
            .run_slot(
                key(&launch, false, 0),
                || connect(&launch, root.path()),
                |_, _| panic!("no warm-ups"),
                slot,
            )
            .unwrap();
        assert_eq!(result["executed"], 1);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn reset_failure_discards_worker_and_replacement_repeats_warmups() {
    let root = tempfile::tempdir().unwrap();
    let bad = launch_record("reset-failure");
    let good = launch_record("normal");
    let mut owner = ReusableWorker::default();
    let reuse_key = key(&bad, true, 0);
    owner
        .run_slot(
            reuse_key.clone(),
            || connect(&bad, root.path()),
            |_, _| unreachable!(),
            slot,
        )
        .unwrap();
    assert!(matches!(
        owner.run_slot(reuse_key, || unreachable!(), |_, _| unreachable!(), slot),
        Err(WorkerReuseError::Session(_))
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let mut warmups = vec![];
    let result = owner
        .run_slot(
            key(&good, true, 2),
            || connect(&good, root.path()),
            |session, index| {
                warmups.push(index);
                slot(session).map(|_| ())
            },
            slot,
        )
        .unwrap();
    assert_eq!(warmups, [0, 1]);
    assert_eq!(result["executed"], 3);
}

#[test]
fn failed_warmup_discards_process_and_restarts_at_zero() {
    let root = tempfile::tempdir().unwrap();
    let launch = launch_record("crash");
    let mut owner = ReusableWorker::default();
    for _ in 0..2 {
        let mut indices = vec![];
        let result: Result<(), _> = owner.run_slot(
            key(&launch, true, 2),
            || connect(&launch, root.path()),
            |session, index| {
                indices.push(index);
                slot(session).map(|_| ())
            },
            |_| panic!("must not measure"),
        );
        assert!(matches!(
            result,
            Err(WorkerReuseError::Warmup { index: 0, .. })
        ));
        assert_eq!(indices, [0]);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn every_reuse_key_component_separates_workers() {
    let launch = launch_record("normal");
    let original = key(&launch, true, 0);
    let mut variants = vec![];
    let mut changed = original.clone();
    changed.selected_capabilities = vec![Capability::FreshSequence];
    variants.push(changed);
    let mut changed = original.clone();
    changed.role = WorkerRole::ProblemEvaluator;
    variants.push(changed);
    let mut changed = original.clone();
    changed.definition = RecordId::from_digest([8; 32]);
    variants.push(changed);
    let mut changed = original.clone();
    changed.resolved.source_bundle = Some(RecordId::from_digest([3; 32]));
    variants.push(changed);
    let mut changed = original.clone();
    changed.resolved.environment = RecordId::from_digest([4; 32]);
    variants.push(changed);
    let mut changed = original.clone();
    changed.resolved.launch = RecordId::from_digest([5; 32]);
    variants.push(changed);
    let mut changed = original.clone();
    changed.policy.warmup_runs = 1;
    variants.push(changed);
    for changed in variants {
        assert_ne!(original, changed);
    }
}

#[test]
fn discarding_a_warmed_worker_repeats_warmups_for_the_same_key() {
    let root = tempfile::tempdir().unwrap();
    let launch = launch_record("normal");
    let reuse_key = key(&launch, true, 2);
    let mut owner = ReusableWorker::default();
    for _ in 0..2 {
        let mut indices = vec![];
        let result = owner
            .run_slot(
                reuse_key.clone(),
                || connect(&launch, root.path()),
                |session, index| {
                    indices.push(index);
                    slot(session).map(|_| ())
                },
                slot,
            )
            .unwrap();
        assert_eq!(indices, [0, 1]);
        assert_eq!(result["executed"], 3);
        owner.discard();
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn key_change_replaces_the_worker_and_repeats_warmups() {
    let root = tempfile::tempdir().unwrap();
    let launch = launch_record("normal");
    let mut reuse_key = key(&launch, true, 1);
    let mut owner = ReusableWorker::default();
    owner
        .run_slot(
            reuse_key.clone(),
            || connect(&launch, root.path()),
            |session, _| slot(session).map(|_| ()),
            slot,
        )
        .unwrap();
    reuse_key.policy.warmup_runs = 2;
    let mut indices = vec![];
    let result = owner
        .run_slot(
            reuse_key,
            || connect(&launch, root.path()),
            |session, index| {
                indices.push(index);
                slot(session).map(|_| ())
            },
            slot,
        )
        .unwrap();
    assert_eq!(indices, [0, 1]);
    assert_eq!(result["executed"], 3);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn incompatible_timing_policies_fail_before_launch() {
    let launch = launch_record("normal");
    let mut owner = ReusableWorker::default();
    let mut cold = key(&launch, true, 0);
    cold.policy.timing_scope = TimingScope::ColdEndToEnd;
    for reuse_key in [cold, key(&launch, false, 1)] {
        let result: Result<(), WorkerReuseError> = owner.run_slot(
            reuse_key,
            || panic!("must not launch"),
            |_, _| panic!("must not warm"),
            |_| panic!("must not measure"),
        );
        assert!(matches!(result, Err(WorkerReuseError::InvalidPolicy)));
    }
}

#[test]
fn cleanup_restores_private_paths_and_permissions_without_following_links() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("keep"), b"outside").unwrap();
    let mut launch = launch_record("normal");
    launch
        .environment_variables
        .insert("CUSTOM_PRIVATE".into(), "{worker}/custom/nested".into());
    launch
        .environment_variables
        .insert("OUTSIDE".into(), outside.path().to_str().unwrap().into());
    let reuse_key = key(&launch, true, 2);
    let mut owner = ReusableWorker::default();
    let first = owner
        .run_slot(
            reuse_key.clone(),
            || connect(&launch, root.path()),
            |session, _| slot(session).map(|_| ()),
            slot,
        )
        .unwrap();
    assert_eq!(first["executed"], 3);
    let next = owner
        .run_slot(
            reuse_key,
            || panic!("must reuse"),
            |_, _| unreachable!(),
            slot,
        )
        .unwrap();
    assert_eq!(first["pid"], next["pid"]);
    assert_eq!(next["executed"], 4);
    owner.discard();
    assert_eq!(fs::read(outside.path().join("keep")).unwrap(), b"outside");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
