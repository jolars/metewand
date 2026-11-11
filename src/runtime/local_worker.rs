//! Trusted local-worker launch and private working-directory allocation.

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{fs, path::Path, process::Command};

use crate::core::records::{ResolvedLaunchRecord, WorkerWorkingDirectory};
use thiserror::Error;

#[cfg(unix)]
use crate::runtime::worker_environment::WORKER_UMASK;
use crate::runtime::{
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
    // Rscript appends a forward slash to TMPDIR when launching an expression.
    // Windows verbatim paths disable slash conversion, so expose conventional
    // paths to interpreters after resolving the private root's actual location.
    #[cfg(windows)]
    let private_root = interpreter_directory(private_root);
    let private_working_directory = match launch.working_directory {
        WorkerWorkingDirectory::PrivateRunDirectory => {
            crate::runtime::platform_fs::private_tempdir_in(&private_root, "worker-")
                .map_err(LocalWorkerLaunchError::CreatePrivateWorkingDirectory)?
        }
    };

    for value in variables.values_mut() {
        if let Some(relative) = value.strip_prefix("{worker}/") {
            let mut path = private_working_directory.path().to_owned();
            for component in relative.split('/') {
                path.push(component);
                if !path.exists() {
                    crate::runtime::platform_fs::create_private_directory(&path)
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

    #[cfg(target_os = "linux")]
    if variables.contains_key(crate::runtime::oci::IMAGE_ENV)
        || variables.contains_key(crate::runtime::apptainer::IMAGE_ENV)
    {
        variables.insert(
            "METEWAND_OCI_ROOT".into(),
            private_root.to_string_lossy().into_owned(),
        );
    }
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .env_clear()
        .envs(&variables)
        .current_dir(private_working_directory.path());

    if let Some(language) = variables.get("METEWAND_ADAPTER_LANGUAGE") {
        let path =
            crate::runtime::language_adapter::stage(language, private_working_directory.path())
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
    #[cfg(target_os = "linux")]
    let provenance = {
        let mut provenance = provenance;
        if launch
            .environment_variables
            .contains_key(crate::runtime::oci::IMAGE_ENV)
            || launch
                .environment_variables
                .contains_key(crate::runtime::apptainer::IMAGE_ENV)
        {
            provenance.version = 3;
            provenance.umask = None;
            for name in [
                crate::runtime::oci::IMAGE_ENV,
                crate::runtime::apptainer::IMAGE_ENV,
                "METEWAND_OCI_SOURCE_ROOT",
            ] {
                provenance.environment_variables.remove(name);
            }
        }
        provenance
    };
    process.retain_environment(provenance);
    process.retain_private_working_directory(private_working_directory);
    Ok(process)
}

#[cfg(windows)]
fn interpreter_directory(path: std::path::PathBuf) -> std::path::PathBuf {
    use std::{ffi::OsString, path::Prefix};

    let mut components = path.components();
    let Some(std::path::Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let root = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => OsString::from(format!("{}:\\", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => {
            let mut root = OsString::from("\\\\");
            root.push(server);
            root.push("\\");
            root.push(share);
            root.push("\\");
            root
        }
        _ => return path,
    };
    let mut result = std::path::PathBuf::from(root);
    for component in components {
        if let std::path::Component::Normal(name) = component {
            result.push(name);
        }
    }
    result
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
