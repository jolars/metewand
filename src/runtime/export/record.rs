use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    core::{
        export::{CompatibilityClass, EXPORT_SCHEMA_VERSION, ExportRecord, ExportRecordType},
        inspection::InspectionRecord,
    },
    runtime::evidence::document_profile,
};

use super::{ExportError, canonical_bytes, hex, text};

/// An exportable record backed by verified observation and coordination data.
pub(super) struct Record<'a> {
    pub(super) kind: ExportRecordType,
    pub(super) id: &'a str,
    pub(super) candidate_id: &'a str,
    pub(super) row: &'a Value,
    pub(super) slot_id: Option<&'a str>,
    pub(super) run_id: Option<&'a str>,
    pub(super) attempt_id: Option<&'a str>,
    pub(super) attempt: &'a Value,
    pub(super) slot: &'a Value,
    run: &'a Value,
    observation_slot_id: Option<&'a str>,
    observation_slot: &'a Value,
    metrics: &'a Value,
    result: &'a Value,
    pub(super) compatibility_id: String,
    pub(super) compatibility: CompatibilityClass,
}

impl<'a> Record<'a> {
    pub(super) fn new(
        source: &'a InspectionRecord,
        graph: &'a BTreeMap<String, InspectionRecord>,
    ) -> Result<Self, ExportError> {
        let id = source.id.as_str();
        let row = &source.record;
        let kind = match source.kind.as_str() {
            "applicability" => ExportRecordType::Applicability,
            "attempt" => ExportRecordType::Attempt,
            "resolved-attempt-slot" => ExportRecordType::Slot,
            _ => ExportRecordType::Observation,
        };
        let attempt_id = if kind == ExportRecordType::Attempt {
            Some(id)
        } else {
            row["attempt_id"].as_str()
        };
        let attempt = lookup(graph, attempt_id);
        let slot_id = if kind == ExportRecordType::Slot {
            Some(id)
        } else if kind == ExportRecordType::Observation {
            row["attempt_slot_id"].as_str()
        } else {
            row["slot_id"].as_str()
        };
        let slot = lookup(graph, slot_id);
        let run_id = slot["resolved_run_id"].as_str();
        let run = lookup(graph, run_id);
        let candidate_id = row["candidate_id"]
            .as_str()
            .or_else(|| run["candidate_id"].as_str())
            .ok_or(ExportError::InvalidEvidence("missing candidate identity"))?;
        let observation_slot_id = if kind == ExportRecordType::Slot {
            slot["fresh_sequence"]["observation_slot_id"].as_str()
        } else if kind == ExportRecordType::Observation {
            row["slot_id"].as_str()
        } else {
            None
        };
        let observation_slot = lookup(graph, observation_slot_id);
        let document = &row["document"];
        let metrics = lookup(graph, document["metrics"]["id"].as_str());
        let result = lookup(graph, document["result"]["id"].as_str());
        let (compatibility_id, compatibility) = compatibility(id, slot, document)?;
        Ok(Self {
            kind,
            id,
            candidate_id,
            row,
            slot_id,
            run_id,
            attempt_id,
            attempt,
            slot,
            run,
            observation_slot_id,
            observation_slot,
            metrics,
            result,
            compatibility_id,
            compatibility,
        })
    }

