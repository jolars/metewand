//! Typed command events and terminal reports for the Gate-1 execution path.

use std::sync::Arc;

use serde::Serialize;

use crate::core::records::{
    LogicalAttemptSlotRecord, RecordId, ResolvedAttemptSlotRecord, RunAttemptRecord,
    RunObservationRecord,
};

/// Invocation controls that do not alter scientific slot identities.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Verify prepared Nix environments against `metewand.lock`.
    pub locked: bool,
    /// Publish coordination records to shared storage with a disposable local view.
    pub shared: bool,
    /// Union of stable IDs to execute; an empty list selects the complete plan.
    pub selected_ids: Vec<String>,
    /// Named experiment union, or the manifest default when empty.
    pub experiments: Vec<String>,
    /// Name filters applied after experiment selection and before stable IDs.
    pub filters: crate::core::planning::CandidateFilters,
    /// Maximum new attempts allocated by this invocation, including retries.
    pub max_attempts: Option<u64>,
    /// Authorize one additional attempt for selected failures or active slots.
    pub retry: bool,
}

/// A stable technical or scientific reason for an attempt outcome.
#[derive(Clone, Debug, Serialize)]
pub struct RunDiagnostic {
    /// Machine-readable failure code.
    pub code: String,
    /// Human-readable explanation from the same outcome.
    pub message: String,
}

/// Terminal status of one durably published attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAttemptStatus {
    Censored,
    /// The independently evaluated result met its completion rule.
    Accepted,
    /// A concurrent candidate won the slot finalization claim.
    Superseded,
    /// Execution, validation, evaluation, or scientific completion failed.
    Failed,
}

/// The public command view of one finalized attempt.
#[derive(Clone, Debug, Serialize)]
pub struct RunAttemptOutput<D> {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observations: Vec<ProfileObservationOutput<D>>,
    /// Logical position from the read-only plan.
    pub logical_slot_id: RecordId<LogicalAttemptSlotRecord>,
    /// Position after binding the execution dependencies.
    pub resolved_slot_id: RecordId<ResolvedAttemptSlotRecord>,
    /// Finalized attempt identity.
    pub attempt_id: RecordId<RunAttemptRecord>,
    /// Accepted, failed, or superseded terminal outcome.
    pub status: RunAttemptStatus,
    /// Independently published observation, when a valid result exists.
    pub observation_id: Option<RecordId<RunObservationRecord>>,
    /// Attempt directory relative to the command's output directory.
    pub attempt_directory: String,
    /// Observation directory relative to the command's output directory.
    pub observation_directory: Option<String>,
    /// Problem-owned canonical result data, when valid.
    pub result: Option<D>,
    /// Independently evaluated metrics, when valid.
    pub metrics: Option<D>,
    /// Reason for failure, including a missed scientific completion target.
    pub diagnostic: Option<RunDiagnostic>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileObservationOutput<D> {
    pub observation_id: RecordId<RunObservationRecord>,
    pub observation_directory: String,
    pub result: D,
    pub metrics: D,
    pub profile: crate::core::records::ProfileObservationRecord,
}

#[derive(Clone, Debug, Serialize)]
pub struct CalibrationAttemptOutput<D> {
    pub pilot: crate::core::adaptive::CalibrationPilot,
    pub attempt: RunAttemptOutput<D>,
}

/// A monotonically sequenced event from one invocation.
#[derive(Clone, Debug, Serialize)]
pub struct RunEvent<D> {
    /// Command owning this event.
    pub command: &'static str,
    /// Zero-based position in this invocation's append-only event stream.
    pub sequence: usize,
    /// Typed lifecycle transition.
    #[serde(flatten)]
    pub kind: RunEventKind<D>,
}

/// Lifecycle transitions emitted around execution and durable publication.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEventKind<D> {
    /// Resolution and preflight completed, and execution is about to begin.
    RunStarted {
        /// Total measured attempt slots to execute.
        attempt_slots: usize,
    },
    /// One resolved attempt is about to launch its workers.
    AttemptStarted {
        /// Logical position from the plan.
        logical_slot_id: RecordId<LogicalAttemptSlotRecord>,
        /// Position after resolution.
        resolved_slot_id: RecordId<ResolvedAttemptSlotRecord>,
    },
    /// The terminal attempt and any observation have been durably published.
    AttemptFinished {
        /// The same typed outcome used in the final report.
        attempt: Arc<RunAttemptOutput<D>>,
    },
    /// Dispatch has ended and the invocation totals are available.
    RunFinished {
        /// Number of accepted attempts.
        accepted_attempts: usize,
        #[serde(skip_serializing_if = "is_zero")]
        censored_attempts: usize,
        /// Number of failed attempts.
        failed_attempts: usize,
        /// Number of concurrent candidates superseded by another finalizer.
        superseded_attempts: usize,
    },
}

