use metewand_core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    identity::run_attempt_id,
    records::{ContentDigest, RecordId, ResolvedAttemptSlotRecord, RunAttemptRecord},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};

use super::{RunStore, RunStoreError};

/// An execution phase; terminal outcomes require `commit_finalization`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptPhase {
    Materializing,
    Starting,
    Preparing,
    Executing,
    Checkpointing,
    Validating,
    Evaluating,
    Finalizing,
}

impl AttemptPhase {
    fn name(self) -> &'static str {
        match self {
            Self::Materializing => "materializing",
            Self::Starting => "starting",
            Self::Preparing => "preparing",
            Self::Executing => "executing",
            Self::Checkpointing => "checkpointing",
            Self::Validating => "validating",
            Self::Evaluating => "evaluating",
            Self::Finalizing => "finalizing",
        }
    }
}

/// Explicit authorization to allocate one retry, rechecked under the write lock.
#[derive(Clone, Copy, Debug)]
pub enum RetryAuthorization {
    FirstAttempt,
    Retry { next_index: u64, max_index: u64 },
}

/// A caller-generated unique fencing token and its wall-clock expiration.
#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub id: String,
    pub owner_token: String,
    pub expires_at_ns: u64,
}

/// An opaque reservation guard carrying the expected revision and owner tokens.
///
/// Clone it to retain a snapshot. Successful transitions return a new guard;
/// previous revisions cannot write. Tokens must be kept private to the owner.
#[derive(Clone, Debug)]
pub struct OwnedAttempt {
    pub(super) id: RecordId<RunAttemptRecord>,
    pub(super) slot: RecordId<ResolvedAttemptSlotRecord>,
    pub(super) retry_index: u64,
    pub(super) revision: u64,
    pub(super) updated_at_ns: u64,
    pub(super) lease_id: String,
    pub(super) owner_token: String,
}

impl OwnedAttempt {
    #[must_use]
    pub fn id(&self) -> RecordId<RunAttemptRecord> {
        self.id
    }
    #[must_use]
    pub fn slot(&self) -> RecordId<ResolvedAttemptSlotRecord> {
        self.slot
    }
    #[must_use]
    pub fn retry_index(&self) -> u64 {
        self.retry_index
    }
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }
    #[must_use]
    pub fn updated_at_ns(&self) -> u64 {
        self.updated_at_ns
    }
}

/// The immutable outcome the publisher must write before committing completion.
#[derive(Clone, Debug)]
pub enum TerminalOutcome {
    Accepted,
    Censored {
        reason: String,
    },
    Superseded,
    Failed {
        code: String,
        details: CanonicalValue,
    },
}

impl TerminalOutcome {
    pub(super) fn state(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Censored { .. } => "censored",
            Self::Superseded => "superseded",
            Self::Failed { .. } => "failed",
        }
    }

    pub(super) fn details(&self) -> Result<Value, RunStoreError> {
        Ok(match self {
            Self::Failed { code, details } => {
                if code.trim().is_empty() || !details.as_json().is_object() {
                    return Err(invalid("failure requires a code and object diagnostic"));
                }
                json!({"failure_code": code, "diagnostic": details.as_json()})
            }
            Self::Censored { reason } => {
                if reason.trim().is_empty() {
                    return Err(invalid("censoring requires a reason"));
                }
                json!({"reason": reason})
            }
            _ => json!({}),
        })
    }
}

