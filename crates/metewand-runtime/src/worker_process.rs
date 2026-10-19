//! POSIX worker transport and bounded standard-stream capture.

use std::{
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
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
use tempfile::TempDir;
use thiserror::Error;

use crate::{redaction::Redactor, worker_environment::WorkerEnvironmentProvenance};

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
    /// Realized worker settings, with redacted secret channels.
    pub environment: Option<WorkerEnvironmentProvenance>,
    /// Worker's process exit status.
    pub status: ExitStatus,
    /// Bounded standard-output capture.
    pub stdout: CapturedWorkerLog,
    /// Bounded standard-error capture.
    pub stderr: CapturedWorkerLog,
}

/// Bounded logs available at an execution boundary, without implying process exit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerLogSnapshot {
    /// Standard output drained by the snapshot boundary.
    pub stdout: CapturedWorkerLog,
    /// Standard error drained by the snapshot boundary.
    pub stderr: CapturedWorkerLog,
    /// Whether both streams reached EOF and were completely drained.
    pub complete: bool,
}

impl From<WorkerProcessOutput> for WorkerLogSnapshot {
    fn from(output: WorkerProcessOutput) -> Self {
        Self {
            stdout: output.stdout,
            stderr: output.stderr,
            complete: true,
        }
    }
}

#[derive(Default)]
struct LiveLog {
    bytes: Vec<u8>,
    total_bytes: u64,
}

impl std::fmt::Debug for LiveLog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveLog")
            .field("retained_bytes", &self.bytes.len())
            .field("total_bytes", &self.total_bytes)
            .finish_non_exhaustive()
    }
}

impl LiveLog {
    fn snapshot(&self, limit: usize, redactor: &Redactor) -> CapturedWorkerLog {
        // Keep a possible secret suffix private until enough following bytes
        // arrive to determine whether it participates in a complete match.
        let visible = self.bytes.len().saturating_sub(redactor.lookahead());
        let mut bytes = redactor.prefix(&self.bytes, visible.min(limit));
        bytes.truncate(limit);
        CapturedWorkerLog {
            bytes,
            total_bytes: self.total_bytes,
            truncated: self.total_bytes > visible.min(limit) as u64,
        }
    }
}

