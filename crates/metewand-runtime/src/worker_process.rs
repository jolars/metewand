//! POSIX worker transport and bounded standard-stream capture.

use std::{
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
};

use metewand_protocol::framing::FrameReader;
#[cfg(target_os = "espidf")]
use rustix::io::fcntl_dupfd;
#[cfg(not(target_os = "espidf"))]
use rustix::io::fcntl_dupfd_cloexec;
use rustix::io::{FdFlags, fcntl_setfd};
#[cfg(any(
    target_vendor = "apple",
    target_os = "aix",
    target_os = "espidf",
    target_os = "haiku",
    target_os = "horizon",
    target_os = "nto"
))]
use rustix::pipe::pipe;
#[cfg(not(any(
    target_vendor = "apple",
    target_os = "aix",
    target_os = "espidf",
    target_os = "haiku",
    target_os = "horizon",
    target_os = "nto"
)))]
use rustix::pipe::{PipeFlags, pipe_with};
use thiserror::Error;

/// Name of the environment variable containing the worker's protocol read descriptor.
pub const PROTOCOL_READ_FD_ENV: &str = "METEWAND_PROTOCOL_READ_FD";

/// Name of the environment variable containing the worker's protocol write descriptor.
pub const PROTOCOL_WRITE_FD_ENV: &str = "METEWAND_PROTOCOL_WRITE_FD";

/// Independent retained-byte limits for a worker's standard streams.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerLogLimits {
    stdout_bytes: usize,
    stderr_bytes: usize,
}

impl WorkerLogLimits {
    /// Creates explicit standard-output and standard-error capture limits.
    #[must_use]
    pub const fn new(stdout_bytes: usize, stderr_bytes: usize) -> Self {
        Self {
            stdout_bytes,
            stderr_bytes,
        }
    }
}

/// One completely drained standard stream with a bounded retained prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedWorkerLog {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
}

impl CapturedWorkerLog {
    /// Returns the retained byte prefix.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the total number of bytes drained from the stream.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Reports whether any drained bytes were omitted from the retained prefix.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Consumes the capture and returns its retained byte prefix.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Exit status and completely drained standard-stream logs for a worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerProcessOutput {
    /// Worker's process exit status.
    pub status: ExitStatus,
    /// Bounded standard-output capture.
    pub stdout: CapturedWorkerLog,
    /// Bounded standard-error capture.
    pub stderr: CapturedWorkerLog,
}

/// A spawned worker using dedicated version-1 POSIX protocol pipes.
///
/// Standard output and standard error are reserved for logs and begin draining
/// on separate threads immediately after the process is spawned. Protocol
/// requests and responses use the inherited descriptors named by
/// [`PROTOCOL_READ_FD_ENV`] and [`PROTOCOL_WRITE_FD_ENV`].
#[derive(Debug)]
pub struct PosixWorkerProcess {
    child: Child,
    protocol_reader: Option<FrameReader<File>>,
    protocol_writer: Option<File>,
    stdout_drain: JoinHandle<io::Result<CapturedWorkerLog>>,
    stderr_drain: JoinHandle<io::Result<CapturedWorkerLog>>,
}

