//! Reusable execution boundary and trusted POSIX local-process implementation.

use crate::{
    local_worker::{
        LocalWorkerLaunchError, launch_trusted_local_worker,
        launch_trusted_local_worker_with_secrets,
    },
    worker_environment::WorkerSecrets,
    worker_process::{PosixWorkerProcess, WorkerLogLimits, WorkerProcessError},
};
use metewand_core::records::ResolvedLaunchRecord;
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

/// Required execution guarantees, checked before allocating a worker.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutorRequirements {
    /// Require containment even when a descendant creates a new session.
    pub complete_process_tree: bool,
}

/// Explicit guarantees of an executor backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutorCapabilities {
    /// Ordinary descendants share a private process group.
    pub process_groups: bool,
    /// All descendants remain contained, including new sessions.
    pub complete_process_tree: bool,
    /// Each worker receives a private, owner-only working directory.
    pub private_working_directory: bool,
}

/// Launch and lifecycle boundary independent of environment resolution.
pub trait Executor {
    /// Backend-specific handle used by the protocol transport.
    type Process;

    /// Backend-specific capability, launch, and lifecycle failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Reports supported guarantees and rejects insufficient required controls.
    ///
    /// # Errors
    /// Returns an error when a required guarantee cannot be provided.
    fn preflight(
        &self,
        requirements: ExecutorRequirements,
    ) -> Result<ExecutorCapabilities, Self::Error>;

    /// Allocates a private directory and launches the resolved worker.
    ///
    /// # Errors
    /// Returns a preflight, directory allocation, or process launch error.
    fn spawn(
        &self,
        launch: &ResolvedLaunchRecord,
        private_root: &Path,
        logs: WorkerLogLimits,
        requirements: ExecutorRequirements,
    ) -> Result<Self::Process, Self::Error>;

    /// Interrupts execution, allows a bounded grace period, then forces exit.
    ///
    /// # Errors
    /// Returns an error on signal failure, observation failure, or failure to exit.
    fn terminate(&self, process: &mut Self::Process) -> Result<(), Self::Error>;
}

/// Trusted local executor with POSIX process-group cleanup.
#[derive(Clone, Copy, Debug)]
pub struct LocalProcessExecutor {
    grace: Duration,
}

impl LocalProcessExecutor {
    /// Sets the grace period between interrupt and forced termination.
    #[must_use]
    pub const fn new(grace: Duration) -> Self {
        Self { grace }
    }

    /// Launches with ephemeral values for declared secret environment channels.
    ///
    /// # Errors
    /// Returns a preflight, environment validation, allocation, or launch error.
    pub fn spawn_with_secrets(
        &self,
        launch: &ResolvedLaunchRecord,
        private_root: &Path,
        logs: WorkerLogLimits,
        requirements: ExecutorRequirements,
        secrets: &WorkerSecrets,
    ) -> Result<PosixWorkerProcess, ExecutorError> {
        self.preflight(requirements)?;
        Ok(launch_trusted_local_worker_with_secrets(
            launch,
            private_root,
            logs,
            secrets,
        )?)
    }
}

impl Executor for LocalProcessExecutor {
    type Process = PosixWorkerProcess;
    type Error = ExecutorError;

    fn preflight(
        &self,
        requirements: ExecutorRequirements,
    ) -> Result<ExecutorCapabilities, ExecutorError> {
        if requirements.complete_process_tree {
            return Err(ExecutorError::UnsupportedCompleteProcessTree);
        }
        Ok(ExecutorCapabilities {
            process_groups: true,
            complete_process_tree: false,
            private_working_directory: true,
        })
    }

    fn spawn(
        &self,
        launch: &ResolvedLaunchRecord,
        private_root: &Path,
        logs: WorkerLogLimits,
        requirements: ExecutorRequirements,
    ) -> Result<Self::Process, ExecutorError> {
        self.preflight(requirements)?;
        Ok(launch_trusted_local_worker(launch, private_root, logs)?)
    }

    fn terminate(&self, process: &mut Self::Process) -> Result<(), ExecutorError> {
        process.interrupt()?;
        // The leader may exit before its descendants; preserve the full grace
        // period and always signal the group afterward.
        let start = Instant::now();
        while start.elapsed() < self.grace {
            process.try_wait()?;
            thread::sleep(
                (self.grace - start.elapsed().min(self.grace)).min(Duration::from_millis(5)),
            );
        }
        process.kill()?;
        let start = Instant::now();
        loop {
            if process.try_wait()?.is_some() {
                return Ok(());
            }
            if start.elapsed() >= Duration::from_secs(1) {
                return Err(ExecutorError::TerminationDeadline);
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Typed launch, preflight, and termination failures.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// POSIX process groups cannot contain descendants that create new sessions.
    #[error("local process executor cannot enforce complete process-tree containment")]
    UnsupportedCompleteProcessTree,
    /// Directory allocation or process creation failed.
    #[error(transparent)]
    Launch(#[from] LocalWorkerLaunchError),
    /// Process signaling or observation failed.
    #[error(transparent)]
    Process(#[from] WorkerProcessError),
    /// The kernel did not report exit within the forced termination bound.
    #[error("worker did not exit within one second after forced termination")]
    TerminationDeadline,
}
