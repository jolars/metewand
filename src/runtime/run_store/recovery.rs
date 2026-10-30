use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions, TryLockError},
    io,
    path::{Component, Path, PathBuf},
};

use crate::core::{canonical::CanonicalValue, identity::run_attempt_id, records::ContentDigest};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    ArtifactRegistration, OwnedAttempt, RunStore, RunStoreError, TerminalAttempt, TerminalOutcome,
    attempts::{commit_terminal_in, invalid, timestamp, transition, validate_acceptance},
    publisher::{build_outcome_document, register_artifact},
};
use crate::runtime::{
    local_tree::hash_local_tree,
    publication::{
        prepare_directory, publish_prepared_directory, rename_no_replace,
        verify_finalized_directory, write_file,
    },
};

/// Results of an explicit recovery pass, ordered by store path or attempt ID.
///
/// Imported artifacts establish byte integrity only. They never create an
/// observation or replace a selection. Rejected objects remain on disk for
/// diagnosis; quarantined stages are moved intact out of the publication area.
#[derive(Debug, Default)]
pub struct RecoveryReport {
    pub terminals: Vec<TerminalAttempt>,
    pub imported_artifacts: Vec<ArtifactRegistration>,
    pub quarantined_stages: Vec<PathBuf>,
    pub busy_stages: Vec<PathBuf>,
    pub rejected_artifacts: Vec<RejectedArtifact>,
}

#[derive(Debug)]
pub struct RejectedArtifact {
    pub relative_path: PathBuf,
    pub reason: String,
}

struct Orphan {
    artifact: ArtifactRegistration,
    document: CanonicalValue,
}

struct Stage {
    relative_path: PathBuf,
    lease_id: Option<String>,
    _lock: Option<File>,
}

struct StoredArtifact {
    artifact: ArtifactRegistration,
    storage_kind: String,
}

struct Evidence {
    outcome: TerminalOutcome,
    references: Vec<(String, String)>,
    finished: u64,
    roots: Vec<StoredArtifact>,
}

