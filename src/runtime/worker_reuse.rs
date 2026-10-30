//! Serial worker ownership, slot cleanup, and process-local warm-up accounting.

use std::{fs, io};

use crate::core::{
    manifest::TimingScope,
    records::{
        ExecutionPolicyRecord, ImplementationDefinitionRecord, RecordId,
        ResolvedOneShotSpecificationRecord, ResolvedWorkerRecord,
    },
};
use crate::protocol::WorkerRole;
use thiserror::Error;

use crate::runtime::worker_session::{WorkerSession, WorkerSessionError};

/// Complete eligibility key for retaining a worker between logical slots.
///
/// Resolved worker dependencies include source bundle, environment, and exact
/// launch identities. Comparing the complete policy also separates timeout,
/// resource, timing, and warm-up semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerReuseKey {
    /// Negotiated operations must match before a session can be retained.
    pub selected_capabilities: Vec<crate::protocol::Capability>,
    /// Protocol role selected before startup.
    pub role: WorkerRole,
    /// Authored worker definition, including arguments and source declarations.
    pub definition: RecordId<ImplementationDefinitionRecord>,
    /// Immutable resolved source, environment, and launch identities.
    pub resolved: ResolvedWorkerRecord,
    /// Complete effective execution policy.
    pub policy: ExecutionPolicyRecord,
}

/// Serial owner of at most one reusable implementation process.
///
/// Version 1 permits process-local state only as governed by `reset` and
/// `prepare`; no writable filesystem cache survives a slot boundary. Warm-ups
/// use the same slot callback contract as measurements, so callers must prepare
/// clean state and validate each warm-up before returning success.
#[derive(Debug, Default)]
pub struct ReusableWorker {
    retained: Option<(
        WorkerReuseKey,
        Option<RecordId<ResolvedOneShotSpecificationRecord>>,
        WorkerSession,
    )>,
}

impl ReusableWorker {
    /// Runs all required warm-ups on a new worker, followed by one measured slot.
    ///
    /// `launch` must establish the identity and dependencies in `key`. Each
    /// callback must run a complete slot, including `prepare`, output validation,
    /// and evaluation. Warm-up indices select the planned warm-up seeds and
    /// records; they restart at zero after replacement. Failures return to the
    /// scheduler without retrying a measured slot or manufacturing success.
    ///
    /// # Errors
    /// Returns invalid policy, launch, reset, cleanup, warm-up, or slot failures.
    pub fn run_slot<T, E: std::error::Error + 'static>(
        &mut self,
        key: WorkerReuseKey,
        launch: impl FnOnce() -> Result<WorkerSession, E>,
        warmup: impl FnMut(&mut WorkerSession, u64) -> Result<(), E>,
        measured: impl FnOnce(&mut WorkerSession) -> Result<T, E>,
    ) -> Result<T, WorkerReuseError<E>> {
        self.run_slot_scoped(key, None, launch, warmup, measured)
    }

    /// Runs the warm-ups planned for a specification while retaining an eligible worker.
    ///
    /// A new specification repeats its warm-ups on the same process after reset.
    /// Replacement always repeats the current specification's warm-ups as well.
    ///
    /// # Errors
    /// Returns the same lifecycle and slot failures as [`Self::run_slot`].
    pub fn run_slot_for_specification<T, E: std::error::Error + 'static>(
        &mut self,
        key: WorkerReuseKey,
        specification: RecordId<ResolvedOneShotSpecificationRecord>,
        launch: impl FnOnce() -> Result<WorkerSession, E>,
        warmup: impl FnMut(&mut WorkerSession, u64) -> Result<(), E>,
        measured: impl FnOnce(&mut WorkerSession) -> Result<T, E>,
    ) -> Result<T, WorkerReuseError<E>> {
        self.run_slot_scoped(key, Some(specification), launch, warmup, measured)
    }

    fn run_slot_scoped<T, E: std::error::Error + 'static>(
        &mut self,
        key: WorkerReuseKey,
        specification: Option<RecordId<ResolvedOneShotSpecificationRecord>>,
        launch: impl FnOnce() -> Result<WorkerSession, E>,
        mut warmup: impl FnMut(&mut WorkerSession, u64) -> Result<(), E>,
        measured: impl FnOnce(&mut WorkerSession) -> Result<T, E>,
    ) -> Result<T, WorkerReuseError<E>> {
        if key.role != WorkerRole::Implementation
            || (key.policy.warmup_runs > 0 && !key.policy.worker_reuse)
            || (key.policy.timing_scope == TimingScope::ColdEndToEnd
                && (key.policy.worker_reuse || key.policy.warmup_runs > 0))
        {
            return Err(WorkerReuseError::InvalidPolicy);
        }
        // Taking ownership ensures every error path drops the process rather
        // than leaving a possibly dirty worker available to a later slot.
        let previous = self.retained.take();
        let (mut session, needs_warmups) = match previous {
            Some((previous_key, previous_specification, mut session))
                if key.policy.worker_reuse && previous_key == key && session.is_usable() =>
            {
                reset_and_clear::<E>(&mut session)?;
                (session, previous_specification != specification)
            }
            previous => {
                drop(previous);
                (
                    launch().map_err(|source| WorkerReuseError::Launch { source })?,
                    true,
                )
            }
        };
        if needs_warmups {
            for index in 0..key.policy.warmup_runs {
                warmup(&mut session, index)
                    .map_err(|source| WorkerReuseError::Warmup { index, source })?;
                reset_and_clear::<E>(&mut session)?;
            }
        }
        let output = measured(&mut session).map_err(|source| WorkerReuseError::Slot { source })?;
        if key.policy.worker_reuse && session.is_usable() {
            self.retained = Some((key, specification, session));
        }
        Ok(output)
    }

    /// Discards a retained process after an external validation or integrity failure.
    pub fn discard(&mut self) {
        self.retained.take();
    }
}

