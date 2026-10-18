use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use metewand_core::{canonical::CanonicalValue, records::ContentDigest};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{
    ArtifactRegistration, OwnedAttempt, RUN_STORE_SCHEMA_VERSION, RunStore, RunStoreError,
    TerminalAttempt, TerminalOutcome,
    attempts::{commit_finalization_in, fence, invalid, timestamp, validate_finalization},
};
use crate::{
    local_tree::hash_local_tree,
    publication::{
        prepare_directory, publish_prepared_directory, verify_finalized_directory, write_file,
    },
};

/// Directory artifact kinds that can be published independently of completion.
///
/// Observation publication here registers bytes only; the trusted orchestrator
/// must separately validate and transactionally register its record and selection.
#[derive(Clone, Copy, Debug)]
pub enum ArtifactKind {
    Observation,
    Result,
    Metrics,
    Dataset,
    Source,
    Environment,
    Other,
}

impl ArtifactKind {
    fn name(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Result => "result",
            Self::Metrics => "metrics",
            Self::Dataset => "dataset",
            Self::Source => "source",
            Self::Environment => "environment",
            Self::Other => "other",
        }
    }
}

/// Verified bytes whose database registration has committed.
#[derive(Debug)]
pub struct PublishedArtifact {
    pub artifact: ArtifactRegistration,
}

/// Terminal completion returned only after durable publication and database commit.
#[derive(Debug)]
pub struct PublishedFinalization {
    pub artifact: ArtifactRegistration,
    pub terminal: TerminalAttempt,
}

// Keep the kernel lock outside the object inventory and across rename and commit.
// Registered stages survive errors for explicit recovery rather than disappearing
// while their database leases still describe an unresolved publication.
struct Stage {
    directory: PathBuf,
    object: PathBuf,
    lease_id: String,
    acquired_at_ns: u64,
    _lock: File,
}

impl RunStore {
    /// Publishes a complete artifact and protects it with the reservation lease.
    ///
    /// `populate` writes only beneath the supplied private directory and must
    /// relinquish every writer before returning. It must perform any required
    /// domain validation; this API validates the complete filesystem inventory.
    /// The publisher owns `manifest.json`. `clock` returns current epoch
    /// nanoseconds before staging and again after publication, so time spent
    /// copying, validating, or synchronizing cannot extend an expired lease.
    /// No database transaction stays open during `populate` or filesystem work.
    ///
    /// # Errors
    /// Rejects stale ownership, unsafe entries, conflicting content, expired
    /// leases, and synchronization, rename, population, or database failures.
    pub fn publish_artifact(
        &mut self,
        attempt: &OwnedAttempt,
        kind: ArtifactKind,
        populate: impl FnOnce(&Path) -> io::Result<()>,
        mut clock: impl FnMut() -> u64,
    ) -> Result<PublishedArtifact, RunStoreError> {
        let stage = self.stage(attempt, clock())?;
        populate(&stage.object)?;
        let mut artifact = self.publish_stage(&stage, kind.name())?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock().max(stage.acquired_at_ns);
        artifact.verified_at_ns = now;
        check_stage(&tx, attempt, &stage, now)?;
        register_artifact(&tx, &artifact, kind.name())?;
        tx.execute(
            "INSERT OR IGNORE INTO leased_artifacts VALUES (?1, ?2)",
            params![attempt.lease_id, artifact.id],
        )?;
        release_stage(&tx, &stage, now)?;
        tx.commit()?;
        stage.cleanup();
        Ok(PublishedArtifact { artifact })
    }

