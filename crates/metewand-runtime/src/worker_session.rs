//! Typed one-shot worker exchanges with phase-specific deadlines.

use std::{
    io::{self, Write},
    process::ExitStatus,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use metewand_protocol::{
    Acknowledgement, Capability, CheckApplicabilityRequest, CheckApplicabilityResponse,
    EvaluateRequest, EvaluateResponse, ExecuteRequest, ExecuteResponse, HandshakeError,
    HelloRequest, HelloResponse, MaterializeRequest, MaterializeResponse, MessageEncodeError,
    NegotiatedSession, OperationResponse, OperationResponseDecodeError, PrepareRequest, RequestId,
    RequestTracker, RequestTrackerError, ResetRequest, ShutdownRequest, WireMessage, WireResponse,
    WorkerFailure, WorkerIdentity, WorkerRole, decode_operation_response, encode_message,
    framing::{FrameError, FrameReader},
    validate_hello_response,
};
use serde_json::Value;
use thiserror::Error;

use crate::{
    executor::{Executor, LocalProcessExecutor},
    redaction::Redactor,
    worker_environment::WorkerEnvironmentProvenance,
    worker_process::{PosixWorkerProcess, WorkerPipe, WorkerProcessError, WorkerProcessOutput},
};

/// Fully resolved operational deadline for every Gate-1 worker phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseTimeouts {
    /// Version negotiation and identity verification.
    pub hello: Duration,
    /// Dataset materialization.
    pub materialize: Duration,
    /// Implementation preparation.
    pub prepare: Duration,
    /// One-shot implementation execution and result serialization.
    pub execute: Duration,
    /// Reused-worker state reset.
    pub reset: Duration,
    /// Independent result evaluation.
    pub evaluate: Duration,
    /// Shutdown response and clean process exit.
    pub shutdown: Duration,
}

impl PhaseTimeouts {
    pub(crate) fn provenance(self) -> Value {
        serde_json::json!({
            "hello": self.hello.as_nanos().to_string(),
            "materialize": self.materialize.as_nanos().to_string(),
            "prepare": self.prepare.as_nanos().to_string(),
            "execute": self.execute.as_nanos().to_string(),
            "reset": self.reset.as_nanos().to_string(),
            "evaluate": self.evaluate.as_nanos().to_string(),
            "shutdown": self.shutdown.as_nanos().to_string(),
        })
    }
}

impl Default for PhaseTimeouts {
    fn default() -> Self {
        let limit = Duration::from_secs(10);
        Self {
            hello: limit,
            materialize: limit,
            prepare: limit,
            execute: limit,
            reset: limit,
            evaluate: limit,
            shutdown: limit,
        }
    }
}

/// A version-1 worker phase with an independent operational deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerPhase {
    /// Session handshake.
    Hello,
    /// Dataset materialization.
    Materialize,
    /// Untimed implementation support check.
    CheckApplicability,
    /// Implementation preparation.
    Prepare,
    /// One-shot implementation execution.
    Execute,
    /// Implementation reset.
    Reset,
    /// Independent evaluation.
    Evaluate,
    /// Session shutdown.
    Shutdown,
}

impl std::fmt::Display for WorkerPhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Hello => "hello",
            Self::Materialize => "materialize",
            Self::CheckApplicability => "check_applicability",
            Self::Prepare => "prepare",
            Self::Execute => "execute",
            Self::Reset => "reset",
            Self::Evaluate => "evaluate",
            Self::Shutdown => "shutdown",
        })
    }
}

/// One negotiated worker process that executes serial typed requests.
#[derive(Debug)]
pub struct WorkerSession {
    redactor: Redactor,
    role: WorkerRole,
    negotiated: Option<NegotiatedSession>,
    requests: RequestTracker,
    timeouts: PhaseTimeouts,
    scientific_deadline: Option<Instant>,
    process: Option<PosixWorkerProcess>,
    protocol_sender: Option<Sender<ProtocolCommand>>,
    protocol_thread: Option<JoinHandle<()>>,
    usable: bool,
}

