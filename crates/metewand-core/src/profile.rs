//! Scientific profile policy and the shared buffered artifact contract.

use crate::{canonical::MAX_SAFE_INTEGER, manifest::Name, schema::SchemaDeclaration};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

/// Common completion and collection limits, independent of delivery.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProfilePolicy {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub metric: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub direction: Option<Direction>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub threshold: Option<f64>,
    #[serde(
        default,
        deserialize_with = "nonempty_thresholds",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub record_thresholds: Vec<f64>,
    pub max_observations: u64,
    #[serde(
        default,
        skip_serializing_if = "is_zero",
        deserialize_with = "positive_bound"
    )]
    pub max_checkpoint_bytes: u64,
    #[serde(
        default,
        skip_serializing_if = "is_zero",
        deserialize_with = "positive_bound"
    )]
    pub max_trace_points: u64,
    #[serde(
        default,
        skip_serializing_if = "is_zero",
        deserialize_with = "positive_bound"
    )]
    pub max_buffer_bytes: u64,
    #[serde(
        default,
        skip_serializing_if = "is_zero",
        deserialize_with = "positive_bound"
    )]
    pub terminal_reserve_bytes: u64,
    pub max_active_time: String,
}

fn positive_bound<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value == 0 || value > MAX_SAFE_INTEGER {
        return Err(serde::de::Error::custom(
            "bounds must be positive safe integers",
        ));
    }
    Ok(value)
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

fn nonempty_thresholds<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<f64>, D::Error> {
    let values = Vec::deserialize(deserializer)?;
    if values.is_empty() {
        return Err(serde::de::Error::custom("omit unused recording thresholds"));
    }
    Ok(values)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    AtMost,
    AtLeast,
}