    /// Publishes the claimed terminal outcome before its final database transaction.
    ///
    /// Enter finalizing first, using `begin_finalization` for acceptance. Honor
    /// its claim decision when choosing `outcome`. The caller must validate the
    /// scientific completion rule and all transitive dependencies before calling.
    /// `populate` may add diagnostic files or public records, but must relinquish
    /// every writer and may not write `outcome.json` or `manifest.json`.
    ///
    /// The runtime writes a canonical, versioned coordination outcome binding
    /// this reservation, revision, outcome, and dependency metadata. It validates
    /// the complete inventory, synchronizes every file and directory, publishes
    /// with atomic no-replace rename, and synchronizes both rename parents.
    /// Only then does a transaction recheck ownership, staging lease, claim,
    /// observation acceptance, and references and commit the terminal state.
    /// `clock` is sampled before staging, before writing the outcome, and after
    /// publication, after acquiring the final database write lock. The immutable
    /// finish time is the second sample, clamped to staging acquisition and the
    /// preceding transition. The final sample fences the commit against expiry.
    ///
    /// # Errors
    /// Rejects invalid outcomes or references, stale or expired owners, missing
    /// success claims, unsafe or conflicting content, and filesystem or database
    /// failures. Errors can leave leased stages or complete orphans, never a
    /// terminal receipt. Recovery is a separate operation.
    pub fn publish_finalization(
        &mut self,
        attempt: &OwnedAttempt,
        outcome: TerminalOutcome,
        references: &[(String, String)],
        populate: impl FnOnce(&Path) -> io::Result<()>,
        mut clock: impl FnMut() -> u64,
    ) -> Result<PublishedFinalization, RunStoreError> {
        outcome.details()?;
        let stage = self.stage(attempt, clock())?;
        populate(&stage.object)?;
        let finished = clock().max(stage.acquired_at_ns);
        let document = self.outcome_document(attempt, &outcome, references, finished, &stage)?;
        write_file(
            &stage.object.join("outcome.json"),
            &document.to_canonical_bytes()?,
        )?;
        let mut artifact = self.publish_stage(&stage, "attempt")?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock().max(finished);
        artifact.verified_at_ns = now;
        check_stage(&tx, attempt, &stage, now)?;
        if coordination_snapshot(&tx, attempt)? != document.as_json()["coordination"] {
            return Err(invalid("coordination records changed during publication"));
        }
        // Fencing uses the post-publication clock, while the terminal record and
        // projection share the immutable pre-publication finish timestamp.
        let terminal = commit_finalization_in(
            &tx,
            attempt,
            outcome,
            artifact.clone(),
            references,
            finished,
            now,
        )?;
        tx.commit()?;
        stage.cleanup();
        Ok(PublishedFinalization { artifact, terminal })
    }

