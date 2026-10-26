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
    pub max_trace_points: u64,
    pub max_buffer_bytes: u64,
    pub terminal_reserve_bytes: u64,
    pub max_active_time: String,
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
        if [
            self.max_observations,
            self.max_trace_points,
            self.max_buffer_bytes,
            self.terminal_reserve_bytes,
        ]
        .iter()
        .any(|v| *v == 0 || *v > MAX_SAFE_INTEGER)
            || self.terminal_reserve_bytes > self.max_buffer_bytes
        {
            return Err("profile bounds must be positive safe integers; terminal reserve must fit the buffer".into());
        }
        self.active_time_ns()?;
        match (&self.metric, self.direction, self.threshold) {
            (None, None, None) if self.record_thresholds.is_empty() => {
                if self.max_trace_points < self.max_observations {
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
    pub implementations: BTreeMap<Name, BufferedBinding>,
}

impl ProfileDefinition {
    pub fn declaration_value(&self) -> Value {
        let mut value = serde_json::to_value(&self.policy).expect("validated profile");
        value["implementations"] = self.implementations.iter().map(|(name,b)| (name.as_str().to_owned(),serde_json::json!({
            "delivery":b.delivery,"measurement-basis":b.measurement_basis,"instrumented-scope":b.instrumented_scope,
            "instrumentation-schema":b.instrumentation_schema.declaration_value(),"instrumentation":b.instrumentation,"check-schedule":b.check_schedule
        }))).collect::<serde_json::Map<_,_>>().into();
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