impl PosixWorkerProcess {
    /// Spawns a worker with dedicated protocol pipes and bounded log drains.
    ///
    /// The command is consumed because its inherited-descriptor setup is valid
    /// for exactly one spawn attempt. Existing values for the two protocol
    /// descriptor environment variables and standard-stream configuration are
    /// replaced.
    ///
    /// # Errors
    ///
    /// Returns a typed transport error if pipes cannot be created, the process
    /// cannot be spawned, or either log-drain thread cannot be started.
    pub fn spawn(
        mut command: Command,
        log_limits: WorkerLogLimits,
    ) -> Result<Self, WorkerProcessError> {
        let (worker_request_reader, parent_request_writer) =
            protocol_pipe().map_err(WorkerProcessError::CreateRequestPipe)?;
        let (parent_response_reader, worker_response_writer) =
            protocol_pipe().map_err(WorkerProcessError::CreateResponsePipe)?;

        let worker_read_fd = worker_request_reader.as_raw_fd();
        let worker_write_fd = worker_response_writer.as_raw_fd();
        command
            .env(PROTOCOL_READ_FD_ENV, worker_read_fd.to_string())
            .env(PROTOCOL_WRITE_FD_ENV, worker_write_fd.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // SAFETY: `fcntl(F_SETFD)` is async-signal-safe, and both raw
        // descriptors remain owned by this stack frame until `spawn` returns.
        // Clearing only `FD_CLOEXEC` preserves the descriptor numbers recorded
        // in the child's environment without exposing the other pipe ends.
        unsafe {
            command.pre_exec(move || {
                inherit_for_exec(worker_read_fd)?;
                inherit_for_exec(worker_write_fd)?;
                Ok(())
            });
        }

        let mut child = command.spawn().map_err(WorkerProcessError::Spawn)?;
        drop(worker_request_reader);
        drop(worker_response_writer);

        let Some(stdout) = child.stdout.take() else {
            abort_spawn(&mut child);
            return Err(WorkerProcessError::MissingLogPipe {
                stream: WorkerLogStream::Stdout,
            });
        };
        let Some(stderr) = child.stderr.take() else {
            abort_spawn(&mut child);
            return Err(WorkerProcessError::MissingLogPipe {
                stream: WorkerLogStream::Stderr,
            });
        };

        let stdout_drain =
            match start_log_drain(WorkerLogStream::Stdout, stdout, log_limits.stdout_bytes) {
                Ok(drain) => drain,
                Err(source) => {
                    abort_spawn(&mut child);
                    return Err(WorkerProcessError::StartLogDrain {
                        stream: WorkerLogStream::Stdout,
                        source,
                    });
                }
            };
        let stderr_drain =
            match start_log_drain(WorkerLogStream::Stderr, stderr, log_limits.stderr_bytes) {
                Ok(drain) => drain,
                Err(source) => {
                    abort_spawn(&mut child);
                    let _ = stdout_drain.join();
                    return Err(WorkerProcessError::StartLogDrain {
                        stream: WorkerLogStream::Stderr,
                        source,
                    });
                }
            };

        Ok(Self {
            child,
            protocol_reader: Some(FrameReader::new(File::from(parent_response_reader))),
            protocol_writer: Some(File::from(parent_request_writer)),
            stdout_drain,
            stderr_drain,
        })
    }

    /// Returns the bounded protocol response reader.
    pub fn protocol_reader(&mut self) -> &mut FrameReader<File> {
        self.protocol_reader
            .as_mut()
            .expect("protocol I/O can be transferred only by the runtime session")
    }

    /// Returns the protocol request writer.
    pub fn protocol_writer(&mut self) -> &mut File {
        self.protocol_writer
            .as_mut()
            .expect("protocol I/O can be transferred only by the runtime session")
    }

    pub(crate) fn take_protocol_io(&mut self) -> (FrameReader<File>, File) {
        let reader = self
            .protocol_reader
            .take()
            .expect("protocol I/O must be transferred exactly once");
        let writer = self
            .protocol_writer
            .take()
            .expect("protocol I/O must be transferred exactly once");
        (reader, writer)
    }

    /// Returns the operating-system process identifier.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Checks whether the worker has exited without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error if the child status cannot be observed.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, WorkerProcessError> {
        self.child.try_wait().map_err(WorkerProcessError::Observe)
    }

    /// Requests immediate termination of the worker process.
    ///
    /// Process-tree containment and the interrupt/grace/force sequence belong
    /// to the executor layer built on this transport.
    ///
    /// # Errors
    ///
    /// Returns an error if the termination signal cannot be sent.
    pub fn kill(&mut self) -> Result<(), WorkerProcessError> {
        self.child.kill().map_err(WorkerProcessError::Kill)
    }

    /// Closes the protocol pipes, waits for exit, and joins both log drains.
    ///
    /// # Errors
    ///
    /// Returns a typed transport error if waiting or draining fails, or if a
    /// drain thread panics.
    pub fn wait(self) -> Result<WorkerProcessOutput, WorkerProcessError> {
        let Self {
            mut child,
            protocol_reader,
            protocol_writer,
            stdout_drain,
            stderr_drain,
        } = self;
        drop(protocol_reader);
        drop(protocol_writer);

        let status = match child.wait() {
            Ok(status) => status,
            Err(source) => {
                abort_spawn(&mut child);
                let _ = stdout_drain.join();
                let _ = stderr_drain.join();
                return Err(WorkerProcessError::Wait(source));
            }
        };
        let stdout = join_log_drain(WorkerLogStream::Stdout, stdout_drain);
        let stderr = join_log_drain(WorkerLogStream::Stderr, stderr_drain);

        Ok(WorkerProcessOutput {
            status,
            stdout: stdout?,
            stderr: stderr?,
        })
    }
}

/// Standard stream associated with a worker transport failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerLogStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl std::fmt::Display for WorkerLogStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdout => formatter.write_str("stdout"),
            Self::Stderr => formatter.write_str("stderr"),
        }
    }
}