impl WorkerSession {
    /// Starts protocol I/O and establishes a negotiated worker session.
    ///
    /// Selected fresh-sequence and streaming-profile
    /// capabilities remain reserved until their subprotocols are implemented.
    ///
    /// # Errors
    ///
    /// Returns a typed phase, transport, protocol, timeout, or handshake error.
    pub fn connect(
        process: PosixWorkerProcess,
        role: WorkerRole,
        worker_id: WorkerIdentity,
        selected_capabilities: &[Capability],
        timeouts: PhaseTimeouts,
    ) -> Result<Self, WorkerSessionError> {
        Self::connect_with_deadline(
            process,
            role,
            worker_id,
            selected_capabilities,
            timeouts,
            None,
        )
    }

    /// Connects a worker whose launch belongs to an already-started timed scope.
    ///
    /// # Errors
    ///
    /// Returns the same failures as `connect`, including scientific deadline expiry.
    pub fn connect_with_deadline(
        mut process: PosixWorkerProcess,
        role: WorkerRole,
        worker_id: WorkerIdentity,
        selected_capabilities: &[Capability],
        timeouts: PhaseTimeouts,
        scientific_deadline: Option<Instant>,
    ) -> Result<Self, WorkerSessionError> {
        let (reader, writer) = process.take_protocol_io();
        let (protocol_sender, protocol_thread) = match start_protocol_io(reader, writer) {
            Ok(protocol) => protocol,
            Err(source) => {
                let _ = process.kill();
                let _ = process.wait();
                return Err(WorkerSessionError::StartProtocolThread { source });
            }
        };
        let mut session = Self {
            redactor: process.redactor(),
            role,
            negotiated: None,
            requests: RequestTracker::new(),
            timeouts,
            scientific_deadline,
            process: Some(process),
            protocol_sender: Some(protocol_sender),
            protocol_thread: Some(protocol_thread),
            usable: true,
        };

        let id = session.begin_request(WorkerPhase::Hello)?;
        let request = HelloRequest::new(id, role, worker_id);
        let timeout = session.timeouts.hello;
        let response =
            session.exchange::<_, HelloResponse>(WorkerPhase::Hello, &request, timeout)?;
        let mut selected_capabilities = selected_capabilities.to_vec();
        if role == WorkerRole::ProblemEvaluator
            && response
                .capabilities()
                .contains(&Capability::ProblemSemantics)
        {
            selected_capabilities.push(Capability::ProblemSemantics);
        }
        let negotiated = match validate_hello_response(&request, &response, &selected_capabilities)
        {
            Ok(negotiated) => negotiated,
            Err(source) => {
                session.discard();
                return Err(WorkerSessionError::Handshake { source });
            }
        };
        session.negotiated = Some(negotiated);
        Ok(session)
    }

    /// Sets the absolute deadline shared by exchanges inside a scientific scope.
    ///
    /// The orchestrator clears this deadline before untimed phases. Reusing an
    /// absolute deadline prevents each exchange from restarting the budget.
    pub fn set_scientific_deadline(&mut self, deadline: Option<Instant>) {
        self.scientific_deadline = deadline;
    }

    /// Returns the verified handshake metadata.
    #[must_use]
    pub fn negotiated(&self) -> &NegotiatedSession {
        self.negotiated
            .as_ref()
            .expect("a public worker session has completed its handshake")
    }

    /// Returns realized worker settings while the session owns its process.
    #[must_use]
    pub fn environment(&self) -> Option<&WorkerEnvironmentProvenance> {
        self.process
            .as_ref()
            .and_then(PosixWorkerProcess::environment)
    }

    /// Captures the worker's bounded process-lifetime log prefixes.
    #[must_use]
    pub fn log_snapshot(&self) -> crate::worker_process::WorkerLogSnapshot {
        self.process
            .as_ref()
            .expect("a usable session owns its worker process")
            .log_snapshot()
    }

    pub(crate) fn is_usable(&self) -> bool {
        self.usable
    }

    pub(crate) fn private_working_directory(&self) -> Option<&std::path::Path> {
        self.process
            .as_ref()
            .and_then(PosixWorkerProcess::private_working_directory)
    }

