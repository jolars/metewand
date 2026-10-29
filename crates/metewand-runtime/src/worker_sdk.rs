//! Small native one-shot workers over the version-1 process boundary.
//!
//! Callbacks own preparation, solving, scientific conversion, and evaluation.
//! These helpers own transport and standard JSON artifacts. The executor still
//! validates outputs and owns authoritative timing and publication.

#[cfg(unix)]
use std::os::fd::{FromRawFd, OwnedFd};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    time::Instant,
};

use metewand_core::canonical::{CanonicalJsonError, CanonicalValue, MAX_SAFE_INTEGER};
use metewand_protocol::{
    Acknowledgement, Capability, CheckApplicabilityRequest, CheckApplicabilityResponse,
    EvaluateRequest, EvaluateResponse, EvaluationVerdict, ExecuteRequest, ExecuteResponse,
    HelloRequest, HelloResponse, MaterializeRequest, MaterializeResponse, PrepareRequest,
    RequestId, ResetRequest, SdkMetadata, ShutdownRequest, WireMessage, WorkerFailure,
    WorkerFailureCode, WorkerFailureResponse, WorkerRole, decode_message, encode_message,
    framing::FrameReader, negotiate_protocol,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// A callback result that can become a structured worker failure.
pub type WorkerResult<T> = Result<T, WorkerError>;

/// A callback or artifact failure, distinct from a terminal transport failure.
#[derive(Debug, Error)]
#[error("{}", .0.message())]
pub struct WorkerError(WorkerFailure);

impl WorkerError {
    /// Reports an ordinary callback or artifact failure.
    #[must_use]
    pub fn operation_failed(message: impl Into<String>) -> Self {
        let message = message.into();
        failure(
            WorkerFailureCode::OperationFailed,
            if message.is_empty() {
                "Worker operation failed.".to_owned()
            } else {
                message
            },
        )
    }
}

impl From<WorkerFailure> for WorkerError {
    fn from(value: WorkerFailure) -> Self {
        Self(value)
    }
}

impl From<io::Error> for WorkerError {
    fn from(value: io::Error) -> Self {
        Self::operation_failed(value.to_string())
    }
}

impl From<CanonicalJsonError> for WorkerError {
    fn from(value: CanonicalJsonError) -> Self {
        Self::operation_failed(value.to_string())
    }
}

impl From<serde_json::Error> for WorkerError {
    fn from(value: serde_json::Error) -> Self {
        Self::operation_failed(value.to_string())
    }
}

/// A terminal transport, framing, or uncorrelatable-envelope failure.
#[derive(Debug, Error)]
pub enum ServeError {
    /// The dedicated pipes could not be opened, read, or written.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A frame violates the existing version-1 framing rules.
    #[error(transparent)]
    Frame(#[from] metewand_protocol::framing::FrameError),
    /// A frame contains numbers outside the canonical JSON domain.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// No trustworthy request ID or bounded response is available.
    #[error("{0}")]
    Protocol(&'static str),
}

/// An implementation whose algorithm and state belong entirely to the adapter.
pub trait Implementation {
    /// Solver output converted and serialized after diagnostic solve timing.
    type Output: Serialize;

    /// Declares whether the optional support callback is implemented.
    fn supports_applicability(&self) -> bool {
        false
    }

    /// Checks support without preparing or executing the solver.
    ///
    /// # Errors
    /// Returns a failure when support cannot be determined.
    fn check_applicability(
        &mut self,
        _request: &CheckApplicabilityRequest,
    ) -> WorkerResult<CheckApplicabilityResponse> {
        Err(failure(
            WorkerFailureCode::UnsupportedOperation,
            "This implementation does not support applicability checks.",
        ))
    }

    /// Establishes clean state, consuming the supplied parameters and seed.
    ///
    /// # Errors
    /// Return a failure when preparation cannot complete.
    fn prepare(&mut self, request: &PrepareRequest) -> WorkerResult<()>;

    /// Runs the solver. Only this callback contributes to diagnostic implementation time.
    ///
    /// # Errors
    /// Return a failure when solving cannot complete.
    fn execute(&mut self) -> WorkerResult<Self::Output> {
        Err(failure(
            WorkerFailureCode::UnsupportedOperation,
            "One-shot execution is not implemented.",
        ))
    }

    fn supports_one_shot(&self) -> bool {
        true
    }
    fn supports_buffered_profile(&self) -> bool {
        false
    }
    fn supports_streaming_profile(&self) -> bool {
        false
    }
    fn execute_stream(
        &mut self,
        _stream: &mut crate::streaming::StreamingContext<'_>,
    ) -> WorkerResult<()> {
        Err(failure(
            WorkerFailureCode::UnsupportedOperation,
            "Streaming execution is not implemented.",
        ))
    }
    fn execute_buffered(
        &mut self,
        _trace: &mut crate::buffered::BufferedTrace,
    ) -> WorkerResult<()> {
        Err(failure(
            WorkerFailureCode::UnsupportedOperation,
            "Buffered execution is not implemented.",
        ))
    }

    /// Converts solver output to scientific JSON data outside diagnostic solve timing.
    ///
    /// # Errors
    /// Return a failure when conversion cannot complete.
    fn result(&mut self, output: Self::Output) -> WorkerResult<Value> {
        Ok(serde_json::to_value(output)?)
    }

    /// Supplies optional diagnostic statistics outside diagnostic solve timing.
    ///
    /// # Errors
    /// Return a failure when statistics cannot be produced.
    fn statistics(&mut self, _output: &Self::Output) -> WorkerResult<Value> {
        Ok(json!({}))
    }

    /// Writes a standard result, or supplies a benchmark-owned artifact writer.
    ///
    /// # Errors
    /// Return a failure unless all files are closed and the relative manifest exists.
    fn write_result(&mut self, output: Self::Output, directory: &Path) -> WorkerResult<String> {
        write_result(directory, self.result(output)?)
    }

    /// Releases slot-scoped state before process reuse. Unsupported by default.
    ///
    /// # Errors
    /// Return a failure unless the next preparation can safely establish clean state.
    fn reset(&mut self) -> WorkerResult<()> {
        Err(failure(
            WorkerFailureCode::UnsupportedOperation,
            "This implementation does not support reset.",
        ))
    }

    /// Releases resources before the worker exits.
    ///
    /// # Errors
    /// Return a failure if cleanup fails.
    fn shutdown(&mut self) -> WorkerResult<()> {
        Ok(())
    }
}

/// Metric data paired with an explicit evaluator-owned scientific verdict.
#[derive(Debug)]
pub struct Evaluation {
    /// Scientific metrics, validated as a canonical object before writing.
    pub metrics: Value,
    /// Scientific validity and completion are never inferred by the helper.
    pub verdict: EvaluationVerdict,
}

impl Evaluation {
    /// Pairs metric data with the evaluator's explicit verdict.
    #[must_use]
    pub const fn new(metrics: Value, verdict: EvaluationVerdict) -> Self {
        Self { metrics, verdict }
    }
}

/// An independent evaluator with an optional custom metrics writer.
pub trait Evaluator {
    /// Computes metrics and a verdict using the supplied scientific rules.
    ///
    /// # Errors
    /// Return a failure when evaluation cannot complete.
    fn evaluate(&mut self, request: &EvaluateRequest) -> WorkerResult<Evaluation>;

    /// Writes metrics to the exact assigned path before the response is sent.
    ///
    /// # Errors
    /// Return a failure if a complete metrics document cannot be written.
    fn write_metrics(&mut self, evaluation: &Evaluation, path: &Path) -> WorkerResult<()> {
        write_metrics(path, evaluation.metrics.clone())
    }
}

impl<F: FnMut(&EvaluateRequest) -> WorkerResult<Evaluation>> Evaluator for F {
    fn evaluate(&mut self, request: &EvaluateRequest) -> WorkerResult<Evaluation> {
        self(request)
    }
}

/// Serves an implementation on duplicates of the inherited protocol descriptors.
///
/// # Errors
/// Returns a terminal transport error. Callback failures become wire responses.
pub fn serve_implementation(worker: &mut impl Implementation) -> Result<(), ServeError> {
    let (reader, writer) = inherited_pipes()?;
    serve_implementation_on(worker, reader, writer)
}

/// Serves an evaluator, negotiating `problem_semantics`, on inherited pipes.
///
/// # Errors
/// Returns a terminal transport error. Callback failures become wire responses.
pub fn serve_evaluator(worker: &mut impl Evaluator) -> Result<(), ServeError> {
    let (reader, writer) = inherited_pipes()?;
    serve_evaluator_on(worker, reader, writer)
}

/// Serves a dataset callback on inherited pipes. The callback returns a completed
/// manifest path relative to the assigned output directory.
///
/// # Errors
/// Returns a terminal transport error. Callback failures become wire responses.
pub fn serve_dataset_materializer(
    worker: &mut impl FnMut(&MaterializeRequest) -> WorkerResult<String>,
) -> Result<(), ServeError> {
    let (reader, writer) = inherited_pipes()?;
    serve_dataset_materializer_on(worker, reader, writer)
}

/// Serves a dataset callback on explicit streams.
///
/// # Errors
/// Returns a terminal transport error. Invalid requests and incomplete manifests
/// produce structured failures without terminating the session.
pub fn serve_dataset_materializer_on(
    worker: &mut impl FnMut(&MaterializeRequest) -> WorkerResult<String>,
    reader: impl Read,
    writer: impl Write,
) -> Result<(), ServeError> {
    serve_on(
        WorkerRole::DatasetMaterializer,
        [],
        reader,
        writer,
        |request, _, _| match request["method"].as_str().unwrap_or_default() {
            "materialize" => {
                let request: MaterializeRequest = decode(request)?;
                let manifest = worker(&request)?;
                completed_manifest(Path::new(request.output_dir()), &manifest)?;
                response(&MaterializeResponse::new(request.id().clone(), manifest))
            }
            "shutdown" => {
                let request: ShutdownRequest = decode(request)?;
                response(&Acknowledgement::new(request.id().clone()))
            }
            _ => Err(failure(
                WorkerFailureCode::UnsupportedOperation,
                "This materializer does not accept the requested method.",
            )),
        },
    )
}

/// Serves an implementation on explicit streams without taking their descriptors.
///
/// # Errors
/// Returns a terminal transport error. Successful execution consumes preparation,
/// including when a callback or artifact operation subsequently fails.
pub fn serve_implementation_on(
    worker: &mut impl Implementation,
    reader: impl Read,
    writer: impl Write,
) -> Result<(), ServeError> {
    let epoch = Instant::now();
    implementation_on(worker, reader, writer, || epoch.elapsed().as_nanos())
}

fn implementation_on(
    worker: &mut impl Implementation,
    reader: impl Read,
    writer: impl Write,
    mut clock: impl FnMut() -> u128,
) -> Result<(), ServeError> {
    let mut prepared = false;
    let mut capabilities = Vec::new();
    if worker.supports_one_shot() {
        capabilities.push(Capability::OneShot);
    }
    if worker.supports_buffered_profile() {
        capabilities.push(Capability::BufferedProfile);
    }
    if worker.supports_streaming_profile() {
        capabilities.push(Capability::StreamingProfile);
    }
    if worker.supports_applicability() {
        capabilities.push(Capability::Applicability);
    }
    serve_on(
        WorkerRole::Implementation,
        capabilities,
        reader,
        writer,
        |request, reader, writer| match request["method"].as_str().unwrap_or_default() {
            "execute_stream" => {
                let request: metewand_protocol::ExecuteStreamRequest = decode(request)?;
                if !prepared || !worker.supports_streaming_profile() {
                    return Err(failure(
                        WorkerFailureCode::InvalidRequest,
                        "Streaming execution requires preparation and streaming_profile support.",
                    ));
                }
                prepared = false;
                let profile = serde_json::from_value(request.profile)
                    .map_err(|e| WorkerError::operation_failed(e.to_string()))?;
                let mut exchange = |checkpoint: &metewand_protocol::StreamCheckpoint| {
                    writer.write_all(&response(checkpoint)?)?;
                    writer.flush()?;
                    let frame = reader
                        .read_frame()
                        .map_err(|e| WorkerError::operation_failed(e.to_string()))?
                        .ok_or_else(|| {
                            WorkerError::operation_failed("stream decision pipe closed")
                        })?;
                    decode::<metewand_protocol::StreamDecision>(frame)
                };
                let mut stream = crate::streaming::StreamingContext::new(
                    profile,
                    request.result_root.into(),
                    request.id.clone(),
                    &mut exchange,
                )?;
                worker.execute_stream(&mut stream)?;
                response(&metewand_protocol::ExecuteStreamResponse::new(
                    request.id,
                    stream.count(),
                ))
            }
            "check_applicability" => {
                let request: CheckApplicabilityRequest = decode(request)?;
                response(&worker.check_applicability(&request)?)
            }
            "prepare" => {
                let request: PrepareRequest = decode(request)?;
                prepared = false;
                worker.prepare(&request)?;
                prepared = true;
                response(&Acknowledgement::new(request.id().clone()))
            }
            "execute" => {
                let request: ExecuteRequest = decode(request)?;
                if request.observation_control().is_some() {
                    return Err(failure(
                        WorkerFailureCode::UnsupportedOperation,
                        "This worker does not implement fresh_sequence.",
                    ));
                }
                if !prepared {
                    return Err(failure(
                        WorkerFailureCode::InvalidRequest,
                        "Execute requires a successful prepare.",
                    ));
                }
                prepared = false;
                let start = clock();
                let output = worker.execute()?;
                let elapsed = clock()
                    .checked_sub(start)
                    .and_then(|elapsed| u64::try_from(elapsed).ok())
                    .filter(|elapsed| *elapsed <= MAX_SAFE_INTEGER);
                let statistics = object(worker.statistics(&output)?, "Implementation statistics")?;
                let directory = Path::new(request.result_dir());
                let manifest = worker.write_result(output, directory)?;
                completed_manifest(directory, &manifest)?;
                response(&ExecuteResponse::new(
                    request.id().clone(),
                    elapsed,
                    manifest,
                    statistics.as_object().expect("validated object").clone(),
                ))
            }
            "execute_buffered" => {
                let request: metewand_protocol::ExecuteBufferedRequest = decode(request)?;
                if !prepared || !worker.supports_buffered_profile() {
                    return Err(failure(
                        WorkerFailureCode::InvalidRequest,
                        "Buffered execution requires preparation and buffered_profile support.",
                    ));
                }
                prepared = false;
                let profile = serde_json::from_value(request.profile().clone())
                    .map_err(|e| WorkerError::operation_failed(e.to_string()))?;
                let mut trace = crate::buffered::BufferedTrace::new(
                    profile,
                    PathBuf::from(request.result_root()),
                )?;
                worker.execute_buffered(&mut trace)?;
                let manifest = trace.write()?;
                response(&metewand_protocol::ExecuteBufferedResponse::new(
                    request.id().clone(),
                    manifest,
                ))
            }
            "reset" => {
                let request: ResetRequest = decode(request)?;
                prepared = false;
                worker.reset()?;
                response(&Acknowledgement::new(request.id().clone()))
            }
            "shutdown" => {
                let request: ShutdownRequest = decode(request)?;
                worker.shutdown()?;
                response(&Acknowledgement::new(request.id().clone()))
            }
            _ => Err(failure(
                WorkerFailureCode::UnsupportedOperation,
                "This implementation does not accept the requested method.",
            )),
        },
    )
}

/// Serves an evaluator on explicit streams, requiring negotiated scientific rules.
///
/// # Errors
/// Returns a terminal transport error. Callback failures become wire responses.
pub fn serve_evaluator_on(
    worker: &mut impl Evaluator,
    reader: impl Read,
    writer: impl Write,
) -> Result<(), ServeError> {
    serve_on(
        WorkerRole::ProblemEvaluator,
        [Capability::ProblemSemantics],
        reader,
        writer,
        |request, _, _| match request["method"].as_str().unwrap_or_default() {
            "evaluate" => {
                let request: EvaluateRequest = decode(request)?;
                if request.problem_semantics().is_none() {
                    return Err(failure(
                        WorkerFailureCode::InvalidRequest,
                        "Expected problem_semantics object.",
                    ));
                }
                let evaluation = worker.evaluate(&request)?;
                let path = Path::new(request.metrics_path());
                worker.write_metrics(&evaluation, path)?;
                if !path.is_file() {
                    return Err(WorkerError::operation_failed(
                        "The evaluator returned before writing metrics.",
                    ));
                }
                response(&EvaluateResponse::new(
                    request.id().clone(),
                    evaluation.verdict,
                ))
            }
            "shutdown" => {
                let request: ShutdownRequest = decode(request)?;
                response(&Acknowledgement::new(request.id().clone()))
            }
            _ => Err(failure(
                WorkerFailureCode::UnsupportedOperation,
                "This evaluator does not accept the requested method.",
            )),
        },
    )
}

fn serve_on<R: Read, W: Write>(
    role: WorkerRole,
    capabilities: impl AsRef<[Capability]>,
    reader: R,
    mut writer: W,
    mut dispatch: impl FnMut(Value, &mut FrameReader<R>, &mut W) -> WorkerResult<Vec<u8>>,
) -> Result<(), ServeError> {
    let mut reader = FrameReader::new(reader);
    let mut ready = false;
    loop {
        let request = reader.read_frame()?.ok_or(ServeError::Protocol(
            "Protocol stream ended before shutdown.",
        ))?;
        let request = CanonicalValue::try_from(request)?.as_json().clone();
        let id: RequestId =
            serde_json::from_value(request.get("id").cloned().unwrap_or(Value::Null))
                .map_err(|_| ServeError::Protocol("A request must have a nonempty string ID."))?;
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let result = if !ready {
            hello(request, role, capabilities.as_ref()).inspect(|_| {
                ready = true;
            })
        } else if method.is_empty() || method == "hello" {
            Err(failure(
                WorkerFailureCode::InvalidRequest,
                "Expected an operation after negotiation.",
            ))
        } else {
            catch_unwind(AssertUnwindSafe(|| {
                dispatch(request, &mut reader, &mut writer)
            }))
            .unwrap_or_else(|payload| {
                let text = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("Non-string panic payload.");
                // Wire diagnostics survive failures even when stderr is unavailable.
                let mut bounded: String = text.chars().take(8192).collect();
                if bounded.len() < text.len() {
                    bounded.push_str(" [panic truncated]");
                }
                Err(WorkerError::operation_failed(format!(
                    "Worker callback panicked: {bounded}"
                )))
            })
        };
        let success = result.is_ok();
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => encode_failure(id, error)?,
        };
        writer.write_all(&frame)?;
        writer.flush()?;
        if !ready || (method == "shutdown" && success) {
            return Ok(());
        }
    }
}

fn hello(request: Value, role: WorkerRole, capabilities: &[Capability]) -> WorkerResult<Vec<u8>> {
    let request: HelloRequest = decode(request)?;
    if request.role() != role {
        return Err(failure(
            WorkerFailureCode::InvalidRequest,
            "The handshake selected the wrong worker role.",
        ));
    }
    negotiate_protocol(request.protocols())
        .map_err(|error| failure(WorkerFailureCode::UnsupportedOperation, error.to_string()))?;
    response(&HelloResponse::new(
        request.id().clone(),
        request.worker_id().clone(),
        Some(
            SdkMetadata::new("metewand-rust", env!("CARGO_PKG_VERSION"))
                .expect("nonempty package metadata"),
        ),
        capabilities.iter().copied(),
    ))
}

fn decode<M: WireMessage>(request: Value) -> WorkerResult<M> {
    decode_message(request)
        .map_err(|error| failure(WorkerFailureCode::InvalidRequest, error.to_string()))
}

fn response(message: &impl WireMessage) -> WorkerResult<Vec<u8>> {
    encode_message(message).map_err(|error| WorkerError::operation_failed(error.to_string()))
}

fn failure(code: WorkerFailureCode, message: impl Into<String>) -> WorkerError {
    WorkerFailure::new(code, message, None)
        .expect("nonempty worker failure message")
        .into()
}

fn encode_failure(id: RequestId, error: WorkerError) -> Result<Vec<u8>, ServeError> {
    let error =
        if CanonicalValue::try_from(serde_json::to_value(&error.0).expect("serializable failure"))
            .is_ok()
        {
            error.0
        } else {
            failure(
                WorkerFailureCode::InternalError,
                "Worker failure contains invalid JSON data.",
            )
            .0
        };
    encode_message(&WorkerFailureResponse::new(id.clone(), error))
        .or_else(|_| {
            encode_message(&WorkerFailureResponse::new(
                id,
                failure(
                    WorkerFailureCode::InternalError,
                    "Worker failure could not be encoded within the protocol limit.",
                )
                .0,
            ))
        })
        .map_err(|_| {
            ServeError::Protocol("Request ID leaves no room for a bounded failure response.")
        })
}

#[cfg(unix)]
fn inherited_pipes() -> Result<(File, File), ServeError> {
    let descriptor = |name| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|value| *value >= 0)
            .ok_or(ServeError::Protocol(
                "Separate protocol read and write descriptors must be supplied.",
            ))
    };
    let read = descriptor("METEWAND_PROTOCOL_READ_FD")?;
    let write = descriptor("METEWAND_PROTOCOL_WRITE_FD")?;
    if read == write {
        return Err(ServeError::Protocol(
            "The protocol requires separate descriptors.",
        ));
    }
    fn duplicate(descriptor: i32) -> io::Result<File> {
        // fcntl checks arbitrary descriptor numbers without borrowing ownership.
        let duplicated = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicated < 0 {
            return Err(io::Error::last_os_error());
        }
        // The successful duplication creates a new, exclusively owned descriptor.
        Ok(File::from(unsafe { OwnedFd::from_raw_fd(duplicated) }))
    }
    Ok((duplicate(read)?, duplicate(write)?))
}