    fn stage(&mut self, attempt: &OwnedAttempt, now_ns: u64) -> Result<Stage, RunStoreError> {
        let now = timestamp(now_ns)?.max(timestamp(attempt.updated_at_ns())?);
        let root = self
            .database_path()
            .parent()
            .expect("database has a root")
            .to_owned();
        let objects = root.join("artifacts");
        match fs::create_dir(&objects) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        if !fs::symlink_metadata(&objects)?.is_dir() {
            return Err(RunStoreError::UnsafePath(objects));
        }
        // Synchronize even an existing parent: another creator may not yet have
        // completed its own parent synchronization.
        File::open(&root)?.sync_all()?;
        let directory = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&objects)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join("lock"))?;
        lock.lock()?;
        lock.sync_all()?;
        let object = directory.path().join("object");
        fs::create_dir(&object)?;
        File::open(directory.path())?.sync_all()?;
        File::open(&objects)?.sync_all()?;
        let relative = directory
            .path()
            .strip_prefix(&root)
            .expect("stage is inside store")
            .to_str()
            .expect("generated staging path is UTF-8");
        let lease_id = format!("staging:{relative}");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        fence(&tx, attempt, now)?;
        tx.execute("INSERT INTO leases(id, owner_token, attempt_id, purpose, staging_path, acquired_at_ns, expires_at_ns)
            SELECT ?1, owner_token, attempt_id, 'staging', ?2, ?3, expires_at_ns FROM leases WHERE id = ?4",
            params![lease_id, relative, now, attempt.lease_id])?;
        tx.commit()?;
        let directory = directory.keep();
        Ok(Stage {
            directory,
            object,
            lease_id,
            acquired_at_ns: now as u64,
            _lock: lock,
        })
    }

    fn publish_stage(
        &self,
        stage: &Stage,
        kind: &str,
    ) -> Result<ArtifactRegistration, RunStoreError> {
        prepare_directory(&stage.object)?;
        let content =
            hash_local_tree(&stage.object).map_err(crate::publication::PublicationError::from)?;
        let manifest = ContentDigest::new(
            Sha256::digest(fs::read(stage.object.join("manifest.json"))?).into(),
        );
        let name = format!("{kind}-{}", hex(content));
        let parent = stage
            .directory
            .parent()
            .expect("stage has an artifact parent");
        let destination = parent.join(&name);
        publish_prepared_directory(&stage.object, &destination)?;
        verify_finalized_directory(&destination)?;
        if hash_local_tree(&destination).map_err(crate::publication::PublicationError::from)?
            != content
        {
            return Err(crate::publication::PublicationError::Conflict(destination).into());
        }
        Ok(ArtifactRegistration {
            id: format!("run-store-v1:{name}"),
            relative_path: format!("artifacts/{name}"),
            content_sha256: content,
            manifest_sha256: manifest,
            verified_at_ns: 0,
        })
    }

    fn outcome_document(
        &mut self,
        attempt: &OwnedAttempt,
        outcome: &TerminalOutcome,
        references: &[(String, String)],
        finished: u64,
        stage: &Stage,
    ) -> Result<CanonicalValue, RunStoreError> {
        let now = timestamp(finished)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_stage(&tx, attempt, stage, finished)?;
        validate_finalization(&tx, attempt, outcome, now)?;
        let mut references = references.to_vec();
        references.sort();
        if references.windows(2).any(|pair| pair[0] == pair[1])
            || references
                .iter()
                .any(|(role, _)| role.trim().is_empty() || role == "final")
        {
            return Err(invalid(
                "dependency roles must be nonempty, non-final, and unique per artifact",
            ));
        }
        let dependencies = references.iter().map(|(role, id)| {
            tx.query_row("SELECT kind, relative_path, content_sha256, storage_kind, manifest_sha256 FROM artifacts WHERE id = ?1",
                [id], |r| Ok(json!({"role":role, "id":id, "kind":r.get::<_, String>(0)?,
                    "relative_path":r.get::<_, String>(1)?, "content_sha256":r.get::<_, String>(2)?,
                    "storage_kind":r.get::<_, String>(3)?, "manifest_sha256":r.get::<_, Option<String>>(4)?})))
                .map_err(RunStoreError::from)
        }).collect::<Result<Vec<_>, _>>()?;
        let started: i64 = tx.query_row(
            "SELECT started_at_ns FROM attempts WHERE id = ?1",
            [attempt.id().to_string()],
            |r| r.get(0),
        )?;
        let coordination = coordination_snapshot(&tx, attempt)?;
        let document = CanonicalValue::try_from(json!({
            "run_store_version": RUN_STORE_SCHEMA_VERSION,
            "attempt_id": attempt.id(), "slot_id":attempt.slot(), "retry_index":attempt.retry_index(),
            "finalizing_revision":attempt.revision(), "state":outcome.state(), "details":outcome.details()?,
            "started_at_ns":started.to_string(), "finished_at_ns":finished.to_string(), "references":dependencies,
            "coordination": coordination,
        }))?;
        tx.commit()?;
        Ok(document)
    }
}