    pub(super) fn provenance(&self) -> &'a Value {
        &self.row["document"]["provenance"]
    }

    pub(super) fn timing(&self) -> &'a Value {
        let document = &self.row["document"];
        if document["timing"].is_object() {
            &document["timing"]
        } else {
            &self.provenance()["data"]["measurements"]
        }
    }

    pub(super) fn warmup(&self) -> Option<bool> {
        self.slot["role"].as_str().map(|role| role == "warmup")
    }

    pub(super) fn metric_values(&self) -> &'a Value {
        &self.metrics["document"]["data"]
    }

    pub(super) fn metric_schema(&self) -> Option<&'a str> {
        self.metrics["document"]["schema"].as_str()
    }

    pub(super) fn result_schema(&self) -> Option<&'a str> {
        self.result["document"]["schema"].as_str()
    }

    pub(super) fn order_key(&self) -> (&str, Option<&str>, Option<u64>, Option<&str>, &str) {
        (
            self.candidate_id,
            self.slot_id,
            self.attempt["retry_index"].as_u64(),
            self.attempt_id,
            self.id,
        )
    }

    pub(super) fn matches(&self, id: &str) -> bool {
        [
            Some(self.id),
            Some(self.candidate_id),
            self.run["logical_specification_id"].as_str(),
            self.run_id,
            self.slot["logical_slot_id"].as_str(),
            self.slot_id,
            self.observation_slot["logical_slot_id"].as_str(),
            self.observation_slot_id,
            self.attempt_id,
            self.row["slot_id"].as_str(),
            self.run["applicability_id"].as_str(),
        ]
        .contains(&Some(id))
    }

    pub(super) fn index_record(&self) -> ExportRecord {
        let state = text(self.attempt, "state");
        let failure_code = text(self.attempt, "failure_code");
        let outcome = match self.kind {
            ExportRecordType::Slot => text(&self.slot["fresh_sequence"], "disposition"),
            ExportRecordType::Applicability => Some(
                if self.row["applicable"] == true {
                    "applicable"
                } else {
                    "not_applicable"
                }
                .into(),
            ),
            _ if failure_code.as_deref() == Some("invalid_result") => Some("invalid".into()),
            _ => state.clone(),
        };
        let mut profile = document_profile(&self.row["document"]).cloned();
        if let Some(fresh) = self.slot.get("fresh_sequence") {
            let profile = profile.get_or_insert_with(|| fresh.clone());
            profile["progress"] = fresh["progress"].clone();
            profile["budget"] = self
                .row
                .get("fresh_budget")
                .or_else(|| self.attempt.get("fresh_budget"))
                .cloned()
                .unwrap_or(Value::Null);
        }
        ExportRecord {
            profile,
            schema_version: EXPORT_SCHEMA_VERSION,
            record_type: self.kind,
            id: self.id.to_owned(),
            candidate_id: self.candidate_id.to_owned(),
            logical_run_id: text(self.run, "logical_specification_id"),
            resolved_run_id: self.run_id.map(str::to_owned),
            logical_slot_id: text(self.slot, "logical_slot_id"),
            resolved_slot_id: self.slot_id.map(str::to_owned),
            logical_observation_slot_id: text(self.observation_slot, "logical_slot_id"),
            resolved_observation_slot_id: self.observation_slot_id.map(str::to_owned),
            attempt_id: self.attempt_id.map(str::to_owned),
            retry_index: self.attempt["retry_index"].as_u64(),
            attempt_state: state,
            outcome,
            valid: if self.kind == ExportRecordType::Observation {
                self.row["valid"].as_bool()
            } else if failure_code.as_deref() == Some("invalid_result") {
                Some(false)
            } else {
                None
            },
            selected: self.row["selected"].as_bool(),
            completion: text(self.row, "completion"),
            failure_code,
            warmup: self.warmup(),
            compatibility_id: self.compatibility_id.clone(),
            compatibility: self.compatibility.clone(),
            timing: self.timing().clone(),
            metrics: self.metric_values().clone(),
            metric_schema: self.metric_schema().map(str::to_owned),
            result_schema: self.result_schema().map(str::to_owned),
            provenance: self.provenance().clone(),
            evidence: json!({
                "source": self.row,
                "attempt": self.attempt,
                "attempt_slot": self.slot,
                "observation_slot": self.observation_slot,
                "resolved_run": self.run,
                "metrics": self.metrics,
                "result": self.result,
            }),
        }
    }
}

