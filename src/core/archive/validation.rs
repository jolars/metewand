//! Validates the coordination relationships that an archive must preserve.

use std::collections::{BTreeMap, BTreeSet};

use crate::core::{
    applicability::ApplicabilityDecisionRecord,
    archive::{ArchiveEvidence, RESULT_ARCHIVE_VERSION},
    canonical::CanonicalValue,
    identity::{IdentityKind, hash_identity, run_attempt_id},
    records::{
        RecordId, ResolvedAttemptSlotRecord, ResolvedObservationSlotRecord,
        ResolvedOneShotSpecificationRecord, RunObservationRecord,
    },
};
use serde_json::{Value, json};

use super::ArchiveEvidenceError;

pub(super) fn validate(evidence: &ArchiveEvidence) -> Result<(), ArchiveEvidenceError> {
    if evidence.schema_version != RESULT_ARCHIVE_VERSION {
        return Err(ArchiveEvidenceError::UnsupportedVersion);
    }
    let mut tables: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for record in &evidence.records {
        if let super::ArchiveRecord::Attempt(attempt) = record
            && (attempt.finished_at_ns.is_none()
                || !["accepted", "failed", "censored", "superseded"]
                    .contains(&attempt.state.as_str()))
        {
            return Err(ArchiveEvidenceError::Unsettled);
        }
        tables
            .entry(record.kind())
            .or_default()
            .push(record.value());
    }
    let rows = |kind: &str| tables.get(kind).map_or(&[][..], Vec::as_slice);
    let applicability = index(rows("applicability"), "id")?;
    let runs = index(rows("resolved-specification"), "id")?;
    let slots = index(rows("resolved-attempt-slot"), "id")?;
    let observation_slots = index(rows("resolved-observation-slot"), "id")?;
    let attempts = index(rows("attempt"), "id")?;
    let observations = index(rows("observation"), "id")?;
    let artifacts = index(rows("stored-artifact"), "id")?;
    let members = index(rows("slot-membership"), "observation_slot_id")?;
    let selections = index(rows("observation-selection"), "observation_id")?;
    unique(rows("stored-artifact"), &["path"])?;
    unique(
        rows("resolved-attempt-slot"),
        &["logical_slot_id", "resolved_run_id"],
    )?;
    unique(
        rows("resolved-observation-slot"),
        &["logical_slot_id", "resolved_run_id"],
    )?;
    unique(rows("attempt"), &["slot_id", "retry_index"])?;
    unique(rows("observation-selection"), &["slot_id"])?;
    unique(rows("event"), &["sequence"])?;
    unique(rows("attempt-transition"), &["attempt_id", "revision"])?;
    unique(rows("attempt-transition"), &["event_sequence"])?;
    unique(
        rows("attempt-artifact"),
        &["attempt_id", "role", "artifact_id"],
    )?;
    unique(
        rows("observation-artifact"),
        &["observation_id", "role", "artifact_id"],
    )?;
    for artifact in artifacts.values() {
        if ![
            "attempt",
            "observation",
            "result",
            "metrics",
            "dataset",
            "source",
            "environment",
            "other",
        ]
        .contains(&text(artifact, "kind")?)
            || !["file", "directory"].contains(&text(artifact, "storage_kind")?)
            || (artifact["storage_kind"] == "directory" && artifact["manifest_sha256"].is_null())
        {
            return Err(invalid("unsupported or incomplete artifact registration"));
        }
        if text(artifact, "id")?.starts_with("run-store-v1:")
            && artifact["id"]
                != format!(
                    "run-store-v1:{}-{}",
                    text(artifact, "kind")?,
                    text(artifact, "sha256")?
                )
        {
            return Err(invalid(
                "stored artifact identity disagrees with its content hash",
            ));
        }
    }
    for decision in applicability.values() {
        if !["applicable", "excluded"].contains(&text(&decision["definition"], "outcome")?) {
            return Err(invalid("unsupported persisted applicability outcome"));
        }
        if text(decision, "id")?.starts_with("run-store-v1:unconditional:") {
            if decision["id"]
                != format!(
                    "run-store-v1:unconditional:{}",
                    text(decision, "candidate_id")?
                )
                || decision["applicable"] != true
            {
                return Err(invalid("invalid legacy unconditional applicability"));
            }
        } else {
            verify_identity::<ApplicabilityDecisionRecord>(
                text(decision, "id")?,
                &decision["definition"],
            )?;
        }
        if decision["definition"]["candidate_id"] != decision["candidate_id"]
            || (decision["definition"]["outcome"] == "applicable")
                != (decision["applicable"] == true)
        {
            return Err(invalid(
                "applicability projection disagrees with its definition",
            ));
        }
    }
    for run in runs.values() {
        verify_identity::<ResolvedOneShotSpecificationRecord>(
            text(run, "id")?,
            &run["definition"],
        )?;
        if run["definition"]["logical_specification"] != run["logical_specification_id"]
            || (!run["definition"]["applicability"].is_null()
                && run["definition"]["applicability"] != run["applicability_id"])
        {
            return Err(invalid(
                "resolved run projection disagrees with its definition",
            ));
        }

        let decision = reference(&applicability, run, "applicability_id")?;
        if decision["applicable"] != true || decision["candidate_id"] != run["candidate_id"] {
            return Err(invalid(
                "resolved run has inconsistent applicability evidence",
            ));
        }
    }
    for slot in slots.values().chain(observation_slots.values()) {
        reference(&runs, slot, "resolved_run_id")?;
        if slot["definition"]["logical_slot"] != slot["logical_slot_id"]
            || slot["definition"]["specification"] != slot["resolved_run_id"]
        {
            return Err(invalid("slot projection disagrees with its definition"));
        }
    }
    for slot in observation_slots.values() {
        verify_identity::<ResolvedObservationSlotRecord>(text(slot, "id")?, &slot["definition"])?;
    }
    for slot in slots.values() {
        verify_identity::<ResolvedAttemptSlotRecord>(text(slot, "id")?, &slot["definition"])?;
        if !["measurement", "warmup"].iter().any(|v| slot["role"] == *v)
            || !["one_shot", "buffered", "fresh_observation", "stream"]
                .iter()
                .any(|v| slot["kind"] == *v)
        {
            return Err(invalid("unsupported attempt slot role or delivery kind"));
        }
    }
    for member in members.values() {
        let slot = reference(&slots, member, "attempt_slot_id")?;
        let observation = reference(&observation_slots, member, "observation_slot_id")?;
        if slot["resolved_run_id"] != member["resolved_run_id"]
            || observation["resolved_run_id"] != member["resolved_run_id"]
        {
            return Err(invalid("observation membership crosses resolved runs"));
        }
    }
    for slot in observation_slots.values() {
        reference(&members, slot, "id")?;
    }
    let mut accepted_slots = BTreeSet::new();
    for attempt in attempts.values() {
        reference(&slots, attempt, "slot_id")?;
        if !["accepted", "failed", "censored", "superseded"]
            .iter()
            .any(|v| attempt["state"] == *v)
            || attempt["finished_at_ns"].is_null()
        {
            return Err(ArchiveEvidenceError::Unsettled);
        }
        let artifact = reference(&artifacts, attempt, "artifact_id")?;
        if artifact["kind"] != "attempt" {
            return Err(invalid(
                "terminal attempt references the wrong artifact kind",
            ));
        }
        if (attempt["state"] == "failed") != !attempt["failure_code"].is_null() {
            return Err(invalid("failure code disagrees with the attempt state"));
        }
        let start = timestamp(attempt, "started_at_ns")?;
        let finished = timestamp(attempt, "finished_at_ns")?;
        if finished < start || attempt["updated_at_ns"] != attempt["finished_at_ns"] {
            return Err(invalid("inconsistent terminal attempt timestamps"));
        }
        if attempt["state"] == "accepted" && !accepted_slots.insert(text(attempt, "slot_id")?) {
            return Err(invalid("several accepted attempts occupy one slot"));
        }
        if let Ok(slot) =
            text(attempt, "slot_id")?.parse::<crate::core::records::RecordId<ResolvedAttemptSlotRecord>>()
        {
            let expected =
                run_attempt_id(slot, integer(attempt, "retry_index")?).map_err(evidence_error)?;
            if expected.to_string() != text(attempt, "id")? {
                return Err(invalid(
                    "attempt identity disagrees with its slot and retry index",
                ));
            }
        }
        let mut transitions = rows("attempt-transition")
            .iter()
            .filter(|r| r["attempt_id"] == attempt["id"])
            .collect::<Vec<_>>();
        transitions.sort_by_key(|row| row["revision"].as_u64());
        if transitions.len() as u64
            != integer(attempt, "revision")?
                .checked_add(1)
                .ok_or_else(|| invalid("attempt revision overflows"))?
        {
            return Err(invalid("attempt transition history is incomplete"));
        }
        let mut previous: Option<&Value> = None;
        for (revision, transition) in transitions.iter().enumerate() {
            if integer(transition, "revision")? != revision as u64 {
                return Err(invalid("attempt revisions are not contiguous"));
            }
            if let Some(previous) = previous {
                if !allowed_transition(text(previous, "to_state")?, text(transition, "to_state")?) {
                    return Err(invalid("unsupported attempt state transition"));
                }
                if transition["previous_revision"] != previous["revision"]
                    || transition["from_state"] != previous["to_state"]
                    || timestamp(transition, "occurred_at_ns")?
                        < timestamp(previous, "occurred_at_ns")?
                {
                    return Err(invalid("attempt transitions do not form an ordered chain"));
                }
            } else if transition["to_state"] != "reserved"
                || !transition["from_state"].is_null()
                || !transition["previous_revision"].is_null()
                || timestamp(transition, "occurred_at_ns")? != start
            {
                return Err(invalid(
                    "attempt history does not start with its reservation",
                ));
            }
            let event = rows("event")
                .iter()
                .find(|event| event["sequence"] == transition["event_sequence"])
                .ok_or_else(|| invalid("attempt transition lacks its event"))?;
            if event["attempt_id"] != attempt["id"]
                || event["occurred_at_ns"] != transition["occurred_at_ns"]
            {
                return Err(invalid("attempt transition disagrees with its event"));
            }
            previous = Some(transition);
        }
        if previous.is_none_or(|last| {
            last["to_state"] != attempt["state"]
                || last["occurred_at_ns"] != attempt["finished_at_ns"]
        }) {
            return Err(invalid(
                "attempt projection disagrees with its transition history",
            ));
        }
    }
    for transition in rows("attempt-transition") {
        reference(&attempts, transition, "attempt_id")?;
    }
    for event in rows("event") {
        if !event["attempt_id"].is_null() {
            reference(&attempts, event, "attempt_id")?;
        }
    }
    for observation in observations.values() {
        verify_observation(observation)?;
        let attempt = reference(&attempts, observation, "attempt_id")?;
        let member = reference(&members, observation, "slot_id")?;
        let artifact = reference(&artifacts, observation, "artifact_id")?;
        if attempt["slot_id"] != observation["attempt_slot_id"]
            || member["attempt_slot_id"] != observation["attempt_slot_id"]
            || artifact["kind"] != "observation"
            || observation["valid"] != true
        {
            return Err(invalid(
                "observation disagrees with its producing attempt, slot, or artifact",
            ));
        }
        if observation["selected"] == true {
            let selection = reference(&selections, observation, "id")?;
            if selection["slot_id"] != observation["slot_id"] || attempt["state"] != "accepted" {
                return Err(invalid(
                    "selected observation has no accepted producing attempt",
                ));
            }
        } else if selections.contains_key(text(observation, "id")?) {
            return Err(invalid(
                "observation selection flag disagrees with the selection record",
            ));
        }
    }
    for selection in selections.values() {
        reference(&observations, selection, "observation_id")?;
    }
    for attempt in attempts
        .values()
        .filter(|attempt| attempt["state"] == "accepted")
    {
        let slot = reference(&slots, attempt, "slot_id")?;
        if slot["role"] == "measurement" {
            let produced = observations
                .values()
                .filter(|observation| observation["attempt_id"] == attempt["id"])
                .collect::<Vec<_>>();
            if produced.is_empty()
                || produced
                    .iter()
                    .any(|observation| observation["selected"] != true)
            {
                return Err(invalid(
                    "accepted measurement lacks its selected observations",
                ));
            }
        }
    }
    for edge in rows("attempt-artifact") {
        reference(&attempts, edge, "attempt_id")?;
        reference(&artifacts, edge, "artifact_id")?;
    }
    for edge in rows("observation-artifact") {
        reference(&observations, edge, "observation_id")?;
        reference(&artifacts, edge, "artifact_id")?;
    }
    validate_fresh(&tables, &runs, &slots, &attempts, &members)?;
    Ok(())
}