/// Metadata supplied by the trusted publisher for a completed attempt directory.
///
/// This is a database boundary, not a publication API. Before calling
/// `commit_finalization`, the publisher must verify the immutable outcome and
/// complete inventory, synchronize and publish the directory, and synchronize
/// its parent. The supplied identity and hashes must describe those bytes.
/// The transaction validates metadata and references, not filesystem durability.
#[derive(Clone, Debug)]
pub struct ArtifactRegistration {
    pub id: String,
    pub relative_path: String,
    pub content_sha256: ContentDigest,
    pub manifest_sha256: ContentDigest,
    pub verified_at_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimDisposition {
    Claimed,
    Superseded,
}

/// Committed claim decision, available before the outcome document is written.
#[derive(Clone, Debug)]
pub struct Finalization {
    pub attempt: OwnedAttempt,
    pub disposition: ClaimDisposition,
}

/// A terminal database outcome returned only after its transaction commits.
#[derive(Clone, Debug)]
pub struct TerminalAttempt {
    pub id: RecordId<RunAttemptRecord>,
    pub outcome: TerminalOutcome,
    pub finished_at_ns: u64,
    pub event_sequence: i64,
}

impl RunStore {
    /// Reserves an already registered resolved execution slot.
    ///
    /// The planner must have validated and registered its resolved records and
    /// applicability before calling this coordination API. No worker work is
    /// performed while the write transaction is open.
    ///
    /// # Errors
    /// Rejects absent or satisfied slots, unauthorized or exhausted retries,
    /// invalid leases, canonical integer overflow, and database errors.
    pub fn reserve(
        &mut self,
        slot: RecordId<ResolvedAttemptSlotRecord>,
        authorization: RetryAuthorization,
        lease: LeaseRequest,
        now_ns: u64,
    ) -> Result<OwnedAttempt, RunStoreError> {
        let now = timestamp(now_ns)?;
        validate_lease(&lease, now)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM attempt_slots WHERE id = ?1)",
            [slot.to_string()],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("resolved execution slot is not registered"));
        }
        if satisfied(&tx, &slot.to_string())? {
            return Err(RunStoreError::SatisfiedSlot);
        }
        let previous: Option<i64> = tx.query_row(
            "SELECT max(retry_index) FROM attempts WHERE slot_id = ?1",
            [slot.to_string()],
            |r| r.get(0),
        )?;
        let next = match previous {
            Some(n) => n.checked_add(1).ok_or(invalid("retry index overflow"))?,
            None => 0,
        };
        match authorization {
            RetryAuthorization::FirstAttempt if next == 0 => (),
            RetryAuthorization::Retry {
                next_index,
                max_index,
            } if next > 0
                && next_index == next as u64
                && next_index <= max_index
                && max_index <= MAX_SAFE_INTEGER => {}
            _ => return Err(invalid("retry is unauthorized, stale, or exhausted")),
        }
        let id = run_attempt_id(slot, next as u64)?;
        tx.execute("INSERT INTO attempts(id, slot_id, retry_index, state, revision, started_at_ns, updated_at_ns)
            VALUES (?1, ?2, ?3, 'reserved', 0, ?4, ?4)", params![id.to_string(), slot.to_string(), next, now])?;
        insert_lease(&tx, &id.to_string(), "reservation", &lease, now)?;
        append_transition(
            &tx,
            &id.to_string(),
            None,
            "reserved",
            0,
            now,
            json!({"slot":slot.to_string(), "retry_index":next}),
        )?;
        tx.commit()?;
        Ok(OwnedAttempt {
            id,
            slot,
            retry_index: next as u64,
            revision: 0,
            updated_at_ns: now_ns,
            lease_id: lease.id,
            owner_token: lease.owner_token,
        })
    }

    /// Advances one execution edge, atomically appending history and an event.
    ///
    /// Backward wall-clock adjustments are clamped to the previous transition.
    /// Use `begin_finalization` for a prospective successful finalizer.
    ///
    /// # Errors
    /// Rejects stale revisions, expired or released owners, illegal edges, and
    /// database errors. Terminal rows cannot advance.
    pub fn advance(
        &mut self,
        attempt: &OwnedAttempt,
        phase: AttemptPhase,
        now_ns: u64,
    ) -> Result<OwnedAttempt, RunStoreError> {
        let now = timestamp(now_ns)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = fence(&tx, attempt, now)?;
        let advanced = transition(&tx, attempt, &state, phase.name(), now, json!({}), None)?;
        tx.commit()?;
        Ok(advanced)
    }

    /// Renews the reservation, owned finalization lease, and live staging leases together.
    ///
    /// # Errors
    /// Rejects expired/released leases, stale revisions, shortened expiration,
    /// invalid timestamps, and database errors. Expired claims require recovery.
    pub fn renew(
        &mut self,
        attempt: &OwnedAttempt,
        expires_at_ns: u64,
        now_ns: u64,
    ) -> Result<(), RunStoreError> {
        let now = timestamp(now_ns)?;
        let expires = timestamp(expires_at_ns)?;
        if expires <= now {
            return Err(invalid("lease expiration must be in the future"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        fence(&tx, attempt, now)?;
        let mut query = tx.prepare("SELECT id, expires_at_ns FROM leases WHERE attempt_id = ?1 AND released_at_ns IS NULL AND (purpose IN ('reservation', 'finalization') OR (purpose = 'staging' AND expires_at_ns > ?2))")?;
        let leases = query
            .query_map(
                params![
                    attempt.id.to_string(),
                    now.max(timestamp(attempt.updated_at_ns)?)
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(query);
        for (id, old_expiry) in leases {
            if expires < old_expiry {
                return Err(invalid("lease renewal cannot shorten expiration"));
            }
            let updated = tx.execute("UPDATE leases SET expires_at_ns = ?2 WHERE id = ?1 AND owner_token = ?3 AND released_at_ns IS NULL AND expires_at_ns > ?4", params![id, expires, attempt.owner_token, now])?;
            if updated != 1 {
                return Err(RunStoreError::StaleAttempt);
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Enters finalizing and competes for the slot's leased success claim.
    ///
    /// The decision is recorded with the transition. Even an expired competing
    /// claim yields `Superseded`; this API never steals a claim from recovery.
    /// The publisher must use this decision in its immutable outcome document.
    ///
    /// # Errors
    /// Rejects stale owners, illegal transitions, invalid leases, and database errors.
    pub fn begin_finalization(
        &mut self,
        attempt: &OwnedAttempt,
        lease: LeaseRequest,
        now_ns: u64,
    ) -> Result<Finalization, RunStoreError> {
        let now = timestamp(now_ns)?.max(timestamp(attempt.updated_at_ns)?);
        validate_lease(&lease, now)?;
        if lease.owner_token != attempt.owner_token {
            return Err(invalid(
                "finalization lease must have the reservation owner",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = fence(&tx, attempt, now)?;
        let competing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM finalization_claims WHERE slot_id = ?1)",
            [attempt.slot.to_string()],
            |r| r.get(0),
        )?;
        let disposition = if competing || satisfied(&tx, &attempt.slot.to_string())? {
            ClaimDisposition::Superseded
        } else {
            ClaimDisposition::Claimed
        };
        let advanced = transition(
            &tx,
            attempt,
            &state,
            "finalizing",
            now,
            json!({"claim": if disposition == ClaimDisposition::Claimed { "claimed" } else { "superseded" }}),
            None,
        )?;
        if disposition == ClaimDisposition::Claimed {
            insert_lease(&tx, &attempt.id.to_string(), "finalization", &lease, now)?;
            tx.execute("INSERT INTO finalization_claims(slot_id, attempt_id, lease_id) VALUES (?1, ?2, ?3)", params![attempt.slot.to_string(), attempt.id.to_string(), lease.id])?;
        }
        tx.commit()?;
        Ok(Finalization {
            attempt: advanced,
            disposition,
        })
    }

    /// Commits a previously published immutable outcome and its artifact roots.
    ///
    /// The trusted publisher must complete the operations documented on
    /// `ArtifactRegistration` first, and must validate problem-owned scientific
    /// rules and all transitive dependencies. This transaction rechecks fencing,
    /// claims, selected observation membership, validity, and completion. Each
    /// `(role, artifact_id)` dependency must already be registered. Failed,
    /// censored, and superseded attempts retain observations and references.
    ///
    /// # Errors
    /// Rejects expired owners, stale revisions, missing claims or dependencies,
    /// incomplete observation selections, malformed metadata, and database errors.
    pub fn commit_finalization(
        &mut self,
        attempt: &OwnedAttempt,
        outcome: TerminalOutcome,
        artifact: ArtifactRegistration,
        references: &[(String, String)],
        now_ns: u64,
    ) -> Result<TerminalAttempt, RunStoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let terminal =
            commit_finalization_in(&tx, attempt, outcome, artifact, references, now_ns, now_ns)?;
        tx.commit()?;
        Ok(terminal)
    }
}

pub(super) fn commit_finalization_in(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    outcome: TerminalOutcome,
    artifact: ArtifactRegistration,
    references: &[(String, String)],
    now_ns: u64,
    checked_at_ns: u64,
) -> Result<TerminalAttempt, RunStoreError> {
    let now = timestamp(now_ns)?.max(timestamp(attempt.updated_at_ns)?);
    let checked = timestamp(checked_at_ns)?.max(now);
    validate_finalization(tx, attempt, &outcome, checked)?;
    commit_terminal_in(
        tx,
        attempt,
        outcome,
        artifact,
        references,
        now_ns,
        checked_at_ns,
    )
}

// Recovery performs its own expiry and evidence checks while holding the write
// lock. Both paths then use the same terminal projection and claim release.
pub(super) fn commit_terminal_in(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    outcome: TerminalOutcome,
    artifact: ArtifactRegistration,
    references: &[(String, String)],
    now_ns: u64,
    checked_at_ns: u64,
) -> Result<TerminalAttempt, RunStoreError> {
    let now = timestamp(now_ns)?.max(timestamp(attempt.updated_at_ns)?);
    let checked = timestamp(checked_at_ns)?.max(now);
    #[cfg(test)]
    crate::fault_injection::hit("finalization.projection.before");
    let verified = timestamp(artifact.verified_at_ns)?;
    if artifact.id.is_empty() || verified > checked {
        return Err(invalid("invalid finalized artifact metadata"));
    }
    let details = outcome.details()?;
    let state = "finalizing";
    tx.execute("INSERT INTO artifacts(id, kind, relative_path, content_sha256, storage_kind, manifest_sha256, verified_at_ns)
            VALUES (?1, 'attempt', ?2, ?3, 'directory', ?4, ?5)", params![artifact.id, artifact.relative_path,
                hex(artifact.content_sha256), hex(artifact.manifest_sha256), verified])?;
    tx.execute(
        "INSERT INTO attempt_artifacts VALUES (?1, 'final', ?2)",
        params![attempt.id.to_string(), artifact.id],
    )?;
    for (role, artifact_id) in references {
        tx.execute(
            "INSERT INTO attempt_artifacts VALUES (?1, ?2, ?3)",
            params![attempt.id.to_string(), role, artifact_id],
        )?;
    }
    let failure = match &outcome {
        TerminalOutcome::Failed { code, .. } => Some(code.as_str()),
        _ => None,
    };
    let finished = transition(
        tx,
        attempt,
        state,
        outcome.state(),
        now,
        details,
        Some(TerminalProjection {
            artifact_id: &artifact.id,
            failure_code: failure,
        }),
    )?;
    let event_sequence = tx.query_row(
        "SELECT event_sequence FROM attempt_transitions WHERE attempt_id = ?1 AND revision = ?2",
        params![attempt.id.to_string(), timestamp(finished.revision)?],
        |r| r.get(0),
    )?;
    tx.execute(
        "DELETE FROM finalization_claims WHERE attempt_id = ?1",
        [attempt.id.to_string()],
    )?;
    tx.execute(
        "UPDATE leases SET released_at_ns = ?2 WHERE attempt_id = ?1 AND released_at_ns IS NULL",
        params![attempt.id.to_string(), checked],
    )?;
    #[cfg(test)]
    crate::fault_injection::hit("finalization.projection.after");
    Ok(TerminalAttempt {
        id: attempt.id,
        outcome,
        finished_at_ns: finished.updated_at_ns,
        event_sequence,
    })
}

pub(super) fn validate_finalization(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    outcome: &TerminalOutcome,
    now: i64,
) -> Result<(), RunStoreError> {
    let state = fence(tx, attempt, now)?;
    if state != "finalizing" {
        return Err(invalid("terminal outcome requires finalizing"));
    }
    if matches!(outcome, TerminalOutcome::Accepted) {
        let owns_claim: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM finalization_claims WHERE attempt_id = ?1)",
            [attempt.id.to_string()],
            |r| r.get(0),
        )?;
        if !owns_claim {
            return Err(invalid("acceptance requires this attempt's claim"));
        }
        validate_acceptance(tx, attempt)?;
    }
    Ok(())
}

pub(super) fn invalid(message: &'static str) -> RunStoreError {
    RunStoreError::InvalidAttempt(message)
}
pub(super) fn timestamp(value: u64) -> Result<i64, RunStoreError> {
    i64::try_from(value).map_err(|_| invalid("timestamp exceeds SQLite's signed integer range"))
}
fn validate_lease(lease: &LeaseRequest, now: i64) -> Result<(), RunStoreError> {
    if lease.id.is_empty() || lease.owner_token.is_empty() || timestamp(lease.expires_at_ns)? <= now
    {
        return Err(invalid(
            "lease requires unique tokens and a future expiration",
        ));
    }
    Ok(())
}
fn insert_lease(
    tx: &Transaction<'_>,
    attempt: &str,
    purpose: &str,
    lease: &LeaseRequest,
    now: i64,
) -> Result<(), RunStoreError> {
    tx.execute("INSERT INTO leases(id, owner_token, attempt_id, purpose, acquired_at_ns, expires_at_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![lease.id, lease.owner_token, attempt, purpose, now, timestamp(lease.expires_at_ns)?])?;
    Ok(())
}
fn satisfied(db: &Connection, slot: &str) -> Result<bool, RunStoreError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM attempts WHERE slot_id = ?1 AND state = 'accepted')",
        [slot],
        |r| r.get(0),
    )?)
}

// Every writer checks fencing after taking the write lock. Expiry is not
// permission to release or replace a claim; only recovery may resolve it.
pub(super) fn fence(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    now: i64,
) -> Result<String, RunStoreError> {
    let state = tx.query_row("SELECT a.state FROM attempts a JOIN leases l ON l.attempt_id = a.id
        WHERE a.id = ?1 AND a.slot_id = ?2 AND a.retry_index = ?3 AND a.revision = ?4
        AND a.finished_at_ns IS NULL AND l.id = ?5 AND l.owner_token = ?6
        AND l.purpose = 'reservation' AND l.released_at_ns IS NULL AND l.expires_at_ns > ?7
        AND NOT EXISTS (SELECT 1 FROM finalization_claims c JOIN leases f ON f.id = c.lease_id
            WHERE c.attempt_id = a.id AND (f.owner_token != ?6 OR f.released_at_ns IS NOT NULL OR f.expires_at_ns <= ?7))",
        params![attempt.id.to_string(), attempt.slot.to_string(), timestamp(attempt.retry_index)?, timestamp(attempt.revision)?, attempt.lease_id, attempt.owner_token, now.max(timestamp(attempt.updated_at_ns)?)], |r| r.get::<_, String>(0)).optional()?;
    state.ok_or(RunStoreError::StaleAttempt)
}

pub(super) struct TerminalProjection<'a> {
    artifact_id: &'a str,
    failure_code: Option<&'a str>,
}

