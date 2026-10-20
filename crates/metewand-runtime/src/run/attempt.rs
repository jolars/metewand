use std::{
    io,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use metewand_core::{
    canonical::CanonicalValue,
    records::{RunAttemptOutcome, RunAttemptRecord},
    run_documents::attempt_document,
};

use crate::{
    fixed_dataset_run::FixedDatasetResult,
    publication::{PublicationRequest, copy_prepared, prepare_fixed_dataset},
    run_store::{
        ArtifactKind, AttemptPhase, ClaimDisposition, LeaseRequest, OwnedAttempt,
        RetryAuthorization, RunStore, RunStoreError, TerminalOutcome,
    },
};

use super::{RunError, resolution::ResolvedMeasurement};

const LEASE_DURATION_NS: u64 = 30_000_000_000;

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock follows epoch")
        .as_nanos()
        .try_into()
        .expect("epoch nanoseconds fit u64")
}

pub(super) struct AttemptExecution {
    store: RunStore,
    owned: Arc<Mutex<OwnedAttempt>>,
    owner: String,
    stop: mpsc::Sender<()>,
    heartbeat: Option<thread::JoinHandle<Result<(), RunStoreError>>>,
}

impl AttemptExecution {
    pub fn reserve(
        store: &RunStore,
        measurement: &ResolvedMeasurement,
        retry: bool,
    ) -> Result<Option<Self>, RunError> {
        let mut store = store.reconnect()?;
        let token = tempfile::Builder::new()
            .prefix("metewand-owner-")
            .tempdir()?;
        let owner = token.path().to_string_lossy().into_owned();
        let started = now();
        let owned = match store.reserve(
            measurement.attempt_slot.id,
            RetryAuthorization::Next { retry },
            LeaseRequest {
                id: format!("reservation:{owner}"),
                owner_token: owner.clone(),
                expires_at_ns: started + LEASE_DURATION_NS,
            },
            started,
        ) {
            Ok(owned) => owned,
            Err(RunStoreError::SatisfiedSlot | RunStoreError::RetryNotAuthorized) => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let owned = Arc::new(Mutex::new(owned));
        let mut heartbeat_store = store.reconnect()?;
        let heartbeat_owned = owned.clone();
        let (stop, receiver) = mpsc::channel();
        let heartbeat = thread::Builder::new()
            .name("metewand-lease".into())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(Duration::from_secs(5)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let owned = heartbeat_owned.lock().expect("lease owner lock");
                            let current = now();
                            if let Err(error) =
                                heartbeat_store.renew(&owned, current + LEASE_DURATION_NS, current)
                            {
                                if heartbeat_store.attempt_is_terminal(owned.id())? {
                                    return Ok(());
                                }
                                return Err(error);
                            }
                        }
                    }
                }
            })?;
        Ok(Some(Self {
            store,
            owned,
            owner,
            stop,
            heartbeat: Some(heartbeat),
        }))
    }

    pub fn output_root(&self) -> std::path::PathBuf {
        self.store
            .database_path()
            .parent()
            .expect("store root")
            .to_owned()
    }

    pub fn retry_index(&self) -> u64 {
        self.owned.lock().expect("lease owner lock").retry_index()
    }

    pub fn advance(&mut self, phase: AttemptPhase) -> Result<(), RunStoreError> {
        let mut owned = self.owned.lock().expect("lease owner lock");
        *owned = self.store.advance(&owned, phase, now())?;
        Ok(())
    }

    pub fn publish_result(
        &mut self,
        result: &FixedDatasetResult,
        request: PublicationRequest<'_>,
    ) -> Result<(crate::publication::PublishedOneShot, String, String), RunError> {
        let mut prepared = prepare_fixed_dataset(result, request)?;
        let success = matches!(
            prepared.attempt.record.outcome,
            RunAttemptOutcome::Accepted { .. }
        );
        let superseded = if success {
            let mut owned = self.owned.lock().expect("lease owner lock");
            let current = now();
            let finalization = self.store.begin_finalization(
                &owned,
                LeaseRequest {
                    id: format!("finalization:{}", self.owner),
                    owner_token: self.owner.clone(),
                    expires_at_ns: current + LEASE_DURATION_NS,
                },
                current,
            )?;
            *owned = finalization.attempt;
            finalization.disposition == ClaimDisposition::Superseded
        } else {
            self.advance(AttemptPhase::Finalizing)?;
            false
        };
        if superseded {
            prepared.attempt.record.outcome = RunAttemptOutcome::Superseded {
                observation: Some(prepared.observation.id),
            };
        }
        let owned = self.owned.lock().expect("lease owner lock").clone();
        let observation = self.store.publish_artifact(
            &owned,
            ArtifactKind::Observation,
            |path| copy_prepared(prepared.observation_files.path(), path),
            now,
        )?;
        self.store.register_observation(
            &owned,
            &prepared.observation,
            &observation.artifact,
            success && !superseded,
            now(),
        )?;
        let outcome = terminal(&prepared.attempt.record.outcome)?;
        let document = attempt_document(&prepared.attempt.record)
            .map_err(io::Error::other)?
            .to_canonical_bytes()?;
        let published = self.store.publish_finalization(
            &owned,
            outcome,
            &[("observation".into(), observation.artifact.id)],
            |path| {
                copy_prepared(prepared.attempt_files.path(), path)?;
                std::fs::write(path.join("attempt.json"), &document)
            },
            now,
        )?;
        self.finish_heartbeat()?;
        let attempt_path = published.artifact.relative_path;
        let observation_path = observation.artifact.relative_path;
        let root = self.store.database_path().parent().expect("store root");
        Ok((
            crate::publication::PublishedOneShot {
                observation: prepared.observation,
                attempt: prepared.attempt,
                attempt_directory: root.join(&attempt_path),
                observation_directory: root.join(&observation_path),
            },
            attempt_path,
            observation_path,
        ))
    }

    pub fn publish_failure(&mut self, record: &RunAttemptRecord) -> Result<String, RunError> {
        self.advance(AttemptPhase::Finalizing)?;
        let owned = self.owned.lock().expect("lease owner lock").clone();
        let document = attempt_document(record)
            .map_err(io::Error::other)?
            .to_canonical_bytes()?;
        let published = self.store.publish_finalization(
            &owned,
            terminal(&record.outcome)?,
            &[],
            |path| std::fs::write(path.join("attempt.json"), &document),
            now,
        )?;
        self.finish_heartbeat()?;
        Ok(published.artifact.relative_path)
    }

    fn finish_heartbeat(&mut self) -> Result<(), RunError> {
        let _ = self.stop.send(());
        if let Some(handle) = self.heartbeat.take() {
            handle
                .join()
                .map_err(|_| RunError::ExecutionThreadPanic)??;
        }
        Ok(())
    }
}

impl Drop for AttemptExecution {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(handle) = self.heartbeat.take() {
            let _ = handle.join();
        }
    }
}

fn terminal(outcome: &RunAttemptOutcome) -> Result<TerminalOutcome, RunError> {
    Ok(match outcome {
        RunAttemptOutcome::Accepted { .. } => TerminalOutcome::Accepted,
        RunAttemptOutcome::Superseded { .. } => TerminalOutcome::Superseded,
        RunAttemptOutcome::Failed { diagnostic, .. } => TerminalOutcome::Failed {
            code: diagnostic.code.clone(),
            details: CanonicalValue::try_from(
                serde_json::json!({"message": diagnostic.message, "details": diagnostic.details.as_ref().map(CanonicalValue::as_json)}),
            )?,
        },
    })
}
