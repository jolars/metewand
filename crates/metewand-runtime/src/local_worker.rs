//! Trusted local-worker launch and private working-directory allocation.

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{fs, path::Path, process::Command};

use metewand_core::records::{ResolvedLaunchRecord, WorkerWorkingDirectory};
use thiserror::Error;

#[cfg(unix)]
use crate::worker_environment::WORKER_UMASK;
use crate::{
    redaction::Redactor,
    worker_environment::{
        WorkerEnvironmentError, WorkerEnvironmentProvenance, WorkerSecrets,
        resolve_worker_environment,
    },
    worker_process::{WorkerLogLimits, WorkerProcess, WorkerProcessError},
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
) -> Result<WorkerProcess, LocalWorkerLaunchError> {
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
) -> Result<WorkerProcess, LocalWorkerLaunchError> {
    let mut variables = resolve_worker_environment(&launch.environment_variables)?;
    secrets.validate(&variables)?;
    let private_root = fs::canonicalize(private_root)
        .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;
    let private_working_directory = match launch.working_directory {
        WorkerWorkingDirectory::PrivateRunDirectory => {
            crate::platform_fs::private_tempdir_in(&private_root, "worker-")
                .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?
        }
    };

    for value in variables.values_mut() {
        if let Some(relative) = value.strip_prefix("{worker}/") {
            let mut path = private_working_directory.path().to_owned();
            for component in relative.split('/') {
                path.push(component);
                if !path.exists() {
                    crate::platform_fs::create_private_directory(&path)
                        .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;
                }
            }
            *value = path
                .to_str()
                .ok_or(WorkerEnvironmentError::InvalidPath)?
                .to_owned();
        }
    }
    let provenance = WorkerEnvironmentProvenance::new(
        private_working_directory.path().to_owned(),
        variables.clone(),
    );
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

    if let Some(language) = variables.get("METEWAND_ADAPTER_LANGUAGE") {
        let path = crate::language_adapter::stage(language, private_working_directory.path())
            .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?;
        command.env("METEWAND_ADAPTER_PATH", path);
    }

    // SAFETY: POSIX umask is async-signal-safe. Applying it only in the
    // forked child avoids races with other orchestrator threads.
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            libc::umask(WORKER_UMASK);
            Ok(())
        });
    }

    #[cfg(unix)]
    let mut process = WorkerProcess::spawn_redacted(command, log_limits, Redactor::new(secrets))
        .map_err(LocalWorkerLaunchError::Spawn)?;
    #[cfg(windows)]
    let mut process =
        WorkerProcess::spawn_windows(command, log_limits, Redactor::new(secrets), true)
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