    pub(crate) fn process_usage(&self) -> crate::resource_accounting::ProcessUsage {
        self.process.as_ref().map_or_else(
            crate::resource_accounting::ProcessUsage::default,
            |process| crate::resource_accounting::ProcessUsage::observe(process.id()),
        )
    }

    /// Requests one dataset artifact from a materializer worker.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for a wrong role, worker failure, timeout, or
    /// protocol or transport violation.
    pub fn materialize(
        &mut self,
        source_dir: impl Into<String>,
        dataset_parameters: Value,
        dataset_seed: u64,
        output_dir: impl Into<String>,
    ) -> Result<MaterializeResponse, WorkerSessionError> {
        self.require_role(WorkerPhase::Materialize, WorkerRole::DatasetMaterializer)?;
        let id = self.begin_request(WorkerPhase::Materialize)?;
        let request =
            MaterializeRequest::new(id, source_dir, dataset_parameters, dataset_seed, output_dir);
        let timeout = self.timeouts.materialize;
        self.exchange(WorkerPhase::Materialize, &request, timeout)
    }

    /// Checks intrinsic support before entering any measured scope.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for an unselected capability, wrong role,
    /// worker failure, timeout, or protocol violation.
    pub fn check_applicability(
        &mut self,
        dataset_dir: String,
        dataset_manifest: String,
        problem_contract_id: String,
        problem_parameters: Value,
        implementation_parameters: Value,
    ) -> Result<(CheckApplicabilityRequest, CheckApplicabilityResponse), WorkerSessionError> {
        let phase = WorkerPhase::CheckApplicability;
        self.require_role(phase, WorkerRole::Implementation)?;
        if !self
            .negotiated()
            .selected_capabilities()
            .contains(&Capability::Applicability)
        {
            return Err(WorkerSessionError::CapabilityNotSelected {
                phase,
                capability: Capability::Applicability,
            });
        }
        let id = self.begin_request(phase)?;
        let request = CheckApplicabilityRequest::new(
            id,
            dataset_dir,
            dataset_manifest,
            problem_contract_id,
            problem_parameters,
            implementation_parameters,
        );
        let response = self.exchange(phase, &request, self.timeouts.prepare)?;
        Ok((request, response))
    }

    /// Establishes clean implementation state for one attempt.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for a wrong role, worker failure, timeout, or
    /// protocol or transport violation.
    pub fn prepare(
        &mut self,
        dataset_dir: impl Into<String>,
        problem_contract_id: impl Into<String>,
        problem_parameters: Value,
        implementation_parameters: Value,
        implementation_seed: u64,
    ) -> Result<Acknowledgement, WorkerSessionError> {
        self.require_role(WorkerPhase::Prepare, WorkerRole::Implementation)?;
        let id = self.begin_request(WorkerPhase::Prepare)?;
        let request = PrepareRequest::new(
            id,
            dataset_dir,
            problem_contract_id,
            problem_parameters,
            implementation_parameters,
            implementation_seed,
        );
        let timeout = self.timeouts.prepare;
        self.exchange(WorkerPhase::Prepare, &request, timeout)
    }

    /// Executes one prepared implementation and receives its result manifest.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when `one_shot` was not selected, for a wrong
    /// role, worker failure, timeout, or protocol or transport violation.
    pub fn execute(
        &mut self,
        result_dir: impl Into<String>,
    ) -> Result<ExecuteResponse, WorkerSessionError> {
        self.require_role(WorkerPhase::Execute, WorkerRole::Implementation)?;
        if !self
            .negotiated()
            .selected_capabilities()
            .contains(&Capability::OneShot)
        {
            return Err(WorkerSessionError::CapabilityNotSelected {
                phase: WorkerPhase::Execute,
                capability: Capability::OneShot,
            });
        }
        let id = self.begin_request(WorkerPhase::Execute)?;
        let request = ExecuteRequest::new(id, result_dir);
        let timeout = self.timeouts.execute;
        self.exchange(WorkerPhase::Execute, &request, timeout)
    }

