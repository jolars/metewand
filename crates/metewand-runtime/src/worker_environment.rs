//! Deterministic worker settings and ephemeral, redacted secret bindings.

use std::{collections::BTreeMap, fmt, path::PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::worker_process::{PROTOCOL_READ_FD_ENV, PROTOCOL_WRITE_FD_ENV};

/// Declaration of a secret environment channel in a resolved launch.
pub const SECRET_TEMPLATE: &str = "{secret}";

/// Fixed child umask, applied after fork without changing the orchestrator.
pub const WORKER_UMASK: libc::mode_t = 0o077;

/// Returns the stable, path-independent environment supplied to every worker.
#[must_use]
pub fn worker_environment_template() -> BTreeMap<String, String> {
    [
        ("LANG", "C"),
        ("LC_ALL", "C"),
        ("TZ", "UTC"),
        ("HOME", "{worker}/home"),
        ("TMPDIR", "{worker}/tmp"),
        ("TMP", "{worker}/tmp"),
        ("TEMP", "{worker}/tmp"),
        ("XDG_CONFIG_HOME", "{worker}/config"),
        ("XDG_CACHE_HOME", "{worker}/cache"),
        ("XDG_DATA_HOME", "{worker}/data"),
        ("XDG_STATE_HOME", "{worker}/state"),
        ("XDG_RUNTIME_DIR", "{worker}/runtime"),
        ("PYTHONUSERBASE", "{worker}/data/python"),
        ("PYTHONNOUSERSITE", "1"),
        ("PYTHONDONTWRITEBYTECODE", "1"),
        ("PIP_CACHE_DIR", "{worker}/cache/pip"),
        ("UV_CACHE_DIR", "{worker}/cache/uv"),
        ("R_USER", "{worker}/home"),
        ("R_LIBS_USER", "{worker}/cache/r/library"),
        ("JULIA_DEPOT_PATH", "{worker}/cache/julia"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect()
}

/// Validates an allowlist and fills in the mandatory worker settings.
///
/// # Errors
/// Returns an error for malformed entries or conflicting runtime-owned values.
pub fn resolve_worker_environment(
    allowlist: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, WorkerEnvironmentError> {
    let mut variables = worker_environment_template();
    for (name, value) in allowlist {
        validate_name(name)?;
        if value.contains('\0') {
            return Err(WorkerEnvironmentError::InvalidValue);
        }
        if let Some(relative) = value.strip_prefix("{worker}/")
            && relative
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(WorkerEnvironmentError::InvalidPath);
        }
        if matches!(name.as_str(), PROTOCOL_READ_FD_ENV | PROTOCOL_WRITE_FD_ENV)
            || variables.get(name).is_some_and(|fixed| fixed != value)
        {
            return Err(WorkerEnvironmentError::ReservedVariable { name: name.clone() });
        }
        variables.insert(name.clone(), value.clone());
    }
    Ok(variables)
}

/// Ephemeral values for explicitly declared `{secret}` environment channels.
///
/// This type deliberately has no serialization or identity implementation.
/// Its debug representation exposes channel names only.
#[derive(Default)]
pub struct WorkerSecrets {
    pub(crate) values: BTreeMap<String, String>,
}

impl WorkerSecrets {
    /// Admits nonempty, NUL-free secret values without reading ambient variables.
    ///
    /// # Errors
    /// Returns a value-free error for invalid names or secret values.
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, WorkerEnvironmentError> {
        for (name, value) in &values {
            validate_name(name)?;
            if value.is_empty() || value.contains('\0') {
                return Err(WorkerEnvironmentError::InvalidSecret);
            }
        }
        Ok(Self { values })
    }

    pub(crate) fn validate(
        &self,
        variables: &BTreeMap<String, String>,
    ) -> Result<(), WorkerEnvironmentError> {
        if variables
            .iter()
            .any(|(name, value)| (value == SECRET_TEMPLATE) != self.values.contains_key(name))
            || self.values.keys().any(|name| !variables.contains_key(name))
        {
            return Err(WorkerEnvironmentError::SecretChannels);
        }
        Ok(())
    }
}

impl fmt::Debug for WorkerSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerSecrets")
            .field("channels", &self.values.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// Actual worker settings retained as attempt provenance, never as identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkerEnvironmentProvenance {
    /// Compatibility version of this provenance record.
    pub version: u32,
    /// Allocated current directory.
    pub working_directory: PathBuf,
    /// Realized values with secret values replaced by `{secret}`.
    pub environment_variables: BTreeMap<String, String>,
    /// Fixed octal umask.
    pub umask: String,
}

/// Invalid worker environment, with no secret values in diagnostics.
#[derive(Debug, Error)]
pub enum WorkerEnvironmentError {
    /// Variable names must be portable environment identifiers.
    #[error("worker environment names must be nonempty ASCII identifiers")]
    InvalidName,
    /// Ordinary values cannot contain NUL.
    #[error("worker environment values must not contain NUL")]
    InvalidValue,
    /// Runtime settings cannot be overridden by an allowlist.
    #[error("worker environment variable `{name}` is reserved by the runtime")]
    ReservedVariable {
        /// Public variable name.
        name: String,
    },
    /// Empty secrets cannot be reliably redacted.
    #[error("worker secrets must be nonempty and must not contain NUL")]
    InvalidSecret,
    /// Values and declarations must have exactly the same channel names.
    #[error("worker secrets must match the declared secret channels exactly")]
    SecretChannels,
    /// Private paths require an absolute root and normalized directory templates.
    #[error("worker storage requires an absolute UTF-8 root and normalized directory templates")]
    InvalidPath,
}

fn validate_name(name: &str) -> Result<(), WorkerEnvironmentError> {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(WorkerEnvironmentError::InvalidName);
    }
    Ok(())
}