#[cfg(windows)]
fn inherited_pipes() -> Result<(File, File), ServeError> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{
            DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HANDLE_FLAG_INHERIT,
            INVALID_HANDLE_VALUE, SetHandleInformation,
        },
        System::Threading::GetCurrentProcess,
    };
    let handle = |name| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value != 0 && (*value as HANDLE) != INVALID_HANDLE_VALUE)
            .map(|value| value as HANDLE)
            .ok_or(ServeError::Protocol(
                "Separate protocol read and write handles must be supplied.",
            ))
    };
    let read = handle(crate::worker_process::PROTOCOL_READ_HANDLE_ENV)?;
    let write = handle(crate::worker_process::PROTOCOL_WRITE_HANDLE_ENV)?;
    if read == write {
        return Err(ServeError::Protocol(
            "The protocol requires separate handles.",
        ));
    }
    fn duplicate(handle: HANDLE) -> io::Result<File> {
        let mut duplicated = std::ptr::null_mut();
        // The launch handle stays borrowed. Both it and the owned duplicate must
        // stop propagating into benchmark-created subprocesses.
        unsafe {
            if SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            let process = GetCurrentProcess();
            if DuplicateHandle(
                process,
                handle,
                process,
                &mut duplicated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(File::from(OwnedHandle::from_raw_handle(duplicated)))
        }
    }
    Ok((duplicate(read)?, duplicate(write)?))
}