    /// Executes a selected buffered profile without checkpoint round trips.
    pub fn execute_buffered(
        &mut self,
        result_root: impl Into<String>,
        profile: Value,
    ) -> Result<metewand_protocol::ExecuteBufferedResponse, WorkerSessionError> {
        self.require_role(WorkerPhase::Execute, WorkerRole::Implementation)?;
        if !self
            .negotiated()
            .selected_capabilities()
            .contains(&Capability::BufferedProfile)
        {
            return Err(WorkerSessionError::CapabilityNotSelected {
                phase: WorkerPhase::Execute,
                capability: Capability::BufferedProfile,
            });
        }
        let id = self.begin_request(WorkerPhase::Execute)?;
        let request = metewand_protocol::ExecuteBufferedRequest::new(id, result_root, profile);
        self.exchange(WorkerPhase::Execute, &request, self.timeouts.execute)
    }

    /// Clears slot-scoped state before an implementation worker is reused.
    ///
    /// A failed reset discards the worker because clean state can no longer be
    /// established.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for a wrong role, worker failure, timeout, or
    /// protocol or transport violation.
    pub fn reset(&mut self) -> Result<Acknowledgement, WorkerSessionError> {
        self.require_role(WorkerPhase::Reset, WorkerRole::Implementation)?;
        let id = self.begin_request(WorkerPhase::Reset)?;
        let request = ResetRequest::new(id);
        let timeout = self.timeouts.reset;
        let result = self.exchange(WorkerPhase::Reset, &request, timeout);
        if result.is_err() {
            self.discard();
        }
        result
    }

    /// Runs the problem-owned evaluator for one canonical result.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for a wrong role, worker failure, timeout, or
    /// protocol or transport violation.
    pub fn evaluate(
        &mut self,
        dataset_dir: impl Into<String>,
        problem_contract_id: impl Into<String>,
        problem_parameters: Value,
        problem_semantics: Value,
        result_dir: impl Into<String>,
        metrics_path: impl Into<String>,
    ) -> Result<EvaluateResponse, WorkerSessionError> {
        self.require_role(WorkerPhase::Evaluate, WorkerRole::ProblemEvaluator)?;
        let id = self.begin_request(WorkerPhase::Evaluate)?;
        let mut request = EvaluateRequest::new(
            id,
            dataset_dir,
            problem_contract_id,
            problem_parameters,
            result_dir,
            metrics_path,
        );
        if self
            .negotiated()
            .selected_capabilities()
            .contains(&Capability::ProblemSemantics)
        {
            request = request.with_problem_semantics(problem_semantics);
        }
        let timeout = self.timeouts.evaluate;
        self.exchange(WorkerPhase::Evaluate, &request, timeout)
    }

    /// Requests clean shutdown, waits for exit within the shutdown deadline,
    /// and returns the final bounded logs.
    ///
    /// # Errors
    ///
    /// Returns a typed worker, timeout, process, protocol, or transport failure.
    pub fn shutdown(mut self) -> Result<WorkerProcessOutput, WorkerSessionError> {
        self.ensure_usable()?;
        let started = Instant::now();
        let limit = self.timeouts.shutdown;
        let id = self.begin_request(WorkerPhase::Shutdown)?;
        let request = ShutdownRequest::new(id);
        self.exchange::<_, Acknowledgement>(WorkerPhase::Shutdown, &request, limit)?;
        self.stop_protocol_io();

        loop {
            let status = self
                .process
                .as_mut()
                .expect("a usable session owns its worker process")
                .try_wait()
                .map_err(|source| WorkerSessionError::Process {
                    phase: WorkerPhase::Shutdown,
                    source,
                })?;
            if status.is_some() {
                break;
            }
            if started.elapsed() >= limit {
                self.discard_after_timeout();
                return Err(WorkerSessionError::Timeout {
                    phase: WorkerPhase::Shutdown,
                    limit,
                });
            }
            thread::sleep(Duration::from_millis(1));
        }

        self.verify_controls()?;
        let output = self
            .process
            .take()
            .expect("a usable session owns its worker process")
            .wait()
            .map_err(|source| WorkerSessionError::Process {
                phase: WorkerPhase::Shutdown,
                source,
            })?;
        if !output.status.success() {
            return Err(WorkerSessionError::UnsuccessfulExit {
                phase: WorkerPhase::Shutdown,
                status: output.status,
            });
        }
        Ok(output)
    }

