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
    profile_references: Vec<(String, String)>,
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
            profile_references: vec![],
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

    pub fn reserve_fresh_time(
        &mut self,
        limit: Option<Duration>,
    ) -> Result<Option<Duration>, RunError> {
        let owned = self.owned.lock().expect("lease owner lock");
        Ok(self
            .store
            .reserve_fresh_time(
                &owned,
                limit.and_then(|v| u64::try_from(v.as_nanos()).ok()),
                now(),
            )?
            .map(Duration::from_nanos))
    }

    pub fn record_fresh_time(
        &mut self,
        measurements: &metewand_core::measurements::ExecutionMeasurements,
    ) -> Result<(), RunError> {
        use metewand_core::measurements::Measurement;
        let measured = match measurements.timed_wall_time_ns {
            Measurement::Complete { value } | Measurement::Partial { value, .. } => value,
            Measurement::Unavailable {
                reason: metewand_core::measurements::MissingReason::NotStarted,
            } => 0,
            Measurement::Unavailable { .. } => return Ok(()),
        };
        let owned = self.owned.lock().expect("lease owner lock");
        self.store.record_fresh_time(&owned, measured, now())?;
        Ok(())
    }

    pub fn fresh_budget_exhausted(&self) -> Result<bool, RunError> {
        let owned = self.owned.lock().expect("lease owner lock");
        Ok(self.store.fresh_budget_exhausted(&owned)?)
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
            success
                && !superseded
                && !matches!(
                    prepared.observation.record.profile,
                    Some(metewand_core::records::ProfileObservationRecord::Fresh(_))
                ),
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

    pub fn publish_buffer(
        &mut self,
        directory: &std::path::Path,
    ) -> Result<serde_json::Value, RunError> {
        let owned = self.owned.lock().expect("lease owner lock").clone();
        let published = self.store.publish_artifact(
            &owned,
            ArtifactKind::Other,
            |path| copy_prepared(directory, path),
            now,
        )?;
        self.profile_references
            .push(("buffer".into(), published.artifact.id.clone()));
        Ok(
            serde_json::json!({"id":published.artifact.id,"directory":published.artifact.relative_path}),
        )
    }

    pub fn publish_profile_observation(
        &mut self,
        result: &FixedDatasetResult,
        request: PublicationRequest<'_>,
    ) -> Result<
        (
            metewand_core::records::IdentifiedRecord<metewand_core::records::RunObservationRecord>,
            String,
        ),
        RunError,
    > {
        let prepared = prepare_fixed_dataset(result, request)?;
        let owned = self.owned.lock().expect("lease owner lock").clone();
        let published = self.store.publish_artifact(
            &owned,
            ArtifactKind::Observation,
            |path| copy_prepared(prepared.observation_files.path(), path),
            now,
        )?;
        self.store.register_observation(
            &owned,
            &prepared.observation,
            &published.artifact,
            false,
            now(),
        )?;
        self.profile_references
            .push(("observation".into(), published.artifact.id));
        Ok((prepared.observation, published.artifact.relative_path))
    }

    pub fn publish_buffered(&mut self, record: &mut RunAttemptRecord) -> Result<String, RunError> {
        use metewand_core::records::ProfileAttemptStatus;
        let RunAttemptOutcome::Buffered { status, .. } = &mut record.outcome else {
            return Err(RunError::Unsupported("expected buffered outcome".into()));
        };
        if *status == ProfileAttemptStatus::Accepted {
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
            if finalization.disposition == ClaimDisposition::Superseded {
                *status = ProfileAttemptStatus::Superseded;
            }
        } else {
            self.advance(AttemptPhase::Finalizing)?;
        }
        let owned = self.owned.lock().expect("lease owner lock").clone();
        let document = attempt_document(record)
            .map_err(io::Error::other)?
            .to_canonical_bytes()?;
        let published = self.store.publish_finalization(
            &owned,
            terminal(&record.outcome)?,
            &self.profile_references,
            |path| std::fs::write(path.join("attempt.json"), &document),
            now,
        )?;
        self.finish_heartbeat()?;
        Ok(published.artifact.relative_path)
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
        RunAttemptOutcome::Buffered {
            status,
            reason,
            diagnostic,
            ..
        } => match status {
            metewand_core::records::ProfileAttemptStatus::Accepted => TerminalOutcome::Accepted,
            metewand_core::records::ProfileAttemptStatus::Censored => TerminalOutcome::Censored {
                reason: reason.clone(),
            },
            metewand_core::records::ProfileAttemptStatus::Superseded => TerminalOutcome::Superseded,
            metewand_core::records::ProfileAttemptStatus::Failed => TerminalOutcome::Failed {
                code: diagnostic
                    .as_ref()
                    .map_or("buffered_failure", |d| d.code.as_str())
                    .to_owned(),
                details: CanonicalValue::try_from(serde_json::json!({"message":reason}))?,
            },
        },
        RunAttemptOutcome::Censored { reason, .. } => TerminalOutcome::Censored {
            reason: reason.clone(),
        },
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
