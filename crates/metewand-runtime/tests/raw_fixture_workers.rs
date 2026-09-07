#![cfg(unix)]

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Duration,
};

use metewand_core::schema::SchemaCatalog;
use metewand_protocol::{
    Capability, OperationResponseDecodeError, RequestTrackerError, WorkerIdentity, WorkerRole,
    framing::{CorrelationError, FrameError, ResponseCorrelator},
};
use metewand_runtime::{
    worker_output::WorkerOutputValidator,
    worker_process::{PosixWorkerProcess, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerPhase, WorkerSession, WorkerSessionError},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DATASET_SCHEMA: &str = "schemas/dataset.json";
const RESULT_SCHEMA: &str = "schemas/result.json";
const METRICS_SCHEMA: &str = "schemas/metrics.json";
type FrameErrorMatcher = fn(&FrameError) -> bool;

#[test]
fn raw_role_workers_form_a_complete_validated_chain() {
    let source = TempDir::new().unwrap();
    let dataset = TempDir::new().unwrap();
    let result = TempDir::new().unwrap();
    let metrics = TempDir::new().unwrap();
    let schemas = fixture_schema_catalog();
    let validator = WorkerOutputValidator::new(&schemas).unwrap();

    let mut materializer = connect("dataset-materializer", WorkerRole::DatasetMaterializer, &[]);
    assert!(materializer.negotiated().sdk().is_none());
    let materialized = materializer
        .materialize(
            path_string(source.path()),
            json!({"values": [1, 2, 3]}),
            17,
            path_string(dataset.path()),
        )
        .unwrap();
    let validated_dataset = validator
        .validate_materialized_dataset(dataset.path(), &materialized, Path::new(DATASET_SCHEMA))
        .unwrap();
    assert_eq!(
        validated_dataset.manifest_path(),
        Path::new("dataset-manifest.json")
    );
    assert_eq!(validated_dataset.files()[0].path(), Path::new("data.json"));
    materializer.shutdown().unwrap();

    let mut implementation = connect(
        "implementation",
        WorkerRole::Implementation,
        &[Capability::OneShot],
    );
    assert_eq!(
        implementation.negotiated().reported_capabilities(),
        &[Capability::OneShot]
    );
    implementation
        .prepare(
            path_string(dataset.path()),
            "mw1-problem-contract-fixture",
            json!({"offset": 2}),
            json!({"algorithm": "sum"}),
            23,
        )
        .unwrap();
    let executed = implementation.execute(path_string(result.path())).unwrap();
    assert_eq!(executed.implementation_time_ns(), None);
    assert_eq!(executed.statistics(), &json!({"input_count": 3}));
    let validated_result = validator
        .validate_execution_result(result.path(), &executed, Path::new(RESULT_SCHEMA))
        .unwrap();
    assert_eq!(validated_result.data().as_json(), &json!({"answer": 8}));
    implementation.reset().unwrap();
    implementation.shutdown().unwrap();

    let metrics_path = metrics.path().join("metrics.json");
    let mut evaluator = connect("problem-evaluator", WorkerRole::ProblemEvaluator, &[]);
    evaluator
        .evaluate(
            path_string(dataset.path()),
            "mw1-problem-contract-fixture",
            json!({"offset": 2}),
            path_string(result.path()),
            path_string(&metrics_path),
        )
        .unwrap();
    let validated_metrics = validator
        .validate_metrics(
            metrics.path(),
            Path::new("metrics.json"),
            Path::new(METRICS_SCHEMA),
        )
        .unwrap();
    assert_eq!(
        validated_metrics.data().as_json(),
        &json!({"absolute_error": 0})
    );
    evaluator.shutdown().unwrap();
}

#[test]
fn raw_fixture_entrypoints_are_executable() {
    for name in [
        "dataset-materializer",
        "implementation",
        "problem-evaluator",
        "protocol-failure",
    ] {
        let metadata = fs::metadata(fixture_path(name)).unwrap();
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "fixture entrypoint {name} must be executable"
        );
    }
}

#[test]
fn fragmented_fixture_responses_form_valid_frames() {
    let session = connect_fault("fragmented_reads").unwrap();
    session.shutdown().unwrap();
}