fn validate_fresh(
    tables: &BTreeMap<&str, Vec<Value>>,
    runs: &BTreeMap<&str, &Value>,
    slots: &BTreeMap<&str, &Value>,
    attempts: &BTreeMap<&str, &Value>,
    memberships: &BTreeMap<&str, &Value>,
) -> Result<(), ArchiveEvidenceError> {
    let rows = |kind: &str| tables.get(kind).map_or(&[][..], Vec::as_slice);
    let members = index(rows("fresh-member"), "slot_id")?;
    unique(
        rows("fresh-member"),
        &["resolved_run_id", "measurement_index", "schedule_index"],
    )?;
    unique(rows("fresh-budget"), &["attempt_id"])?;
    unique(rows("fresh-disposition"), &["slot_id"])?;
    unique(
        rows("fresh-policy"),
        &["resolved_run_id", "measurement_index"],
    )?;
    unique(
        rows("fresh-sequence"),
        &["resolved_specification_id", "measurement_index"],
    )?;
    for member in members.values() {
        let slot = reference(slots, member, "slot_id")?;
        let membership = reference(memberships, member, "observation_slot_id")?;
        if slot["resolved_run_id"] != member["resolved_run_id"]
            || slot["kind"] != "fresh_observation"
            || membership["attempt_slot_id"] != member["slot_id"]
        {
            return Err(invalid("inconsistent fresh-sequence membership"));
        }
        if !rows("fresh-policy").iter().any(|p| {
            p["resolved_run_id"] == member["resolved_run_id"]
                && p["measurement_index"] == member["measurement_index"]
        }) {
            return Err(invalid("fresh member has no sequence policy"));
        }
    }
    for slot in slots
        .values()
        .filter(|slot| slot["kind"] == "fresh_observation")
    {
        reference(&members, slot, "id")?;
    }
    for budget in rows("fresh-budget") {
        let attempt = reference(attempts, budget, "attempt_id")?;
        reference(&members, attempt, "slot_id")?;
        if budget["charged_ns"]
            .as_u64()
            .is_some_and(|charged| charged > budget["reserved_ns"].as_u64().unwrap_or(0))
            || budget["reserved_ns"] == 0
        {
            return Err(invalid("fresh time charge exceeds its reservation"));
        }
    }
    for disposition in rows("fresh-disposition") {
        let member = reference(&members, disposition, "slot_id")?;
        let cause = reference(attempts, disposition, "causing_attempt_id")?;
        let causing_member = reference(&members, cause, "slot_id")?;
        if member["resolved_run_id"] != causing_member["resolved_run_id"]
            || member["measurement_index"] != causing_member["measurement_index"]
            || !["skipped_target_reached", "active_time_limit"]
                .iter()
                .any(|reason| disposition["reason"] == *reason)
        {
            return Err(invalid(
                "fresh stopping disposition crosses sequences or has an unsupported reason",
            ));
        }
    }
    for policy in rows("fresh-policy") {
        reference(runs, policy, "resolved_run_id")?;
        if !rows("fresh-sequence").iter().any(|p| {
            p["resolved_specification_id"] == policy["resolved_run_id"]
                && p["measurement_index"] == policy["measurement_index"]
        }) {
            return Err(invalid("fresh policy lacks its progress record"));
        }
    }
    for progress in rows("fresh-sequence") {
        let run = reference(runs, progress, "resolved_specification_id")?;
        if progress["logical_specification_id"] != run["logical_specification_id"]
            || !rows("fresh-policy").iter().any(|p| {
                p["resolved_run_id"] == progress["resolved_specification_id"]
                    && p["measurement_index"] == progress["measurement_index"]
            })
        {
            return Err(invalid("fresh progress lacks a matching run and policy"));
        }
        let sequence_members = members
            .values()
            .filter(|member| {
                member["resolved_run_id"] == progress["resolved_specification_id"]
                    && member["measurement_index"] == progress["measurement_index"]
            })
            .collect::<Vec<_>>();
        let accepted = attempts
            .values()
            .filter(|attempt| {
                attempt["state"] == "accepted"
                    && sequence_members
                        .iter()
                        .any(|member| member["slot_id"] == attempt["slot_id"])
            })
            .count() as u64;
        if integer(progress, "scheduled_points")? != sequence_members.len() as u64
            || integer(progress, "accepted_points")? != accepted
        {
            return Err(invalid(
                "fresh progress counts disagree with committed attempts",
            ));
        }
        let mut consumed = 0_u64;
        let mut conservative = 0_u64;
        for budget in rows("fresh-budget") {
            let attempt = reference(attempts, budget, "attempt_id")?;
            if sequence_members
                .iter()
                .any(|member| member["slot_id"] == attempt["slot_id"])
            {
                let reserved = integer(budget, "reserved_ns")?;
                let charged = budget["charged_ns"].as_u64().unwrap_or(reserved);
                consumed = consumed
                    .checked_add(charged)
                    .ok_or_else(|| invalid("fresh time total overflows"))?;
                if budget["charged_ns"].is_null() {
                    conservative = conservative
                        .checked_add(reserved)
                        .ok_or_else(|| invalid("fresh conservative total overflows"))?;
                }
            }
        }
        let policy = rows("fresh-policy")
            .iter()
            .find(|p| {
                p["resolved_run_id"] == progress["resolved_specification_id"]
                    && p["measurement_index"] == progress["measurement_index"]
            })
            .expect("policy checked above");
        let policy: crate::core::profile::ProfilePolicy =
            serde_json::from_value(policy["policy"].clone()).map_err(evidence_error)?;
        policy.validate().map_err(evidence_error)?;
        let limit = policy.active_time_ns().map_err(evidence_error)?;
        let target = rows("observation").iter().any(|observation| {
            observation["completion"] == "reached"
                && attempts
                    .get(observation["attempt_id"].as_str().unwrap_or(""))
                    .is_some_and(|attempt| {
                        attempt["state"] == "accepted"
                            && sequence_members
                                .iter()
                                .any(|member| member["slot_id"] == attempt["slot_id"])
                    })
        });
        let attempted = sequence_members
            .iter()
            .filter(|member| {
                attempts
                    .values()
                    .any(|attempt| attempt["slot_id"] == member["slot_id"])
            })
            .count() as u64;
        let (status, reason) = if target {
            ("accepted", Some("target_reached"))
        } else if policy.metric.is_none() && accepted == sequence_members.len() as u64 {
            ("accepted", Some("fixed_count"))
        } else if consumed >= limit {
            ("censored", Some("active_time_limit"))
        } else if attempted == sequence_members.len() as u64 {
            ("censored", Some("schedule_exhausted"))
        } else {
            ("partial", None)
        };
        if progress["status"] != status
            || progress["reason"] != serde_json::to_value(reason).expect("reason serializes")
        {
            return Err(invalid(
                "fresh progress status disagrees with its committed observations and policy",
            ));
        }
        if integer(progress, "consumed_time_ns")? != consumed
            || integer(progress, "conservative_time_ns")? != conservative
        {
            return Err(invalid(
                "fresh progress accounting disagrees with its reservations",
            ));
        }
    }
    Ok(())
}

