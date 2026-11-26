use std::collections::{BTreeMap, BTreeSet};

use crate::core::{
    profile::{FreshSequenceProgress, ProfilePolicy},
    records::{
        ProblemInstanceRecord, RecordId, ResolvedAttemptSlotRecord,
        ResolvedOneShotSpecificationRecord,
    },
    run_output::{ProfileGroup, ProfileOutcomes, ProfileRunStatus, ProfileTarget},
};
use crate::runtime::inspection::Inspection;
use serde_json::Value;

#[derive(Default)]
pub(super) struct Selection {
    groups: BTreeMap<(String, RecordId<ProblemInstanceRecord>), Group>,
}

struct Group {
    summary: ProfileGroup,
    trajectories: BTreeSet<String>,
    fresh: BTreeMap<(RecordId<ResolvedOneShotSpecificationRecord>, u64), BTreeSet<String>>,
}

impl Selection {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add(
        &mut self,
        experiment: &str,
        problem_instance: RecordId<ProblemInstanceRecord>,
        comparison: &Value,
        policy: &ProfilePolicy,
        slot: RecordId<ResolvedAttemptSlotRecord>,
        specification: RecordId<ResolvedOneShotSpecificationRecord>,
        fresh_index: Option<u64>,
    ) {
        let group = self.groups.entry((experiment.into(), problem_instance)).or_insert_with(|| Group {
            summary: ProfileGroup {
                experiment: experiment.into(),
                problem_instance_id: problem_instance.to_string(),
                dataset: comparison["dataset"].as_str().unwrap_or("unit").into(),
                problem: comparison["problem"].as_str().unwrap_or("").into(),
                parameters: serde_json::json!({
                    "dataset": comparison["parameters"]["dataset"].as_object().cloned().unwrap_or_default(),
                    "problem": comparison["parameters"]["problem"].as_object().cloned().unwrap_or_default(),
                }),
                target: policy.metric.as_ref().map(|metric| ProfileTarget {
                    metric: metric.clone(),
                    direction: policy.direction.expect("validated target"),
                    threshold: policy.threshold.expect("validated target"),
                    reached: 0,
                }),
                ..ProfileGroup::default()
            },
            trajectories: BTreeSet::new(),
            fresh: BTreeMap::new(),
        });
        if let Some(index) = fresh_index {
            group
                .fresh
                .entry((specification, index))
                .or_default()
                .insert(slot.to_string());
        } else {
            group.trajectories.insert(slot.to_string());
        }
    }

