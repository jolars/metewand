use crate::core::{
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
    pub disposition: Option<String>,
    pub accepted: bool,
    pub attempts: u64,
    pub active: bool,
}

impl RunStore {
    /// Persists an immutable support decision without allocating attempt slots.
    pub fn register_applicability(
        &mut self,
        decision: &IdentifiedRecord<crate::core::applicability::ApplicabilityDecisionRecord>,
    ) -> Result<(), RunStoreError> {
        use crate::core::applicability::ApplicabilityOutcome;
        if record_id(&decision.record)? != decision.id
            || decision.record.outcome == ApplicabilityOutcome::Unchecked
        {
            return Err(invalid(
                "applicability decision must be resolved and match its identity",
            ));
        }
        let record = String::from_utf8(canonical_identity_bytes(&decision.record)?)
            .expect("canonical UTF-8");
        self.connection.execute("INSERT INTO applicability_decisions VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO NOTHING",
            params![decision.id.to_string(), decision.record.candidate_id.to_string(), decision.record.outcome == ApplicabilityOutcome::Applicable, record])?;
        let existing: String = self.connection.query_row(
            "SELECT record_json FROM applicability_decisions WHERE id = ?1",
            [decision.id.to_string()],
            |r| r.get(0),
        )?;
        if existing != record {
            return Err(invalid("applicability identity has conflicting contents"));
        }
        Ok(())
    }

    /// Registers immutable one-shot coordination records in one transaction.
    ///
    /// Checked runs require an already registered applicable decision. Legacy
    /// unconditional runs retain their existing coordination identity.
    pub fn register_one_shot(
        &mut self,
        candidate: RecordId<LogicalCandidateRecord>,
        run: &IdentifiedRecord<ResolvedOneShotSpecificationRecord>,
        attempt: &IdentifiedRecord<ResolvedAttemptSlotRecord>,
        observation: &IdentifiedRecord<ResolvedObservationSlotRecord>,
    ) -> Result<(), RunStoreError> {
        self.register_execution(
            candidate,
            run,
            attempt,
            std::slice::from_ref(observation),
            "one_shot",
        )
    }

    pub fn register_streaming(
        &mut self,
        candidate: RecordId<LogicalCandidateRecord>,
        run: &IdentifiedRecord<ResolvedOneShotSpecificationRecord>,
        attempt: &IdentifiedRecord<ResolvedAttemptSlotRecord>,
        observations: &[IdentifiedRecord<ResolvedObservationSlotRecord>],
    ) -> Result<(), RunStoreError> {
        let version: u32 = self
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != super::FRESH_RUN_STORE_SCHEMA_VERSION {
            return Err(invalid(
                "streaming profiles require a version-3 output directory",
            ));
        }
        self.register_execution(candidate, run, attempt, observations, "stream")
    }

    pub fn register_buffered(
        &mut self,
        candidate: RecordId<LogicalCandidateRecord>,
        run: &IdentifiedRecord<ResolvedOneShotSpecificationRecord>,
        attempt: &IdentifiedRecord<ResolvedAttemptSlotRecord>,
        observations: &[IdentifiedRecord<ResolvedObservationSlotRecord>],
    ) -> Result<(), RunStoreError> {
        let version: u32 = self
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if ![
            super::BUFFERED_RUN_STORE_SCHEMA_VERSION,
            super::FRESH_RUN_STORE_SCHEMA_VERSION,
        ]
        .contains(&version)
        {
            return Err(invalid(
                "buffered profiles require a new version-2 output directory",
            ));
        }
        self.register_execution(candidate, run, attempt, observations, "buffered")
    }