fn compatibility(
    id: &str,
    slot: &Value,
    document: &Value,
) -> Result<(String, CompatibilityClass), ExportError> {
    let provenance = &document["provenance"];
    let context = &provenance["data"]["context"];
    let execution = &provenance["data"]["execution"];
    let measurements = &provenance["data"]["measurements"];
    let controls = execution
        .get("controls")
        .or_else(|| context.get("controls"))
        .cloned()
        .unwrap_or(Value::Null);
    let control_evidence = if execution["controls"].is_array() {
        "observed"
    } else if context["controls"].is_array() {
        "declared"
    } else {
        "unavailable"
    };
    let mut statuses = BTreeSet::new();
    if let Some(controls) = controls.as_array() {
        for control in controls {
            statuses.insert(control["status"].as_str().unwrap_or("unknown"));
        }
    }
    let control_class = if control_evidence != "observed"
        || statuses.is_empty()
        || statuses
            .iter()
            .any(|status| !matches!(*status, "enforced" | "best_effort" | "unsupported"))
    {
        "unknown"
    } else if statuses.contains("unsupported") {
        "unsupported"
    } else if statuses.contains("best_effort") {
        "best_effort"
    } else if statuses.iter().all(|s| *s == "enforced") {
        "enforced"
    } else {
        "unknown"
    };
    let policy = context
        .get("execution_policy")
        .cloned()
        .unwrap_or(Value::Null);
    let selected_profile = execution
        .get("profile_policy")
        .or_else(|| provenance["data"]["profile"].get("selected"));
    let profile_class = if context.get("fresh_sequence").is_some()
        || slot.get("fresh_sequence").is_some()
        || document["profile"]["delivery"] == "fresh_sequence"
    {
        Some(json!({"delivery":"fresh_sequence","measurement_basis":"executor_observed"}))
    } else {
        selected_profile.map(|p|json!({"delivery":"buffered_profile","measurement_basis":"instrumented","instrumented_scope":p["instrumented_scope"],"clock":"monotonic_ns","scope":p["scope"]}))
    };
    let profile_class = if document["profile"]["delivery"] == "streaming_profile"
        || provenance["data"]["profile"]["buffer"]["delivery"] == "streaming_profile"
        || context.get("streaming_profile").is_some()
    {
        Some(
            json!({"delivery":"streaming_profile","measurement_basis":"executor_observed","clock":"active_wall_time","excluded_phases":["checkpoint_validation","evaluation","publication","stop_acknowledgment"]}),
        )
    } else {
        profile_class
    };
    let mut compatibility = CompatibilityClass {
        profile: profile_class,
        execution_semantics_version: provenance["execution_semantics_version"]
            .as_u64()
            .and_then(|n| n.try_into().ok()),
        timing_scope: text(execution, "timing_scope").or_else(|| text(context, "timing_scope")),
        primary_time: text(&document["timing"], "primary_time")
            .or_else(|| text(&policy, "primary_time")),
        environment_class: text(execution, "environment_class").or_else(|| {
            text(
                &context["implementation"]["environment"]["fingerprint"],
                "class",
            )
        }),
        control_class: control_class.into(),
        control_evidence: control_evidence.into(),
        controls,
        execution_policy: policy,
        measurements_version: measurements["version"]
            .as_u64()
            .and_then(|n| n.try_into().ok()),
        accounting: {
            let mut accounting = json!({"cpu_time_scope": measurements["cpu_time_scope"], "peak_rss_scope": measurements["peak_rss_scope"], "cpu_time": coverage(&measurements["cpu_time_ns"]), "peak_rss": coverage(&measurements["peak_rss_bytes"])});
            if let Some(source) = measurements.get("cpu_accounting") {
                accounting["cpu_accounting"] = source.clone();
            }
            accounting
        },
        unclassified_record: None,
    };
    if compatibility.execution_semantics_version.is_none()
        || compatibility.timing_scope.is_none()
        || compatibility.primary_time.is_none()
        || compatibility.environment_class.is_none()
        || control_class == "unknown"
        || compatibility.execution_policy.is_null()
        || compatibility.measurements_version.is_none()
    {
        compatibility.unclassified_record = Some(id.to_owned());
    }
    let compatibility_id = format!(
        "export-v1:{}",
        hex(&Sha256::digest(canonical_bytes(&compatibility)?))
    );
    Ok((compatibility_id, compatibility))
}

fn coverage(value: &Value) -> Value {
    json!({"status": value["status"], "reason": value["reason"]})
}

fn lookup<'a>(graph: &'a BTreeMap<String, InspectionRecord>, id: Option<&str>) -> &'a Value {
    id.and_then(|id| graph.get(id))
        .map(|record| &record.record)
        .unwrap_or(&Value::Null)
}