    pub(super) fn summarize(
        self,
        inspection: &Inspection,
        fresh: &[FreshSequenceProgress],
        bound_reached: bool,
        blocked_calibration: bool,
        busy_repetitions: &BTreeSet<(RecordId<ResolvedOneShotSpecificationRecord>, u64)>,
    ) -> Option<ProfileOutcomes> {
        if self.groups.is_empty() {
            return None;
        }
        let mut attempts = BTreeMap::new();
        let mut active = BTreeSet::new();
        let mut superseded = BTreeSet::new();
        for attempt in inspection.run_attempts() {
            if attempt.finished_at_ns.is_none() {
                active.insert(attempt.slot_id.as_str());
            } else if attempt.state == "superseded" {
                superseded.insert(attempt.slot_id.as_str());
            } else {
                let best = attempts.entry(attempt.slot_id.as_str()).or_insert(attempt);
                if best.state != "accepted"
                    && (attempt.state == "accepted" || attempt.retry_index > best.retry_index)
                {
                    *best = attempt;
                }
            }
        }
        let reached: BTreeSet<_> = inspection
            .run_observations()
            .iter()
            .filter(|observation| observation.valid && observation.completion == "reached")
            .map(|observation| observation.attempt_id.as_str())
            .collect();
        let records = inspection.records();
        let slot = |id: &str| {
            let attempt = attempts.get(id);
            if let Some(attempt) = attempt.filter(|a| a.state == "accepted") {
                return Outcome::Accepted(reached.contains(attempt.id.as_str()));
            }
            if active.contains(id) {
                return Outcome::Blocked;
            }
            if let Some(attempt) = attempt {
                let row = &records[&attempt.id].record;
                if attempt.state == "censored" {
                    return Outcome::Censored(
                        row["outcome"]["details"]["reason"]
                            .as_str()
                            .or_else(|| row["document"]["outcome"]["reason"].as_str())
                            .unwrap_or("unknown"),
                    );
                }
                if row["failure_code"] != "interrupted_attempt" {
                    return Outcome::Failed;
                }
            }
            match records
                .get(id)
                .and_then(|r| r.record["fresh_sequence"]["disposition"].as_str())
            {
                Some("skipped_target_reached") => Outcome::Accepted(true),
                Some("active_time_limit") if attempt.is_none() => {
                    Outcome::Censored("active_time_limit")
                }
                Some(_) => Outcome::Blocked,
                None if attempt.is_none() && superseded.contains(id) => Outcome::Blocked,
                None => Outcome::Pending,
            }
        };
        let progress: BTreeMap<_, _> = fresh
            .iter()
            .map(|p| {
                (
                    (p.resolved_specification_id.as_str(), p.measurement_index),
                    p,
                )
            })
            .collect();
        let mut blocked = blocked_calibration;
        let mut groups = Vec::new();
        for group in self.groups.into_values() {
            let mut summary = group.summary;
            let mut count = |outcome, hit| {
                if hit && let Some(target) = &mut summary.target {
                    target.reached += 1;
                }
                match outcome {
                    Outcome::Accepted(_) => summary.accepted += 1,
                    Outcome::Censored(reason) => {
                        summary.censored += 1;
                        *summary
                            .censoring_reasons
                            .entry(reason.to_owned())
                            .or_default() += 1;
                    }
                    Outcome::Failed => summary.failed += 1,
                    Outcome::Blocked => {
                        blocked = true;
                        summary.unfinished += 1;
                    }
                    Outcome::Pending => summary.unfinished += 1,
                    Outcome::Partial => summary.partial += 1,
                }
            };
            for id in group.trajectories {
                let outcome = slot(&id);
                count(outcome, matches!(outcome, Outcome::Accepted(true)));
            }
            for ((specification, index), points) in group.fresh {
                let mut failure = false;
                let mut unfinished = false;
                let mut busy = false;
                for point in &points {
                    match slot(point) {
                        Outcome::Failed => failure = true,
                        Outcome::Blocked => busy = true,
                        Outcome::Pending => unfinished = true,
                        _ => {}
                    }
                }
                let specification_id = specification.to_string();
                let progress = progress.get(&(specification_id.as_str(), index));
                // An accepted point can reach the target while another point remains failed.
                let hit = progress.is_some_and(|p| p.reason.as_deref() == Some("target_reached"));
                let outcome = if failure {
                    Outcome::Failed
                } else if busy || (unfinished && busy_repetitions.contains(&(specification, index)))
                {
                    Outcome::Blocked
                } else if unfinished {
                    Outcome::Pending
                } else {
                    match progress {
                        Some(_) if hit => Outcome::Accepted(true),
                        Some(p) if p.reason.as_deref() == Some("fixed_count") => {
                            Outcome::Accepted(false)
                        }
                        Some(p) if p.reason.as_deref() == Some("active_time_limit") => {
                            Outcome::Censored("active_time_limit")
                        }
                        Some(p)
                            if p.reason.as_deref() == Some("schedule_exhausted")
                                && p.accepted_points == p.scheduled_points =>
                        {
                            Outcome::Censored("schedule_exhausted")
                        }
                        _ => Outcome::Partial,
                    }
                };
                count(outcome, hit);
            }
            groups.push(summary);
        }
        let status = if groups.iter().any(|group| group.failed > 0) {
            ProfileRunStatus::Failed
        } else if groups.iter().any(|group| group.unfinished > 0) {
            if bound_reached && !blocked {
                ProfileRunStatus::Paused
            } else {
                ProfileRunStatus::Incomplete
            }
        } else {
            ProfileRunStatus::Complete
        };
        Some(ProfileOutcomes { status, groups })
    }
}

#[derive(Clone, Copy)]
enum Outcome<'a> {
    Accepted(bool),
    Censored(&'a str),
    Failed,
    Blocked,
    Pending,
    Partial,
}