fn reset_and_clear<E: std::error::Error + 'static>(
    session: &mut WorkerSession,
) -> Result<(), WorkerReuseError<E>> {
    // A scientific deadline belongs to the previous slot; reset has its own
    // operational deadline and lies outside the next timing envelope.
    session.set_scientific_deadline(None);
    session.reset()?;
    let root = session
        .private_working_directory()
        .ok_or(WorkerReuseError::MissingPrivateDirectory)?;
    // A worker can change its directory mode; restore the executor's private
    // boundary before making it available to another slot.
    crate::runtime::platform_fs::set_private_directory(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if file_type.is_symlink_dir() {
                // Windows removes directory symlinks and junctions through
                // the directory API, which leaves their targets untouched.
                fs::remove_dir(entry.path())?;
                continue;
            }
        }
        if file_type.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            // Removing the link itself prevents following worker-created links.
            fs::remove_file(entry.path())?;
        }
    }
    // Recreate both mandatory and allowlisted private paths using their
    // realized values, without creating anything outside this worker's root.
    let environment = session
        .environment()
        .ok_or(WorkerReuseError::MissingPrivateDirectory)?;
    for value in environment.environment_variables.values() {
        let path = std::path::Path::new(value);
        if path
            .strip_prefix(root)
            .is_ok_and(|relative| !relative.as_os_str().is_empty())
        {
            fs::create_dir_all(path)?;
            crate::runtime::platform_fs::set_private_directory(path)?;
        }
    }
    Ok(())
}

/// Failure that prevents a worker from being retained for another slot.
#[derive(Debug, Error)]
pub enum WorkerReuseError<E: std::error::Error + 'static = WorkerSessionError> {
    /// Reuse and warm-ups must satisfy the version-1 timing contract.
    #[error("invalid worker reuse role or timing policy")]
    InvalidPolicy,
    /// Reset and cleanup require an executor-owned private directory.
    #[error("reused worker has no private working directory")]
    MissingPrivateDirectory,
    /// A private directory could not be cleared or recreated.
    #[error("worker slot cleanup failed: {0}")]
    Cleanup(#[from] io::Error),
    /// Protocol, timeout, crash, or reset failure.
    #[error(transparent)]
    Session(#[from] WorkerSessionError),
    /// A replacement's process-local warm-up failed.
    #[error("worker warm-up {index} failed: {source}")]
    Warmup {
        /// Zero-based warm-up index on this process.
        index: u64,
        /// Typed underlying worker failure.
        source: E,
    },
    /// A replacement process could not be launched and negotiated.
    #[error("worker launch failed: {source}")]
    Launch {
        /// Caller-owned typed launch failure.
        source: E,
    },
    /// Execution, validation, or evaluation of the requested measured slot failed.
    #[error("worker slot failed: {source}")]
    Slot {
        /// Caller-owned typed slot failure.
        source: E,
    },
}