fn index<'a>(
    rows: &'a [Value],
    key: &str,
) -> Result<BTreeMap<&'a str, &'a Value>, ArchiveEvidenceError> {
    let mut output = BTreeMap::new();
    for row in rows {
        if output.insert(text(row, key)?, row).is_some() {
            return Err(invalid("duplicate evidence identity or coordination key"));
        }
    }
    Ok(output)
}

fn unique(rows: &[Value], keys: &[&str]) -> Result<(), ArchiveEvidenceError> {
    let mut seen = BTreeSet::new();
    for row in rows {
        let key = keys
            .iter()
            .map(|key| row[key].to_string())
            .collect::<Vec<_>>();
        if !seen.insert(key) {
            return Err(invalid("duplicate evidence coordination key"));
        }
    }
    Ok(())
}

fn reference<'a>(
    index: &BTreeMap<&str, &'a Value>,
    row: &Value,
    key: &str,
) -> Result<&'a Value, ArchiveEvidenceError> {
    index
        .get(text(row, key)?)
        .copied()
        .ok_or_else(|| invalid(format!("missing referenced evidence: {key}")))
}
fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str, ArchiveEvidenceError> {
    row[key]
        .as_str()
        .ok_or_else(|| invalid(format!("missing string evidence field: {key}")))
}
fn integer(row: &Value, key: &str) -> Result<u64, ArchiveEvidenceError> {
    row[key]
        .as_u64()
        .ok_or_else(|| invalid(format!("missing integer evidence field: {key}")))
}
fn timestamp(row: &Value, key: &str) -> Result<u64, ArchiveEvidenceError> {
    text(row, key)?
        .parse()
        .map_err(|_| invalid("invalid evidence timestamp"))
}