pub(super) fn transition(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    from: &str,
    to: &str,
    now: i64,
    details: Value,
    terminal: Option<TerminalProjection<'_>>,
) -> Result<OwnedAttempt, RunStoreError> {
    let artifact_id = terminal.as_ref().map(|t| t.artifact_id);
    let failure_code = terminal.and_then(|t| t.failure_code);
    let revision = attempt
        .revision
        .checked_add(1)
        .filter(|r| *r <= MAX_SAFE_INTEGER)
        .ok_or(invalid("revision exceeds canonical integer range"))?;
    let now = now.max(timestamp(attempt.updated_at_ns)?);
    append_transition(
        tx,
        &attempt.id.to_string(),
        Some(from),
        to,
        revision,
        now,
        details,
    )?;
    let updated = tx.execute("UPDATE attempts SET state = ?2, revision = ?3, updated_at_ns = ?4,
        finished_at_ns = ?5, failure_code = ?6, final_artifact_id = ?7 WHERE id = ?1 AND revision = ?8",
        params![attempt.id.to_string(), to, timestamp(revision)?, now, artifact_id.map(|_| now), failure_code, artifact_id, timestamp(attempt.revision)?])?;
    if updated != 1 {
        return Err(RunStoreError::StaleAttempt);
    }
    Ok(OwnedAttempt {
        revision,
        updated_at_ns: now as u64,
        ..attempt.clone()
    })
}