impl RunStore {
    /// Recovers expired attempts and abandoned publications in this store.
    ///
    /// This explicit mutation never runs during `open`. `clock` must supply
    /// current epoch nanoseconds in production. A pass serializes with other
    /// recovery passes and stage creation, skips kernel-locked stages, and
    /// rechecks expiration and revision under the database write lock. Live
    /// owners retain their stages and may resume through their existing guards.
    ///
    /// Only a verified, published outcome matching the authoritative attempt,
    /// coordination snapshot, dependencies, and success claim can be imported.
    /// Otherwise an expired attempt receives a durably published
    /// `interrupted_attempt` failure before its claim is released. Completed
    /// staging directories never establish success. Recovery allocates no retry.
    ///
    /// # Errors
    /// Returns lock contention, unsafe paths, invalid authoritative state, and
    /// database, synchronization, or publication failures. Earlier transactions
    /// in a pass may already have committed; repeating recovery is safe.
    pub fn recover(
        &mut self,
        mut clock: impl FnMut() -> u64,
    ) -> Result<RecoveryReport, RunStoreError> {
        let _recovery_lock = coordination_lock(self, false)?;
        let now = timestamp(clock())? as u64;
        let root = self
            .database_path()
            .parent()
            .expect("store has a root")
            .to_owned();
        let objects = root.join("artifacts");
        ensure_directory(&objects)?;
        crate::runtime::platform_fs::complete_namespace_change(&root)?;
        let mut report = RecoveryReport::default();
        let (stages, busy_attempts) = self.lock_stages(&root, &objects, &mut report)?;
        let orphans = self.find_orphans(&root, &objects, now, &mut report)?;
        let mut query = self
            .connection
            .prepare("SELECT id FROM attempts WHERE finished_at_ns IS NULL ORDER BY id")?;
        let attempts = query
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(query);
        for id in attempts {
            if busy_attempts.contains(&id) {
                continue;
            }
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let checked = timestamp(clock().max(now))?;
            let Some((mut attempt, state)) = expired_attempt(&tx, &id, checked)? else {
                continue;
            };
            if state != "finalizing" {
                attempt = transition(
                    &tx,
                    &attempt,
                    &state,
                    "finalizing",
                    checked,
                    json!({"recovery":true}),
                    None,
                )?;
            }
            tx.commit()?;
            let mut valid = Vec::new();
            for orphan in orphans
                .iter()
                .filter(|o| o.document.as_json()["attempt_id"].as_str() == Some(id.as_str()))
            {
                match self.check_orphan(&attempt, orphan, checked as u64) {
                    Ok(evidence) => valid.push((orphan, evidence)),
                    Err(error @ (RunStoreError::Sqlite(_) | RunStoreError::StaleAttempt)) => {
                        return Err(error);
                    }
                    Err(error) => reject(&mut report, &orphan.artifact, error),
                }
            }
            // Several different immutable outcomes for one revision are
            // ambiguous evidence. Never choose a success by directory ordering.
            let terminal = if valid.len() == 1 {
                let (orphan, evidence) = valid.pop().expect("one candidate");
                for artifact in &evidence.roots {
                    verify_registered(&root, artifact)?;
                    sync_tree(&safe_path(&root, &artifact.artifact.relative_path)?)?;
                }
                sync_tree(&root.join(&orphan.artifact.relative_path))?;
                crate::runtime::platform_fs::complete_namespace_change(&objects)?;
                let tx = self
                    .connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let checked = timestamp(clock().max(now).max(checked as u64))?;
                require_expired(&tx, &attempt, checked)?;
                validate_orphan(&tx, &attempt, orphan, checked as u64)?;
                let mut artifact = orphan.artifact.clone();
                artifact.verified_at_ns = checked as u64;
                let terminal = commit_terminal_in(
                    &tx,
                    &attempt,
                    evidence.outcome,
                    artifact,
                    &evidence.references,
                    evidence.finished,
                    checked as u64,
                )?;
                tx.commit()?;
                terminal
            } else {
                for (orphan, _) in valid {
                    reject(
                        &mut report,
                        &orphan.artifact,
                        invalid("ambiguous completed outcomes for one attempt"),
                    );
                }
                self.fail_interrupted(&root, &objects, &attempt, &mut clock, checked as u64)?
            };
            report.terminals.push(terminal);
        }
        // Claim resolution comes first. Quarantine is repeatable if a crash
        // occurs after rename but before the lease-release transaction commits.
        for stage in stages {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let checked = timestamp(clock().max(now))?;
            let recoverable = match &stage.lease_id {
                None => true,
                Some(id) => tx.query_row(
                    "SELECT l.released_at_ns IS NOT NULL OR l.expires_at_ns <= ?2 OR a.finished_at_ns IS NOT NULL
                     FROM leases l LEFT JOIN attempts a ON a.id = l.attempt_id WHERE l.id = ?1",
                    params![id, checked], |r| r.get::<_, bool>(0),
                )?,
            };
            if !recoverable {
                continue;
            }
            let source = root.join(&stage.relative_path);
            // Windows cannot move a directory containing an open lock file.
            // Recovery owns the global exclusive lock, and acquiring this stage
            // lock established that no original publisher still owns the stage.
            #[cfg(windows)]
            drop(stage._lock);
            if source.try_exists()? {
                let quarantine = objects.join(".quarantine");
                ensure_directory(&quarantine)?;
                crate::runtime::platform_fs::complete_namespace_change(&objects)?;
                let target = quarantine.join(source.file_name().expect("stage has a name"));
                rename_no_replace(&source, &target)?;
                crate::runtime::platform_fs::complete_namespace_change(&objects)?;
                crate::runtime::platform_fs::complete_namespace_change(&quarantine)?;
                report
                    .quarantined_stages
                    .push(target.strip_prefix(&root).expect("inside store").to_owned());
            }
            crate::runtime::platform_fs::complete_namespace_change(&objects)?;
            let quarantine = objects.join(".quarantine");
            if quarantine.try_exists()? {
                if !crate::runtime::platform_fs::is_real_directory(&fs::symlink_metadata(&quarantine)?) {
                    return Err(RunStoreError::UnsafePath(quarantine));
                }
                crate::runtime::platform_fs::complete_namespace_change(&quarantine)?;
            }
            if let Some(id) = &stage.lease_id {
                tx.execute("UPDATE leases SET released_at_ns = max(?2, acquired_at_ns) WHERE id = ?1 AND released_at_ns IS NULL", params![id, checked])?;
            }
            tx.commit()?;
        }
        report
            .rejected_artifacts
            .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(report)
    }