fn invalid(message: impl Into<String>) -> ArchiveEvidenceError {
    ArchiveEvidenceError::Invalid(message.into())
}
fn evidence_error(error: impl std::fmt::Display) -> ArchiveEvidenceError {
    invalid(error.to_string())
}

fn verify_identity<T: IdentityKind>(
    id: &str,
    representation: &Value,
) -> Result<(), ArchiveEvidenceError> {
    if representation["version"] != crate::core::IDENTITY_VERSION {
        return Err(invalid("unsupported canonical identity version"));
    }
    let expected = id.parse::<RecordId<T>>().map_err(evidence_error)?;
    let canonical = CanonicalValue::try_from(representation.clone())
        .map_err(evidence_error)?
        .to_canonical_bytes()
        .map_err(evidence_error)?;
    if expected != RecordId::from_digest(hash_identity::<T>(&canonical)) {
        return Err(invalid(
            "canonical identity does not match its recorded representation",
        ));
    }
    Ok(())
}

fn verify_observation(row: &Value) -> Result<(), ArchiveEvidenceError> {
    let document = &row["definition"];
    if document["id"] != row["id"]
        || document["slot_id"] != row["slot_id"]
        || document["attempt_id"] != row["attempt_id"]
        || document["validity"]["valid"] != row["valid"]
        || document["validity"]["completion"] != row["completion"]
    {
        return Err(invalid(
            "observation projection disagrees with its document",
        ));
    }
    let duration = |key: &str| -> Result<Value, ArchiveEvidenceError> {
        if document["timing"][key].is_null() {
            return Ok(Value::Null);
        }
        let ns = integer(&document["timing"], key)?;
        Ok(json!({"seconds":ns/1_000_000_000,"nanoseconds":ns%1_000_000_000}))
    };
    let mut representation = json!({"version":crate::core::IDENTITY_VERSION,"slot":row["slot_id"],"producing_attempt":row["attempt_id"],"result":document["result"],"metrics":document["metrics"],"completion":row["completion"],"timing":{
        "primary_time":document["timing"]["primary_time"],"timed_wall_time":duration("timed_wall_time_ns")?,"implementation_time":duration("implementation_time_ns")?,"evaluation_time":duration("evaluation_time_ns")?,"cpu_time":duration("cpu_time_ns")?,"peak_rss_bytes":document["timing"]["peak_rss_bytes"]}});
    if let Some(profile) = document.get("profile") {
        representation["profile"] = profile.clone();
    }
    verify_identity::<RunObservationRecord>(text(row, "id")?, &representation)
}

fn allowed_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("reserved", "materializing")
            | ("materializing", "starting")
            | ("starting", "preparing")
            | ("preparing", "executing")
            | ("executing", "checkpointing" | "validating")
            | ("checkpointing", "validating")
            | ("validating", "evaluating")
            | ("evaluating", "executing")
            | (
                "reserved"
                    | "materializing"
                    | "starting"
                    | "preparing"
                    | "executing"
                    | "checkpointing"
                    | "validating"
                    | "evaluating",
                "finalizing"
            )
            | (
                "finalizing",
                "accepted" | "censored" | "superseded" | "failed"
            )
    )
}