fn append_transition(
    tx: &Transaction<'_>,
    id: &str,
    from: Option<&str>,
    to: &str,
    revision: u64,
    now: i64,
    mut details: Value,
) -> Result<(), RunStoreError> {
    let fields = details
        .as_object_mut()
        .ok_or(invalid("event details must be an object"))?;
    fields.insert("from".into(), json!(from));
    fields.insert("to".into(), json!(to));
    fields.insert("revision".into(), json!(revision));
    let payload = String::from_utf8(CanonicalValue::try_from(details)?.to_canonical_bytes()?)
        .expect("canonical JSON is UTF-8");
    let kind = if revision == 0 {
        "attempt_reserved"
    } else if to == "finalizing" {
        "attempt_finalizing"
    } else if matches!(to, "accepted" | "censored" | "superseded" | "failed") {
        "attempt_finished"
    } else {
        "attempt_transition"
    };
    tx.execute("INSERT INTO events(attempt_id, occurred_at_ns, kind, payload_json) VALUES (?1, ?2, ?3, ?4)", params![id, now, kind, payload])?;
    let sequence = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO attempt_transitions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            timestamp(revision)?,
            revision.checked_sub(1).map(timestamp).transpose()?,
            from,
            to,
            now,
            sequence
        ],
    )?;
    Ok(())
}

