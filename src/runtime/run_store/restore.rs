//! Rebuild local coordination from verified, terminal portable evidence.

use std::{fs, path::Path};

use rusqlite::{Transaction, TransactionBehavior, params};

use crate::core::{
    archive::{ArchiveEvidence, ArchiveRecord},
    canonical::CanonicalValue,
};

use super::{
    RunStore, RunStoreError,
    attempts::{invalid, timestamp},
    durable::verify_schema,
};

impl RunStore {
    pub(crate) fn restore(root: &Path, evidence: &ArchiveEvidence) -> Result<(), RunStoreError> {
        evidence
            .validate()
            .map_err(|_| invalid("restoration requires valid terminal evidence"))?;
        for name in [
            "run.sqlite3",
            "run.sqlite3-wal",
            "run.sqlite3-shm",
            "run.sqlite3-journal",
            ".run-store-init.lock",
            ".run-store-recovery.lock",
        ] {
            match fs::symlink_metadata(root.join(name)) {
                Ok(_) => {
                    return Err(invalid(
                        "archive payload collides with run coordination files",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        // A saved subset does not reveal every delivery in its original plan.
        // The current layout lets that plan continue without changing its records.
        let mut store = Self::open_fresh(root)?;
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Verified terminal histories enter as a whole. Lifecycle triggers
        // govern new execution, while deferred foreign keys check this replay.
        let triggers = tx
            .prepare("SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' ORDER BY name")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (name, _) in &triggers {
            tx.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
        }
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        for record in &evidence.records {
            insert(&tx, record)?;
        }
        for (_, sql) in triggers {
            tx.execute_batch(&sql)?;
        }
        tx.commit()?;
        verify_schema(&store.connection)?;
        // Publication must contain a self-contained database after all WAL
        // handles close, so a new host needs no process-local sidecars.
        store
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        drop(store);
        crate::runtime::platform_fs::sync_owned_file(&root.join("run.sqlite3"))?;
        Ok(())
    }
}

fn json(value: &serde_json::Value) -> Result<String, RunStoreError> {
    Ok(
        String::from_utf8(CanonicalValue::try_from(value.clone())?.to_canonical_bytes()?)
            .expect("canonical JSON is UTF-8"),
    )
}

fn insert(tx: &Transaction<'_>, record: &ArchiveRecord) -> Result<(), RunStoreError> {
    match record {
        ArchiveRecord::Applicability(r) => {
            tx.execute("INSERT INTO applicability_decisions (id,candidate_id,applicable,record_json) VALUES (?1,?2,?3,?4)", params![r.id,r.candidate_id,r.applicable,json(&r.definition)?])?;
        }
        ArchiveRecord::ResolvedSpecification(r) => {
            tx.execute("INSERT INTO resolved_runs (id,logical_specification_id,candidate_id,applicability_id,record_json) VALUES (?1,?2,?3,?4,?5)", params![r.id,r.logical_specification_id,r.candidate_id,r.applicability_id,json(&r.definition)?])?;
        }
        ArchiveRecord::ResolvedAttemptSlot(r) => {
            tx.execute("INSERT INTO attempt_slots (id,logical_slot_id,resolved_run_id,kind,role,record_json) VALUES (?1,?2,?3,?4,?5,?6)", params![r.id,r.logical_slot_id,r.resolved_run_id,r.kind,r.role,json(&r.definition)?])?;
        }
        ArchiveRecord::ResolvedObservationSlot(r) => {
            tx.execute("INSERT INTO observation_slots (id,logical_slot_id,resolved_run_id,record_json) VALUES (?1,?2,?3,?4)", params![r.id,r.logical_slot_id,r.resolved_run_id,json(&r.definition)?])?;
        }
        ArchiveRecord::SlotMembership(r) => {
            tx.execute("INSERT INTO attempt_observation_slots (attempt_slot_id,observation_slot_id,resolved_run_id) VALUES (?1,?2,?3)", params![r.attempt_slot_id,r.observation_slot_id,r.resolved_run_id])?;
        }
        ArchiveRecord::Attempt(r) => {
            if r.lease_expires_at_ns.is_some() {
                return Err(invalid("restoration cannot retain active ownership leases"));
            }
            tx.execute("INSERT INTO attempts (id,slot_id,retry_index,state,revision,started_at_ns,updated_at_ns,finished_at_ns,failure_code,final_artifact_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![r.id,r.slot_id,timestamp(r.retry_index)?,r.state,timestamp(r.revision)?,r.started_at_ns,r.updated_at_ns,r.finished_at_ns,r.failure_code,r.artifact_id])?;
        }
        ArchiveRecord::AttemptTransition(r) => {
            tx.execute("INSERT INTO attempt_transitions (attempt_id,revision,previous_revision,from_state,to_state,occurred_at_ns,event_sequence) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![r.attempt_id,timestamp(r.revision)?,r.previous_revision.map(timestamp).transpose()?,r.from_state,r.to_state,r.occurred_at_ns,timestamp(r.event_sequence)?])?;
        }
        ArchiveRecord::Observation(r) => {
            tx.execute("INSERT INTO observations (id,slot_id,producing_attempt_id,attempt_slot_id,valid,completion,artifact_id,record_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![r.id,r.slot_id,r.attempt_id,r.attempt_slot_id,r.valid,r.completion,r.artifact_id,json(&r.definition)?])?;
        }
        ArchiveRecord::ObservationSelection(r) => {
            tx.execute("INSERT INTO selected_observations (slot_id,observation_id,selected_at_ns) VALUES (?1,?2,?3)", params![r.slot_id,r.observation_id,r.selected_at_ns])?;
        }
        ArchiveRecord::StoredArtifact(r) => {
            tx.execute("INSERT INTO artifacts (id,kind,relative_path,content_sha256,storage_kind,manifest_sha256,verified_at_ns) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![r.id,r.kind,r.path,r.sha256,r.storage_kind,r.manifest_sha256,r.verified_at_ns])?;
        }
        ArchiveRecord::AttemptArtifact(r) => {
            tx.execute(
                "INSERT INTO attempt_artifacts (attempt_id,role,artifact_id) VALUES (?1,?2,?3)",
                params![r.attempt_id, r.role, r.artifact_id],
            )?;
        }
        ArchiveRecord::ObservationArtifact(r) => {
            tx.execute("INSERT INTO observation_artifacts (observation_id,role,artifact_id) VALUES (?1,?2,?3)", params![r.observation_id,r.role,r.artifact_id])?;
        }
        ArchiveRecord::Event(r) => {
            tx.execute("INSERT INTO events (sequence,attempt_id,occurred_at_ns,kind,payload_json) VALUES (?1,?2,?3,?4,?5)", params![timestamp(r.sequence)?,r.attempt_id,r.occurred_at_ns,r.kind,json(&r.payload)?])?;
        }
        ArchiveRecord::FreshPolicy(r) => {
            tx.execute("INSERT INTO fresh_sequences (resolved_run_id,measurement_index,policy_json) VALUES (?1,?2,?3)", params![r.resolved_run_id,timestamp(r.measurement_index)?,json(&r.policy)?])?;
        }
        ArchiveRecord::FreshMember(r) => {
            tx.execute("INSERT INTO fresh_members (slot_id,resolved_run_id,measurement_index,schedule_index,control_json) VALUES (?1,?2,?3,?4,?5)", params![r.slot_id,r.resolved_run_id,timestamp(r.measurement_index)?,timestamp(r.schedule_index)?,json(&r.control)?])?;
        }
        ArchiveRecord::FreshBudget(r) => {
            tx.execute(
                "INSERT INTO fresh_budget (attempt_id,reserved_ns,charged_ns) VALUES (?1,?2,?3)",
                params![
                    r.attempt_id,
                    timestamp(r.reserved_ns)?,
                    r.charged_ns.map(timestamp).transpose()?
                ],
            )?;
        }
        ArchiveRecord::FreshDisposition(r) => {
            tx.execute("INSERT INTO fresh_dispositions (slot_id,reason,causing_attempt_id) VALUES (?1,?2,?3)", params![r.slot_id,r.reason,r.causing_attempt_id])?;
        }
        // Progress is derived from the restored policies, charges, and selections.
        ArchiveRecord::FreshSequence(_) => (),
    }
    Ok(())
}
