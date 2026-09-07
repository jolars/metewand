//! Minimal trusted local-worker launch support for the conformance kernel.

use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
};

use metewand_core::records::{ResolvedLaunchRecord, WorkerWorkingDirectory};
use tempfile::Builder;
use thiserror::Error;

use crate::worker_process::{PosixWorkerProcess, WorkerLogLimits, WorkerProcessError};

/// Launches an already resolved worker as a trusted local process.
///
/// The worker receives only the resolved launch's allowlisted variables plus
/// the protocol descriptors added by [`PosixWorkerProcess`]. Its private
/// working directory is created beneath `private_root` and retained for the
/// lifetime of the process handle.
///
/// This Gate-1 launch path is a reproducibility boundary, not a sandbox for
/// hostile code. Process-tree controls and the complete worker environment
/// policy belong to the reusable local executor.
///
/// # Errors
///
/// Returns an error if the private working directory cannot be created or the
/// worker process cannot be spawned.
pub fn launch_trusted_local_worker(
    launch: &ResolvedLaunchRecord,
    private_root: &Path,
    log_limits: WorkerLogLimits,
) -> Result<PosixWorkerProcess, LocalWorkerLaunchError> {
    let private_working_directory = match launch.working_directory {
        WorkerWorkingDirectory::PrivateRunDirectory => Builder::new()
            .prefix("worker-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(private_root)
            .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?,
    };
    // The creation mode prevents a permissive window; resetting it afterward
    // keeps restrictive umasks from making the worker directory unusable.
    fs::set_permissions(
        private_working_directory.path(),
        Permissions::from_mode(0o700),
    )
    .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;

    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .env_clear()
        .envs(&launch.environment_variables)
        .current_dir(private_working_directory.path());

    let mut process =
        PosixWorkerProcess::spawn(command, log_limits).map_err(LocalWorkerLaunchError::Spawn)?;
    process.retain_private_working_directory(private_working_directory);
    Ok(process)
}

/// Failure to launch a trusted worker through the minimal local path.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalWorkerLaunchError {
    /// A private working directory could not be allocated.
    #[error("failed to create a private worker directory: {0}")]
    CreatePrivateWorkingDirectory(#[source] std::io::Error),

    /// The configured worker process could not be spawned.
    #[error("failed to launch the trusted local worker: {0}")]
    Spawn(#[source] WorkerProcessError),
}