/// Failure to establish or complete a POSIX worker transport.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WorkerProcessError {
    /// The orchestrator-to-worker pipe could not be created.
    #[error("failed to create the worker protocol request pipe: {0}")]
    CreateRequestPipe(#[source] io::Error),

    /// The worker-to-orchestrator pipe could not be created.
    #[error("failed to create the worker protocol response pipe: {0}")]
    CreateResponsePipe(#[source] io::Error),

    /// The configured worker process could not be spawned.
    #[error("failed to spawn the worker process: {0}")]
    Spawn(#[source] io::Error),

    /// A standard-stream pipe was unexpectedly unavailable after spawn.
    #[error("spawned worker has no piped {stream}")]
    MissingLogPipe {
        /// Missing standard stream.
        stream: WorkerLogStream,
    },

    /// A concurrent standard-stream drain could not be started.
    #[error("failed to start the worker {stream} drain: {source}")]
    StartLogDrain {
        /// Standard stream whose drain could not be started.
        stream: WorkerLogStream,
        /// Thread creation failure.
        #[source]
        source: io::Error,
    },

    /// The worker's status could not be observed without blocking.
    #[error("failed to observe the worker process: {0}")]
    Observe(#[source] io::Error),

    /// Immediate worker termination failed.
    #[error("failed to kill the worker process: {0}")]
    Kill(#[source] io::Error),

    /// Waiting for worker termination failed.
    #[error("failed to wait for the worker process: {0}")]
    Wait(#[source] io::Error),

    /// Reading a standard stream failed before EOF.
    #[error("failed to drain worker {stream}: {source}")]
    Drain {
        /// Standard stream that failed.
        stream: WorkerLogStream,
        /// Read failure.
        #[source]
        source: io::Error,
    },

    /// A standard-stream drain thread panicked.
    #[error("worker {stream} drain thread panicked")]
    DrainThreadPanicked {
        /// Standard stream whose drain panicked.
        stream: WorkerLogStream,
    },
}

fn start_log_drain(
    stream: WorkerLogStream,
    reader: impl Read + Send + 'static,
    limit: usize,
) -> io::Result<JoinHandle<io::Result<CapturedWorkerLog>>> {
    thread::Builder::new()
        .name(format!("metewand-worker-{stream}"))
        .spawn(move || drain_log(reader, limit))
}

fn drain_log(mut reader: impl Read, limit: usize) -> io::Result<CapturedWorkerLog> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut total_bytes = 0_u64;
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));

        let retained = read.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }

    Ok(CapturedWorkerLog {
        bytes,
        total_bytes,
        truncated,
    })
}

fn join_log_drain(
    stream: WorkerLogStream,
    drain: JoinHandle<io::Result<CapturedWorkerLog>>,
) -> Result<CapturedWorkerLog, WorkerProcessError> {
    drain
        .join()
        .map_err(|_| WorkerProcessError::DrainThreadPanicked { stream })?
        .map_err(|source| WorkerProcessError::Drain { stream, source })
}

fn inherit_for_exec(raw_fd: RawFd) -> io::Result<()> {
    // SAFETY: The caller guarantees that `raw_fd` remains open until the
    // `pre_exec` closure has completed.
    let descriptor = unsafe { BorrowedFd::borrow_raw(raw_fd) };
    fcntl_setfd(descriptor, FdFlags::empty()).map_err(io::Error::from)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "aix",
    target_os = "espidf",
    target_os = "haiku",
    target_os = "horizon",
    target_os = "nto"
)))]
fn protocol_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let (reader, writer) = pipe_with(PipeFlags::CLOEXEC).map_err(io::Error::from)?;
    dedicated_pipe_ends(reader, writer)
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "aix",
    target_os = "espidf",
    target_os = "haiku",
    target_os = "horizon",
    target_os = "nto"
))]
fn protocol_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let (reader, writer) = pipe().map_err(io::Error::from)?;
    fcntl_setfd(&reader, FdFlags::CLOEXEC).map_err(io::Error::from)?;
    fcntl_setfd(&writer, FdFlags::CLOEXEC).map_err(io::Error::from)?;
    dedicated_pipe_ends(reader, writer)
}

fn dedicated_pipe_ends(reader: OwnedFd, writer: OwnedFd) -> io::Result<(OwnedFd, OwnedFd)> {
    Ok((dedicated_fd(reader)?, dedicated_fd(writer)?))
}

fn dedicated_fd(descriptor: OwnedFd) -> io::Result<OwnedFd> {
    if descriptor.as_raw_fd() > 2 {
        return Ok(descriptor);
    }

    duplicate_cloexec(&descriptor)
}

#[cfg(not(target_os = "espidf"))]
fn duplicate_cloexec(descriptor: &OwnedFd) -> io::Result<OwnedFd> {
    fcntl_dupfd_cloexec(descriptor, 3).map_err(io::Error::from)
}

#[cfg(target_os = "espidf")]
fn duplicate_cloexec(descriptor: &OwnedFd) -> io::Result<OwnedFd> {
    let duplicate = fcntl_dupfd(descriptor, 3).map_err(io::Error::from)?;
    fcntl_setfd(&duplicate, FdFlags::CLOEXEC).map_err(io::Error::from)?;
    Ok(duplicate)
}

fn abort_spawn(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