/// A spawned worker using dedicated version-1 POSIX protocol pipes.
///
/// Standard output and standard error are reserved for logs and begin draining
/// on separate threads immediately after the process is spawned. Protocol
/// requests and responses use the inherited descriptors named by
/// [`PROTOCOL_READ_FD_ENV`] and [`PROTOCOL_WRITE_FD_ENV`].
#[derive(Debug)]
pub struct PosixWorkerProcess {
    environment: Option<WorkerEnvironmentProvenance>,
    redactor: Redactor,
    child: Child,
    group: ProcessGroup,
    private_working_directory: Option<TempDir>,
    protocol_reader: Option<FrameReader<File>>,
    protocol_writer: Option<File>,
    stdout_drain: JoinHandle<io::Result<CapturedWorkerLog>>,
    stderr_drain: JoinHandle<io::Result<CapturedWorkerLog>>,
    stdout_live: Arc<Mutex<LiveLog>>,
    stderr_live: Arc<Mutex<LiveLog>>,
    log_limits: WorkerLogLimits,
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
        command: Command,
        log_limits: WorkerLogLimits,
    ) -> Result<Self, WorkerProcessError> {
        Self::spawn_redacted(command, log_limits, Redactor::default())
    }

    pub(crate) fn spawn_redacted(
        mut command: Command,
        log_limits: WorkerLogLimits,
        redactor: Redactor,
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
            .process_group(0)
            .stdin(Stdio::null())
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

        let stdout_live = Arc::new(Mutex::new(LiveLog::default()));
        let stderr_live = Arc::new(Mutex::new(LiveLog::default()));
        let stdout_drain = match start_log_drain(
            WorkerLogStream::Stdout,
            stdout,
            log_limits.stdout_bytes,
            redactor.clone(),
            Arc::clone(&stdout_live),
        ) {
            Ok(drain) => drain,
            Err(source) => {
                abort_spawn(&mut child);
                return Err(WorkerProcessError::StartLogDrain {
                    stream: WorkerLogStream::Stdout,
                    source,
                });
            }
        };
        let stderr_drain = match start_log_drain(
            WorkerLogStream::Stderr,
            stderr,
            log_limits.stderr_bytes,
            redactor.clone(),
            Arc::clone(&stderr_live),
        ) {
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

        let group = ProcessGroup(child.id() as libc::pid_t);
        Ok(Self {
            environment: None,
            redactor,
            group,
            child,
            private_working_directory: None,
            protocol_reader: Some(FrameReader::new(File::from(parent_response_reader))),
            protocol_writer: Some(File::from(parent_request_writer)),
            stdout_drain,
            stderr_drain,
            stdout_live,
            stderr_live,
            log_limits,
        })
    }

    /// Captures a bounded process-lifetime log prefix without stopping the worker.
    ///
    /// Streams are asynchronous, so this snapshot explicitly remains incomplete.
    #[must_use]
    pub fn log_snapshot(&self) -> WorkerLogSnapshot {
        WorkerLogSnapshot {
            stdout: self
                .stdout_live
                .lock()
                .expect("log capture mutex poisoned")
                .snapshot(self.log_limits.stdout_bytes, &self.redactor),
            stderr: self
                .stderr_live
                .lock()
                .expect("log capture mutex poisoned")
                .snapshot(self.log_limits.stderr_bytes, &self.redactor),
            complete: false,
        }
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

    pub(crate) fn retain_private_working_directory(&mut self, directory: TempDir) {
        self.private_working_directory = Some(directory);
    }

    pub(crate) fn retain_environment(&mut self, environment: WorkerEnvironmentProvenance) {
        self.environment = Some(environment);
    }

    /// Returns actual worker settings with all secret channels redacted.
    #[must_use]
    pub fn environment(&self) -> Option<&WorkerEnvironmentProvenance> {
        self.environment.as_ref()
    }

    pub(crate) fn redactor(&self) -> Redactor {
        self.redactor.clone()
    }

    /// Returns the private current directory retained for a local worker.
    ///
    /// Direct transport launches return `None`; the trusted local launch path
    /// returns its allocated directory until the process handle is consumed.
    #[must_use]
    pub fn private_working_directory(&self) -> Option<&Path> {
        self.private_working_directory.as_ref().map(TempDir::path)
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

    /// Interrupts the worker's process group, including ordinary descendants.
    ///
    /// # Errors
    /// Returns an error if the signal cannot be sent.
    pub fn interrupt(&mut self) -> Result<(), WorkerProcessError> {
        self.group
            .signal(libc::SIGINT)
            .map_err(WorkerProcessError::Kill)
    }

    /// Forces termination of the worker's process group.
    ///
    /// Descendants that deliberately create a new session can escape this
    /// reproducibility control; it is not a security boundary.
    ///
    /// # Errors
    /// Returns an error if the signal cannot be sent.
    pub fn kill(&mut self) -> Result<(), WorkerProcessError> {
        self.group
            .signal(libc::SIGKILL)
            .map_err(WorkerProcessError::Kill)
    }

    /// Closes the protocol pipes, waits for exit, and joins both log drains.
    ///
    /// # Errors
    ///
    /// Returns a typed transport error if waiting or draining fails, or if a
    /// drain thread panics.
    pub fn wait(self) -> Result<WorkerProcessOutput, WorkerProcessError> {
        let Self {
            environment,
            mut child,
            group,
            private_working_directory: _private_working_directory,
            protocol_reader,
            protocol_writer,
            stdout_drain,
            stderr_drain,
            ..
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
        group
            .signal(libc::SIGKILL)
            .map_err(WorkerProcessError::Kill)?;
        let stdout = join_log_drain(WorkerLogStream::Stdout, stdout_drain);
        let stderr = join_log_drain(WorkerLogStream::Stderr, stderr_drain);

        Ok(WorkerProcessOutput {
            environment,
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
    redactor: Redactor,
    live: Arc<Mutex<LiveLog>>,
) -> io::Result<JoinHandle<io::Result<CapturedWorkerLog>>> {
    thread::Builder::new()
        .name(format!("metewand-worker-{stream}"))
        .spawn(move || drain_log_shared(reader, limit, &redactor, Some(&live)))
}

#[cfg(test)]
fn drain_log(
    reader: impl Read,
    limit: usize,
    redactor: &Redactor,
) -> io::Result<CapturedWorkerLog> {
    drain_log_shared(reader, limit, redactor, None)
}

fn drain_log_shared(
    mut reader: impl Read,
    limit: usize,
    redactor: &Redactor,
    live: Option<&Mutex<LiveLog>>,
) -> io::Result<CapturedWorkerLog> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut total_bytes = 0_u64;
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    // Retain enough lookahead to recognize a secret crossing the byte limit.
    // Filtering after EOF also handles arbitrary pipe-read fragmentation.
    let retained_limit = limit.saturating_add(redactor.lookahead());

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));

        let retained = read.min(retained_limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
        if let Some(live) = live {
            let mut live = live.lock().expect("log capture mutex poisoned");
            live.bytes.clone_from(&bytes);
            live.total_bytes = total_bytes;
        }
    }

    // Lookahead determines whether the original prefix ends inside a secret;
    // it must never become visible after earlier replacements shorten output.
    bytes = redactor.prefix(&bytes, limit);
    truncated |= bytes.len() > limit || total_bytes > u64::try_from(limit).unwrap_or(u64::MAX);
    bytes.truncate(limit);

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
    let _ = ProcessGroup(child.id() as libc::pid_t).signal(libc::SIGKILL);
    let _ = child.wait();
}

// Retaining the group until log drains finish prevents ordinary descendants
// from keeping the pipes open after the group leader exits.
#[derive(Debug)]
struct ProcessGroup(libc::pid_t);

impl ProcessGroup {
    fn signal(&self, signal: libc::c_int) -> io::Result<()> {
        // SAFETY: A negative process ID addresses the private worker group.
        if unsafe { libc::kill(-self.0, signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = self.signal(libc::SIGKILL);
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;
    use crate::worker_environment::WorkerSecrets;
    use std::collections::BTreeMap;

    #[test]
    fn live_snapshots_withhold_fragmented_secret_suffixes_and_stay_bounded() {
        let secrets =
            WorkerSecrets::new(BTreeMap::from([("TOKEN".into(), "abcdef".into())])).unwrap();
        let redactor = Redactor::new(&secrets);
        for length in 0..=14 {
            let bytes = &b"xxabcdef tail!"[..length];
            let live = LiveLog {
                bytes: bytes.to_vec(),
                total_bytes: length as u64,
            };
            assert!(!format!("{live:?}").contains("abcdef"));
            let snapshot = live.snapshot(8, &redactor);
            assert!(snapshot.bytes().len() <= 8);
            if length < 8 {
                assert!(!snapshot.bytes().contains(&b'a'));
            } else {
                assert!(!snapshot.bytes().windows(2).any(|bytes| bytes == b"ab"));
            }
            assert_eq!(snapshot.total_bytes(), length as u64);
        }
    }

    struct Fragmented<'a>(&'a [u8]);

    impl Read for Fragmented<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(2).min(self.0.len());
            buffer[..count].copy_from_slice(&self.0[..count]);
            self.0 = &self.0[count..];
            Ok(count)
        }
    }

    #[test]
    fn redacts_fragmented_overlapping_and_non_utf8_logs() {
        let secrets = WorkerSecrets::new(BTreeMap::from([
            ("FIRST".into(), "abc".into()),
            ("SECOND".into(), "bcd".into()),
        ]))
        .unwrap();
        let log = drain_log(Fragmented(b"\xff abcd tail"), 100, &Redactor::new(&secrets)).unwrap();
        assert_eq!(log.bytes(), b"\xff [REDACTED] tail");
        assert_eq!(log.total_bytes(), 11);
        assert!(!log.truncated());
    }

    #[test]
    fn redacts_secrets_crossing_retention_and_read_boundaries() {
        let secrets =
            WorkerSecrets::new(BTreeMap::from([("TOKEN".into(), "abcdef".into())])).unwrap();
        let redactor = Redactor::new(&secrets);
        for limit in 0..10 {
            let log = drain_log(Fragmented(b"xxabcdef more log bytes"), limit, &redactor).unwrap();
            let expected = b"xx[REDACTED] more log bytes";
            assert_eq!(log.bytes(), &expected[..limit]);
            assert_eq!(log.total_bytes(), 23);
            assert!(log.truncated());
        }
        let mut bytes = vec![b'x'; 8190];
        bytes.extend_from_slice(b"abcdef tail");
        let log = drain_log(bytes.as_slice(), 8193, &redactor).unwrap();
        assert_eq!(&log.bytes()[8190..], b"[RE");
        assert!(log.truncated());
    }

    #[test]
    fn redaction_never_exposes_lookahead_after_shortening_earlier_secrets() {
        let secret = "abcdefghijklmnopqrstuvwxyz";
        let secrets =
            WorkerSecrets::new(BTreeMap::from([("TOKEN".into(), secret.into())])).unwrap();
        let bytes = format!("{secret}{}{secret} tail", "x".repeat(38));
        let log = drain_log(bytes.as_bytes(), 50, &Redactor::new(&secrets)).unwrap();
        assert_eq!(
            log.bytes(),
            format!("[REDACTED]{}", "x".repeat(24)).as_bytes()
        );
        assert!(log.truncated());
    }
}
