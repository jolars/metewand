//! Phase diagnostics and explicit accounting completeness, retained as provenance.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Compatibility version of diagnostic measurement provenance.
pub const MEASUREMENTS_VERSION: u32 = 1;

/// An executor-observed quantity in the unit named by its containing field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Measurement {
    /// The complete declared phase or accounting scope was observed.
    Complete {
        /// Observed nanoseconds or bytes.
        value: u64,
    },
    /// A useful value that does not cover the complete requested scope.
    Partial {
        /// Observed nanoseconds or bytes.
        value: u64,
        /// Why this cannot be used as complete accounting.
        reason: PartialReason,
    },
    /// No value is claimed, including when the operation did not run.
    Unavailable {
        /// Why no value is available.
        reason: MissingReason,
    },
}

/// Stable reasons for incomplete observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialReason {
    /// The phase ended without a successful complete response.
    Interrupted,
    /// Descendants are not included in the observed process usage.
    ProcessOnly,
}

/// Stable reasons for missing measurements.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason {
    /// Execution never reached this phase.
    NotStarted,
    /// The caller supplied a resolved dataset rather than constructing one.
    ProvidedDataset,
    /// An existing construction was reused without running its materializer.
    CacheHit,
    /// The platform has no supported accounting source.
    Unsupported,
    /// The process exited or its accounting files could not be read.
    AccountingUnavailable,
    /// The value exceeds the canonical JSON safe-integer range.
    OutOfRange,
}

impl Measurement {
    /// Represents an unavailable quantity without inventing a zero value.
    #[must_use]
    pub const fn unavailable(reason: MissingReason) -> Self {
        Self::Unavailable { reason }
    }

    /// Records an elapsed interval, retaining interrupted exchanges as partial.
    #[must_use]
    pub fn elapsed(duration: Duration, completed: bool) -> Self {
        match u64::try_from(duration.as_nanos()) {
            Ok(value) if value <= 9_007_199_254_740_991 => {
                if completed {
                    Self::Complete { value }
                } else {
                    Self::Partial {
                        value,
                        reason: PartialReason::Interrupted,
                    }
                }
            }
            _ => Self::unavailable(MissingReason::OutOfRange),
        }
    }

    /// Returns a value only when it covers the entire declared scope.
    #[must_use]
    pub const fn complete_value(self) -> Option<u64> {
        match self {
            Self::Complete { value } => Some(value),
            _ => None,
        }
    }
}

/// The temporal boundary of a resource measurement, independent of coverage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingScope {
    /// The continuous implementation prepare/execute envelope.
    PrepareAndExecute,
    /// The implementation process from launch through receipt of its result.
    WorkerLifetimeThroughExecute,
}

/// Version-1 diagnostic measurements shared by successful and failed attempts.
///
/// These values supplement authoritative observation timing. They are retained
/// in provenance and do not change logical, resolved, or observation identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionMeasurements {
    /// Compatibility version of this provenance payload.
    pub version: u32,
    /// Shared dataset generation or transformation request, excluding launch.
    pub materialization_time_ns: Measurement,
    /// Implementation launch through completed hello negotiation.
    pub worker_startup_time_ns: Measurement,
    /// Independent evaluator launch through completed hello negotiation.
    pub evaluator_startup_time_ns: Measurement,
    /// Complete implementation prepare exchange.
    pub prepare_time_ns: Measurement,
    /// Complete execute exchange, including result writing and serialization.
    pub execute_request_time_ns: Measurement,
    /// Continuous orchestrator-observed selected timing scope.
    pub timed_wall_time_ns: Measurement,
    /// Independent evaluate exchange, excluding metric validation.
    pub evaluation_time_ns: Measurement,
    /// Implementation CPU use; partial values never satisfy full-tree accounting.
    pub cpu_time_ns: Measurement,
    /// Observed implementation peak resident-set size.
    pub peak_rss_bytes: Measurement,
    /// Clock interval used for the CPU delta.
    pub cpu_time_scope: AccountingScope,
    /// Clock interval used for the memory high-water mark.
    pub peak_rss_scope: AccountingScope,
}

impl Default for ExecutionMeasurements {
    fn default() -> Self {
        let not_started = Measurement::unavailable(MissingReason::NotStarted);
        Self {
            version: MEASUREMENTS_VERSION,
            materialization_time_ns: Measurement::unavailable(MissingReason::ProvidedDataset),
            worker_startup_time_ns: not_started,
            evaluator_startup_time_ns: not_started,
            prepare_time_ns: not_started,
            execute_request_time_ns: not_started,
            timed_wall_time_ns: not_started,
            evaluation_time_ns: not_started,
            cpu_time_ns: not_started,
            peak_rss_bytes: not_started,
            cpu_time_scope: AccountingScope::PrepareAndExecute,
            peak_rss_scope: AccountingScope::WorkerLifetimeThroughExecute,
        }
    }
}
