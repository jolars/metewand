//! Lossless result projections and tidy rows, independent of storage and the CLI.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod analysis;

/// Compatibility version of indexes, tidy rows, and export receipts.
pub const EXPORT_SCHEMA_VERSION: u32 = 1;

/// The scientific entity represented by a row.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportRecordType {
    Slot,
    Applicability,
    Attempt,
    Observation,
    Trace,
}

/// Guarantees and measurement boundaries that must stay visible in comparisons.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityClass {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
    pub execution_semantics_version: Option<u32>,
    pub timing_scope: Option<String>,
    pub primary_time: Option<String>,
    pub environment_class: Option<String>,
    pub control_class: String,
    pub control_evidence: String,
    pub controls: Value,
    pub execution_policy: Value,
    pub measurements_version: Option<u32>,
    pub accounting: Value,
    /// Missing evidence cannot establish compatibility between different records.
    pub unclassified_record: Option<String>,
}

/// One complete index record. Raw evidence preserves fields unknown to readers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
    pub schema_version: u32,
    pub record_type: ExportRecordType,
    pub id: String,
    pub candidate_id: String,
    pub logical_run_id: Option<String>,
    pub resolved_run_id: Option<String>,
    pub logical_slot_id: Option<String>,
    pub resolved_slot_id: Option<String>,
    pub logical_observation_slot_id: Option<String>,
    pub resolved_observation_slot_id: Option<String>,
    pub attempt_id: Option<String>,
    pub retry_index: Option<u64>,
    pub attempt_state: Option<String>,
    /// Invalid results retain the stored failed state and an explicit invalid outcome.
    pub outcome: Option<String>,
    pub valid: Option<bool>,
    pub selected: Option<bool>,
    pub completion: Option<String>,
    pub failure_code: Option<String>,
    pub warmup: Option<bool>,
    pub compatibility_id: String,
    pub compatibility: CompatibilityClass,
    pub timing: Value,
    pub metrics: Value,
    pub metric_schema: Option<String>,
    pub result_schema: Option<String>,
    pub provenance: Value,
    pub evidence: Value,
}

/// One metric coordinate, or a null coordinate for an entity without metrics.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TidyRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_time_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_wall_time_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrumented_time_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrumented_total_time_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provisional_metric_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provisional_metric_value: Option<Value>,
    #[serde(flatten)]
    pub record: ExportRecord,
    /// RFC 6901 pointer into evaluator-owned metric data.
    pub metric_name: Option<String>,
    pub metric_value: Value,
    /// Unavailable annotations remain null rather than being guessed from names.
    pub metric_unit: Option<String>,
    pub metric_direction: Option<String>,
    pub timed_wall_time_ns: Option<u64>,
    pub timed_wall_time_status: String,
    pub implementation_time_ns: Option<u64>,
    pub evaluation_time_ns: Option<u64>,
    pub cpu_time_ns: Option<u64>,
    pub cpu_time_status: String,
    pub peak_rss_bytes: Option<u64>,
    pub peak_rss_status: String,
}

impl ExportRecord {
    /// Preserves unavailable and partial accounting when projecting a quantity.
    #[must_use]
    pub fn measurement(&self, key: &str) -> (Option<u64>, String) {
        if let Some(value) = self.timing[key].as_u64() {
            return (Some(value), "complete".to_owned());
        }
        let measurement = self
            .timing
            .get(key)
            .filter(|v| v.is_object())
            .unwrap_or(&self.provenance["data"]["measurements"][key]);
        (
            measurement["value"].as_u64(),
            measurement["status"]
                .as_str()
                .unwrap_or("unavailable")
                .to_owned(),
        )
    }

