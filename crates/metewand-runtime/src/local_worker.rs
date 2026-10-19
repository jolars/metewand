//! Trusted local-worker launch and private working-directory allocation.

use std::{
    fs::{self, DirBuilder, Permissions},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        process::CommandExt,
    },
    path::Path,
    process::Command,
};

use metewand_core::records::{ResolvedLaunchRecord, WorkerWorkingDirectory};
use tempfile::Builder;
use thiserror::Error;

use crate::{
    redaction::Redactor,
    worker_environment::{
        WORKER_UMASK, WorkerEnvironmentError, WorkerEnvironmentProvenance, WorkerSecrets,
        resolve_worker_environment,
    },
    worker_process::{PosixWorkerProcess, WorkerLogLimits, WorkerProcessError},
};

/// Launches an already resolved worker as a trusted local process.
///
/// The worker receives the fixed worker settings, the resolved launch's
/// allowlisted variables, and the protocol descriptors. Its private
/// working directory is created beneath `private_root` and retained for the
/// lifetime of the process handle.
///
/// This Gate-1 launch path is a reproducibility boundary, not a sandbox for
/// hostile code. The transport creates a private process group. Capability
/// requirements and bounded termination belong to the reusable local executor.
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
    launch_trusted_local_worker_with_secrets(
        launch,
        private_root,
        log_limits,
        &WorkerSecrets::default(),
    )
}

/// Launches a worker with values for exactly its declared secret channels.
///
/// Values remain outside the resolved launch and are filtered from captured
/// logs and typed-session responses. Private settings cannot be overridden.
///
/// # Errors
/// Returns a value-free error for invalid settings or secret bindings, or a
/// directory allocation or process launch error.
pub fn launch_trusted_local_worker_with_secrets(
    launch: &ResolvedLaunchRecord,
    private_root: &Path,
    log_limits: WorkerLogLimits,
    secrets: &WorkerSecrets,
) -> Result<PosixWorkerProcess, LocalWorkerLaunchError> {
    let mut variables = resolve_worker_environment(&launch.environment_variables)?;
    secrets.validate(&variables)?;
    let private_root = fs::canonicalize(private_root)
        .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;
    let private_working_directory = match launch.working_directory {
        WorkerWorkingDirectory::PrivateRunDirectory => Builder::new()
            .prefix("worker-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(&private_root)
            .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?,
    };
    // The creation mode prevents a permissive window; resetting it afterward
    // keeps restrictive umasks from making the worker directory unusable.
    fs::set_permissions(
        private_working_directory.path(),
        Permissions::from_mode(0o700),
    )
    .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;

    let directory = private_working_directory
        .path()
        .to_str()
        .ok_or(WorkerEnvironmentError::InvalidPath)?;
    for value in variables.values_mut() {
        if let Some(relative) = value.strip_prefix("{worker}/") {
            let mut path = private_working_directory.path().to_owned();
            for component in relative.split('/') {
                path.push(component);
                if !path.exists() {
                    DirBuilder::new()
                        .mode(0o700)
                        .create(&path)
                        .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;
                    fs::set_permissions(&path, Permissions::from_mode(0o700))
                        .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;
                }
            }
            *value = format!("{directory}/{relative}");
        }
    }
    let provenance = WorkerEnvironmentProvenance {
        version: 1,
        working_directory: private_working_directory.path().to_owned(),
        environment_variables: variables.clone(),
        umask: "0077".to_owned(),
    };
    variables.extend(
        secrets
            .values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );

    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .env_clear()
        .envs(&variables)
        .current_dir(private_working_directory.path());

    // SAFETY: POSIX umask is async-signal-safe. Applying it only in the
    // forked child avoids races with other orchestrator threads.
    unsafe {
        command.pre_exec(|| {
            libc::umask(WORKER_UMASK);
            Ok(())
        });
    }

    let mut process =
        PosixWorkerProcess::spawn_redacted(command, log_limits, Redactor::new(secrets))
            .map_err(LocalWorkerLaunchError::Spawn)?;
    process.retain_environment(provenance);
    process.retain_private_working_directory(private_working_directory);
    Ok(process)
}

/// Failure to launch a trusted worker through the minimal local path.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalWorkerLaunchError {
    /// The allowlist or its secret bindings violate worker environment rules.
    #[error(transparent)]
    Environment(#[from] WorkerEnvironmentError),
    /// A private working directory could not be allocated.
    #[error("failed to create a private worker directory: {0}")]
    CreatePrivateWorkingDirectory(#[source] std::io::Error),

    /// The configured worker process could not be spawned.
    #[error("failed to launch the trusted local worker: {0}")]
    Spawn(#[source] WorkerProcessError),
}
