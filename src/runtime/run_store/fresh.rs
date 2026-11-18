//! Fresh executions share a durable budget and a terminal target decision.

use std::{fs, io, path::Path};

use crate::core::{
    canonical::CanonicalValue,
    materialization::hex,
    profile::{FreshControl, FreshSequenceProgress, ProfilePolicy},
    records::{
        ContentDigest, RecordId, ResolvedAttemptSlotRecord, ResolvedOneShotSpecificationRecord,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    OwnedAttempt, RunStore, RunStoreError, TerminalOutcome,
    attempts::{fence, invalid, timestamp},
};

pub(super) fn supported(db: &Connection) -> Result<bool, RunStoreError> {
    Ok(db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))? == 3)
}

pub(crate) struct FreshPointRegistration<'a> {
    pub measurement_index: u64,
    pub control: &'a FreshControl,
    pub policy: &'a ProfilePolicy,
}

pub(super) fn register_member(
    tx: &Transaction<'_>,
    run: RecordId<ResolvedOneShotSpecificationRecord>,
    attempt: RecordId<ResolvedAttemptSlotRecord>,
    point: FreshPointRegistration<'_>,
) -> Result<(), RunStoreError> {
    let FreshPointRegistration {
        measurement_index,
        control,
        policy,
    } = point;
    let policy = canonical(serde_json::to_value(policy).expect("profile policy"))?;
    let control_json = canonical(control.observation_control.clone())?;
    tx.execute(
        "INSERT INTO fresh_sequences VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",
        params![run.to_string(), timestamp(measurement_index)?, policy],
    )?;
    let previous: String = tx.query_row(
        "SELECT policy_json FROM fresh_sequences WHERE resolved_run_id=?1 AND measurement_index=?2",
        params![run.to_string(), timestamp(measurement_index)?],
        |r| r.get(0),
    )?;
    if previous != policy {
        return Err(invalid("conflicting fresh-sequence policy"));
    }
    tx.execute(
        "INSERT INTO fresh_members VALUES (?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",
        params![
            attempt.to_string(),
            run.to_string(),
            timestamp(measurement_index)?,
            timestamp(control.schedule_index)?,
            control_json
        ],
    )?;
    let previous: (String,u64,u64,String) = tx.query_row("SELECT resolved_run_id,measurement_index,schedule_index,control_json FROM fresh_members WHERE slot_id=?1", [attempt.to_string()], |r| Ok((r.get(0)?,unsigned(r,1)?,unsigned(r,2)?,r.get(3)?)))?;
    if previous
        != (
            run.to_string(),
            measurement_index,
            control.schedule_index,
            control_json,
        )
    {
        return Err(invalid("conflicting fresh-sequence membership"));
    }
    Ok(())
}

impl RunStore {
    /// Reserve immediately before entering the selected timing scope, after warm-ups.
    pub(crate) fn reserve_fresh_time(
        &mut self,
        attempt: &OwnedAttempt,
        point_limit: Option<u64>,
        now: u64,
    ) -> Result<Option<u64>, RunStoreError> {
        let tx = self.transaction()?;
        fence(&tx, attempt, timestamp(now)?)?;
        let Some((run, index)) = membership(&tx, &attempt.slot().to_string())? else {
            return Ok(None);
        };
        let (limit, used) = allowance(&tx, &run, index)?;
        let remaining = limit.saturating_sub(used);
        let reserved = remaining.min(point_limit.unwrap_or(remaining));
        if reserved == 0 {
            return Err(invalid("fresh-sequence active time is exhausted"));
        }
        tx.execute(
            "INSERT INTO fresh_budget VALUES (?1,?2,NULL)",
            params![attempt.id().to_string(), timestamp(reserved)?],
        )?;
        tx.commit()?;
        Ok(Some(reserved))
    }

