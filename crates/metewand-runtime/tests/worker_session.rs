#![cfg(unix)]

use std::{
    env,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    os::fd::FromRawFd,
    process::Command,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use metewand_protocol::{
    Capability, HandshakeError, WorkerFailureCode, WorkerIdentity, WorkerRole,
};
use metewand_runtime::{
    worker_process::{PosixWorkerProcess, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerPhase, WorkerSession, WorkerSessionError},
};
use serde_json::{Value, json};

const FIXTURE_MODE: &str = "METEWAND_SESSION_FIXTURE_MODE";
const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn fixture_worker() {
    match env::var(FIXTURE_MODE).as_deref() {
        Ok("materializer") => run_materializer_fixture(),
        Ok("implementation") => run_implementation_fixture(),
        Ok("evaluator") => run_evaluator_fixture(),
        Ok("failure") => run_failure_fixture(),
        Ok("timeout") => run_timeout_fixture(),
        Ok("interrupt-timeout") => run_interrupt_timeout_fixture(),
        Ok("shared-deadline") => run_shared_deadline_fixture(),
        Ok("hello-timeout") => run_hello_timeout_fixture(),
        Ok("shutdown-timeout") => run_shutdown_timeout_fixture(),
        Ok("reserved") => run_reserved_fixture(),
        _ => {}
    }
}

#[test]
fn runs_materialize_and_shutdown_exchanges() {
    let mut session = connect("materializer", WorkerRole::DatasetMaterializer, &[]);
    let response = session
        .materialize("/bundle", json!({"rows": 100}), 17, "/dataset-output")
        .expect("materialization must succeed");
    assert_eq!(response.manifest(), "dataset-manifest.json");

    let output = session.shutdown().expect("shutdown must succeed");
    assert!(output.status.success());
}

#[test]
fn runs_prepare_execute_reset_and_shutdown_exchanges() {
    let mut session = connect(
        "implementation",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    session
        .prepare(
            "/dataset",
            "mw1-problem-contract-abc",
            json!({"lambda": 0.1}),
            json!({"solver": "fast"}),
            23,
        )
        .expect("preparation must succeed");
    let response = session.execute("/result").expect("execution must succeed");
    assert_eq!(response.implementation_time_ns(), Some(18_342_011));
    assert_eq!(response.manifest(), "result.json");
    assert_eq!(response.statistics(), &json!({"iterations": 37}));
    session.reset().expect("reset must succeed");

    let output = session.shutdown().expect("shutdown must succeed");
    assert!(output.status.success());
}

#[test]
fn runs_evaluate_and_shutdown_exchanges() {
    let mut session = connect("evaluator", WorkerRole::ProblemEvaluator, &[]);
    session
        .evaluate(
            "/dataset",
            "mw1-problem-contract-abc",
            json!({"lambda": 0.1}),
            "/result",
            "/metrics.json",
        )
        .expect("evaluation must succeed");

    let output = session.shutdown().expect("shutdown must succeed");
    assert!(output.status.success());
}

#[test]
fn preserves_structured_worker_failures_with_their_phase() {
    let mut session = connect(
        "failure",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    let error = session
        .prepare(
            "/dataset",
            "mw1-problem-contract-abc",
            json!({}),
            json!({}),
            1,
        )
        .expect_err("fixture preparation must fail");

    assert!(matches!(
        error,
        WorkerSessionError::WorkerFailure {
            phase: WorkerPhase::Prepare,
            ref failure,
        } if failure.code() == WorkerFailureCode::OperationFailed
            && failure.message() == "fixture preparation failed"
            && failure.details() == Some(&json!({"reason": "fixture"}))
    ));
}

#[test]
fn applies_the_timeout_for_the_current_phase_and_discards_the_worker() {
    let process = spawn_fixture("timeout");
    let mut timeouts = timeouts();
    timeouts.execute = Duration::from_millis(20);
    let mut session = WorkerSession::connect(
        process,
        WorkerRole::Implementation,
        worker_identity(),
        &[Capability::OneShot],
        timeouts,
    )
    .expect("handshake must succeed");

    assert!(matches!(
        session.execute("/result"),
        Err(WorkerSessionError::Timeout {
            phase: WorkerPhase::Execute,
            limit,
        }) if limit == Duration::from_millis(20)
    ));
    assert!(matches!(
        session.reset(),
        Err(WorkerSessionError::SessionUnusable)
    ));
}

#[test]
fn a_scientific_deadline_discards_a_worker_before_the_operational_limit() {
    let mut session = connect(
        "timeout",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    session.set_scientific_deadline(Some(Instant::now() + Duration::from_millis(20)));
    assert!(matches!(
        session.execute("/result"),
        Err(WorkerSessionError::ScientificTimeout {
            phase: WorkerPhase::Execute
        })
    ));
    assert!(matches!(
        session.reset(),
        Err(WorkerSessionError::SessionUnusable)
    ));
}

#[test]
fn an_expired_scientific_deadline_prevents_another_exchange() {
    let mut session = connect(
        "implementation",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    session.set_scientific_deadline(Some(Instant::now()));
    assert!(matches!(
        session.prepare("/dataset", "contract", json!({}), json!({}), 0),
        Err(WorkerSessionError::ScientificTimeout {
            phase: WorkerPhase::Prepare
        })
    ));
    assert!(matches!(
        session.reset(),
        Err(WorkerSessionError::SessionUnusable)
    ));
}

#[test]
fn an_operational_limit_still_bounds_a_longer_scientific_scope() {
    let mut limits = timeouts();
    limits.execute = Duration::from_millis(20);
    let mut session = WorkerSession::connect_with_deadline(
        spawn_fixture("timeout"),
        WorkerRole::Implementation,
        worker_identity(),
        &[Capability::OneShot],
        limits,
        Some(Instant::now() + Duration::from_secs(5)),
    )
    .expect("handshake must succeed");
    assert!(
        matches!(session.execute("/result"), Err(WorkerSessionError::Timeout {
        phase: WorkerPhase::Execute, limit,
    }) if limit == Duration::from_millis(20))
    );
}

#[test]
fn a_scientific_scope_can_end_before_untimed_exchanges() {
    let mut session = connect(
        "implementation",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    session.set_scientific_deadline(Some(Instant::now()));
    session.set_scientific_deadline(None);
    session
        .prepare(
            "/dataset",
            "mw1-problem-contract-abc",
            json!({"lambda": 0.1}),
            json!({"solver": "fast"}),
            23,
        )
        .expect("clearing the scientific scope restores operational limits");
    session.execute("/result").unwrap();
    session.reset().unwrap();
    session.shutdown().unwrap();
}

#[test]
fn a_cold_scope_deadline_also_bounds_the_handshake() {
    assert!(matches!(
        WorkerSession::connect_with_deadline(
            spawn_fixture("hello-timeout"),
            WorkerRole::Implementation,
            worker_identity(),
            &[Capability::OneShot],
            timeouts(),
            Some(Instant::now() + Duration::from_millis(20)),
        ),
        Err(WorkerSessionError::ScientificTimeout {
            phase: WorkerPhase::Hello
        })
    ));
}

#[test]
fn preparation_and_execution_consume_one_shared_deadline() {
    let mut session = connect(
        "shared-deadline",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    session.set_scientific_deadline(Some(Instant::now() + Duration::from_millis(500)));
    session
        .prepare("/dataset", "contract", json!({}), json!({}), 0)
        .unwrap();
    assert!(matches!(
        session.execute("/result"),
        Err(WorkerSessionError::ScientificTimeout {
            phase: WorkerPhase::Execute,
        })
    ));
}

#[test]
fn a_zero_operational_limit_is_not_a_scientific_timeout() {
    let mut limits = timeouts();
    limits.execute = Duration::ZERO;
    let mut session = WorkerSession::connect(
        spawn_fixture("timeout"),
        WorkerRole::Implementation,
        worker_identity(),
        &[Capability::OneShot],
        limits,
    )
    .unwrap();
    assert!(matches!(
        session.execute("/result"),
        Err(WorkerSessionError::Timeout {
            phase: WorkerPhase::Execute,
            limit: Duration::ZERO,
        })
    ));
}

#[test]
fn scientific_and_operational_timeouts_interrupt_before_forcing_exit() {
    for scientific in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("interrupted");
        let mut command = fixture_command("interrupt-timeout");
        command.env("METEWAND_INTERRUPT_MARKER", &marker);
        let process = PosixWorkerProcess::spawn(command, WorkerLogLimits::new(1024, 1024)).unwrap();
        let mut limits = timeouts();
        if !scientific {
            limits.execute = Duration::from_millis(40);
        }
        let mut session = WorkerSession::connect(
            process,
            WorkerRole::Implementation,
            worker_identity(),
            &[Capability::OneShot],
            limits,
        )
        .unwrap();
        if scientific {
            session.set_scientific_deadline(Some(Instant::now() + Duration::from_millis(40)));
        }
        let error = session.execute("/result").unwrap_err();
        match error {
            WorkerSessionError::ScientificTimeout { .. } if scientific => {}
            WorkerSessionError::Timeout { .. } if !scientific => {}
            error => panic!("unexpected timeout kind: {error:?}"),
        }
        assert_eq!(std::fs::read(&marker).unwrap(), b"interrupted");
        assert!(matches!(
            session.reset(),
            Err(WorkerSessionError::SessionUnusable)
        ));
    }
}

#[test]
fn applies_a_distinct_timeout_to_the_handshake() {
    let mut timeouts = timeouts();
    timeouts.hello = Duration::from_millis(20);
    let error = WorkerSession::connect(
        spawn_fixture("hello-timeout"),
        WorkerRole::DatasetMaterializer,
        worker_identity(),
        &[],
        timeouts,
    )
    .expect_err("fixture handshake must time out");

    assert!(matches!(
        error,
        WorkerSessionError::Timeout {
            phase: WorkerPhase::Hello,
            limit,
        } if limit == Duration::from_millis(20)
    ));
}

#[test]
fn shutdown_timeout_includes_process_exit_after_acknowledgement() {
    let mut timeouts = timeouts();
    timeouts.shutdown = Duration::from_millis(20);
    let session = WorkerSession::connect(
        spawn_fixture("shutdown-timeout"),
        WorkerRole::DatasetMaterializer,
        worker_identity(),
        &[],
        timeouts,
    )
    .expect("handshake must succeed");

    assert!(matches!(
        session.shutdown(),
        Err(WorkerSessionError::Timeout {
            phase: WorkerPhase::Shutdown,
            limit,
        }) if limit == Duration::from_millis(20)
    ));
}

#[test]
fn keeps_later_subprotocol_capabilities_reserved_during_connect() {
    let error = WorkerSession::connect(
        spawn_fixture("reserved"),
        WorkerRole::Implementation,
        worker_identity(),
        &[Capability::StreamingProfile],
        timeouts(),
    )
    .expect_err("the streaming subprotocol is not implemented yet");

    assert!(matches!(
        error,
        WorkerSessionError::Handshake {
            source: HandshakeError::ReservedCapability {
                capability: Capability::StreamingProfile,
            },
        }
    ));
}

#[test]
fn rejects_a_phase_that_does_not_belong_to_the_negotiated_role() {
    let mut session = connect(
        "implementation",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    assert!(matches!(
        session.materialize("/bundle", json!({}), 1, "/output"),
        Err(WorkerSessionError::InvalidRole {
            phase: WorkerPhase::Materialize,
            role: WorkerRole::Implementation,
        })
    ));
    session
        .prepare(
            "/dataset",
            "mw1-problem-contract-abc",
            json!({"lambda": 0.1}),
            json!({"solver": "fast"}),
            23,
        )
        .expect("role error must not consume a request");
    session.execute("/result").expect("execution must succeed");
    session.reset().expect("reset must succeed");
    session.shutdown().expect("shutdown must succeed");
}

fn connect(mode: &str, role: WorkerRole, capabilities: &[Capability]) -> WorkerSession {
    WorkerSession::connect(
        spawn_fixture(mode),
        role,
        worker_identity(),
        capabilities,
        timeouts(),
    )
    .expect("fixture handshake must succeed")
}

fn spawn_fixture(mode: &str) -> PosixWorkerProcess {
    PosixWorkerProcess::spawn(fixture_command(mode), WorkerLogLimits::new(1024, 1024))
        .expect("fixture worker must spawn")
}

fn fixture_command(mode: &str) -> Command {
    let mut command = Command::new(env::current_exe().expect("test executable must have a path"));
    command
        .arg("--exact")
        .arg("fixture_worker")
        .arg("--nocapture")
        .env(FIXTURE_MODE, mode);
    command
}

fn worker_identity() -> WorkerIdentity {
    WorkerIdentity::from_str(WORKER_ID).expect("fixture worker identity must be valid")
}

fn timeouts() -> PhaseTimeouts {
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

fn run_materializer_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(&mut reader, &mut writer, "dataset_materializer", json!([]));
    assert_eq!(
        read_message(&mut reader),
        json!({
            "id": "1", "method": "materialize", "source_dir": "/bundle",
            "dataset_parameters": {"rows": 100}, "dataset_seed": 17,
            "output_dir": "/dataset-output",
        })
    );
    write_message(
        &mut writer,
        &json!({"id": "1", "ok": true, "dataset": {"manifest": "dataset-manifest.json"}}),
    );
    accept_shutdown(&mut reader, &mut writer, "2");
}

fn run_implementation_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(
        &mut reader,
        &mut writer,
        "implementation",
        json!(["one_shot"]),
    );
    assert_eq!(
        read_message(&mut reader),
        json!({
            "id": "1", "method": "prepare", "dataset_dir": "/dataset",
            "problem_contract_id": "mw1-problem-contract-abc",
            "problem_parameters": {"lambda": 0.1},
            "implementation_parameters": {"solver": "fast"},
            "implementation_seed": 23,
        })
    );
    write_message(&mut writer, &json!({"id": "1", "ok": true}));
    assert_eq!(
        read_message(&mut reader),
        json!({"id": "2", "method": "execute", "result_dir": "/result"})
    );
    write_message(
        &mut writer,
        &json!({
            "id": "2", "ok": true, "implementation_time_ns": 18342011,
            "result": {"manifest": "result.json"}, "statistics": {"iterations": 37},
        }),
    );
    assert_eq!(
        read_message(&mut reader),
        json!({"id": "3", "method": "reset"})
    );
    write_message(&mut writer, &json!({"id": "3", "ok": true}));
    accept_shutdown(&mut reader, &mut writer, "4");
}

fn run_evaluator_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(&mut reader, &mut writer, "problem_evaluator", json!([]));
    assert_eq!(
        read_message(&mut reader),
        json!({
            "id": "1", "method": "evaluate", "dataset_dir": "/dataset",
            "problem_contract_id": "mw1-problem-contract-abc",
            "problem_parameters": {"lambda": 0.1}, "result_dir": "/result",
            "metrics_path": "/metrics.json",
        })
    );
    write_message(
        &mut writer,
        &json!({"id": "1", "ok": true, "verdict": "reached"}),
    );
    accept_shutdown(&mut reader, &mut writer, "2");
}

fn run_failure_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(
        &mut reader,
        &mut writer,
        "implementation",
        json!(["one_shot"]),
    );
    let request = read_message(&mut reader);
    assert_eq!(request["method"], "prepare");
    write_message(
        &mut writer,
        &json!({
            "id": "1", "ok": false,
            "error": {
                "code": "operation_failed", "message": "fixture preparation failed",
                "details": {"reason": "fixture"},
            },
        }),
    );
}

fn run_shared_deadline_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(
        &mut reader,
        &mut writer,
        "implementation",
        json!(["one_shot"]),
    );
    let prepare = read_message(&mut reader);
    assert_eq!(prepare["method"], "prepare");
    thread::sleep(Duration::from_millis(300));
    write_message(&mut writer, &json!({"id": prepare["id"], "ok": true}));
    let execute = read_message(&mut reader);
    assert_eq!(execute["method"], "execute");
    thread::sleep(Duration::from_millis(300));
    write_message(
        &mut writer,
        &json!({
            "id": execute["id"], "ok": true, "implementation_time_ns": null,
            "result": {"manifest": "result.json"}, "statistics": {},
        }),
    );
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn record_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

fn run_interrupt_timeout_fixture() {
    // SAFETY: The handler only stores to a lock-free atomic and lives for the
    // process lifetime. This fixture runs in its own child process.
    unsafe {
        libc::signal(
            libc::SIGINT,
            record_interrupt as *const () as libc::sighandler_t,
        );
    }
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(
        &mut reader,
        &mut writer,
        "implementation",
        json!(["one_shot"]),
    );
    assert_eq!(read_message(&mut reader)["method"], "execute");
    while !INTERRUPTED.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(1));
    }
    std::fs::write(
        env::var_os("METEWAND_INTERRUPT_MARKER").unwrap(),
        b"interrupted",
    )
    .unwrap();
    thread::sleep(Duration::from_secs(5));
}

fn run_timeout_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(
        &mut reader,
        &mut writer,
        "implementation",
        json!(["one_shot"]),
    );
    let request = read_message(&mut reader);
    assert_eq!(request["method"], "execute");
    thread::sleep(Duration::from_secs(5));
}

fn run_hello_timeout_fixture() {
    let (_reader, _writer) = protocol_streams();
    thread::sleep(Duration::from_secs(5));
}

fn run_shutdown_timeout_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(&mut reader, &mut writer, "dataset_materializer", json!([]));
    accept_shutdown(&mut reader, &mut writer, "1");
    thread::sleep(Duration::from_secs(5));
}