    pub(super) fn register_execution(
        &mut self,
        candidate: RecordId<LogicalCandidateRecord>,
        run: &IdentifiedRecord<ResolvedOneShotSpecificationRecord>,
        attempt: &IdentifiedRecord<ResolvedAttemptSlotRecord>,
        observations: &[IdentifiedRecord<ResolvedObservationSlotRecord>],
        kind: &str,
    ) -> Result<(), RunStoreError> {
        let observation = observations
            .first()
            .ok_or_else(|| invalid("execution requires observation slots"))?;
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
        let decision = if let Some(decision) = run.record.applicability {
            let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM applicability_decisions WHERE id = ?1 AND candidate_id = ?2 AND applicable = 1)",
                params![decision.to_string(), candidate.to_string()], |r| r.get(0))?;
            if !valid {
                return Err(invalid("run requires a registered applicable decision"));
            }
            decision.to_string()
        } else {
            let decision = format!("run-store-v1:unconditional:{candidate}");
            let decision_record = json!({"candidate_id": candidate, "applicable": true, "outcome": "applicable", "code": "unconditional_one_shot", "reason": "The implementation declares support for every admitted configuration.", "checked_inputs": null, "basis": "unconditional_one_shot"}).to_string();
            tx.execute("INSERT INTO applicability_decisions VALUES (?1, ?2, 1, ?3) ON CONFLICT(id) DO NOTHING",
                params![decision, candidate.to_string(), decision_record])?;
            decision
        };
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
        tx.execute("INSERT INTO attempt_slots VALUES (?1, ?2, ?3, ?5, 'measurement', ?4) ON CONFLICT(id) DO NOTHING", params![attempt.id.to_string(), attempt.record.logical_slot.to_string(), run.id.to_string(), attempt_record,kind])?;
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
        for extra in &observations[1..] {
            if record_id(&extra.record)? != extra.id || extra.record.specification != run.id {
                return Err(invalid("invalid buffered observation slot"));
            }
            let document = String::from_utf8(canonical_identity_bytes(&extra.record)?)
                .expect("canonical UTF-8");
            tx.execute(
                "INSERT INTO observation_slots VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO NOTHING",
                params![
                    extra.id.to_string(),
                    extra.record.logical_slot.to_string(),
                    run.id.to_string(),
                    document
                ],
            )?;
            let existing: String = tx.query_row(
                "SELECT record_json FROM observation_slots WHERE id=?1",
                [extra.id.to_string()],
                |r| r.get(0),
            )?;
            if existing != document {
                return Err(invalid("conflicting buffered observation identity"));
            }
            tx.execute("INSERT INTO attempt_observation_slots VALUES (?1,?2,?3) ON CONFLICT(observation_slot_id) DO NOTHING",params![attempt.id.to_string(),extra.id.to_string(),run.id.to_string()])?;
        }
        let existing_kind: String = tx.query_row(
            "SELECT kind FROM attempt_slots WHERE id=?1",
            [attempt.id.to_string()],
            |r| r.get(0),
        )?;
        if existing_kind != kind {
            return Err(invalid("conflicting execution delivery kind"));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn slot_progress(
        &self,
        slot: RecordId<ResolvedAttemptSlotRecord>,
    ) -> Result<SlotProgress, RunStoreError> {
        let disposition = if super::fresh::supported(&self.connection)? {
            self.connection
                .query_row(
                    "SELECT reason FROM fresh_dispositions WHERE slot_id=?1",
                    [slot.to_string()],
                    |r| r.get(0),
                )
                .optional()?
        } else {
            None
        };
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM attempts WHERE slot_id = ?1 AND state = 'accepted'), count(*), coalesce(max(finished_at_ns IS NULL), 0) FROM attempts WHERE slot_id = ?1", [slot.to_string()], |r| Ok(SlotProgress { disposition, accepted: r.get(0)?, attempts: r.get::<_, i64>(1)? as u64, active: r.get(2)? }))?)
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
                    crate::core::records::CompletionStatus::NotRequired => "not_required",
                    crate::core::records::CompletionStatus::Reached => "reached",
                    crate::core::records::CompletionStatus::NotReached => "not_reached",
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
        if let Some(crate::core::records::ProfileObservationRecord::Streaming(profile)) =
            &observation.record.profile
        {
            let kind: String = tx.query_row(
                "SELECT kind FROM attempt_slots WHERE id=?1",
                [attempt.slot().to_string()],
                |r| r.get(0),
            )?;
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM events WHERE attempt_id=?1 AND kind='stream_checkpoint'",
                [attempt.id().to_string()],
                |r| r.get(0),
            )?;
            if kind != "stream" || select || count as u64 != profile.checkpoint_sequence {
                return Err(invalid(
                    "streaming checkpoint must commit once, in sequence, without selection",
                ));
            }
            let payload = crate::core::canonical::CanonicalValue::try_from(
                json!({"observation_id":observation.id,"checkpoint":profile}),
            )?
            .to_canonical_bytes()?;
            #[cfg(test)]
            crate::runtime::fault_injection::hit("stream.checkpoint.before_commit");
            tx.execute("INSERT INTO events(attempt_id,occurred_at_ns,kind,payload_json) VALUES (?1,?2,'stream_checkpoint',?3)", params![attempt.id().to_string(), now, String::from_utf8(payload).expect("canonical UTF-8")])?;
        }
        tx.commit()?;
        #[cfg(test)]
        if matches!(
            observation.record.profile,
            Some(crate::core::records::ProfileObservationRecord::Streaming(
                _
            ))
        ) {
            crate::runtime::fault_injection::hit("stream.checkpoint.after_commit");
        }
        Ok(())
    }
}