    /// Record the measured charge now, but refund only in the terminal transaction.
    /// Cleanup after a deadline cannot overdraw its reservation; the full observed
    /// interval remains in the attempt's timing measurements.
    pub(crate) fn record_fresh_time(
        &mut self,
        attempt: &OwnedAttempt,
        measured: u64,
        now: u64,
    ) -> Result<(), RunStoreError> {
        let tx = self.transaction()?;
        fence(&tx, attempt, timestamp(now)?)?;
        tx.execute(
            "UPDATE fresh_budget SET charged_ns=min(?2,reserved_ns) WHERE attempt_id=?1",
            params![attempt.id().to_string(), timestamp(measured)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn fresh_budget_exhausted(
        &self,
        attempt: &OwnedAttempt,
    ) -> Result<bool, RunStoreError> {
        let Some((run, index)) = membership(&self.connection, &attempt.slot().to_string())? else {
            return Ok(false);
        };
        let (limit, used) = allowance(&self.connection, &run, index)?;
        let refund:u64=self.connection.query_row("SELECT coalesce((SELECT reserved_ns-coalesce(charged_ns,reserved_ns) FROM fresh_budget WHERE attempt_id=?1),0)",[attempt.id().to_string()],|r|unsigned(r,0))?;
        Ok(used.saturating_sub(refund) >= limit)
    }

    pub fn fresh_progress(&self) -> Result<Vec<FreshSequenceProgress>, RunStoreError> {
        progress(&self.connection)
    }
}

fn canonical(value: Value) -> Result<String, RunStoreError> {
    Ok(
        String::from_utf8(CanonicalValue::try_from(value)?.to_canonical_bytes()?)
            .expect("canonical UTF-8"),
    )
}

fn membership(db: &Connection, slot: &str) -> Result<Option<(String, u64)>, RunStoreError> {
    if !supported(db)? {
        return Ok(None);
    }
    Ok(db
        .query_row(
            "SELECT resolved_run_id,measurement_index FROM fresh_members WHERE slot_id=?1",
            [slot],
            |r| Ok((r.get(0)?, unsigned(r, 1)?)),
        )
        .optional()?)
}

fn allowance(db: &Connection, run: &str, index: u64) -> Result<(u64, u64), RunStoreError> {
    let policy: String = db.query_row(
        "SELECT policy_json FROM fresh_sequences WHERE resolved_run_id=?1 AND measurement_index=?2",
        params![run, timestamp(index)?],
        |r| r.get(0),
    )?;
    let policy: ProfilePolicy =
        serde_json::from_str(&policy).map_err(|_| invalid("invalid persisted profile policy"))?;
    let limit = policy
        .active_time_ns()
        .map_err(|_| invalid("invalid persisted active-time bound"))?;
    let used = db.query_row("SELECT coalesce(sum(CASE WHEN a.finished_at_ns IS NULL THEN b.reserved_ns ELSE coalesce(b.charged_ns,b.reserved_ns) END),0) FROM fresh_budget b JOIN attempts a ON a.id=b.attempt_id JOIN fresh_members m ON m.slot_id=a.slot_id WHERE m.resolved_run_id=?1 AND m.measurement_index=?2",params![run,timestamp(index)?],|r|unsigned(r,0))?;
    Ok((limit, used))
}

pub(super) fn check_reservation(db: &Connection, slot: &str) -> Result<(), RunStoreError> {
    let Some((run, index)) = membership(db, slot)? else {
        if supported(db)?
            && db.query_row(
                "SELECT kind='fresh_observation' FROM attempt_slots WHERE id=?1",
                [slot],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(invalid(
                "fresh slot requires complete repetition membership before reservation",
            ));
        }
        return Ok(());
    };
    let skipped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fresh_dispositions WHERE slot_id=?1)",
        [slot],
        |r| r.get(0),
    )?;
    if skipped {
        return Err(RunStoreError::SatisfiedSlot);
    }
    let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM attempts a JOIN fresh_members m ON m.slot_id=a.slot_id WHERE m.resolved_run_id=?1 AND m.measurement_index=?2 AND a.finished_at_ns IS NULL)",params![run,timestamp(index)?],|r|r.get(0))?;
    if active {
        return Err(RunStoreError::SequenceBusy);
    }
    let (limit, used) = allowance(db, &run, index)?;
    if used >= limit {
        return Err(RunStoreError::SatisfiedSlot);
    }
    Ok(())
}