pub(super) fn validate_acceptance(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
) -> Result<(), RunStoreError> {
    let (kind, role): (String, String) = tx.query_row(
        "SELECT kind, role FROM attempt_slots WHERE id = ?1",
        [attempt.slot.to_string()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let total: i64 = tx.query_row(
        "SELECT count(*) FROM attempt_observation_slots WHERE attempt_slot_id = ?1",
        [attempt.slot.to_string()],
        |r| r.get(0),
    )?;
    if role == "warmup" {
        if total != 0 {
            return Err(invalid("warm-up cannot select measured observations"));
        }
        return Ok(());
    }
    if total == 0 {
        return Err(invalid("measurement requires observation slots"));
    }
    let complete: i64 = tx.query_row(
        "SELECT count(*) FROM attempt_observation_slots m
        JOIN selected_observations s ON s.slot_id = m.observation_slot_id
        JOIN observations o ON o.id = s.observation_id
        JOIN artifacts a ON a.id = o.artifact_id AND a.kind = 'observation'
        WHERE m.attempt_slot_id = ?1 AND o.valid = 1
        AND (?2 = 'stream' OR EXISTS (SELECT 1 FROM observations produced
            JOIN artifacts published ON published.id = produced.artifact_id AND published.kind = 'observation'
            WHERE produced.slot_id = m.observation_slot_id AND produced.producing_attempt_id = ?3
            AND produced.valid = 1 AND produced.completion != 'not_reached'))",
        params![attempt.slot.to_string(), kind, attempt.id.to_string()],
        |r| r.get(0),
    )?;
    if complete != total {
        return Err(invalid(
            "required selections or this attempt's complete observations are missing",
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