    #[must_use]
    pub fn tidy_rows(&self) -> Vec<TidyRow> {
        let diagnostic = self.record_type == ExportRecordType::Trace;
        let values = if diagnostic {
            metric_values(
                &self
                    .profile
                    .as_ref()
                    .map_or(Value::Null, |p| p["provisional_metrics"].clone()),
            )
        } else {
            metric_values(&self.metrics)
        };
        let values = if values.is_empty() {
            vec![(None, Value::Null)]
        } else {
            values
                .into_iter()
                .map(|(name, value)| (Some(name), value))
                .collect()
        };
        values
            .into_iter()
            .map(|(metric_name, metric_value)| {
                let mut record = self.clone();
                // The metric coordinate carries the value; the immutable evidence
                // still retains the complete original metrics envelope.
                record.metrics = Value::Null;
                let (timed_wall_time_ns, timed_wall_time_status) =
                    self.measurement("timed_wall_time_ns");
                let (cpu_time_ns, cpu_time_status) = self.measurement("cpu_time_ns");
                let (peak_rss_bytes, peak_rss_status) = self.measurement("peak_rss_bytes");
                let profile = self.profile.as_ref();
                let provisional = metric_name
                    .as_ref()
                    .and_then(|name| profile.and_then(|p| p["provisional_metrics"].pointer(name)))
                    .cloned();
                TidyRow {
                    active_time_ns: profile.and_then(|p| p["active_time_ns"].as_u64()),
                    elapsed_wall_time_ns: profile.and_then(|p| p["elapsed_wall_time_ns"].as_u64()),
                    checkpoint_sequence: profile.and_then(|p| p["checkpoint_sequence"].as_u64()),
                    instrumented_time_ns: profile.and_then(|p| p["instrumented_time_ns"].as_u64()),
                    instrumented_total_time_ns: profile
                        .and_then(|p| p["instrumented_total_time_ns"].as_u64()),
                    provisional_metric_name: if diagnostic {
                        metric_name.clone()
                    } else {
                        None
                    },
                    provisional_metric_value: if diagnostic {
                        Some(metric_value.clone())
                    } else {
                        provisional
                    },
                    record,
                    metric_name: if diagnostic { None } else { metric_name },
                    metric_value: if diagnostic {
                        Value::Null
                    } else {
                        metric_value
                    },
                    metric_unit: None,
                    metric_direction: None,
                    timed_wall_time_ns,
                    timed_wall_time_status,
                    implementation_time_ns: self.timing["implementation_time_ns"].as_u64(),
                    evaluation_time_ns: self.measurement("evaluation_time_ns").0,
                    cpu_time_ns,
                    cpu_time_status,
                    peak_rss_bytes,
                    peak_rss_status,
                }
            })
            .collect()
    }
}

/// Flattens objects in bytewise key order while retaining arrays as metric values.
#[must_use]
pub fn metric_values(value: &Value) -> Vec<(String, Value)> {
    fn visit(value: &Value, pointer: String, rows: &mut Vec<(String, Value)>) {
        if let Some(object) = value.as_object().filter(|object| !object.is_empty()) {
            let sorted = object.iter().collect::<std::collections::BTreeMap<_, _>>();
            for (key, value) in sorted {
                let key = key.replace('~', "~0").replace('/', "~1");
                visit(value, format!("{pointer}/{key}"), rows);
            }
        } else {
            rows.push((pointer, value.clone()));
        }
    }
    let mut rows = Vec::new();
    if value.as_object().is_some_and(|object| !object.is_empty()) {
        visit(value, String::new(), &mut rows);
    }
    rows
}

/// Escapes a UTF-8 CSV cell without changing its contents.
#[must_use]
pub fn csv_cell(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// Supported standalone table encodings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Json,
    Jsonl,
    Csv,
}

impl ExportFormat {
    #[must_use]
    pub const fn filename(self) -> &'static str {
        match self {
            Self::Json => "results.json",
            Self::Jsonl => "results.jsonl",
            Self::Csv => "results.csv",
        }
    }
}

/// Exact selection and content lineage for a standalone table.
#[derive(Clone, Debug, Serialize)]
pub struct ExportManifest {
    pub schema_version: u32,
    pub tool_version: String,
    pub selector: Vec<String>,
    pub candidate_ids: Vec<String>,
    pub attempt_ids: Vec<String>,
    pub observation_ids: Vec<String>,
    pub result_schemas: Vec<String>,
    pub metric_schemas: Vec<String>,
    pub compatibility_classes: std::collections::BTreeMap<String, CompatibilityClass>,
    pub files: std::collections::BTreeMap<String, String>,
}

/// One command receipt, rendered identically in human and machine modes.
#[derive(Clone, Debug, Serialize)]
pub struct ExportReport {
    pub command: &'static str,
    pub schema_version: u32,
    pub records: usize,
    pub rows: usize,
    pub destination: Option<String>,
    pub index_path: Option<String>,
    pub manifest: ExportManifest,
}