    fn begin_request(&mut self, phase: WorkerPhase) -> Result<RequestId, WorkerSessionError> {
        self.ensure_usable()?;
        self.requests
            .begin_request()
            .map_err(|source| WorkerSessionError::Request { phase, source })
    }

    fn exchange<Q, R>(
        &mut self,
        phase: WorkerPhase,
        request: &Q,
        timeout: Duration,
    ) -> Result<R, WorkerSessionError>
    where
        Q: WireMessage,
        R: WireResponse,
    {
        self.verify_controls()?;
        let started = Instant::now();
        let scientific_remaining = self
            .scientific_deadline
            .map(|deadline| deadline.saturating_duration_since(started));
        let scientific_limited = scientific_remaining.is_some_and(|remaining| remaining <= timeout);
        let limit = scientific_remaining.map_or(timeout, |remaining| timeout.min(remaining));
        let timeout_error = || {
            if scientific_limited {
                WorkerSessionError::ScientificTimeout { phase }
            } else {
                WorkerSessionError::Timeout {
                    phase,
                    limit: timeout,
                }
            }
        };
        if limit.is_zero() {
            self.discard_after_timeout();
            return Err(timeout_error());
        }
        let encoded = match encode_message(request) {
            Ok(encoded) => encoded,
            Err(source) => {
                self.discard();
                return Err(WorkerSessionError::Encode { phase, source });
            }
        };
        if started.elapsed() >= limit {
            self.discard_after_timeout();
            return Err(timeout_error());
        }
        let (reply_sender, reply_receiver) = mpsc::channel();
        let command = ProtocolCommand {
            encoded,
            reply: reply_sender,
        };
        if self
            .protocol_sender
            .as_ref()
            .expect("a usable session owns its protocol sender")
            .send(command)
            .is_err()
        {
            self.discard();
            return Err(WorkerSessionError::ProtocolThreadStopped { phase });
        }

        let mut value = match reply_receiver.recv_timeout(limit.saturating_sub(started.elapsed())) {
            Ok(Ok(Some(value))) => value,
            Ok(Ok(None)) => {
                self.discard();
                return Err(WorkerSessionError::ProtocolClosed { phase });
            }
            Ok(Err(ProtocolIoError::Write(source))) => {
                self.discard();
                return Err(WorkerSessionError::ProtocolWrite { phase, source });
            }
            Ok(Err(ProtocolIoError::Read(source))) => {
                self.discard();
                if !self.redactor.is_empty() {
                    return Err(WorkerSessionError::RedactedProtocolRead { phase });
                }
                return Err(WorkerSessionError::ProtocolRead { phase, source });
            }
            Err(RecvTimeoutError::Timeout) => {
                self.discard_after_timeout();
                return Err(timeout_error());
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.discard();
                return Err(WorkerSessionError::ProtocolThreadStopped { phase });
            }
        };

        self.redactor.value(&mut value);

        let decoded = decode_operation_response::<R>(value, &mut self.requests);
        if started.elapsed() >= limit {
            self.discard_after_timeout();
            return Err(timeout_error());
        }
        self.verify_controls()?;
        match decoded {
            Ok(OperationResponse::Success(response)) => Ok(response),
            Ok(OperationResponse::Failure(failure)) => {
                Err(WorkerSessionError::WorkerFailure { phase, failure })
            }
            Err(source) => {
                self.discard();
                Err(WorkerSessionError::Response { phase, source })
            }
        }
    }

