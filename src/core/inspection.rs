//! Shared records for read-only status and identity explanations.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Value, json};

use crate::core::{
    canonical::CanonicalValue,
    identity::{
        CanonicalResource, IdentityError, IdentityKind, IdentityRecord, canonical_identity_bytes,
        identify_canonical,
    },
    records::{IdentifiedRecord, RecordId},
};

/// Content-addressed failure information, independent of its producing attempt.
#[derive(Debug)]
pub enum DiagnosticResource {}

impl IdentityKind for DiagnosticResource {
    const KIND: &'static str = "diagnostic";
}

impl CanonicalResource for DiagnosticResource {}

/// Identifies a persisted diagnostic by its code, message, and optional details.
pub fn diagnostic_id(diagnostic: &Value) -> Result<RecordId<DiagnosticResource>, IdentityError> {
    identify_canonical(&CanonicalValue::try_from(json!({
        "code": diagnostic["code"],
        "message": diagnostic["message"],
        "details": diagnostic.get("details").unwrap_or(&Value::Null),
    }))?)
}

/// One inspectable identity and the evidence available for it.
#[derive(Clone, Debug, Serialize)]
pub struct InspectionRecord {
    pub id: String,
    pub kind: String,
    pub label: Option<String>,
    /// Canonical identity representation or persisted coordination record.
    pub record: Value,
    /// Other identities connected by planning or durable coordination.
    pub related_ids: Vec<String>,
    /// Repository paths or paths relative to the selected output directory.
    pub paths: Vec<String>,
}

impl InspectionRecord {
    pub fn new(id: String, kind: impl Into<String>, record: Value) -> Self {
        Self {
            id,
            kind: kind.into(),
            label: None,
            record,
            related_ids: Vec::new(),
            paths: Vec::new(),
        }
    }

    pub fn identified<T: IdentityRecord>(
        identified: &IdentifiedRecord<T>,
    ) -> Result<Self, IdentityError> {
        Ok(Self::new(
            identified.id.to_string(),
            T::KIND,
            serde_json::from_slice(&canonical_identity_bytes(&identified.record)?)
                .expect("identity bytes are JSON"),
        ))
    }

    pub fn relate(&mut self, ids: impl IntoIterator<Item = String>) {
        self.related_ids
            .extend(ids.into_iter().filter(|id| id != &self.id));
        self.related_ids.sort();
        self.related_ids.dedup();
    }
}

/// A stable ID accepted by inspection, including internal version-1 store keys.
#[must_use]
pub fn is_inspection_id(id: &str) -> bool {
    if let Some(candidate) = id.strip_prefix("run-store-v1:unconditional:") {
        return candidate
            .parse::<RecordId<crate::core::records::LogicalCandidateRecord>>()
            .is_ok();
    }
    if let Some(stored) = id.strip_prefix("run-store-v1:")
        && !stored.starts_with("unconditional:")
    {
        let Some((kind, digest)) = stored.rsplit_once('-') else {
            return false;
        };
        return [
            "attempt",
            "observation",
            "result",
            "metrics",
            "dataset",
            "source",
            "environment",
            "other",
        ]
        .contains(&kind)
            && digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    }
    let public = id;
    let Some((kind, digest)) = public.rsplit_once('-') else {
        return false;
    };
    kind.starts_with("mw1-")
        && kind.len() > 4
        && kind.as_bytes()[4].is_ascii_lowercase()
        && kind[4..]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Collects literal identity references in a serialized record in byte order.
pub fn referenced_ids(value: &Value) -> Vec<String> {
    fn visit(value: &Value, ids: &mut BTreeSet<String>) {
        match value {
            Value::String(id) if is_inspection_id(id) => {
                ids.insert(id.clone());
            }
            Value::Array(values) => values.iter().for_each(|value| visit(value, ids)),
            Value::Object(values) => values.values().for_each(|value| visit(value, ids)),
            _ => (),
        }
    }
    let mut ids = BTreeSet::new();
    visit(value, &mut ids);
    ids.into_iter().collect()
}

/// Current scheduling decision for a measured execution slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotState {
    SkippedTargetReached,
    Censored,
    Pending,
    Active,
    Satisfied,
    RetryRequired,
    RecoveryRequired,
}

/// Planned and resolved positions remain distinct even when inputs change.
#[derive(Clone, Debug, Serialize)]
pub struct SlotStatus {
    pub logical_slot_id: String,
    pub resolved_slot_id: Option<String>,
    pub logical_run_id: String,
    pub resolved_run_id: Option<String>,
    pub candidate_id: String,
    /// Null when inspection did not load a current manifest.
    pub in_current_plan: Option<bool>,
    pub state: SlotState,
    pub attempt_ids: Vec<String>,
    pub selected_observation_ids: Vec<String>,
}

/// The authoritative projection of one execution, including nonterminal work.
#[derive(Clone, Debug, Serialize)]
pub struct AttemptStatus {
    pub id: String,
    pub slot_id: String,
    pub retry_index: u64,
    pub state: String,
    pub revision: u64,
    /// Decimal strings preserve epoch nanoseconds in the canonical JSON domain.
    pub started_at_ns: String,
    pub updated_at_ns: String,
    pub finished_at_ns: Option<String>,
    pub lease_expires_at_ns: Option<String>,
    pub recovery_required: bool,
    pub artifact_id: Option<String>,
    pub diagnostic_id: Option<String>,
}

/// An observation can remain valid without being selected or accepted.
#[derive(Clone, Debug, Serialize)]
pub struct ObservationStatus {
    pub id: String,
    pub slot_id: String,
    pub attempt_id: String,
    pub artifact_id: String,
    pub valid: bool,
    pub completion: String,
    pub selected: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct StatusSummary {
    #[serde(skip_serializing_if = "zero")]
    pub skipped_target_reached_slots: usize,
    #[serde(skip_serializing_if = "zero")]
    pub censored_slots: usize,
    pub attempt_slots: usize,
    pub pending_slots: usize,
    pub active_slots: usize,
    pub satisfied_slots: usize,
    pub retry_required_slots: usize,
    pub recovery_required_slots: usize,
    pub accepted_attempts: usize,
    pub failed_attempts: usize,
    pub superseded_attempts: usize,
    pub censored_attempts: usize,
    pub active_attempts: usize,
    pub selected_observations: usize,
}

/// The same status evidence is rendered by human and machine interfaces.
#[derive(Debug, Serialize)]
pub struct StatusReport {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comparison: Vec<crate::core::comparison::ComparisonRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fresh_sequences: Vec<crate::core::profile::FreshSequenceProgress>,
    pub command: &'static str,
    pub benchmark: Option<String>,
    pub manifest_hash: Option<String>,
    pub output_directory: Option<String>,
    pub slots: Vec<SlotStatus>,
    pub attempts: Vec<AttemptStatus>,
    pub observations: Vec<ObservationStatus>,
    pub diagnostics: Vec<InspectionRecord>,
    pub summary: StatusSummary,
}

#[derive(Debug, Serialize)]
pub struct Explanation {
    pub command: &'static str,
    pub subject: InspectionRecord,
}

fn zero(value: &usize) -> bool {
    *value == 0
}
