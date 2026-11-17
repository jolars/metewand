//! Adaptive calibration rules and the durable evidence for a common solver budget.

use serde::{Deserialize, Serialize};

use crate::core::{canonical::MAX_SAFE_INTEGER, duration::positive_nanoseconds};

#[derive(Debug)]
pub enum AdaptiveBudgetGroupResource {}

impl crate::core::identity::IdentityKind for AdaptiveBudgetGroupResource {
    const KIND: &'static str = "adaptive-budget-group";
}

impl crate::core::identity::CanonicalResource for AdaptiveBudgetGroupResource {}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AdaptiveBudget {
    #[serde(default = "initial_pilot_time")]
    pub initial_pilot_time: String,
    #[serde(default = "growth_factor")]
    pub growth_factor: u64,
    #[serde(default = "multiplier")]
    pub multiplier: u64,
    #[serde(default = "minimum_time")]
    pub minimum_time: String,
    #[serde(
        default,
        deserialize_with = "fallback_time",
        skip_serializing_if = "Option::is_none"
    )]
    pub fallback_time: Option<String>,
    #[serde(default = "watchdog_grace")]
    pub watchdog_grace: String,
}

fn initial_pilot_time() -> String {
    "1 s".into()
}
fn growth_factor() -> u64 {
    2
}
fn multiplier() -> u64 {
    11
}
fn minimum_time() -> String {
    "1 ms".into()
}
fn watchdog_grace() -> String {
    "10 s".into()
}
fn fallback_time<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

impl AdaptiveBudget {
    pub fn validate(&self, ceiling: Option<u64>) -> Result<(), String> {
        positive_nanoseconds(&self.initial_pilot_time)?;
        let minimum = positive_nanoseconds(&self.minimum_time)?;
        let fallback = self
            .fallback_time
            .as_deref()
            .map(positive_nanoseconds)
            .transpose()?;
        let grace = positive_nanoseconds(&self.watchdog_grace)?;
        if ceiling.is_some_and(|limit| minimum > limit || fallback.is_some_and(|time| time > limit))
            || fallback.is_some_and(|time| time < minimum)
        {
            return Err("adaptive minimum-time must fit max-active-time; fallback-time must lie between minimum-time and max-active-time".into());
        }
        if fallback.is_some() && ceiling.is_none() {
            return Err("fallback-time requires a max-active-time ceiling; unbounded calibration waits for convergence".into());
        }
        if !(2..=MAX_SAFE_INTEGER).contains(&self.growth_factor)
            || !(1..=MAX_SAFE_INTEGER).contains(&self.multiplier)
            || ceiling.is_some_and(|limit| {
                limit
                    .checked_add(grace)
                    .is_none_or(|n| n > MAX_SAFE_INTEGER)
            })
        {
            return Err("adaptive growth-factor must be at least two, multiplier must be positive, and watchdog duration must fit safe nanoseconds".into());
        }
        Ok(())
    }

    pub fn initial_limit(&self, ceiling: Option<u64>) -> Result<u64, String> {
        self.validate(ceiling)?;
        let limit = positive_nanoseconds(&self.initial_pilot_time)?;
        Ok(ceiling.map_or(limit, |ceiling| limit.min(ceiling)))
    }

    pub fn next_limit(&self, previous: u64, ceiling: Option<u64>) -> Result<Option<u64>, String> {
        self.validate(ceiling)?;
        if ceiling.is_some_and(|limit| previous >= limit) {
            return Ok(None);
        }
        let next = u128::from(previous) * u128::from(self.growth_factor);
        let next = ceiling.map_or(next, |limit| next.min(u128::from(limit)));
        if next > u128::from(MAX_SAFE_INTEGER) {
            return Err("adaptive pilot duration exceeds the supported nanosecond range".into());
        }
        Ok(Some(next as u64))
    }

    /// Millisecond rounding preserves the existing worker duration contract.
    pub fn effective_limit(
        &self,
        reference: Option<u64>,
        ceiling: Option<u64>,
    ) -> Result<u64, String> {
        self.validate(ceiling)?;
        let minimum = positive_nanoseconds(&self.minimum_time)?;
        let limit = if let Some(time) = reference {
            (u128::from(time) * u128::from(self.multiplier)).div_ceil(1_000_000) * 1_000_000
        } else {
            u128::from(
                self.fallback_time
                    .as_deref()
                    .map(positive_nanoseconds)
                    .transpose()?
                    .or(ceiling)
                    .ok_or("unbounded adaptive calibration requires verified convergence")?,
            )
        }
        .max(u128::from(minimum));
        let limit = ceiling.map_or(limit, |ceiling| limit.min(u128::from(ceiling)));
        let grace = positive_nanoseconds(&self.watchdog_grace)?;
        if limit + u128::from(grace) > u128::from(MAX_SAFE_INTEGER) {
            return Err("adaptive limit and watchdog exceed the supported nanosecond range".into());
        }
        Ok(limit as u64)
    }
}

/// A separate resolved execution, never a measured repetition.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CalibrationPilot {
    pub group_id: String,
    pub round: u64,
    pub limit_ns: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PilotEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub round: u64,
    pub limit_ns: u64,
    pub candidate_id: String,
    pub attempt_id: String,
    pub attempt_directory: String,
    pub status: String,
    pub convergence_time_ns: Option<u64>,
    pub observation_id: Option<String>,
}

impl PilotEvidence {
    pub fn was_time_limited(&self) -> bool {
        matches!(
            (self.status.as_str(), self.reason.as_deref()),
            ("censored", Some("active_time_limit")) | ("failed", Some("scientific_timeout"))
        )
    }
}

/// Frozen before measurement, with references only to fully verified pilots.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveBudgetDecision {
    #[serde(deserialize_with = "decision_version")]
    pub schema_version: u32,
    pub group_id: String,
    pub timing_basis: String,
    pub instrumented_scope: String,
    pub multiplier: u64,
    pub minimum_limit_ns: u64,
    pub ceiling_limit_ns: Option<u64>,
    pub effective_limit_ns: u64,
    pub reference_attempt_id: Option<String>,
    pub reference_observation_id: Option<String>,
    pub reference_time_ns: Option<u64>,
    pub fallback: bool,
    pub pilots: Vec<PilotEvidence>,
}

fn decision_version<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    match u32::deserialize(deserializer)? {
        1 => Ok(1),
        _ => Err(serde::de::Error::custom(
            "unsupported adaptive budget decision version",
        )),
    }
}