impl ProfilePolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_observations == 0 || self.max_observations > MAX_SAFE_INTEGER {
            return Err("max-observations must be a positive safe integer".into());
        }
        if self.max_trace_points != 0
            || self.max_buffer_bytes != 0
            || self.terminal_reserve_bytes != 0
        {
            self.validate_buffered()?;
        }
        self.active_time_ns()?;
        match (&self.metric, self.direction, self.threshold) {
            (None, None, None) if self.record_thresholds.is_empty() => {
                if self.max_trace_points != 0 && self.max_trace_points < self.max_observations {
                    return Err(
                        "fixed count requires enough trace points for every observation".into(),
                    );
                }
            }
            (Some(metric), Some(_), Some(target)) if !metric.is_empty() && target.is_finite() => {
                let thresholds = self.thresholds();
                if thresholds.len() as u64 > self.max_observations
                    || thresholds.last() != Some(&target)
                    || thresholds.iter().any(|v| !v.is_finite())
                    || thresholds
                        .windows(2)
                        .any(|v| v[0] == v[1] || !self.reaches(v[1], v[0]))
                {
                    return Err("record thresholds must be distinct, least to most demanding, end at the target, and fit the observation bound".into());
                }
            }
            _ => {
                return Err("profiles require either metric, direction, and threshold together, or a fixed count without thresholds".into());
            }
        }
        Ok(())
    }

    pub fn validate_buffered(&self) -> Result<(), String> {
        if [
            self.max_trace_points,
            self.max_buffer_bytes,
            self.terminal_reserve_bytes,
        ]
        .iter()
        .any(|v| *v == 0 || *v > MAX_SAFE_INTEGER)
            || self.terminal_reserve_bytes > self.max_buffer_bytes
        {
            return Err("buffered bounds must be positive safe integers; terminal reserve must fit the buffer".into());
        }
        Ok(())
    }

    pub fn thresholds(&self) -> Vec<f64> {
        if self.record_thresholds.is_empty() {
            self.threshold.into_iter().collect()
        } else {
            self.record_thresholds.clone()
        }
    }

    pub fn reaches(&self, value: f64, threshold: f64) -> bool {
        match self.direction {
            Some(Direction::AtMost) => value <= threshold,
            Some(Direction::AtLeast) => value >= threshold,
            None => false,
        }
    }

    pub fn active_time_ns(&self) -> Result<u64, String> {
        let (number, multiplier) = [
            ("ms", 1_000_000u64),
            ("s", 1_000_000_000),
            ("min", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ]
        .into_iter()
        .find_map(|(suffix, m)| {
            self.max_active_time
                .trim()
                .strip_suffix(suffix)
                .map(|n| (n.trim(), m))
        })
        .ok_or("max-active-time requires a positive integer and ms, s, min, or h")?;
        if !number.starts_with(|c: char| matches!(c, '1'..='9'))
            || !number.bytes().all(|c| c.is_ascii_digit())
        {
            return Err("max-active-time requires a positive decimal integer".into());
        }
        number
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(multiplier))
            .filter(|n| *n > 0 && *n <= MAX_SAFE_INTEGER)
            .ok_or_else(|| "max-active-time is zero, invalid, or too large".into())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BufferedBinding {
    pub delivery: BufferedDelivery,
    pub measurement_basis: InstrumentedBasis,
    pub instrumented_scope: String,
    pub instrumentation_schema: SchemaDeclaration,
    pub instrumentation: Value,
    pub check_schedule: CheckSchedule,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BufferedDelivery {
    BufferedProfile,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentedBasis {
    Instrumented,
}

#[derive(Clone, Debug)]
pub struct ProfileDefinition {
    pub policy: ProfilePolicy,
    pub implementations: BTreeMap<Name, ProfileBinding>,
}

impl ProfileDefinition {
    pub fn declaration_value(&self) -> Value {
        let mut value = serde_json::to_value(&self.policy).expect("validated profile");
        value["implementations"] = self
            .implementations
            .iter()
            .map(|(name, binding)| (name.as_str().to_owned(), binding.declaration_value()))
            .collect::<serde_json::Map<_, _>>()
            .into();
        value
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckSchedule {
    pub every: u64,
    #[serde(default)]
    pub start: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SelectedBufferedProfile {
    pub policy: ProfilePolicy,
    pub instrumented_scope: String,
    pub scope: InstrumentedScope,
    pub instrumentation: Value,
    pub check_schedule: CheckSchedule,
}

/// The contract owns the interpretation of each instrumented clock.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InstrumentedScope {
    pub clock_origin: String,
    pub included_work: String,
    pub excluded_preparation: String,
    #[serde(default)]
    pub metric_agreement: BTreeMap<String, MetricAgreement>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MetricAgreement {
    pub atol: f64,
    pub rtol: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    TargetReached,
    FixedCount,
    TraceLimit,
    ObservationLimit,
    ByteLimit,
    ActiveTimeLimit,
    ImplementationStopped,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TracePoint {
    pub check_index: u64,
    pub work: BTreeMap<String, u64>,
    pub provisional_metrics: Value,
    pub instrumented_time_ns: u64,
    pub terminal: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BufferedSnapshot {
    pub capture_ordinal: u64,
    pub trace_index: u64,
    pub thresholds: Vec<f64>,
    pub manifest: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BufferedManifest {
    pub schema_version: u32,
    pub complete: bool,
    pub clock: String,
    pub instrumented_scope: String,
    pub instrumented_total_time_ns: u64,
    pub stop_reason: StopReason,
    pub trace: PathBuf,
    pub snapshots: Vec<BufferedSnapshot>,
    pub files: Vec<BufferFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BufferFile {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Delivery-specific authoring fields share a completion policy.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ProfileBinding {
    Buffered(BufferedBinding),
    Fresh(FreshBinding),
    Streaming(StreamingBinding),
}

impl ProfileBinding {
    pub fn schema(&self) -> Option<&SchemaDeclaration> {
        match self {
            Self::Buffered(b) => Some(&b.instrumentation_schema),
            Self::Fresh(b) => Some(&b.control_schema),
            Self::Streaming(_) => None,
        }
    }
    pub fn schema_mut(&mut self) -> Option<&mut SchemaDeclaration> {
        match self {
            Self::Buffered(b) => Some(&mut b.instrumentation_schema),
            Self::Fresh(b) => Some(&mut b.control_schema),
            Self::Streaming(_) => None,
        }
    }
    pub fn declaration_value(&self) -> Value {
        match self {
            Self::Streaming(b) => serde_json::to_value(b).expect("stream binding"),
            Self::Buffered(b) => serde_json::json!({
                "delivery":b.delivery,"measurement-basis":b.measurement_basis,"instrumented-scope":b.instrumented_scope,
                "instrumentation-schema":b.instrumentation_schema.declaration_value(),"instrumentation":b.instrumentation,"check-schedule":b.check_schedule
            }),
            Self::Fresh(b) => {
                serde_json::json!({"delivery":b.delivery,"control-schema":b.control_schema.declaration_value(),"schedule":b.schedule})
            }
        }
    }
    pub fn validate(&self, policy: &ProfilePolicy) -> Result<(), String> {
        match self {
            Self::Streaming(b) => {
                if policy.max_checkpoint_bytes == 0
                    || policy.max_checkpoint_bytes > MAX_SAFE_INTEGER
                {
                    return Err(
                        "streaming profiles require a positive safe max-checkpoint-bytes".into(),
                    );
                }
                b.check_schedule.validate()?;
            }
            Self::Buffered(b) => {
                policy.validate_buffered()?;
                if b.instrumented_scope.is_empty()
                    || !b.instrumentation.is_object()
                    || b.check_schedule.every == 0
                    || b.check_schedule.every > MAX_SAFE_INTEGER
                    || b.check_schedule.start > MAX_SAFE_INTEGER
                {
                    return Err("buffered binding requires a scope, instrumentation object, and positive safe check interval".into());
                }
            }
            Self::Fresh(b) => {
                let count = b.schedule.len();
                if count == 0
                    || count > policy.max_observations
                    || (policy.metric.is_none() && count != policy.max_observations)
                {
                    return Err("fresh schedules must fit max-observations; fixed counts must match exactly".into());
                }
                b.schedule.expand(policy.max_observations)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct StreamingBinding {
    pub delivery: StreamingDelivery,
    pub measurement_basis: ExecutorBasis,
    pub check_schedule: CheckSchedule,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamingDelivery {
    StreamingProfile,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SelectedStreamingProfile {
    pub policy: ProfilePolicy,
    pub check_schedule: CheckSchedule,
}

impl SelectedStreamingProfile {
    /// Maps a planned checkpoint slot to its measurement repetition.
    pub fn measurement_index(&self, slot_index: u64) -> u64 {
        slot_index / self.policy.max_observations
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StreamingObservationRecord {
    pub delivery: StreamingDelivery,
    pub measurement_basis: ExecutorBasis,
    pub checkpoint_sequence: u64,
    pub check_index: u64,
    pub work: BTreeMap<String, u64>,
    pub terminal: bool,
    pub thresholds: Vec<f64>,
    pub active_time_ns: u64,
    pub elapsed_wall_time_ns: u64,
    pub evaluation_time_ns: u64,
    pub decision: String,
    pub stop_reason: Option<StopReason>,
}

impl CheckSchedule {
    pub fn validate(&self) -> Result<(), String> {
        if self.every == 0 || self.every > MAX_SAFE_INTEGER || self.start > MAX_SAFE_INTEGER {
            return Err("check schedules require a positive safe interval and safe start".into());
        }
        Ok(())
    }

    pub fn includes(&self, index: u64) -> bool {
        index >= self.start && (index - self.start).is_multiple_of(self.every)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FreshBinding {
    pub delivery: FreshDelivery,
    pub control_schema: SchemaDeclaration,
    pub schedule: ControlSchedule,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FreshDelivery {
    FreshSequence,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorBasis {
    ExecutorObserved,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlSchedule {
    Explicit {
        controls: Vec<Value>,
    },
    Linear {
        field: String,
        start: f64,
        step: f64,
        count: u64,
    },
    Geometric {
        field: String,
        start: f64,
        ratio: f64,
        count: u64,
    },
}
impl ControlSchedule {
    pub fn len(&self) -> u64 {
        match self {
            Self::Explicit { controls } => controls.len() as u64,
            Self::Linear { count, .. } | Self::Geometric { count, .. } => *count,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn expand(&self, bound: u64) -> Result<Vec<crate::canonical::CanonicalValue>, String> {
        use crate::canonical::CanonicalValue;
        let count = self.len();
        if count == 0 || count > bound || count > MAX_SAFE_INTEGER {
            return Err(
                "control schedules require a positive count within max-observations".into(),
            );
        }
        let mut controls = Vec::new();
        controls
            .try_reserve(usize::try_from(count).map_err(|_| "control count exceeds address space")?)
            .map_err(|_| "control schedule exceeds available memory")?;
        match self {
            Self::Explicit { controls: values } => {
                for value in values {
                    if !value.is_object() {
                        return Err("observation controls must be objects".into());
                    }
                    controls
                        .push(CanonicalValue::try_from(value.clone()).map_err(|e| e.to_string())?);
                }
            }
            Self::Linear { field, start, .. } | Self::Geometric { field, start, .. } => {
                if field.is_empty() {
                    return Err("schedule field must be nonempty".into());
                }
                let mut value = *start;
                for index in 0..count {
                    if !value.is_finite() {
                        return Err("schedule arithmetic must remain finite".into());
                    }
                    let number = if value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER as f64 {
                        serde_json::json!(value as i64)
                    } else {
                        serde_json::json!(value)
                    };
                    controls.push(
                        CanonicalValue::try_from(serde_json::json!({field: number}))
                            .map_err(|e| e.to_string())?,
                    );
                    if index + 1 < count {
                        value = match self {
                            Self::Linear { step, .. } => value + step,
                            Self::Geometric { ratio, .. } => value * ratio,
                            _ => unreachable!(),
                        };
                    }
                }
            }
        }
        Ok(controls)
    }
}

/// Coordinates and exact control for one independently executed fresh point.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[derive(Eq)]
pub struct FreshControl {
    pub schedule_index: u64,
    pub observation_control: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FreshObservationRecord {
    pub delivery: FreshDelivery,
    pub measurement_basis: ExecutorBasis,
    pub measurement_index: u64,
    pub schedule_index: u64,
    pub observation_control: Value,
}

/// Rebuildable repetition progress, separate from individual attempt acceptance.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FreshSequenceProgress {
    pub resolved_specification_id: String,
    pub logical_specification_id: String,
    pub measurement_index: u64,
    pub scheduled_points: u64,
    pub accepted_points: u64,
    pub consumed_time_ns: u64,
    pub conservative_time_ns: u64,
    pub status: String,
    pub reason: Option<String>,
}

/// Public plan coordinates for a fresh execution without changing legacy slot arrays.
#[derive(Clone, Debug, Serialize)]
pub struct FreshPlannedObservation {
    pub observation_slot_id: String,
    pub attempt_slot_id: String,
    pub measurement_index: u64,
    pub schedule_index: u64,
    pub observation_control: Value,
}
