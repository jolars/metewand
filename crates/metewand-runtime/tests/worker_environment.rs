#![cfg(unix)]

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    os::{
        fd::FromRawFd,
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    process::Command,
    time::Duration,
};

use metewand_core::{
    identity::record_id,
    public_schemas::{PublicSchema, public_schema_catalog},
    records::{RecordId, ResolvedLaunchRecord, WorkerWorkingDirectory},
};
use metewand_protocol::WorkerRole;
use metewand_runtime::{
    executor::{ExecutorRequirements, LocalProcessExecutor},
    local_worker::launch_trusted_local_worker,
    worker_environment::{
        SECRET_TEMPLATE, WorkerSecrets, resolve_worker_environment, worker_environment_template,
    },
    worker_process::{PROTOCOL_READ_FD_ENV, PROTOCOL_WRITE_FD_ENV, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerSession},
};
use serde_json::json;

fn launch(mode: &str) -> ResolvedLaunchRecord {
    ResolvedLaunchRecord {
        environment: RecordId::from_digest([1; 32]),
        program: env::current_exe().unwrap().to_str().unwrap().to_owned(),
        args: vec![
            "--exact".into(),
            "fixture_worker".into(),
            "--nocapture".into(),
        ],
        environment_variables: BTreeMap::from([("METEWAND_ENV_FIXTURE".into(), mode.into())]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    }
}

#[test]
fn fixture_worker() {
    let Ok(mode) = env::var("METEWAND_ENV_FIXTURE") else {
        return;
    };
    if mode == "ambient" {
        assert_eq!(env::var("LC_ALL").unwrap(), "C");
        assert_eq!(env::var("TZ").unwrap(), "UTC");
        assert!(env::var_os("AMBIENT_TOKEN").is_none());
        assert!(env::var_os("PYTHONPATH").is_none());
        assert!(env::var_os("R_PROFILE_USER").is_none());
        assert_eq!(
            env::var("HOME").unwrap(),
            env::current_dir().unwrap().join("home").to_str().unwrap()
        );
        return;
    }
    let secret = env::var("TOKEN").unwrap();
    assert!(!secret.is_empty());
    print!("before {secret} after");
    eprint!("error {secret}");
    if mode == "logs" {
        return;
    }

    // SAFETY: This fixture owns each descriptor inherited from the launcher.
    let read =
        unsafe { File::from_raw_fd(env::var(PROTOCOL_READ_FD_ENV).unwrap().parse().unwrap()) };
    // SAFETY: Each inherited endpoint is wrapped exactly once.
    let mut write =
        unsafe { File::from_raw_fd(env::var(PROTOCOL_WRITE_FD_ENV).unwrap().parse().unwrap()) };
    let mut request = String::new();
    BufReader::new(read).read_line(&mut request).unwrap();
    if mode == "malformed" {
        writeln!(write, "{{\"{secret}\":1,\"{secret}\":2}}").unwrap();
    } else {
        let request: serde_json::Value = serde_json::from_str(&request).unwrap();
        writeln!(
            write,
            "{}",
            json!({"id": request["id"], "ok": false, "error": {
                "code": "operation_failed", "message": format!("failed with {secret}"),
                "details": {(&secret): [format!("nested {secret}")]},
            }})
        )
        .unwrap();
    }
}

#[test]
fn fixture_ambient_launcher() {
    let Some(root) = env::var_os("METEWAND_AMBIENT_ROOT") else {
        return;
    };
    let output = launch_trusted_local_worker(
        &launch("ambient"),
        root.as_ref(),
        WorkerLogLimits::new(4096, 4096),
    )
    .unwrap()
    .wait()
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    let parent_file = std::path::Path::new(&root).join("parent-file");
    File::create(&parent_file).unwrap();
    assert_eq!(
        fs::metadata(&parent_file).unwrap().permissions().mode() & 0o777,
        0
    );
    fs::remove_file(parent_file).unwrap();
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}

#[test]
fn hostile_ambient_settings_and_restrictive_umask_do_not_reach_workers() {
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args(["--exact", "fixture_ambient_launcher", "--nocapture"])
        .env("METEWAND_AMBIENT_ROOT", root.path())
        .env("HOME", "/ambient/home")
        .env("TMPDIR", "/ambient/tmp")
        .env("XDG_CACHE_HOME", "/ambient/cache")
        .env("LC_ALL", "invalid-locale")
        .env("TZ", "Pacific/Honolulu")
        .env("PYTHONPATH", "/ambient/python")
        .env("R_PROFILE_USER", "/ambient/Rprofile")
        .env("AMBIENT_TOKEN", "ambient-secret");
    // SAFETY: Changing umask only in this child avoids changing parallel tests.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o777);
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn conflicts_bad_names_and_escaping_paths_fail_before_allocation() {
    let root = tempfile::tempdir().unwrap();
    for (name, value) in [
        ("HOME", "/ambient"),
        ("LC_ALL", "en_US.UTF-8"),
        ("TZ", "local"),
        ("TMPDIR", "/ambient"),
        ("JULIA_DEPOT_PATH", "/ambient"),
        (PROTOCOL_READ_FD_ENV, "0"),
        ("BAD=NAME", "value"),
        ("BAD", "nul\0value"),
        ("EXTRA_CACHE", "{worker}/../escape"),
        ("EXTRA_CACHE", "{worker}//escape"),
    ] {
        let mut launch = launch("ambient");
        launch
            .environment_variables
            .insert(name.into(), value.into());
        assert!(
            launch_trusted_local_worker(&launch, root.path(), WorkerLogLimits::new(0, 0)).is_err(),
            "{name}"
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
    let mut missing_program = launch("ambient");
    missing_program.program = "/missing-worker-program".into();
    assert!(
        launch_trusted_local_worker(&missing_program, root.path(), WorkerLogLimits::new(0, 0))
            .is_err()
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn secret_values_are_ephemeral_and_redacted_in_logs_and_provenance() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch("logs");
    launch
        .environment_variables
        .insert("TOKEN".into(), SECRET_TEMPLATE.into());
    let identity = record_id(&launch).unwrap();
    let executor = LocalProcessExecutor::new(Duration::ZERO);
    for value in ["first-test-secret", "second-test-secret"] {
        let secrets = WorkerSecrets::new(BTreeMap::from([("TOKEN".into(), value.into())])).unwrap();
        assert!(!format!("{secrets:?}").contains(value));
        let process = executor
            .spawn_with_secrets(
                &launch,
                root.path(),
                WorkerLogLimits::new(4096, 4096),
                ExecutorRequirements::default(),
                &secrets,
            )
            .unwrap();
        assert!(!format!("{process:?}").contains(value));
        let provenance = serde_json::to_value(process.environment().unwrap()).unwrap();
        public_schema_catalog()
            .unwrap()
            .validate(
                PublicSchema::WorkerEnvironment.repository_path(),
                &provenance,
            )
            .unwrap();
        assert_eq!(
            provenance["environment_variables"]["TOKEN"],
            SECRET_TEMPLATE
        );
        assert!(!provenance.to_string().contains(value));
        let output = process.wait().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(output.stdout.bytes()).contains("before [REDACTED] after"));
        assert_eq!(output.stderr.bytes(), b"error [REDACTED]");
        assert!(!format!("{output:?}").contains(value));
        assert_eq!(record_id(&launch).unwrap(), identity);
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn secret_bindings_must_exactly_match_declarations() {
    let root = tempfile::tempdir().unwrap();
    let executor = LocalProcessExecutor::new(Duration::ZERO);
    let secrets =
        WorkerSecrets::new(BTreeMap::from([("TOKEN".into(), "test-secret".into())])).unwrap();
    let mut launch = launch("logs");
    for declaration in [None, Some("literal"), Some(SECRET_TEMPLATE)] {
        if let Some(declaration) = declaration {
            launch
                .environment_variables
                .insert("TOKEN".into(), declaration.into());
        }
        let provided = if declaration == Some(SECRET_TEMPLATE) {
            &WorkerSecrets::default()
        } else {
            &secrets
        };
        let error = executor
            .spawn_with_secrets(
                &launch,
                root.path(),
                WorkerLogLimits::new(0, 0),
                ExecutorRequirements::default(),
                provided,
            )
            .unwrap_err();
        assert!(!format!("{error:?}").contains("test-secret"));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
    for value in ["", "has\0nul"] {
        assert!(WorkerSecrets::new(BTreeMap::from([("TOKEN".into(), value.into())])).is_err());
    }
}

#[test]
fn session_failures_redact_nested_values_keys_and_malformed_input() {
    let root = tempfile::tempdir().unwrap();
    for mode in ["failure", "malformed"] {
        let mut launch = launch(mode);
        launch
            .environment_variables
            .insert("TOKEN".into(), SECRET_TEMPLATE.into());
        let value = "secret-with-quote-\"-and-unicode-å";
        let secrets = WorkerSecrets::new(BTreeMap::from([("TOKEN".into(), value.into())])).unwrap();
        let process = LocalProcessExecutor::new(Duration::ZERO)
            .spawn_with_secrets(
                &launch,
                root.path(),
                WorkerLogLimits::new(1024, 1024),
                ExecutorRequirements::default(),
                &secrets,
            )
            .unwrap();
        let timeout = Duration::from_secs(2);
        let error = WorkerSession::connect(
            process,
            WorkerRole::Implementation,
            "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .parse()
                .unwrap(),
            &[],
            PhaseTimeouts {
                hello: timeout,
                materialize: timeout,
                prepare: timeout,
                execute: timeout,
                reset: timeout,
                evaluate: timeout,
                shutdown: timeout,
            },
        )
        .unwrap_err();
        assert!(!format!("{error:?}").contains(value));
        assert!(error.to_string().contains("redacted") || error.to_string().contains("[REDACTED]"));
        if mode == "failure" {
            let metewand_runtime::worker_session::WorkerSessionError::WorkerFailure {
                failure, ..
            } = error
            else {
                panic!("expected a structured failure");
            };
            assert_eq!(failure.message(), "failed with [REDACTED]");
            assert_eq!(
                failure.details(),
                Some(&json!({"[REDACTED]": ["nested [REDACTED]"]}))
            );
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn stable_templates_are_idempotent_and_contain_no_allocated_paths() {
    let template = worker_environment_template();
    assert_eq!(resolve_worker_environment(&template).unwrap(), template);
    assert_eq!(template["HOME"], "{worker}/home");
    assert_eq!(template["TMPDIR"], "{worker}/tmp");
    assert_eq!(template["XDG_CACHE_HOME"], "{worker}/cache");
    assert!(template.values().all(|value| !value.starts_with('/')));
}
