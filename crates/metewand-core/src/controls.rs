//! Executor control declarations and observed enforcement evidence.

use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;

use crate::{manifest::Enforcement, records::ExecutionPolicyRecord};

/// Strength of an executor's evidence for a requested control.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlStatus {
    /// The executor guarantees the control over its complete scope.
    Enforced,
    /// The executor applies or checks the control without preventing all violations.
    BestEffort,
    /// The executor cannot apply the requested control.
    Unsupported,
}

/// One requested control, including its value and explicit guarantee.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ControlRecord {
    /// Stable control name.
    pub name: String,
    /// Effective request, including policy defaults.
    pub requested: Value,
    /// Whether acceptance requires enforcement.
    pub required: bool,
    /// Declared or observed enforcement strength.
    pub status: ControlStatus,
    /// Lifetime over which the guarantee applies.
    pub scope: &'static str,
}

/// A required control was unavailable or lost enforcement.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("required control `{control}` is not enforced")]
pub struct ControlFailure {
    /// Stable name of the first insufficient control.
    pub control: String,
}

/// Rejects insufficient required controls in deterministic report order.
///
/// # Errors
/// Returns the first required control without complete enforcement.
pub fn validate_required_controls(controls: &[ControlRecord]) -> Result<(), ControlFailure> {
    for control in controls {
        if control.required && control.status != ControlStatus::Enforced {
            return Err(ControlFailure {
                control: control.name.clone(),
            });
        }
    }
    Ok(())
}

/// Declares the trusted local backend's lifetime guarantees without host access.
///
/// Resource requests remain visible even when this backend cannot enforce them.
/// A private directory and process group do not provide filesystem isolation or
/// complete descendant containment.
#[must_use]
pub fn local_control_report(policy: &ExecutionPolicyRecord) -> Vec<ControlRecord> {
    local_controls(
        policy.network,
        policy.enforcement == Enforcement::Required,
        policy.cpus,
        policy.threads,
        policy.memory.as_deref(),
    )
}

/// Reports the effective defaults for callers without a manifest policy.
#[must_use]
pub fn default_local_control_report() -> Vec<ControlRecord> {
    local_controls(false, false, None, None, None)
}

fn local_controls(
    network: bool,
    required: bool,
    cpus: Option<u64>,
    threads: Option<u64>,
    memory: Option<&str>,
) -> Vec<ControlRecord> {
    let mut controls = Vec::new();
    let mut add = |name: &str, requested: Value, status| {
        controls.push(ControlRecord {
            name: name.to_owned(),
            requested,
            required,
            status,
            scope: "worker_lifetime",
        })
    };
    add(
        "network",
        json!(network),
        if network {
            ControlStatus::Enforced
        } else {
            ControlStatus::Unsupported
        },
    );
    add(
        "filesystem_isolation",
        json!(true),
        ControlStatus::Unsupported,
    );
    add(
        "dataset_immutability",
        json!(true),
        ControlStatus::BestEffort,
    );
    add("process_tree", json!(true), ControlStatus::BestEffort);
    add(
        "process_tree_accounting",
        json!(true),
        ControlStatus::Unsupported,
    );
    add(
        "private_working_directory",
        json!(true),
        ControlStatus::BestEffort,
    );
    if let Some(cpus) = cpus {
        add("cpus", json!(cpus), ControlStatus::Unsupported);
    }
    if let Some(threads) = threads {
        add("threads", json!(threads), ControlStatus::Unsupported);
    }
    if let Some(memory) = memory {
        add("memory", json!(memory), ControlStatus::Unsupported);
    }
    controls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_local_report_matches_version_one_canonical_vector() {
        let serialized = serde_json::to_vec(&default_local_control_report()).unwrap();
        let canonical = crate::canonical::CanonicalValue::from_slice(&serialized)
            .unwrap()
            .to_canonical_bytes()
            .unwrap();
        assert_eq!(
            canonical,
            include_str!("../../../fixtures/execution-controls/v1.json")
                .trim_end()
                .as_bytes()
        );
    }

    #[test]
    fn runtime_loss_and_best_effort_are_not_enforcement() {
        let mut controls = vec![ControlRecord {
            name: "memory".to_owned(),
            requested: json!("8 GiB"),
            required: true,
            status: ControlStatus::Enforced,
            scope: "worker_lifetime",
        }];
        assert!(validate_required_controls(&controls).is_ok());
        for status in [ControlStatus::BestEffort, ControlStatus::Unsupported] {
            controls[0].status = status;
            assert_eq!(
                validate_required_controls(&controls).unwrap_err().control,
                "memory"
            );
        }
        controls[0].required = false;
        assert!(validate_required_controls(&controls).is_ok());
    }
}
