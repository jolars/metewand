//! User-facing comparisons projected from the same evidence as inspection.

use serde::Serialize;
use serde_json::Value;

use crate::core::run_output::RunDiagnostic;

/// One observation, or an attempt that produced no observation.
#[derive(Clone, Debug, Serialize)]
pub struct ComparisonRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
    pub candidate_id: String,
    pub attempt_id: String,
    pub observation_id: Option<String>,
    pub experiment: Option<String>,
    pub case: Option<String>,
    pub dataset: Option<String>,
    pub problem: Option<String>,
    pub implementation: Option<String>,
    pub parameters: Value,
    pub implementation_repetition: Option<u64>,
    pub measurement_index: Option<u64>,
    pub retry_index: u64,
    pub status: String,
    pub valid: Option<bool>,
    pub completion: Option<String>,
    pub selected: Option<bool>,
    pub metrics: Value,
    pub timed_wall_time_ns: Option<u64>,
    pub timed_wall_time_status: String,
    pub timing_scope: Option<String>,
    pub instrumented_time_ns: Option<u64>,
    pub instrumented_scope: Option<String>,
    pub diagnostic: Option<RunDiagnostic>,
}