fn run_reserved_fixture() {
    let (mut reader, mut writer) = protocol_streams();
    accept_hello(
        &mut reader,
        &mut writer,
        "implementation",
        json!(["streaming_profile"]),
    );
}

fn accept_hello(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    role: &str,
    capabilities: Value,
) {
    assert_eq!(
        read_message(reader),
        json!({
            "id": "0", "method": "hello", "protocols": [1], "role": role,
            "worker_id": WORKER_ID,
        })
    );
    write_message(
        writer,
        &json!({
            "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
            "sdk": null, "capabilities": capabilities,
        }),
    );
}

fn accept_shutdown(reader: &mut impl BufRead, writer: &mut impl Write, id: &str) {
    assert_eq!(
        read_message(reader),
        json!({"id": id, "method": "shutdown"})
    );
    write_message(writer, &json!({"id": id, "ok": true}));
}

fn protocol_streams() -> (BufReader<File>, BufWriter<File>) {
    let read_fd = protocol_fd("METEWAND_PROTOCOL_READ_FD");
    let write_fd = protocol_fd("METEWAND_PROTOCOL_WRITE_FD");
    // SAFETY: The runtime transfers ownership of each inherited descriptor to
    // the worker, and this fixture constructs exactly one owner for each.
    let reader = BufReader::new(unsafe { File::from_raw_fd(read_fd) });
    // SAFETY: See the ownership argument above.
    let writer = BufWriter::new(unsafe { File::from_raw_fd(write_fd) });
    (reader, writer)
}

fn read_message(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("protocol request must be readable");
    assert!(!line.is_empty(), "protocol request ended unexpectedly");
    serde_json::from_str(&line).expect("protocol request must be JSON")
}

fn write_message(writer: &mut impl Write, message: &Value) {
    serde_json::to_writer(&mut *writer, message).expect("protocol response must serialize");
    writer
        .write_all(b"\n")
        .expect("protocol response newline must be writable");
    writer.flush().expect("protocol response must flush");
}

fn protocol_fd(name: &str) -> i32 {
    env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set"))
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be a descriptor number"))
}