    fn verify_controls(&mut self) -> Result<(), WorkerSessionError> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(directory) = self.private_working_directory() {
            let private = std::fs::symlink_metadata(directory).is_ok_and(|metadata| {
                metadata.is_dir() && metadata.permissions().mode() & 0o777 == 0o700
            });
            if !private {
                self.discard();
                return Err(WorkerSessionError::ControlEnforcementLost {
                    control: "private_working_directory".to_owned(),
                });
            }
        }
        Ok(())
    }

    fn require_role(
        &self,
        phase: WorkerPhase,
        expected: WorkerRole,
    ) -> Result<(), WorkerSessionError> {
        self.ensure_usable()?;
        if self.role == expected {
            Ok(())
        } else {
            Err(WorkerSessionError::InvalidRole {
                phase,
                role: self.role,
            })
        }
    }

    fn ensure_usable(&self) -> Result<(), WorkerSessionError> {
        if self.usable {
            Ok(())
        } else {
            Err(WorkerSessionError::SessionUnusable)
        }
    }

    fn stop_protocol_io(&mut self) {
        self.protocol_sender.take();
        if let Some(process) = &self.process {
            process.cancel_protocol_io();
        }
        if let Some(protocol_thread) = self.protocol_thread.take() {
            let _ = protocol_thread.join();
        }
    }

    pub(crate) fn discard_after_timeout(&mut self) {
        if let Some(process) = &mut self.process {
            let _ = LocalProcessExecutor::new(Duration::from_millis(100)).terminate(process);
        }
        // Forced cleanup remains the fallback if the graceful sequence fails.
        self.discard();
    }

    fn discard(&mut self) {
        if !self.usable && self.process.is_none() {
            return;
        }
        self.usable = false;
        if let Some(process) = self.process.as_mut() {
            let _ = process.kill();
        }
        self.stop_protocol_io();
        if let Some(process) = self.process.take() {
            let _ = process.wait();
        }
    }
}

impl Drop for WorkerSession {
    fn drop(&mut self) {
        if self.process.is_some() {
            self.discard();
        }
    }
}

#[derive(Debug)]
struct ProtocolCommand {
    encoded: Vec<u8>,
    reply: Sender<Result<Option<Value>, ProtocolIoError>>,
}

#[derive(Debug)]
enum ProtocolIoError {
    Write(io::Error),
    Read(FrameError),
}

fn start_protocol_io(
    mut reader: FrameReader<WorkerPipe<std::fs::File>>,
    mut writer: WorkerPipe<std::fs::File>,
) -> io::Result<(Sender<ProtocolCommand>, JoinHandle<()>)> {
    let (sender, receiver) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("metewand-worker-protocol".to_owned())
        .spawn(move || run_protocol_io(&mut reader, &mut writer, &receiver))?;
    Ok((sender, thread))
}

fn run_protocol_io(
    reader: &mut FrameReader<WorkerPipe<std::fs::File>>,
    writer: &mut WorkerPipe<std::fs::File>,
    receiver: &Receiver<ProtocolCommand>,
) {
    while let Ok(command) = receiver.recv() {
        let response = writer
            .write_all(&command.encoded)
            .and_then(|()| writer.flush())
            .map_err(ProtocolIoError::Write)
            .and_then(|()| reader.read_frame().map_err(ProtocolIoError::Read));
        let fatal = response.is_err() || matches!(response, Ok(None));
        let _ = command.reply.send(response);
        if fatal {
            break;
        }
    }
}