/// The same prospective effect is embedded in the immutable outcome and committed.
fn effect(
    db: &Connection,
    attempt: &OwnedAttempt,
    outcome: &TerminalOutcome,
) -> Result<Option<&'static str>, RunStoreError> {
    let Some((run, index)) = membership(db, &attempt.slot().to_string())? else {
        return Ok(None);
    };
    let target: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM observations WHERE producing_attempt_id=?1 AND completion='reached')",[attempt.id().to_string()],|r|r.get(0))?;
    if matches!(outcome, TerminalOutcome::Accepted) && target {
        return Ok(Some("skipped_target_reached"));
    }
    let (limit, used) = allowance(db, &run, index)?;
    let refund: u64 = db.query_row("SELECT coalesce((SELECT reserved_ns-coalesce(charged_ns,reserved_ns) FROM fresh_budget WHERE attempt_id=?1),0)",[attempt.id().to_string()],|r|unsigned(r,0))?;
    Ok((used.saturating_sub(refund) >= limit).then_some("active_time_limit"))
}

pub(super) fn finish(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    outcome: &TerminalOutcome,
) -> Result<(), RunStoreError> {
    let Some(reason) = effect(tx, attempt, outcome)? else {
        return Ok(());
    };
    let (run, index) = membership(tx, &attempt.slot().to_string())?.expect("fresh member");
    tx.execute("INSERT INTO fresh_dispositions SELECT m.slot_id,?3,?4 FROM fresh_members m WHERE m.resolved_run_id=?1 AND m.measurement_index=?2 AND NOT EXISTS(SELECT 1 FROM attempts a WHERE a.slot_id=m.slot_id AND a.state='accepted') AND (m.slot_id!=?5 OR ?6!='accepted') ON CONFLICT DO NOTHING",params![run,timestamp(index)?,reason,attempt.id().to_string(),attempt.slot().to_string(),outcome.state()])?;
    Ok(())
}

pub(crate) const MEMBERSHIP_ROLE: &str = "fresh-membership";

fn membership_digest(bytes: &[u8]) -> String {
    hex(ContentDigest::new(Sha256::digest(bytes).into()))
}

pub(crate) fn membership_address(run: &str, index: u64) -> (String, String) {
    let key = membership_digest(format!("{run}/{index}").as_bytes());
    (
        format!("fresh-membership-v1:{key}"),
        format!("fresh-memberships/{key}.json"),
    )
}

pub(crate) fn membership_document(
    run: &str,
    index: u64,
    policy: Value,
    members: Vec<Value>,
) -> Value {
    json!({"schema_version":1,"resolved_run_id":run,"measurement_index":index,"policy":policy,"members":members})
}