    fn lock_stages(
        &self,
        root: &Path,
        objects: &Path,
        report: &mut RecoveryReport,
    ) -> Result<(Vec<Stage>, BTreeSet<String>), RunStoreError> {
        let mut paths = BTreeSet::new();
        for entry in fs::read_dir(objects)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(".staging-"))
            {
                paths.insert(
                    entry
                        .path()
                        .strip_prefix(root)
                        .expect("inside root")
                        .to_owned(),
                );
            }
        }
        let mut query = self.connection.prepare(
            "SELECT staging_path FROM leases WHERE purpose = 'staging' ORDER BY staging_path",
        )?;
        for path in query.query_map([], |r| r.get::<_, String>(0))? {
            let path = PathBuf::from(path?);
            if path.parent() != Some(Path::new("artifacts"))
                || !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".staging-"))
            {
                return Err(RunStoreError::UnsafePath(path));
            }
            paths.insert(path);
        }
        let mut stages = Vec::new();
        let mut busy = BTreeSet::new();
        for relative_path in paths {
            let path = root.join(&relative_path);
            let lease: Option<(String, Option<String>)> = self
                .connection
                .query_row(
                    "SELECT id, attempt_id FROM leases WHERE staging_path = ?1",
                    [crate::runtime::platform_fs::relative_path(&relative_path)?],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (lease_id, attempt_id) = match lease {
                Some((lease, attempt)) => (Some(lease), attempt),
                None => (None, None),
            };
            let mut lock = None;
            match fs::symlink_metadata(&path) {
                Ok(metadata) if !crate::runtime::platform_fs::is_real_directory(&metadata) => {
                    return Err(RunStoreError::UnsafePath(path));
                }
                Ok(_) => match open_lock(&path.join("lock"), false) {
                    Ok(file) => match file.try_lock() {
                        Ok(()) => lock = Some(file),
                        Err(TryLockError::WouldBlock) => {
                            report.busy_stages.push(relative_path);
                            if let Some(id) = attempt_id {
                                busy.insert(id);
                            }
                            continue;
                        }
                        Err(TryLockError::Error(error)) => return Err(error.into()),
                    },
                    Err(RunStoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
            stages.push(Stage {
                relative_path,
                lease_id,
                _lock: lock,
            });
        }
        Ok((stages, busy))
    }

    fn find_orphans(
        &mut self,
        root: &Path,
        objects: &Path,
        now: u64,
        report: &mut RecoveryReport,
    ) -> Result<Vec<Orphan>, RunStoreError> {
        let mut paths = fs::read_dir(objects)?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()?;
        paths.sort();
        let mut orphans = Vec::new();
        for path in paths {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let relative_path = format!("artifacts/{name}");
            if self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM artifacts WHERE relative_path = ?1)",
                [&relative_path],
                |r| r.get::<_, bool>(0),
            )? {
                continue;
            }
            let result = completed_metadata(root, &relative_path, now);
            let (kind, artifact) = match result {
                Ok(value) => value,
                Err(error) => {
                    report.rejected_artifacts.push(RejectedArtifact {
                        relative_path: relative_path.into(),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            if kind == "attempt" {
                match fs::read(path.join("outcome.json"))
                    .map_err(RunStoreError::from)
                    .and_then(|bytes| {
                        let document = CanonicalValue::from_slice(&bytes)?;
                        if document.to_canonical_bytes()? != bytes {
                            return Err(invalid("noncanonical coordination outcome"));
                        }
                        Ok(document)
                    }) {
                    Ok(document) => {
                        let id = document.as_json()["attempt_id"].as_str().unwrap_or("");
                        let recoverable: Option<bool> = self
                            .connection
                            .query_row(
                                "SELECT finished_at_ns IS NULL FROM attempts WHERE id = ?1",
                                [id],
                                |r| r.get(0),
                            )
                            .optional()?;
                        if recoverable == Some(true) {
                            orphans.push(Orphan { artifact, document });
                        } else {
                            reject(
                                report,
                                &artifact,
                                invalid("completed attempt has no nonterminal reservation"),
                            );
                        }
                    }
                    Err(error) => reject(report, &artifact, error),
                }
            } else {
                sync_tree(&path)?;
                crate::runtime::platform_fs::complete_namespace_change(objects)?;
                let tx = self
                    .connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                register_artifact(&tx, &artifact, &kind)?;
                tx.commit()?;
                report.imported_artifacts.push(artifact);
            }
        }
        Ok(orphans)
    }

    fn check_orphan(
        &mut self,
        attempt: &OwnedAttempt,
        orphan: &Orphan,
        now: u64,
    ) -> Result<Evidence, RunStoreError> {
        let root = self
            .database_path()
            .parent()
            .expect("store root")
            .to_owned();
        let tx = self.connection.transaction()?;
        let evidence = validate_orphan(&tx, attempt, orphan, now)?;
        for artifact in &evidence.roots {
            verify_registered(&root, artifact)?;
        }
        tx.commit()?;
        Ok(evidence)
    }

    fn fail_interrupted(
        &mut self,
        root: &Path,
        objects: &Path,
        attempt: &OwnedAttempt,
        clock: &mut impl FnMut() -> u64,
        earliest: u64,
    ) -> Result<TerminalAttempt, RunStoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let finished = clock().max(earliest).max(attempt.updated_at_ns());
        require_expired(&tx, attempt, timestamp(finished)?)?;
        let outcome = TerminalOutcome::Failed {
            code: "interrupted_attempt".into(),
            details: CanonicalValue::try_from(
                json!({"reason":"owner lease expired without an unambiguous verified published outcome"}),
            )?,
        };
        let mut query = tx.prepare("SELECT DISTINCT 'retained', a.artifact_id FROM leased_artifacts a JOIN leases l ON l.id = a.lease_id WHERE l.attempt_id = ?1 ORDER BY a.artifact_id")?;
        let references = query
            .query_map([attempt.id().to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(query);
        let document = build_outcome_document(&tx, attempt, &outcome, &references, finished)?;
        tx.commit()?;
        // The global recovery lock leases this private stage, including the
        // interval before its lock file exists. A crash leaves it quarantinable.
        let directory = tempfile::Builder::new()
            .prefix(".staging-recovery-")
            .tempdir_in(objects)?;
        let lock = open_lock(&directory.path().join("lock"), true)?;
        lock.lock()?;
        let object = directory.path().join("object");
        fs::create_dir(&object)?;
        write_file(
            &object.join("outcome.json"),
            &document.to_canonical_bytes()?,
        )?;
        prepare_directory(&object)?;
        let digest =
            hash_local_tree(&object).map_err(crate::runtime::publication::PublicationError::from)?;
        let relative_path = format!("artifacts/attempt-{}", hex(digest));
        let path = root.join(&relative_path);
        publish_prepared_directory(&object, &path)?;
        let (_, mut artifact) = completed_metadata(root, &relative_path, finished)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let checked = clock().max(finished);
        require_expired(&tx, attempt, timestamp(checked)?)?;
        if build_outcome_document(&tx, attempt, &outcome, &references, finished)? != document {
            return Err(invalid(
                "coordination records changed during recovery publication",
            ));
        }
        artifact.verified_at_ns = checked;
        let terminal = commit_terminal_in(
            &tx,
            attempt,
            outcome,
            artifact,
            &references,
            finished,
            checked,
        )?;
        tx.commit()?;
        Ok(terminal)
    }
}

pub(super) fn coordination_lock(store: &RunStore, shared: bool) -> Result<File, RunStoreError> {
    let root = store.database_path().parent().expect("store root");
    let file = open_lock(&root.join(".run-store-recovery.lock"), true)?;
    let result = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    match result {
        Ok(()) => (),
        Err(TryLockError::WouldBlock) => return Err(RunStoreError::Busy),
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    crate::runtime::durability::sync_all(&file)?;
    crate::runtime::platform_fs::complete_namespace_change(root)?;
    Ok(file)
}

fn open_lock(path: &Path, create: bool) -> Result<File, RunStoreError> {
    let file = crate::runtime::platform_fs::open_regular(
        path,
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false),
    )?;
    if !file.metadata()?.is_file() {
        return Err(RunStoreError::UnsafePath(path.to_owned()));
    }
    Ok(file)
}

fn ensure_directory(path: &Path) -> Result<(), RunStoreError> {
    match fs::create_dir(path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    if !crate::runtime::platform_fs::is_real_directory(&fs::symlink_metadata(path)?) {
        return Err(RunStoreError::UnsafePath(path.to_owned()));
    }
    Ok(())
}

fn expired_attempt(
    db: &Connection,
    id: &str,
    now: i64,
) -> Result<Option<(OwnedAttempt, String)>, RunStoreError> {
    let row = db.query_row(
        "SELECT a.slot_id, a.retry_index, a.revision, a.updated_at_ns, l.id, l.owner_token, a.state
         FROM attempts a JOIN leases l ON l.attempt_id = a.id AND l.purpose = 'reservation'
         WHERE a.id = ?1 AND a.finished_at_ns IS NULL
         AND (l.released_at_ns IS NOT NULL OR l.expires_at_ns <= max(?2, a.updated_at_ns)
             OR EXISTS (SELECT 1 FROM finalization_claims c JOIN leases f ON f.id = c.lease_id
                 WHERE c.attempt_id = a.id AND (f.released_at_ns IS NOT NULL OR f.expires_at_ns <= max(?2, a.updated_at_ns))))",
        params![id, now], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64, r.get::<_, i64>(3)? as u64, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?)),
    ).optional()?;
    row.map(
        |(slot, retry_index, revision, updated_at_ns, lease_id, owner_token, state)| {
            let slot = slot
                .parse()
                .map_err(|_| invalid("invalid resolved attempt slot identity"))?;
            let expected = run_attempt_id(slot, retry_index)?;
            if expected.to_string() != id {
                return Err(invalid("attempt identity disagrees with its reservation"));
            }
            Ok((
                OwnedAttempt {
                    id: expected,
                    slot,
                    retry_index,
                    revision,
                    updated_at_ns,
                    lease_id,
                    owner_token,
                },
                state,
            ))
        },
    )
    .transpose()
}

fn require_expired(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    now: i64,
) -> Result<(), RunStoreError> {
    let current = expired_attempt(tx, &attempt.id().to_string(), now)?;
    if current.as_ref().is_none_or(|(a, state)| {
        a.revision() != attempt.revision()
            || state != "finalizing"
            || a.lease_id != attempt.lease_id
            || a.owner_token != attempt.owner_token
    }) {
        return Err(RunStoreError::StaleAttempt);
    }
    Ok(())
}

fn validate_orphan(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    orphan: &Orphan,
    now: u64,
) -> Result<Evidence, RunStoreError> {
    require_expired(tx, attempt, timestamp(now)?)?;
    let value = orphan.document.as_json();
    let finished = value["finished_at_ns"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or(invalid("invalid immutable finish time"))?;
    if finished < attempt.updated_at_ns() || finished > now {
        return Err(invalid(
            "immutable finish time is outside the attempt history",
        ));
    }
    let details = &value["details"];
    let outcome = match value["state"].as_str() {
        Some("accepted") => TerminalOutcome::Accepted,
        Some("superseded") => TerminalOutcome::Superseded,
        Some("censored") => TerminalOutcome::Censored {
            reason: string(details, "reason")?.into(),
        },
        Some("failed") => TerminalOutcome::Failed {
            code: string(details, "failure_code")?.into(),
            details: CanonicalValue::try_from(details["diagnostic"].clone())?,
        },
        _ => return Err(invalid("unknown coordination outcome")),
    };
    let references = value["references"]
        .as_array()
        .ok_or(invalid("missing dependency metadata"))?
        .iter()
        .map(|v| Ok((string(v, "role")?.to_owned(), string(v, "id")?.to_owned())))
        .collect::<Result<Vec<_>, RunStoreError>>()?;
    if build_outcome_document(tx, attempt, &outcome, &references, finished)? != orphan.document {
        return Err(invalid(
            "completed outcome disagrees with authoritative coordination records",
        ));
    }
    if matches!(outcome, TerminalOutcome::Accepted) {
        let claim: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM finalization_claims c JOIN leases f ON f.id = c.lease_id
             JOIN leases r ON r.attempt_id = c.attempt_id AND r.purpose = 'reservation'
             WHERE c.attempt_id = ?1 AND c.slot_id = ?2 AND f.owner_token = ?3 AND r.owner_token = ?3
             AND f.released_at_ns IS NULL AND r.released_at_ns IS NULL
             AND f.acquired_at_ns <= ?4 AND f.expires_at_ns > ?4 AND r.expires_at_ns > ?4)",
            params![attempt.id().to_string(), attempt.slot().to_string(), attempt.owner_token, timestamp(finished)?], |r| r.get(0),
        )?;
        if !claim {
            return Err(invalid(
                "completed acceptance lacks its original authorized claim",
            ));
        }
        validate_acceptance(tx, attempt)?;
    }
    let mut ids = references
        .iter()
        .map(|(_, id)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut query = tx.prepare(
        "SELECT o.artifact_id FROM observations o LEFT JOIN selected_observations s ON s.observation_id = o.id
         WHERE o.producing_attempt_id = ?1 OR (s.observation_id IS NOT NULL AND o.slot_id IN (
             SELECT observation_slot_id FROM attempt_observation_slots WHERE attempt_slot_id = ?2))
         UNION SELECT oa.artifact_id FROM observation_artifacts oa JOIN observations o ON o.id = oa.observation_id
         LEFT JOIN selected_observations s ON s.observation_id = o.id
         WHERE o.producing_attempt_id = ?1 OR (s.observation_id IS NOT NULL AND o.slot_id IN (
             SELECT observation_slot_id FROM attempt_observation_slots WHERE attempt_slot_id = ?2))",
    )?;
    for id in query.query_map(
        params![attempt.id().to_string(), attempt.slot().to_string()],
        |r| r.get::<_, String>(0),
    )? {
        ids.insert(id?);
    }
    let roots = ids
        .iter()
        .map(|id| registered_metadata(tx, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Evidence {
        outcome,
        references,
        finished,
        roots,
    })
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, RunStoreError> {
    value[key]
        .as_str()
        .ok_or(invalid("missing coordination string field"))
}

fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, RunStoreError> {
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            return Err(RunStoreError::UnsafePath(path));
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path)?;
        if !crate::runtime::platform_fs::is_real_directory(&metadata)
            && !crate::runtime::platform_fs::is_regular_file(&metadata)
        {
            return Err(RunStoreError::UnsafePath(path));
        }
    }
    if path == root {
        return Err(RunStoreError::UnsafePath(path));
    }
    Ok(path)
}

fn completed_metadata(
    root: &Path,
    relative: &str,
    now: u64,
) -> Result<(String, ArtifactRegistration), RunStoreError> {
    let path = safe_path(root, relative)?;
    verify_finalized_directory(&path)?;
    let digest = hash_local_tree(&path).map_err(crate::runtime::publication::PublicationError::from)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(invalid("invalid artifact address"))?;
    let (kind, hash) = name
        .split_once('-')
        .ok_or(invalid("invalid artifact address"))?;
    if !matches!(
        kind,
        "attempt"
            | "observation"
            | "result"
            | "metrics"
            | "dataset"
            | "source"
            | "environment"
            | "other"
    ) || hash != hex(digest)
    {
        return Err(invalid(
            "completed artifact address disagrees with its contents",
        ));
    }
    let manifest_sha256 =
        ContentDigest::new(Sha256::digest(fs::read(path.join("manifest.json"))?).into());
    Ok((
        kind.into(),
        ArtifactRegistration {
            id: format!("run-store-v1:{name}"),
            relative_path: relative.into(),
            content_sha256: digest,
            manifest_sha256,
            verified_at_ns: now,
        },
    ))
}

fn registered_metadata(db: &Connection, id: &str) -> Result<StoredArtifact, RunStoreError> {
    let (relative_path, content, manifest, verified, storage_kind): (String, String, Option<String>, i64, String) = db.query_row(
        "SELECT relative_path, content_sha256, manifest_sha256, verified_at_ns, storage_kind FROM artifacts WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    Ok(StoredArtifact {
        artifact: ArtifactRegistration {
            id: id.into(),
            relative_path,
            content_sha256: parse_digest(&content)?,
            manifest_sha256: parse_digest(&manifest.unwrap_or_else(|| "0".repeat(64)))?,
            verified_at_ns: verified as u64,
        },
        storage_kind,
    })
}

fn verify_registered(root: &Path, stored: &StoredArtifact) -> Result<(), RunStoreError> {
    let artifact = &stored.artifact;
    let path = safe_path(root, &artifact.relative_path)?;
    let metadata = fs::symlink_metadata(&path)?;
    if (stored.storage_kind == "directory" && !crate::runtime::platform_fs::is_real_directory(&metadata))
        || (stored.storage_kind == "file" && !crate::runtime::platform_fs::is_regular_file(&metadata))
    {
        return Err(invalid("dependency storage kind changed"));
    }
    let content = if metadata.is_dir() {
        verify_finalized_directory(&path)?;
        let manifest =
            ContentDigest::new(Sha256::digest(fs::read(path.join("manifest.json"))?).into());
        if manifest != artifact.manifest_sha256 {
            return Err(invalid("dependency completion manifest changed"));
        }
        hash_local_tree(&path).map_err(crate::runtime::publication::PublicationError::from)?
    } else {
        ContentDigest::new(Sha256::digest(fs::read(path)?).into())
    };
    if content != artifact.content_sha256 {
        return Err(invalid("dependency content changed"));
    }
    Ok(())
}

fn sync_tree(path: &Path) -> Result<(), RunStoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if crate::runtime::platform_fs::is_real_directory(&metadata) {
        let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            sync_tree(&entry.path())?;
        }
        crate::runtime::platform_fs::complete_namespace_change(path)?;
    } else if crate::runtime::platform_fs::is_regular_file(&metadata) {
        crate::runtime::platform_fs::sync_owned_file(path)?;
    } else {
        return Err(RunStoreError::UnsafePath(path.to_owned()));
    }
    Ok(())
}

fn reject(report: &mut RecoveryReport, artifact: &ArtifactRegistration, error: RunStoreError) {
    report.rejected_artifacts.push(RejectedArtifact {
        relative_path: artifact.relative_path.clone().into(),
        reason: error.to_string(),
    });
}

fn hex(digest: ContentDigest) -> String {
    digest
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_digest(value: &str) -> Result<ContentDigest, RunStoreError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid registered artifact digest"));
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[2 * index..2 * index + 2], 16)
            .map_err(|_| invalid("invalid registered artifact digest"))?;
    }
    Ok(ContentDigest::new(bytes))
}
