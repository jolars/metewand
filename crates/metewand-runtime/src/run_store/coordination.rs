use metewand_core::{
    identity::{canonical_identity_bytes, record_id},
    records::{
        IdentifiedRecord, LogicalCandidateRecord, RecordId, ResolvedAttemptSlotRecord,
        ResolvedObservationSlotRecord, ResolvedOneShotSpecificationRecord, RunAttemptRecord,
        RunObservationRecord,
    },
    run_documents::observation_document,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::json;

use super::{
    ArtifactRegistration, OwnedAttempt, RunStore, RunStoreError,
    attempts::{fence, invalid, timestamp},
};

/// Current execution history for one resolved slot.
#[derive(Debug)]
pub struct SlotProgress {
    pub accepted: bool,
    pub attempts: u64,
    pub active: bool,
}

impl RunStore {
    /// Registers immutable one-shot coordination records in one transaction.
    ///
    /// Gate 2 has unconditional applicability. Its internal decision key remains
    /// separate from the future public applicability protocol and identities.
    pub fn register_one_shot(
        &mut self,
        candidate: RecordId<LogicalCandidateRecord>,
        run: &IdentifiedRecord<ResolvedOneShotSpecificationRecord>,
        attempt: &IdentifiedRecord<ResolvedAttemptSlotRecord>,
        observation: &IdentifiedRecord<ResolvedObservationSlotRecord>,
    ) -> Result<(), RunStoreError> {
        if record_id(&run.record)? != run.id
            || record_id(&attempt.record)? != attempt.id
            || record_id(&observation.record)? != observation.id
        {
            return Err(invalid("registered record does not match its identity"));
        }
        if attempt.record.specification != run.id || observation.record.specification != run.id {
            return Err(invalid("slot and run identities disagree"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let decision = format!("run-store-v1:unconditional:{candidate}");
        let decision_record = json!({"candidate_id": candidate, "applicable": true, "basis": "unconditional_one_shot"}).to_string();
        tx.execute(
            "INSERT INTO applicability_decisions VALUES (?1, ?2, 1, ?3) ON CONFLICT(id) DO NOTHING",
            params![decision, candidate.to_string(), decision_record],
        )?;
        let run_record =
            String::from_utf8(canonical_identity_bytes(&run.record)?).expect("canonical UTF-8");
        tx.execute(
            "INSERT INTO resolved_runs VALUES (?1, ?2, ?3, ?4, 1, ?5) ON CONFLICT(id) DO NOTHING",
            params![
                run.id.to_string(),
                run.record.logical_specification.to_string(),
                candidate.to_string(),
                decision,
                run_record
            ],
        )?;
        let attempt_record =
            String::from_utf8(canonical_identity_bytes(&attempt.record)?).expect("canonical UTF-8");
        tx.execute("INSERT INTO attempt_slots VALUES (?1, ?2, ?3, 'one_shot', 'measurement', ?4) ON CONFLICT(id) DO NOTHING", params![attempt.id.to_string(), attempt.record.logical_slot.to_string(), run.id.to_string(), attempt_record])?;
        let observation_record = String::from_utf8(canonical_identity_bytes(&observation.record)?)
            .expect("canonical UTF-8");
        tx.execute(
            "INSERT INTO observation_slots VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO NOTHING",
            params![
                observation.id.to_string(),
                observation.record.logical_slot.to_string(),
                run.id.to_string(),
                observation_record
            ],
        )?;
        tx.execute("INSERT INTO attempt_observation_slots VALUES (?1, ?2, ?3) ON CONFLICT(observation_slot_id) DO NOTHING", params![attempt.id.to_string(), observation.id.to_string(), run.id.to_string()])?;
        for (table, id, record) in [
            ("resolved_runs", run.id.to_string(), run_record),
            ("attempt_slots", attempt.id.to_string(), attempt_record),
            (
                "observation_slots",
                observation.id.to_string(),
                observation_record,
            ),
        ] {
            let existing: String = tx.query_row(
                &format!("SELECT record_json FROM {table} WHERE id = ?1"),
                [id],
                |r| r.get(0),
            )?;
            if existing != record {
                return Err(invalid("registered identity has conflicting contents"));
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn slot_progress(
        &self,
        slot: RecordId<ResolvedAttemptSlotRecord>,
    ) -> Result<SlotProgress, RunStoreError> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM attempts WHERE slot_id = ?1 AND state = 'accepted'), count(*), coalesce(max(finished_at_ns IS NULL), 0) FROM attempts WHERE slot_id = ?1", [slot.to_string()], |r| Ok(SlotProgress { accepted: r.get(0)?, attempts: r.get::<_, i64>(1)? as u64, active: r.get(2)? }))?)
    }

    pub(crate) fn attempt_is_terminal(
        &self,
        attempt: RecordId<RunAttemptRecord>,
    ) -> Result<bool, RunStoreError> {
        Ok(self.connection.query_row(
            "SELECT finished_at_ns IS NOT NULL FROM attempts WHERE id = ?1",
            [attempt.to_string()],
            |r| r.get(0),
        )?)
    }

    /// Maps a recorded attempt or observation identity back to its execution slot.
    pub fn slots_for_record(
        &self,
        id: &str,
    ) -> Result<Vec<RecordId<ResolvedAttemptSlotRecord>>, RunStoreError> {
        let mut query = self.connection.prepare("SELECT slot_id FROM attempts WHERE id = ?1 UNION SELECT attempt_slot_id FROM observations WHERE id = ?1")?;
        let rows = query
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|id| {
                id.parse()
                    .map_err(|_| invalid("invalid stored slot identity"))
            })
            .collect()
    }

    /// Registers validated observation bytes and optionally selects them atomically.
    /// The caller must own the success claim before selecting a one-shot result.
    pub fn register_observation(
        &mut self,
        attempt: &OwnedAttempt,
        observation: &IdentifiedRecord<RunObservationRecord>,
        artifact: &ArtifactRegistration,
        select: bool,
        now_ns: u64,
    ) -> Result<(), RunStoreError> {
        if record_id(&observation.record)? != observation.id {
            return Err(invalid("observation record does not match its identity"));
        }
        if observation.record.producing_attempt != attempt.id() {
            return Err(invalid("observation belongs to another attempt"));
        }
        let document = String::from_utf8(
            observation_document(&observation.record)
                .map_err(|_| invalid("invalid observation document"))?
                .to_canonical_bytes()?,
        )
        .expect("canonical UTF-8");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = timestamp(now_ns)?;
        fence(&tx, attempt, now)?;
        tx.execute(
            "INSERT INTO observations VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, 'observation', ?7)",
            params![
                observation.id.to_string(),
                observation.record.slot.to_string(),
                attempt.id().to_string(),
                attempt.slot().to_string(),
                match observation.record.completion {
                    metewand_core::records::CompletionStatus::NotRequired => "not_required",
                    metewand_core::records::CompletionStatus::Reached => "reached",
                    metewand_core::records::CompletionStatus::NotReached => "not_reached",
                },
                artifact.id,
                document
            ],
        )?;
        if select {
            let claim: Option<String> = tx
                .query_row(
                    "SELECT attempt_id FROM finalization_claims WHERE slot_id = ?1",
                    [attempt.slot().to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            if claim.as_deref() != Some(&attempt.id().to_string()) {
                return Err(invalid(
                    "selection requires this attempt's finalization claim",
                ));
            }
            tx.execute("INSERT INTO selected_observations VALUES (?1, ?2, 1, ?3) ON CONFLICT(slot_id) DO NOTHING", params![observation.record.slot.to_string(), observation.id.to_string(), now])?;
        }
        tx.commit()?;
        Ok(())
    }
}