fn repetition(db: &Connection, run: &str, index: u64) -> Result<Value, RunStoreError> {
    let mut query=db.prepare("SELECT json_object('slot_id',slot_id,'schedule_index',schedule_index,'control',json(control_json)) FROM fresh_members WHERE resolved_run_id=?1 AND measurement_index=?2 ORDER BY schedule_index")?;
    let members = query
        .query_map(params![run, timestamp(index)?], |r| r.get::<_, String>(0))?
        .map(|r| {
            serde_json::from_str::<Value>(&r?).map_err(|_| invalid("invalid fresh membership"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let policy: String = db.query_row(
        "SELECT policy_json FROM fresh_sequences WHERE resolved_run_id=?1 AND measurement_index=?2",
        params![run, timestamp(index)?],
        |r| r.get(0),
    )?;
    let policy = serde_json::from_str(&policy).map_err(|_| invalid("invalid profile policy"))?;
    Ok(membership_document(run, index, policy, members))
}

impl RunStore {
    pub(crate) fn publish_fresh_repetitions(
        &mut self,
        repetitions: impl IntoIterator<Item = (RecordId<ResolvedOneShotSpecificationRecord>, u64)>,
        now: u64,
    ) -> Result<(), RunStoreError> {
        let root = self.root.clone();
        let tx = self.transaction()?;
        let now = timestamp(now)?;
        for (run, index) in repetitions {
            publish_membership(&tx, &root, &run.to_string(), index, now)?;
        }
        tx.commit()?;
        Ok(())
    }
}

pub(super) fn ensure_membership(
    tx: &Transaction<'_>,
    root: &Path,
    slot: &str,
    now: i64,
) -> Result<(), RunStoreError> {
    if let Some((run, index)) = membership(tx, slot)? {
        publish_membership(tx, root, &run, index, now)?;
    }
    Ok(())
}

fn publish_membership(
    tx: &Transaction<'_>,
    root: &Path,
    run: &str,
    index: u64,
    now: i64,
) -> Result<(), RunStoreError> {
    let (id, relative) = membership_address(run, index);
    if registered_membership(tx, &id, &relative)? {
        return Ok(());
    }
    let bytes = CanonicalValue::try_from(repetition(tx, run, index)?)?.to_canonical_bytes()?;
    let destination = root.join(&relative);
    let parent = destination.parent().expect("membership directory");
    match fs::create_dir(parent) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    if !crate::runtime::platform_fs::is_real_directory(&fs::symlink_metadata(parent)?) {
        return Err(RunStoreError::UnsafePath(parent.to_owned()));
    }
    crate::runtime::platform_fs::complete_namespace_change(root)?;
    let stage = tempfile::tempdir_in(parent)?;
    let source = stage.path().join("membership.json");
    crate::runtime::publication::write_file(&source, &bytes)?;
    match crate::runtime::publication::rename_no_replace(&source, &destination) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !crate::runtime::platform_fs::is_regular_file(&fs::symlink_metadata(&destination)?)
                || fs::read(&destination)? != bytes
            {
                return Err(invalid("conflicting durable fresh membership"));
            }
            crate::runtime::platform_fs::sync_owned_file(&destination)?;
        }
        Err(error) => return Err(error.into()),
    }
    crate::runtime::platform_fs::complete_namespace_change(parent)?;
    tx.execute(
        "INSERT INTO artifacts VALUES (?1,'other',?2,?3,'file',NULL,?4)",
        params![id, relative, membership_digest(&bytes), now],
    )?;
    Ok(())
}

pub(super) fn membership_reference(
    db: &Connection,
    slot: &str,
) -> Result<Option<(String, String)>, RunStoreError> {
    let Some((run, index)) = membership(db, slot)? else {
        return Ok(None);
    };
    let (id, path) = membership_address(&run, index);
    Ok(registered_membership(db, &id, &path)?.then(|| (MEMBERSHIP_ROLE.to_owned(), id)))
}

fn registered_membership(db: &Connection, id: &str, path: &str) -> Result<bool, RunStoreError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE id=?1 AND relative_path=?2 AND kind='other' AND storage_kind='file')",
        params![id, path],
        |r| r.get(0),
    )?)
}

pub(super) fn verify_membership_reference(
    db: &Connection,
    slot: &str,
    reference: &(String, String),
) -> Result<(), RunStoreError> {
    if membership_reference(db, slot)?.as_ref() != Some(reference) {
        return Err(invalid(
            "fresh membership reference disagrees with its repetition",
        ));
    }
    let (run, index) = membership(db, slot)?.expect("referenced repetition");
    let expected = CanonicalValue::try_from(repetition(db, &run, index)?)?.to_canonical_bytes()?;
    let digest: String = db.query_row(
        "SELECT content_sha256 FROM artifacts WHERE id=?1",
        [&reference.1],
        |r| r.get(0),
    )?;
    if digest != membership_digest(&expected) {
        return Err(invalid(
            "durable fresh membership disagrees with coordination records",
        ));
    }
    Ok(())
}