fn coordination_snapshot(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
) -> Result<serde_json::Value, RunStoreError> {
    fn rows(
        tx: &Transaction<'_>,
        sql: &str,
        id: &str,
    ) -> Result<Vec<serde_json::Value>, RunStoreError> {
        let mut query = tx.prepare(sql)?;
        let documents = query
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        documents
            .iter()
            .map(|text| {
                Ok(CanonicalValue::from_slice(text.as_bytes())?
                    .as_json()
                    .clone())
            })
            .collect()
    }
    let id = attempt.id().to_string();
    Ok(json!({
        "attempt_slots": rows(tx, "SELECT json_object('id', s.id, 'logical_slot_id', s.logical_slot_id, 'resolved_run_id', s.resolved_run_id, 'kind', s.kind, 'role', s.role, 'record', json(s.record_json)) FROM attempt_slots s JOIN attempts a ON a.slot_id = s.id WHERE a.id = ?1", &id)?,
        "resolved_runs": rows(tx, "SELECT json_object('id', r.id, 'logical_specification_id', r.logical_specification_id, 'candidate_id', r.candidate_id, 'applicability_id', r.applicability_id, 'record', json(r.record_json)) FROM resolved_runs r JOIN attempt_slots s ON s.resolved_run_id = r.id JOIN attempts a ON a.slot_id = s.id WHERE a.id = ?1", &id)?,
        "applicability_decisions": rows(tx, "SELECT json_object('id', d.id, 'candidate_id', d.candidate_id, 'applicable', d.applicable, 'record', json(d.record_json)) FROM applicability_decisions d JOIN resolved_runs r ON r.applicability_id = d.id JOIN attempt_slots s ON s.resolved_run_id = r.id JOIN attempts a ON a.slot_id = s.id WHERE a.id = ?1", &id)?,
        "observation_slots": rows(tx, "SELECT json_object('id', o.id, 'logical_slot_id', o.logical_slot_id, 'resolved_run_id', o.resolved_run_id, 'record', json(o.record_json)) FROM observation_slots o JOIN attempt_observation_slots m ON m.observation_slot_id = o.id JOIN attempts a ON a.slot_id = m.attempt_slot_id WHERE a.id = ?1 ORDER BY o.id", &id)?,
        "observations": rows(tx, "SELECT json_object('id', o.id, 'slot_id', o.slot_id, 'producing_attempt_id', o.producing_attempt_id, 'valid', o.valid, 'completion', o.completion, 'artifact_id', o.artifact_id, 'record', json(o.record_json), 'selected_at_ns', CAST(s.selected_at_ns AS TEXT)) FROM observations o LEFT JOIN selected_observations s ON s.observation_id = o.id WHERE o.producing_attempt_id = ?1 OR (s.observation_id IS NOT NULL AND o.slot_id IN (SELECT observation_slot_id FROM attempt_observation_slots m JOIN attempts a ON a.slot_id = m.attempt_slot_id WHERE a.id = ?1)) ORDER BY o.id", &id)?,
    }))
}

impl Stage {
    fn cleanup(&self) {
        // Cleanup cannot revoke a committed receipt. Unremoved released staging
        // directories carry no accepted identity and can be removed by recovery.
        if fs::remove_dir_all(&self.directory).is_ok() {
            let _ = File::open(self.directory.parent().expect("stage has a parent"))
                .and_then(|parent| parent.sync_all());
        }
    }
}

fn check_stage(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    stage: &Stage,
    now: u64,
) -> Result<(), RunStoreError> {
    let now = timestamp(now)?;
    fence(tx, attempt, now)?;
    let live: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM leases WHERE id = ?1 AND attempt_id = ?2
        AND owner_token = ?3 AND purpose = 'staging' AND released_at_ns IS NULL AND expires_at_ns > ?4)",
        params![stage.lease_id, attempt.id().to_string(), attempt.owner_token, now], |r| r.get(0))?;
    if !live {
        return Err(RunStoreError::StaleAttempt);
    }
    Ok(())
}

fn release_stage(tx: &Transaction<'_>, stage: &Stage, now: u64) -> Result<(), RunStoreError> {
    tx.execute(
        "UPDATE leases SET released_at_ns = ?2 WHERE id = ?1 AND released_at_ns IS NULL",
        params![stage.lease_id, timestamp(now)?],
    )?;
    Ok(())
}

fn register_artifact(
    tx: &Transaction<'_>,
    artifact: &ArtifactRegistration,
    kind: &str,
) -> Result<(), RunStoreError> {
    tx.execute("INSERT INTO artifacts VALUES (?1, ?2, ?3, ?4, 'directory', ?5, ?6) ON CONFLICT(id) DO NOTHING",
        params![artifact.id, kind, artifact.relative_path, hex(artifact.content_sha256), hex(artifact.manifest_sha256), timestamp(artifact.verified_at_ns)?])?;
    let existing: Option<(String, String, String, String, Option<String>)> = tx.query_row(
        "SELECT kind, relative_path, content_sha256, storage_kind, manifest_sha256 FROM artifacts WHERE id = ?1",
        [&artifact.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
    if existing
        != Some((
            kind.to_owned(),
            artifact.relative_path.clone(),
            hex(artifact.content_sha256),
            "directory".to_owned(),
            Some(hex(artifact.manifest_sha256)),
        ))
    {
        return Err(invalid(
            "registered artifact metadata conflicts with published content",
        ));
    }
    Ok(())
}

fn hex(digest: ContentDigest) -> String {
    digest
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