/// Counts for the complete selected execution, including failed attempts.
#[derive(Debug, Serialize)]
pub struct RunSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_outcomes: Option<ProfileOutcomes>,
    #[serde(skip_serializing_if = "is_zero")]
    pub censored_profiles: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub censored_attempts: usize,
    /// Number of measured slots selected from the logical plan.
    pub attempt_slots: usize,
    /// Number of durably accepted attempts.
    pub accepted_attempts: usize,
    /// Number of durably failed attempts.
    pub failed_attempts: usize,
    /// Number of durably superseded concurrent candidates.
    pub superseded_attempts: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRunStatus {
    Complete,
    Paused,
    Incomplete,
    Failed,
}

#[derive(Debug, Serialize)]
pub struct ProfileOutcomes {
    pub status: ProfileRunStatus,
    pub groups: Vec<ProfileGroup>,
}

#[derive(Debug, Default, Serialize)]
pub struct ProfileGroup {
    pub experiment: String,
    pub problem_instance_id: String,
    pub dataset: String,
    pub problem: String,
    pub parameters: serde_json::Value,
    pub accepted: usize,
    pub censored: usize,
    pub failed: usize,
    pub unfinished: usize,
    pub partial: usize,
    pub censoring_reasons: std::collections::BTreeMap<String, usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<ProfileTarget>,
}

impl ProfileGroup {
    pub fn selected(&self) -> usize {
        self.accepted + self.censored + self.failed + self.unfinished + self.partial
    }
}

#[derive(Debug, Serialize)]
pub struct ProfileTarget {
    pub metric: String,
    pub direction: crate::core::profile::Direction,
    pub threshold: f64,
    pub reached: usize,
}

/// Sampling progress derived from the committed one-shot measurements.
#[derive(Clone, Debug, Serialize)]
pub struct AdaptiveMeasurementProgress {
    pub candidate_id: String,
    pub resolved_specification_id: String,
    pub policy: crate::core::manifest::AdaptiveMeasurementRepetitions,
    pub accepted_measurements: u64,
    /// Decimal nanoseconds preserve totals beyond the canonical integer range.
    pub primary_time_ns: String,
    pub stop_reason: Option<crate::core::manifest::AdaptiveMeasurementStop>,
}

/// Final command output rendered by both human and machine interfaces.
#[derive(Debug, Serialize)]
pub struct RunReport<D> {
    #[serde(skip)]
    pub one_shot_failures: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub adaptive_measurements: Vec<AdaptiveMeasurementProgress>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub calibration_attempts: Vec<CalibrationAttemptOutput<D>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub adaptive_budgets: Vec<crate::core::adaptive::AdaptiveBudgetDecision>,
    /// Individual measurements and failed attempts from the committed history.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comparison: Vec<crate::core::comparison::ComparisonRow<Option<D>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fresh_sequences: Vec<crate::core::profile::FreshSequenceProgress>,
    /// Command that produced the report.
    pub command: &'static str,
    /// Authored benchmark name.
    pub benchmark: String,
    /// Complete manifest identity, retained as context rather than a dependency.
    pub manifest_hash: String,
    /// User-selected artifact destination, retained outside stable identities.
    pub output_directory: String,
    /// Terminal attempt outcomes in deterministic execution order.
    pub attempts: Vec<Arc<RunAttemptOutput<D>>>,
    /// Append-only lifecycle history, also streamed by JSON Lines output.
    pub events: Vec<RunEvent<D>>,
    /// Counts derived from the same terminal attempts.
    pub summary: RunSummary,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl<D> RunReport<D> {
    pub fn failed(&self) -> bool {
        self.one_shot_failures > 0
            || self
                .summary
                .profile_outcomes
                .as_ref()
                .is_some_and(|profiles| {
                    matches!(
                        profiles.status,
                        ProfileRunStatus::Incomplete | ProfileRunStatus::Failed
                    )
                })
    }
}

/// Receipt for an explicit recovery pass in the v1 machine envelope.
#[derive(Debug, serde::Serialize)]
pub struct RecoveryReceipt {
    pub command: &'static str,
    pub output_directory: std::path::PathBuf,
    pub recovered_attempts: usize,
    pub remaining_attempts: u64,
}