pub(super) fn snapshot(
    tx: &Transaction<'_>,
    attempt: &OwnedAttempt,
    outcome: &TerminalOutcome,
    referenced: bool,
) -> Result<Option<Value>, RunStoreError> {
    let Some((run, index)) = membership(tx, &attempt.slot().to_string())? else {
        return Ok(None);
    };
    let budget: Option<(u64, Option<u64>)> = tx
        .query_row(
            "SELECT reserved_ns,charged_ns FROM fresh_budget WHERE attempt_id=?1",
            [attempt.id().to_string()],
            |r| {
                Ok((
                    unsigned(r, 0)?,
                    r.get::<_, Option<i64>>(1)?.map(|n| n as u64),
                ))
            },
        )
        .optional()?;
    let mut snapshot = json!({
        "resolved_run_id": run,
        "measurement_index": index,
        "budget": budget.map(|(reserved, measured)| json!({"reserved_ns":reserved,"charged_ns":measured})),
        "disposition": effect(tx, attempt, outcome)?,
    });
    if !referenced {
        let repetition = repetition(tx, &run, index)?;
        snapshot["policy"] = repetition["policy"].clone();
        snapshot["members"] = repetition["members"].clone();
    }
    Ok(Some(snapshot))
}

pub(crate) fn progress(db: &Connection) -> Result<Vec<FreshSequenceProgress>, RunStoreError> {
    if !supported(db)? {
        return Ok(vec![]);
    }
    let mut query=db.prepare("SELECT s.resolved_run_id,r.logical_specification_id,s.measurement_index,s.policy_json FROM fresh_sequences s JOIN resolved_runs r ON r.id=s.resolved_run_id ORDER BY s.resolved_run_id,s.measurement_index")?;
    let sequences = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                unsigned(r, 2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    sequences.into_iter().map(|(run,logical,index,policy)| {
        let policy:ProfilePolicy=serde_json::from_str(&policy).map_err(|_|invalid("invalid profile policy"))?;
        let count:u64=db.query_row("SELECT count(*) FROM fresh_members WHERE resolved_run_id=?1 AND measurement_index=?2",params![run,timestamp(index)?],|r|unsigned(r,0))?;
        let (accepted,attempted,active,target):(u64,u64,bool,bool)=db.query_row("SELECT count(DISTINCT CASE WHEN a.state='accepted' THEN m.slot_id END),count(DISTINCT CASE WHEN a.finished_at_ns IS NOT NULL THEN m.slot_id END),coalesce(max(a.finished_at_ns IS NULL AND a.id IS NOT NULL),0),coalesce(max(a.state='accepted' AND EXISTS(SELECT 1 FROM observations o WHERE o.producing_attempt_id=a.id AND o.completion='reached')),0) FROM fresh_members m LEFT JOIN attempts a ON a.slot_id=m.slot_id WHERE m.resolved_run_id=?1 AND m.measurement_index=?2",params![run,timestamp(index)?],|r|Ok((unsigned(r,0)?,unsigned(r,1)?,r.get(2)?,r.get(3)?)))?;
        let (limit,used)=allowance(db,&run,index)?;
        let conservative:u64=db.query_row("SELECT coalesce(sum(CASE WHEN a.finished_at_ns IS NULL THEN b.reserved_ns-coalesce(b.charged_ns,0) WHEN b.charged_ns IS NULL THEN b.reserved_ns ELSE 0 END),0) FROM fresh_budget b JOIN attempts a ON a.id=b.attempt_id JOIN fresh_members m ON m.slot_id=a.slot_id WHERE m.resolved_run_id=?1 AND m.measurement_index=?2",params![run,timestamp(index)?],|r|unsigned(r,0))?;
        let (status,reason)=if target { ("accepted",Some("target_reached")) } else if policy.metric.is_none() && accepted==count { ("accepted",Some("fixed_count")) } else if active { ("partial",None) } else if used>=limit { ("censored",Some("active_time_limit")) } else if attempted==count { ("censored",Some("schedule_exhausted")) } else { ("partial",None) };
        Ok(FreshSequenceProgress { resolved_specification_id:run,logical_specification_id:logical,measurement_index:index,scheduled_points:count,accepted_points:accepted,consumed_time_ns:used,conservative_time_ns:conservative,status:status.into(),reason:reason.map(str::to_owned) })
    }).collect()
}

fn unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
