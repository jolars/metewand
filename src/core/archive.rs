//! Portable, versioned evidence independent of SQLite coordination layouts.

mod validation;

use crate::core::profile::FreshSequenceProgress;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Compatibility version of result archives and their nested evidence.
pub const RESULT_ARCHIVE_VERSION: u32 = 1;

/// A complete inventory; the manifest itself is excluded to avoid a hash cycle.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub schema_version: u32,
    pub tool_version: String,
    pub files: Vec<ArchiveFile>,
}

/// Original executable metadata participates in historical artifact tree hashes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveFile {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub executable: bool,
}

/// The committed coordination evidence needed to interpret the included artifacts.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEvidence {
    pub schema_version: u32,
    pub records: Vec<ArchiveRecord>,
}

/// Nested records inherit the enclosing archive's compatibility version.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "record",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum ArchiveRecord {
    Applicability(ApplicabilityEvidence),
    ResolvedSpecification(ResolvedSpecificationEvidence),
    ResolvedAttemptSlot(AttemptSlotEvidence),
    ResolvedObservationSlot(ObservationSlotEvidence),
    SlotMembership(SlotMembershipEvidence),
    Attempt(AttemptEvidence),
    AttemptTransition(TransitionEvidence),
    Observation(ObservationEvidence),
    ObservationSelection(SelectionEvidence),
    StoredArtifact(StoredArtifactEvidence),
    AttemptArtifact(AttemptArtifactEvidence),
    ObservationArtifact(ObservationArtifactEvidence),
    Event(EventEvidence),
    FreshMember(FreshMemberEvidence),
    FreshBudget(FreshBudgetEvidence),
    FreshDisposition(FreshDispositionEvidence),
    FreshPolicy(FreshPolicyEvidence),
    FreshSequence(FreshSequenceProgress),
}

impl ArchiveRecord {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Applicability(_) => "applicability",
            Self::ResolvedSpecification(_) => "resolved-specification",
            Self::ResolvedAttemptSlot(_) => "resolved-attempt-slot",
            Self::ResolvedObservationSlot(_) => "resolved-observation-slot",
            Self::SlotMembership(_) => "slot-membership",
            Self::Attempt(_) => "attempt",
            Self::AttemptTransition(_) => "attempt-transition",
            Self::Observation(_) => "observation",
            Self::ObservationSelection(_) => "observation-selection",
            Self::StoredArtifact(_) => "stored-artifact",
            Self::AttemptArtifact(_) => "attempt-artifact",
            Self::ObservationArtifact(_) => "observation-artifact",
            Self::Event(_) => "event",
            Self::FreshMember(_) => "fresh-member",
            Self::FreshBudget(_) => "fresh-budget",
            Self::FreshDisposition(_) => "fresh-disposition",
            Self::FreshPolicy(_) => "fresh-policy",
            Self::FreshSequence(_) => "fresh-sequence",
        }
    }

    #[must_use]
    pub fn value(&self) -> Value {
        match self {
            Self::Applicability(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::ResolvedSpecification(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::ResolvedAttemptSlot(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::ResolvedObservationSlot(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::SlotMembership(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::Attempt(record) => serde_json::to_value(record).expect("evidence serializes"),
            Self::AttemptTransition(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::Observation(record) => serde_json::to_value(record).expect("evidence serializes"),
            Self::ObservationSelection(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::StoredArtifact(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::AttemptArtifact(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::ObservationArtifact(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::Event(record) => serde_json::to_value(record).expect("evidence serializes"),
            Self::FreshMember(record) => serde_json::to_value(record).expect("evidence serializes"),
            Self::FreshBudget(record) => serde_json::to_value(record).expect("evidence serializes"),
            Self::FreshDisposition(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
            Self::FreshPolicy(record) => serde_json::to_value(record).expect("evidence serializes"),
            Self::FreshSequence(record) => {
                serde_json::to_value(record).expect("evidence serializes")
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicabilityEvidence {
    pub id: String,
    pub candidate_id: String,
    pub applicable: bool,
    pub definition: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedSpecificationEvidence {
    pub id: String,
    pub logical_specification_id: String,
    pub candidate_id: String,
    pub applicability_id: String,
    pub definition: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptSlotEvidence {
    pub id: String,
    pub logical_slot_id: String,
    pub resolved_run_id: String,
    pub role: String,
    pub kind: String,
    pub definition: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationSlotEvidence {
    pub id: String,
    pub logical_slot_id: String,
    pub resolved_run_id: String,
    pub definition: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SlotMembershipEvidence {
    pub attempt_slot_id: String,
    pub observation_slot_id: String,
    pub resolved_run_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptEvidence {
    pub id: String,
    pub slot_id: String,
    pub retry_index: u64,
    pub state: String,
    pub revision: u64,
    pub started_at_ns: String,
    pub updated_at_ns: String,
    pub finished_at_ns: Option<String>,
    pub failure_code: Option<String>,
    pub artifact_id: Option<String>,
    pub lease_expires_at_ns: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionEvidence {
    pub attempt_id: String,
    pub revision: u64,
    pub previous_revision: Option<u64>,
    pub from_state: Option<String>,
    pub to_state: String,
    pub occurred_at_ns: String,
    pub event_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationEvidence {
    pub id: String,
    pub slot_id: String,
    pub attempt_id: String,
    pub attempt_slot_id: String,
    pub artifact_id: String,
    pub valid: bool,
    pub completion: String,
    pub selected: bool,
    pub definition: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionEvidence {
    pub slot_id: String,
    pub observation_id: String,
    pub selected_at_ns: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredArtifactEvidence {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub sha256: String,
    pub storage_kind: String,
    pub manifest_sha256: Option<String>,
    pub verified_at_ns: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptArtifactEvidence {
    pub attempt_id: String,
    pub role: String,
    pub artifact_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationArtifactEvidence {
    pub observation_id: String,
    pub role: String,
    pub artifact_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventEvidence {
    pub sequence: u64,
    pub attempt_id: Option<String>,
    pub occurred_at_ns: String,
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshMemberEvidence {
    pub slot_id: String,
    pub resolved_run_id: String,
    pub measurement_index: u64,
    pub schedule_index: u64,
    pub control: Value,
    pub observation_slot_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshBudgetEvidence {
    pub attempt_id: String,
    pub reserved_ns: u64,
    pub charged_ns: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshDispositionEvidence {
    pub slot_id: String,
    pub reason: String,
    pub causing_attempt_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshPolicyEvidence {
    pub resolved_run_id: String,
    pub measurement_index: u64,
    pub policy: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveOperation {
    Create,
    Verify,
    Import,
}

/// Human and machine output use the same operation receipt.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveReport {
    pub command: String,
    pub schema_version: u32,
    pub operation: ArchiveOperation,
    pub archive_path: String,
    pub destination: Option<String>,
    pub manifest_sha256: String,
    pub files: u64,
    pub size_bytes: u64,
    pub records: u64,
}

impl ArchiveEvidence {
    /// Checks identities, immutable history, selections, and accounting without performing I/O.
    pub fn validate(&self) -> Result<(), ArchiveEvidenceError> {
        validation::validate(self)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveEvidenceError {
    #[error("unsupported result archive compatibility version")]
    UnsupportedVersion,
    #[error("archive contains an unfinished attempt")]
    Unsettled,
    #[error("invalid archive coordination evidence: {0}")]
    Invalid(String),
}
