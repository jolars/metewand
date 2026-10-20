//! Typed command events and terminal reports for the Gate-1 execution path.

use serde::Serialize;
use serde_json::Value;

use crate::records::{
    LogicalAttemptSlotRecord, RecordId, ResolvedAttemptSlotRecord, RunAttemptRecord,
    RunObservationRecord,
};

/// Invocation controls that do not alter scientific slot identities.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Union of stable IDs to execute; an empty list selects the complete plan.
    pub selected_ids: Vec<String>,
    /// Maximum new attempts allocated by this invocation, including retries.
    pub max_attempts: Option<u64>,
    /// Reopen an existing durable output directory.
    pub resume: bool,
    /// Explicitly authorize one additional attempt per selected unsatisfied slot.
    pub retry: bool,
}

/// A stable technical or scientific reason for a failed attempt.
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
    /// The independently evaluated result met its completion rule.
    Accepted,
    /// A concurrent candidate won the slot finalization claim.
    Superseded,
    /// Execution, validation, evaluation, or scientific completion failed.
    Failed,
}

/// The public command view of one finalized attempt.
#[derive(Clone, Debug, Serialize)]
pub struct RunAttemptOutput {
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
    pub result: Option<Value>,
    /// Independently evaluated metrics, when valid.
    pub metrics: Option<Value>,
    /// Reason for failure, including a missed scientific completion target.
    pub diagnostic: Option<RunDiagnostic>,
}

/// A monotonically sequenced event from one invocation.
#[derive(Clone, Debug, Serialize)]
pub struct RunEvent {
    /// Command owning this event.
    pub command: &'static str,
    /// Zero-based position in this invocation's append-only event stream.
    pub sequence: usize,
    /// Typed lifecycle transition.
    #[serde(flatten)]
    pub kind: RunEventKind,
}

/// Lifecycle transitions emitted around execution and durable publication.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEventKind {
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
        attempt: Box<RunAttemptOutput>,
    },
    /// Every selected attempt has a durable terminal outcome.
    RunFinished {
        /// Number of accepted attempts.
        accepted_attempts: usize,
        /// Number of failed attempts.
        failed_attempts: usize,
        /// Number of concurrent candidates superseded by another finalizer.
        superseded_attempts: usize,
    },
}

/// Counts for the complete selected execution, including failed attempts.
#[derive(Debug, Serialize)]
pub struct RunSummary {
    /// Number of measured slots selected from the logical plan.
    pub attempt_slots: usize,
    /// Number of durably accepted attempts.
    pub accepted_attempts: usize,
    /// Number of durably failed attempts.
    pub failed_attempts: usize,
    /// Number of durably superseded concurrent candidates.
    pub superseded_attempts: usize,
}

/// Final command output rendered by both human and machine interfaces.
#[derive(Debug, Serialize)]
pub struct RunReport {
    /// Command that produced the report.
    pub command: &'static str,
    /// Authored benchmark name.
    pub benchmark: String,
    /// Complete manifest identity, retained as context rather than a dependency.
    pub manifest_hash: String,
    /// User-selected artifact destination, retained outside stable identities.
    pub output_directory: String,
    /// Terminal attempt outcomes in deterministic execution order.
    pub attempts: Vec<RunAttemptOutput>,
    /// Append-only lifecycle history, also streamed by JSON Lines output.
    pub events: Vec<RunEvent>,
    /// Counts derived from the same terminal attempts.
    pub summary: RunSummary,
}
