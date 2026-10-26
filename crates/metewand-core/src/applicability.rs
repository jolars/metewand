//! Configuration-dependent implementation support evidence.

use serde::{Deserialize, Serialize};

use crate::records::{LogicalCandidateRecord, RecordId};

/// Support state before or after resolution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicabilityOutcome {
    /// A declared check has not run yet.
    Unchecked,
    /// The implementation supports this configuration.
    Applicable,
    /// An intrinsic support constraint excludes this configuration.
    NotApplicable,
}

/// Immutable support evidence; nested records use the enclosing format version.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicabilityDecisionRecord {
    /// Logical candidate whose inputs were checked.
    pub candidate_id: RecordId<LogicalCandidateRecord>,
    /// Whether support has been established or excluded.
    pub outcome: ApplicabilityOutcome,
    /// Stable reason code supplied by the implementation or orchestrator.
    pub code: String,
    /// Human-readable reason for the support state.
    pub reason: String,
    /// Exact scientific inputs, including dataset identity and manifest content.
    pub checked_inputs: Option<serde_json::Value>,
    /// Resolved worker, environment, protocol, and observed controls.
    pub evidence: Option<serde_json::Value>,
    /// Optional implementation-owned details.
    pub details: Option<serde_json::Value>,
}