fn object(data: Value, name: &str) -> WorkerResult<Value> {
    if !data.is_object() {
        return Err(WorkerError::operation_failed(format!(
            "{name} must be a JSON object."
        )));
    }
    Ok(CanonicalValue::try_from(data)?.as_json().clone())
}

fn completed_manifest(directory: &Path, manifest: &str) -> WorkerResult<()> {
    let target = output_path(directory, manifest)?;
    if !target.is_file() {
        return Err(WorkerError::operation_failed(
            "The callback returned before writing its manifest.",
        ));
    }
    Ok(())
}

fn output_path(directory: &Path, manifest: &str) -> WorkerResult<PathBuf> {
    if manifest.is_empty()
        || manifest.starts_with('/')
        || manifest.contains(['\\', '\0'])
        || manifest
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(WorkerError::operation_failed(
            "Manifest paths must be normalized and relative.",
        ));
    }
    crate::platform_fs::relative_path(Path::new(manifest))?;
    let mut target = directory.canonicalize()?;
    for component in manifest.split('/') {
        target.push(component);
        match fs::symlink_metadata(&target) {
            Ok(metadata)
                if !crate::platform_fs::is_real_directory(&metadata)
                    && !crate::platform_fs::is_regular_file(&metadata) =>
            {
                return Err(WorkerError::operation_failed(
                    "Output paths must not traverse links or special files.",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(target)
}

/// Reads canonical-domain JSON without accepting duplicate object keys.
///
/// # Errors
/// Returns an I/O or canonical JSON error.
pub fn read_json(path: &Path) -> WorkerResult<Value> {
    Ok(CanonicalValue::from_slice(&fs::read(path)?)?
        .as_json()
        .clone())
}

/// Writes canonical JSON and closes the file before returning.
///
/// # Errors
/// Returns an error before writing for invalid JSON data, or on I/O failure.
pub fn write_json(path: &Path, data: Value) -> WorkerResult<()> {
    let bytes = CanonicalValue::try_from(data)?.to_canonical_bytes()?;
    fs::write(path, bytes)?;
    Ok(())
}

/// Hashes completed files and writes a standard dataset inventory last.
///
/// Each pair supplies a relative path and an author-selected media type. Use a
/// custom writer when the benchmark needs additional representation metadata.
///
/// # Errors
/// Returns an error for duplicate or unsafe paths, nonregular files, empty media
/// types, invalid canonical JSON, or I/O failures.
pub fn write_dataset_manifest(directory: &Path, files: &[(&str, &str)]) -> WorkerResult<String> {
    let mut entries = BTreeMap::new();
    #[cfg(windows)]
    let mut native_names = std::collections::BTreeSet::from(["dataset-manifest.json".to_owned()]);
    for &(path, media_type) in files {
        #[cfg(windows)]
        if !native_names.insert(path.to_lowercase()) {
            return Err(WorkerError::operation_failed(
                "Dataset file names must be distinct on Windows.",
            ));
        }
        if path == "dataset-manifest.json" || entries.contains_key(path) || media_type.is_empty() {
            return Err(WorkerError::operation_failed(
                "Dataset files must be unique, exclude the manifest, and have media types.",
            ));
        }
        let target = output_path(directory, path)?;
        let mut file =
            crate::platform_fs::open_regular(&target, std::fs::OpenOptions::new().read(true))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(WorkerError::operation_failed(
                "Dataset entries must be regular files.",
            ));
        }
        let mut digest = Sha256::new();
        let mut buffer = [0; 8192];
        loop {
            let length = file.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
        }
        entries.insert(
            path,
            json!({
                "path": path, "media_type": media_type,
                "sha256": digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                "size_bytes": metadata.len(),
            }),
        );
    }
    write_json(
        &output_path(directory, "dataset-manifest.json")?,
        json!({"schema_version": 1, "complete": true, "files": entries.into_values().collect::<Vec<_>>()}),
    )?;
    Ok("dataset-manifest.json".to_owned())
}

/// Writes the standard result envelope, using the selected contract's schema.
///
/// # Errors
/// Returns an error for non-object data, unsafe paths, invalid JSON, or failed I/O.
pub fn write_result(directory: &Path, data: Value) -> WorkerResult<String> {
    let data = object(data, "Result data")?;
    write_json(
        &output_path(directory, "result.json")?,
        json!({"schema_version":1,"data":data,"files":[]}),
    )?;
    Ok("result.json".to_owned())
}

/// Writes the standard metrics envelope at the exact assigned path.
///
/// # Errors
/// Returns an error for non-object data, invalid JSON, or failed I/O.
pub fn write_metrics(path: &Path, data: Value) -> WorkerResult<()> {
    let data = object(data, "Metrics data")?;
    write_json(path, json!({"schema_version":1,"data":data}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn conversion_statistics_and_artifacts_follow_diagnostic_timing() {
        struct Solver<'a>(&'a RefCell<Vec<&'static str>>);
        impl Implementation for Solver<'_> {
            type Output = i64;
            fn prepare(&mut self, _: &PrepareRequest) -> WorkerResult<()> {
                self.0.borrow_mut().push("prepare");
                Ok(())
            }
            fn execute(&mut self) -> WorkerResult<i64> {
                self.0.borrow_mut().push("execute");
                Ok(8)
            }
            fn statistics(&mut self, _: &i64) -> WorkerResult<Value> {
                self.0.borrow_mut().push("statistics");
                Ok(json!({}))
            }
            fn result(&mut self, output: i64) -> WorkerResult<Value> {
                self.0.borrow_mut().push("convert");
                Ok(json!({"answer": output}))
            }
        }
        let directory = tempfile::TempDir::new().unwrap();
        let events = RefCell::new(Vec::new());
        let requests = [
            json!({"id":"0","method":"hello","role":"implementation","protocols":[1],
                "worker_id":format!("mw1-resolved-worker-{}", "a".repeat(64))}),
            json!({"id":"1","method":"prepare","dataset_dir":"d","problem_contract_id":"c",
                "problem_parameters":{},"implementation_parameters":{},"implementation_seed":23}),
            json!({"id":"2","method":"execute","result_dir":directory.path()}),
            json!({"id":"3","method":"shutdown"}),
        ]
        .into_iter()
        .flat_map(|request| {
            let mut bytes = serde_json::to_vec(&request).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect::<Vec<_>>();
        let mut output = Vec::new();
        let mut ticks = [100, 125].into_iter();
        implementation_on(&mut Solver(&events), &requests[..], &mut output, || {
            events.borrow_mut().push("clock");
            ticks.next().unwrap()
        })
        .unwrap();
        assert_eq!(
            *events.borrow(),
            [
                "prepare",
                "clock",
                "execute",
                "clock",
                "statistics",
                "convert"
            ]
        );
        let mut reader = FrameReader::new(&output[..]);
        reader.read_frame().unwrap();
        reader.read_frame().unwrap();
        assert_eq!(
            reader.read_frame().unwrap().unwrap()["implementation_time_ns"],
            25
        );
        assert_eq!(
            read_json(&directory.path().join("result.json")).unwrap()["data"],
            json!({"answer":8})
        );
    }
}