#[test]
fn framing_failure_fixtures_produce_every_typed_frame_error() {
    let cases: &[(&str, FrameErrorMatcher)] = &[
        (
            "duplicate_keys",
            |error| matches!(error, FrameError::DuplicateKey { key } if key == "id"),
        ),
        ("invalid_utf8", |error| {
            matches!(error, FrameError::InvalidUtf8 { .. })
        }),
        ("byte_order_mark", |error| {
            matches!(error, FrameError::ByteOrderMark)
        }),
        ("oversized_line", |error| {
            matches!(error, FrameError::LineTooLong { .. })
        }),
        ("malformed_json", |error| {
            matches!(error, FrameError::MalformedJson { .. })
        }),
        ("early_eof", |error| {
            matches!(error, FrameError::EarlyEof { .. })
        }),
    ];

    for (mode, expected) in cases {
        let error = connect_fault(mode).unwrap_err();
        assert!(
            matches!(
                error,
                WorkerSessionError::ProtocolRead {
                    phase: WorkerPhase::Hello,
                    ref source,
                } if expected(source)
            ),
            "unexpected error from {mode}: {error}"
        );
    }
}

#[test]
fn mismatched_id_fixture_produces_a_typed_correlation_failure() {
    let error = connect_fault("mismatched_request_id").unwrap_err();
    assert!(matches!(
        error,
        WorkerSessionError::Response {
            phase: WorkerPhase::Hello,
            source: OperationResponseDecodeError::Request(
                RequestTrackerError::MismatchedRequestId {
                    ref expected,
                    ref actual,
                }
            ),
        } if expected.as_str() == "0" && actual.as_str() == "unexpected"
    ));
}

#[test]
fn extra_response_fixture_produces_a_typed_correlation_failure() {
    let mut process = spawn_fault("extra_response");
    process
        .protocol_writer()
        .write_all(
            format!(
                "{{\"id\":\"0\",\"method\":\"hello\",\"protocols\":[1],\"role\":\"dataset_materializer\",\"worker_id\":\"{WORKER_ID}\"}}\n"
            )
            .as_bytes(),
        )
        .unwrap();
    process.protocol_writer().flush().unwrap();

    let mut correlator = ResponseCorrelator::new("0");
    let first = process.protocol_reader().read_frame().unwrap().unwrap();
    correlator.accept(&first).unwrap();
    let second = process.protocol_reader().read_frame().unwrap().unwrap();
    assert!(matches!(
        correlator.accept(&second),
        Err(CorrelationError::ExtraResponse { ref id }) if id == "0"
    ));

    assert!(process.wait().unwrap().status.success());
}

fn connect(fixture: &str, role: WorkerRole, capabilities: &[Capability]) -> WorkerSession {
    WorkerSession::connect(
        spawn_fixture(fixture),
        role,
        worker_identity(),
        capabilities,
        timeouts(),
    )
    .unwrap()
}

fn connect_fault(mode: &str) -> Result<WorkerSession, WorkerSessionError> {
    WorkerSession::connect(
        spawn_fault(mode),
        WorkerRole::DatasetMaterializer,
        worker_identity(),
        &[],
        timeouts(),
    )
}

fn spawn_fixture(name: &str) -> PosixWorkerProcess {
    PosixWorkerProcess::spawn(
        Command::new(fixture_path(name)),
        WorkerLogLimits::new(4096, 4096),
    )
    .unwrap()
}

fn spawn_fault(mode: &str) -> PosixWorkerProcess {
    let mut command = Command::new(fixture_path("protocol-failure"));
    command.arg(mode);
    PosixWorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap()
}

fn fixture_schema_catalog() -> SchemaCatalog {
    SchemaCatalog::try_new(
        [DATASET_SCHEMA, RESULT_SCHEMA, METRICS_SCHEMA]
            .into_iter()
            .map(|logical_path| {
                let filename = Path::new(logical_path).file_name().unwrap();
                let bytes = fs::read(fixture_path("schemas").join(filename)).unwrap();
                let schema = serde_json::from_slice::<Value>(&bytes).unwrap();
                (PathBuf::from(logical_path), schema)
            }),
    )
    .unwrap()
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/worker-protocol/v1")
        .join(name)
}

fn worker_identity() -> WorkerIdentity {
    WorkerIdentity::from_str(WORKER_ID).unwrap()
}

fn path_string(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
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
