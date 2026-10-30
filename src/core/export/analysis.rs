//! Standalone analysis records retain individual measurements and their evidence.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::core::comparison::ComparisonRow;
use crate::core::export::CompatibilityClass;

/// Compatibility version of the standalone analysis snapshot.
pub const ANALYSIS_SCHEMA_VERSION: u32 = 1;

/// A portable projection, rather than an archive of the result artifacts.
#[derive(Clone, Debug, Serialize)]
pub struct AnalysisSnapshot {
    pub schema_version: u32,
    pub candidates: Vec<AnalysisCandidate>,
    pub observations: Vec<AnalysisObservation>,
    pub attempts: Vec<AnalysisAttempt>,
    pub slots: Vec<AnalysisSlot>,
    pub applicability: Vec<AnalysisApplicability>,
    pub profiles: Vec<AnalysisProfile>,
    pub provenance: BTreeMap<String, AnalysisProvenance>,
    pub compatibility_classes: BTreeMap<String, CompatibilityClass>,
    pub completeness: AnalysisCompleteness,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisCandidate {
    pub id: String,
    pub experiment: Option<String>,
    pub implementation: Option<String>,
    pub dataset: Option<String>,
    pub problem: Option<String>,
    pub case: Option<String>,
    pub parameters: Value,
    pub implementation_repetitions: Option<u64>,
    pub measurement_repetitions: Option<u64>,
    pub execution_policy: Value,
    pub observation_policy: Value,
}

/// Full metrics and independent timing coordinates for one evaluated result.
#[derive(Clone, Debug, Serialize)]
pub struct AnalysisObservation {
    #[serde(flatten)]
    pub comparison: ComparisonRow,
    pub warmup: Option<bool>,
    pub implementation_time_ns: Option<u64>,
    pub evaluation_time_ns: Option<u64>,
    pub active_time_ns: Option<u64>,
    pub elapsed_wall_time_ns: Option<u64>,
    pub statistics: Value,
    pub compatibility_id: String,
    pub provenance_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisAttempt {
    pub id: String,
    pub candidate_id: String,
    pub slot_id: Option<String>,
    pub retry_index: Option<u64>,
    pub state: Option<String>,
    pub failure_code: Option<String>,
    pub warmup: Option<bool>,
    pub started_at_ns: Option<String>,
    pub finished_at_ns: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisSlot {
    pub id: String,
    pub candidate_id: String,
    pub resolved_specification_id: String,
    pub state: String,
    pub selected_observation_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisApplicability {
    pub id: String,
    pub candidate_id: String,
    pub applicable: bool,
    pub evidence: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisProfile {
    pub candidate_id: String,
    pub resolved_specification_id: String,
    pub measurement_index: Option<u64>,
    pub delivery: String,
    pub status: String,
    pub reason: Option<String>,
    pub progress: Value,
}

/// Invariant evidence is stored once and referenced by a snapshot-local hash.
#[derive(Clone, Debug, Serialize)]
pub struct AnalysisProvenance {
    pub tool_version: Option<String>,
    pub benchmark: Option<String>,
    pub manifest_hash: Option<String>,
    pub runner: Value,
    pub host: Value,
    pub repository: Value,
    pub dataset: Value,
    pub implementation: Value,
    pub evaluator: Value,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AnalysisCompleteness {
    pub complete: bool,
    pub issues: Vec<AnalysisIssue>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisIssue {
    pub code: String,
    pub candidate_id: Option<String>,
    pub slot_id: Option<String>,
    pub message: String,
}

impl AnalysisSnapshot {
    /// Historical failures do not invalidate a later accepted selection.
    pub fn check_completeness(&mut self) {
        let mut issues = Vec::new();
        if self.slots.is_empty()
            && (self.applicability.is_empty() || self.applicability.iter().any(|a| a.applicable))
        {
            issues.push(issue(
                "no_measurements",
                None,
                None,
                "No measured scope was recorded.",
            ));
        }
        for slot in &self.slots {
            if !matches!(slot.state.as_str(), "satisfied" | "skipped_target_reached") {
                issues.push(issue(
                    "unsatisfied_slot",
                    Some(&slot.candidate_id),
                    Some(&slot.id),
                    "A registered measurement has no accepted selection or target-based skip.",
                ));
            } else if slot.state == "satisfied" && slot.selected_observation_ids.is_empty() {
                issues.push(issue(
                    "missing_selection",
                    Some(&slot.candidate_id),
                    Some(&slot.id),
                    "An accepted measurement has no selected observation.",
                ));
            } else if slot.selected_observation_ids.iter().any(|id| {
                !self.observations.iter().any(|o| {
                    o.comparison.observation_id.as_ref() == Some(id)
                        && o.comparison.selected == Some(true)
                })
            }) {
                issues.push(issue(
                    "missing_observation",
                    Some(&slot.candidate_id),
                    Some(&slot.id),
                    "A selected observation is missing from the verified measurement scope.",
                ));
            }
        }
        for observation in &self.observations {
            let row = &observation.comparison;
            if row.selected == Some(true) && (row.valid != Some(true) || row.status != "accepted") {
                issues.push(issue(
                    "invalid_selection",
                    Some(&row.candidate_id),
                    None,
                    "A selected observation is invalid or its producing attempt was not accepted.",
                ));
            }
        }
        for profile in &self.profiles {
            if profile.status != "accepted" {
                issues.push(issue(
                    "unfinished_profile",
                    Some(&profile.candidate_id),
                    None,
                    "A profile did not finish its fixed schedule or reach its common target.",
                ));
            }
        }
        for candidate in &self.candidates {
            let Some(expected) = candidate
                .measurement_repetitions
                .zip(candidate.implementation_repetitions)
                .and_then(|(m, i)| m.checked_mul(i))
            else {
                continue;
            };
            if self
                .applicability
                .iter()
                .any(|a| a.candidate_id == candidate.id && !a.applicable)
            {
                continue;
            }
            let observed = if candidate.observation_policy["kind"] == "profile" {
                self.profiles
                    .iter()
                    .filter(|p| p.candidate_id == candidate.id)
                    .count()
            } else {
                self.slots
                    .iter()
                    .filter(|s| s.candidate_id == candidate.id)
                    .count()
            };
            if observed as u64 != expected {
                issues.push(issue(
                    "missing_repetitions",
                    Some(&candidate.id),
                    None,
                    "The recorded measurement scope does not contain the declared repetitions.",
                ));
            }
        }
        self.completeness = AnalysisCompleteness {
            complete: issues.is_empty(),
            issues,
        };
    }
}

fn issue(code: &str, candidate: Option<&str>, slot: Option<&str>, message: &str) -> AnalysisIssue {
    AnalysisIssue {
        code: code.into(),
        candidate_id: candidate.map(str::to_owned),
        slot_id: slot.map(str::to_owned),
        message: message.into(),
    }
}