/// A typed failure from a negotiated one-shot worker session.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WorkerSessionError {
    /// A previously applied lifetime control no longer holds.
    #[error("worker control enforcement lost: {control}")]
    ControlEnforcementLost {
        /// Stable name of the violated control.
        control: String,
    },

    /// Secret-bearing sessions omit raw framing diagnostics that can quote input.
    #[error("worker {phase} protocol read failed (secret-bearing diagnostic redacted)")]
    RedactedProtocolRead {
        /// Failed request phase.
        phase: WorkerPhase,
    },
    /// The protocol I/O thread could not be created.
    #[error("failed to start worker protocol I/O: {source}")]
    StartProtocolThread {
        /// Thread creation failure.
        #[source]
        source: io::Error,
    },
    /// The handshake response failed negotiation or identity validation.
    #[error("worker handshake failed: {source}")]
    Handshake {
        /// Typed handshake failure.
        #[source]
        source: HandshakeError,
    },
    /// A request could not enter the serial request tracker.
    #[error("worker {phase} request could not begin: {source}")]
    Request {
        /// Phase whose request could not begin.
        phase: WorkerPhase,
        /// Request tracking failure.
        #[source]
        source: RequestTrackerError,
    },
    /// A typed request could not be encoded.
    #[error("worker {phase} request could not be encoded: {source}")]
    Encode {
        /// Phase whose request could not be encoded.
        phase: WorkerPhase,
        /// Message encoding failure.
        #[source]
        source: MessageEncodeError,
    },
    /// The protocol request pipe could not be written.
    #[error("worker {phase} request could not be written: {source}")]
    ProtocolWrite {
        /// Phase whose request could not be written.
        phase: WorkerPhase,
        /// Pipe write failure.
        #[source]
        source: io::Error,
    },
    /// The protocol response did not form a valid frame.
    #[error("worker {phase} response framing failed: {source}")]
    ProtocolRead {
        /// Phase whose response could not be framed.
        phase: WorkerPhase,
        /// Framing failure.
        #[source]
        source: FrameError,
    },
    /// The worker closed its response pipe before replying.
    #[error("worker closed the protocol stream during {phase} without a response")]
    ProtocolClosed {
        /// Phase awaiting a response.
        phase: WorkerPhase,
    },
    /// The protocol I/O thread stopped before delivering a response.
    #[error("worker protocol I/O stopped during {phase}")]
    ProtocolThreadStopped {
        /// Phase awaiting a response.
        phase: WorkerPhase,
    },
    /// The response violated correlation or its strict message schema.
    #[error("worker {phase} response is invalid: {source}")]
    Response {
        /// Phase whose response was invalid.
        phase: WorkerPhase,
        /// Response decoding or correlation failure.
        #[source]
        source: OperationResponseDecodeError,
    },
    /// The worker returned a structured operation failure.
    #[error("worker reported a {phase} failure: {} (code: {:?}, details: {:?})", failure.message(), failure.code(), failure.details())]
    WorkerFailure {
        /// Failed worker phase.
        phase: WorkerPhase,
        /// Structured worker-owned failure.
        failure: WorkerFailure,
    },
    /// The complete selected scientific timing scope exceeded its deadline.
    #[error("scientific timing scope expired during worker {phase}")]
    ScientificTimeout {
        /// Worker phase active when the shared deadline expired.
        phase: WorkerPhase,
    },
    /// The phase exceeded its fully resolved deadline.
    #[error("worker {phase} exceeded its {limit:?} timeout")]
    Timeout {
        /// Timed-out worker phase.
        phase: WorkerPhase,
        /// Exact deadline applied to the phase.
        limit: Duration,
    },
    /// The operation does not belong to the worker's negotiated role.
    #[error("worker role {role:?} cannot perform {phase}")]
    InvalidRole {
        /// Rejected operation.
        phase: WorkerPhase,
        /// Negotiated worker role.
        role: WorkerRole,
    },
    /// The operation requires a capability that the plan did not select.
    #[error("worker {phase} requires selected capability {capability:?}")]
    CapabilityNotSelected {
        /// Rejected operation.
        phase: WorkerPhase,
        /// Capability required by the operation.
        capability: Capability,
    },
    /// A transport-level process operation failed.
    #[error("worker process operation failed during {phase}: {source}")]
    Process {
        /// Active phase.
        phase: WorkerPhase,
        /// Process transport failure.
        #[source]
        source: WorkerProcessError,
    },
    /// The worker acknowledged shutdown but exited unsuccessfully.
    #[error("worker exited with {status} after acknowledging {phase}")]
    UnsuccessfulExit {
        /// Active phase.
        phase: WorkerPhase,
        /// Worker exit status.
        status: ExitStatus,
    },
    /// A fatal earlier failure has already discarded this worker.
    #[error("worker session is unusable after an earlier fatal failure")]
    SessionUnusable,
}
