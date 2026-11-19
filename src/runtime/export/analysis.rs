//! Analysis uses persisted evidence, never the exporting host or current sources.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    core::{
        export::{ExportRecordType, analysis::*, measurement},
        profile::FreshSequenceProgress,
    },
    runtime::{
        evidence::document_profile,
        inspection::{
            Inspection, adaptive_decisions, calibration_slots, calibration_specifications,
        },
    },
};

use super::{ExportError, Record, canonical_bytes, hex, text};

pub(super) fn project(
    records: &[Record<'_>],
    inspection: &Inspection,
    tables: &BTreeMap<&str, Vec<Value>>,
) -> Result<AnalysisSnapshot, ExportError> {
    let graph = inspection.records();
    let mut snapshot = AnalysisSnapshot {
        adaptive_measurements: inspection.adaptive_measurements().to_vec(),
        adaptive_budgets: adaptive_decisions(tables)?,
        schema_version: ANALYSIS_SCHEMA_VERSION,
        candidates: Vec::new(),
        observations: Vec::new(),
        attempts: Vec::new(),
        slots: Vec::new(),
        applicability: Vec::new(),
        profiles: Vec::new(),
        provenance: BTreeMap::new(),
        compatibility_classes: BTreeMap::new(),
        completeness: AnalysisCompleteness::default(),
    };
    let mut comparisons = inspection
        .observation_comparisons()
        .collect::<BTreeMap<_, _>>();
    let candidate_ids = records
        .iter()
        .map(|r| r.candidate_id.to_owned())
        .chain(
            tables["resolved-specification"]
                .iter()
                .filter_map(|r| text(r, "candidate_id")),
        )
        .collect::<BTreeSet<_>>();
    for id in candidate_ids {
        let record = records
            .iter()
            .filter(|r| r.candidate_id == id)
            .max_by_key(|r| {
                (
                    r.provenance()["data"]["context"]["analysis"].is_object(),
                    r.kind == ExportRecordType::Observation,
                )
            });
        let context = record
            .map(|r| &r.provenance()["data"]["context"])
            .unwrap_or(&Value::Null);
        let display = &context["comparison"];
        let settings = &context["analysis"];
        snapshot.candidates.push(AnalysisCandidate {
            id,
            experiment: text(display, "experiment"),
            implementation: text(display, "implementation"),
            dataset: text(display, "dataset"),
            problem: text(display, "problem"),
            case: text(display, "case"),
            parameters: display["parameters"].clone(),
            implementation_repetitions: settings["implementation_repetitions"].as_u64(),
            measurement_repetitions: settings["measurement_repetitions"].as_u64(),
            execution_policy: context["execution_policy"].clone(),
            observation_policy: observation_policy(context, record),
        });
    }
    for record in records {
        match record.kind {
            ExportRecordType::Observation => {
                let mut comparison =
                    comparisons
                        .remove(record.id)
                        .ok_or(ExportError::InvalidEvidence(
                            "observation is missing its comparison projection",
                        ))?;
                if comparison
                    .profile
                    .as_ref()
                    .is_some_and(|p| p["delivery"] == "buffered_profile")
                {
                    let candidate = snapshot
                        .candidates
                        .iter()
                        .find(|c| c.id == record.candidate_id)
                        .expect("registered candidate");
                    if let Some(bound) =
                        candidate.observation_policy["profile"]["policy"]["max-observations"]
                            .as_u64()
                            .filter(|bound| *bound > 0)
                    {
                        comparison.measurement_index =
                            comparison.measurement_index.map(|index| index / bound);
                    }
                }
                snapshot.compatibility_classes.insert(
                    record.compatibility_id.clone(),
                    record.compatibility.clone(),
                );
                let context = &record.provenance()["data"]["context"];
                let provenance = AnalysisProvenance {
                    tool_version: text(record.provenance(), "tool_version"),
                    benchmark: text(&context["analysis"], "benchmark"),
                    manifest_hash: text(context, "manifest_hash"),
                    runner: context["runner"].clone(),
                    host: context["host_observation"].clone(),
                    repository: context["repository"].clone(),
                    dataset: context
                        .get("dataset_materialization")
                        .filter(|d| !d.is_null())
                        .unwrap_or(&context["dataset_instance"])
                        .clone(),
                    implementation: context["implementation"].clone(),
                    evaluator: context["evaluator"].clone(),
                };
                let provenance_id = hex(&Sha256::digest(canonical_bytes(&provenance)?));
                snapshot
                    .provenance
                    .insert(provenance_id.clone(), provenance);
                let profile = document_profile(&record.row["document"])
                    .or_else(|| record.slot.get("fresh_sequence"));
                snapshot.observations.push(AnalysisObservation {
                    comparison,
                    warmup: record.warmup(),
                    implementation_time_ns: record.timing()["implementation_time_ns"].as_u64(),
                    evaluation_time_ns: measurement(
                        record.timing(),
                        record.provenance(),
                        "evaluation_time_ns",
                    )
                    .0,
                    active_time_ns: profile.and_then(|p| p["active_time_ns"].as_u64()),
                    elapsed_wall_time_ns: profile.and_then(|p| p["elapsed_wall_time_ns"].as_u64()),
                    statistics: record.provenance()["data"]["implementation_statistics"].clone(),
                    compatibility_id: record.compatibility_id.clone(),
                    provenance_id,
                });
            }
            ExportRecordType::Attempt => {
                snapshot.attempts.push(AnalysisAttempt {
                    id: record.id.to_owned(),
                    candidate_id: record.candidate_id.to_owned(),
                    slot_id: record.slot_id.map(str::to_owned),
                    retry_index: record.attempt["retry_index"].as_u64(),
                    state: text(record.attempt, "state"),
                    failure_code: text(record.attempt, "failure_code"),
                    warmup: record.warmup(),
                    started_at_ns: text(record.row, "started_at_ns"),
                    finished_at_ns: text(record.row, "finished_at_ns"),
                });
            }
            ExportRecordType::Applicability => {
                snapshot.applicability.push(AnalysisApplicability {
                    id: record.id.to_owned(),
                    candidate_id: record.candidate_id.to_owned(),
                    applicable: record.row["applicable"] == true,
                    evidence: record.row.clone(),
                });
            }
            _ => (),
        }
    }
    let pilot_slots = calibration_slots(tables);
    for slot in tables["resolved-attempt-slot"].iter().filter(|s| {
        s["role"] == "measurement" && !s["id"].as_str().is_some_and(|id| pilot_slots.contains(id))
    }) {
        let id = required_text(slot, "id")?;
        let run_id = required_text(slot, "resolved_run_id")?;
        let run = graph
            .get(&run_id)
            .ok_or(ExportError::InvalidEvidence("slot has no resolved run"))?;
        let candidate_id = required_text(&run.record, "candidate_id")?;
        let attempts = snapshot
            .attempts
            .iter()
            .filter(|a| a.slot_id.as_deref() == Some(&id))
            .collect::<Vec<_>>();
        let disposition = tables
            .get("fresh-disposition")
            .and_then(|rows| rows.iter().find(|d| d["slot_id"] == id))
            .and_then(|d| d["reason"].as_str());
        let state = if attempts
            .iter()
            .any(|a| a.state.as_deref() == Some("accepted"))
        {
            "satisfied"
        } else if attempts.iter().any(|a| a.finished_at_ns.is_none()) {
            "active"
        } else if disposition == Some("skipped_target_reached") {
            "skipped_target_reached"
        } else if disposition.is_some() {
            "censored"
        } else if attempts.is_empty() {
            "pending"
        } else {
            "retry_required"
        };
        let selected_observation_ids = tables["observation"]
            .iter()
            .filter(|o| o["attempt_slot_id"] == id && o["selected"] == true)
            .filter_map(|o| text(o, "id"))
            .collect();
        snapshot.slots.push(AnalysisSlot {
            id,
            candidate_id,
            resolved_specification_id: run_id,
            state: state.into(),
            selected_observation_ids,
        });
    }
    if let Some(progress) = tables.get("fresh-sequence") {
        let calibration_runs = calibration_specifications(tables);
        for row in progress {
            let progress: FreshSequenceProgress = serde_json::from_value(row.clone())?;
            if calibration_runs.contains(progress.resolved_specification_id.as_str()) {
                continue;
            }
            let run = graph
                .get(&progress.resolved_specification_id)
                .ok_or(ExportError::InvalidEvidence("profile has no resolved run"))?;
            snapshot.profiles.push(AnalysisProfile {
                candidate_id: required_text(&run.record, "candidate_id")?,
                resolved_specification_id: progress.resolved_specification_id,
                measurement_index: Some(progress.measurement_index),
                delivery: "fresh_sequence".into(),
                status: progress.status,
                reason: progress.reason,
                progress: row.clone(),
            });
        }
    }
    for slot in &snapshot.slots {
        if snapshot.profiles.iter().any(|p| {
            p.delivery == "fresh_sequence"
                && p.resolved_specification_id == slot.resolved_specification_id
        }) {
            continue;
        }
        let candidate = snapshot
            .candidates
            .iter()
            .find(|c| c.id == slot.candidate_id)
            .expect("registered candidate");
        if candidate.observation_policy["kind"] != "profile" {
            continue;
        }
        let observations = snapshot
            .observations
            .iter()
            .filter(|o| {
                o.comparison.candidate_id == slot.candidate_id
                    && slot
                        .selected_observation_ids
                        .iter()
                        .any(|id| o.comparison.observation_id.as_ref() == Some(id))
            })
            .collect::<Vec<_>>();
        let policy = &candidate.observation_policy["profile"]["policy"];
        let has_target = policy["metric"].is_string();
        let finished = slot.state == "satisfied"
            && if has_target {
                observations
                    .iter()
                    .any(|o| o.comparison.completion.as_deref() == Some("reached"))
            } else {
                policy["max-observations"]
                    .as_u64()
                    .is_some_and(|count| observations.len() as u64 == count)
            };
        let attempt_record = records
            .iter()
            .find(|r| r.kind == ExportRecordType::Attempt && r.slot_id == Some(slot.id.as_str()));
        let context = attempt_record.map(|r| &r.provenance()["data"]["context"]);
        snapshot.profiles.push(AnalysisProfile {
            candidate_id: slot.candidate_id.clone(),
            resolved_specification_id: slot.resolved_specification_id.clone(),
            measurement_index: context
                .and_then(|c| c["scheduling"]["comparison_block"]["measurement_index"].as_u64())
                .map(|index| {
                    if candidate.observation_policy["profile"]["binding"]["delivery"]
                        == "buffered_profile"
                    {
                        index
                            / policy["max-observations"]
                                .as_u64()
                                .filter(|bound| *bound > 0)
                                .unwrap_or(1)
                    } else {
                        index
                    }
                }),
            delivery: candidate.observation_policy["profile"]["binding"]["delivery"]
                .as_str()
                .unwrap_or("unavailable")
                .into(),
            status: if finished { "accepted" } else { "partial" }.into(),
            reason: finished.then(|| {
                if has_target {
                    "target_reached"
                } else {
                    "fixed_count"
                }
                .into()
            }),
            progress: Value::Null,
        });
    }
    snapshot.observations.sort_by_key(|o| {
        (
            o.comparison.candidate_id.clone(),
            o.comparison.implementation_repetition,
            o.comparison.measurement_index,
            o.comparison.profile.as_ref().and_then(|p| {
                p["schedule_index"]
                    .as_u64()
                    .or_else(|| p["checkpoint_sequence"].as_u64())
                    .or_else(|| p["trace_index"].as_u64())
            }),
            o.comparison.retry_index,
            o.comparison.observation_id.clone(),
        )
    });
    snapshot.slots.sort_by(|a, b| a.id.cmp(&b.id));
    snapshot.profiles.sort_by(|a, b| {
        (
            &a.candidate_id,
            &a.resolved_specification_id,
            a.measurement_index,
        )
            .cmp(&(
                &b.candidate_id,
                &b.resolved_specification_id,
                b.measurement_index,
            ))
    });
    snapshot.check_completeness();
    Ok(snapshot)
}

pub(super) fn select(mut snapshot: AnalysisSnapshot, records: &[&Record<'_>]) -> AnalysisSnapshot {
    let candidates = records
        .iter()
        .map(|r| r.candidate_id)
        .collect::<BTreeSet<_>>();
    let ids = records.iter().map(|r| r.id).collect::<BTreeSet<_>>();
    snapshot
        .adaptive_measurements
        .retain(|series| candidates.contains(series.candidate_id.as_str()));
    snapshot.adaptive_budgets.retain(|budget| {
        budget
            .pilots
            .iter()
            .any(|pilot| candidates.contains(pilot.candidate_id.as_str()))
    });
    snapshot
        .candidates
        .retain(|c| candidates.contains(c.id.as_str()));
    snapshot
        .slots
        .retain(|s| candidates.contains(s.candidate_id.as_str()));
    snapshot
        .profiles
        .retain(|p| candidates.contains(p.candidate_id.as_str()));
    snapshot
        .applicability
        .retain(|a| candidates.contains(a.candidate_id.as_str()));
    snapshot
        .observations
        .retain(|o| candidates.contains(o.comparison.candidate_id.as_str()));
    snapshot.check_completeness();
    snapshot.observations.retain(|o| {
        o.comparison
            .observation_id
            .as_deref()
            .is_some_and(|id| ids.contains(id))
    });
    snapshot.attempts.retain(|a| ids.contains(a.id.as_str()));
    let metadata = snapshot
        .observations
        .iter()
        .map(|o| o.provenance_id.as_str())
        .collect::<BTreeSet<_>>();
    snapshot
        .provenance
        .retain(|id, _| metadata.contains(id.as_str()));
    let classes = snapshot
        .observations
        .iter()
        .map(|o| o.compatibility_id.as_str())
        .collect::<BTreeSet<_>>();
    snapshot
        .compatibility_classes
        .retain(|id, _| classes.contains(id.as_str()));
    snapshot
}

fn required_text(value: &Value, key: &str) -> Result<String, ExportError> {
    text(value, key).ok_or(ExportError::InvalidEvidence(
        "analysis record lacks an identity",
    ))
}

fn observation_policy(context: &Value, record: Option<&Record<'_>>) -> Value {
    if context.is_null() {
        return Value::Null;
    }
    if context["analysis"]["observation_policy"].is_object() {
        return context["analysis"]["observation_policy"].clone();
    }
    if context["fresh_sequence"]["policy"].is_object() {
        return json!({"kind":"profile","profile":{"policy":context["fresh_sequence"]["policy"],"binding":{"delivery":"fresh_sequence"}}});
    }
    for (key, delivery) in [
        ("streaming_profile", "streaming_profile"),
        ("buffered_profile", "buffered_profile"),
    ] {
        if context[key].is_object() {
            return json!({"kind":"profile","profile":{"policy":context[key]["policy"],"binding":{"delivery":delivery}}});
        }
    }
    if let Some(profile) =
        record.and_then(|r| r.provenance()["data"]["execution"].get("profile_policy"))
    {
        return json!({"kind":"profile","profile":{"policy":profile["policy"],"binding":{"delivery":"buffered_profile"}}});
    }
    json!({"kind":"one_shot"})
}
