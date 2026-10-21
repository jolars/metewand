//! Rebuildable result indexes and standalone tables from verified store snapshots.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use metewand_core::{
    canonical::{CanonicalJsonError, CanonicalValue},
    export::{
        CompatibilityClass, EXPORT_SCHEMA_VERSION, ExportFormat, ExportManifest, ExportRecord,
        ExportRecordType, ExportReport, TidyRow, csv_cell,
    },
    inspection::InspectionRecord,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::inspection::{Inspection, InspectionError};

/// A deterministic projection of one committed snapshot and its verified artifacts.
pub struct ResultSet {
    root: PathBuf,
    pub records: Vec<ExportRecord>,
}

impl ResultSet {
    /// Reads authoritative evidence directly, ignoring any existing index.
    pub fn read(root: &Path) -> Result<Self, ExportError> {
        let mut inspection = Inspection::default();
        inspection.add_store(root)?;
        let graph = inspection.into_records();
        let mut records = Vec::new();
        for source in graph.values().filter(|record| {
            matches!(
                record.kind.as_str(),
                "applicability" | "attempt" | "observation"
            )
        }) {
            records.push(project(source, &graph)?);
        }
        records.sort_by(|a, b| {
            (
                &a.candidate_id,
                &a.resolved_slot_id,
                a.retry_index,
                &a.attempt_id,
                &a.id,
            )
                .cmp(&(
                    &b.candidate_id,
                    &b.resolved_slot_id,
                    b.retry_index,
                    &b.attempt_id,
                    &b.id,
                ))
        });
        Ok(Self {
            root: fs::canonicalize(root)?,
            records,
        })
    }

    pub fn index_bytes(&self) -> Result<Vec<u8>, ExportError> {
        json_lines(&self.records)
    }

    /// Atomically replaces only the explicitly requested, nonauthoritative index.
    pub fn rebuild_index(&self) -> Result<PathBuf, ExportError> {
        let target = self.index_target()?;
        let mut stage = tempfile::NamedTempFile::new_in(&self.root)?;
        stage.write_all(&self.index_bytes()?)?;
        stage.persist(&target).map_err(|error| error.error)?;
        Ok(target)
    }

    fn index_target(&self) -> Result<PathBuf, ExportError> {
        let target = self.root.join("index.jsonl");
        match fs::symlink_metadata(&target) {
            Ok(metadata) if !metadata.is_file() => return Err(ExportError::UnsafeIndex),
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        Ok(target)
    }

    /// Selects exact identities and retains producing attempts and applicability.
    pub fn select(&self, ids: &[String]) -> Result<Vec<ExportRecord>, ExportError> {
        if ids.is_empty() {
            return Ok(self.records.clone());
        }
        let mut selected = BTreeSet::new();
        for id in ids {
            let mut found = false;
            for record in &self.records {
                if selection_ids(record).contains(&id.as_str()) {
                    selected.insert(record.id.clone());
                    found = true;
                }
            }
            if !found {
                return Err(ExportError::UnknownIdentity(id.clone()));
            }
        }
        let attempts = self
            .records
            .iter()
            .filter(|record| selected.contains(&record.id))
            .filter_map(|record| record.attempt_id.as_deref())
            .collect::<BTreeSet<_>>();
        let candidates = self
            .records
            .iter()
            .filter(|record| selected.contains(&record.id))
            .map(|record| record.candidate_id.as_str())
            .collect::<BTreeSet<_>>();
        Ok(self
            .records
            .iter()
            .filter(|record| {
                selected.contains(&record.id)
                    || record
                        .attempt_id
                        .as_deref()
                        .is_some_and(|id| attempts.contains(id))
                    || (record.record_type == ExportRecordType::Applicability
                        && candidates.contains(record.candidate_id.as_str()))
            })
            .cloned()
            .collect())
    }

    /// Writes a new directory with one table and its adjacent lineage manifest.
    /// Existing destinations, including symlinks, are never replaced.
    pub fn export(
        &self,
        destination: Option<&Path>,
        format: ExportFormat,
        ids: &[String],
        rebuild_index: bool,
    ) -> Result<ExportReport, ExportError> {
        let records = self.select(ids)?;
        if rebuild_index {
            self.index_target()?;
        }
        let rows = records
            .iter()
            .flat_map(ExportRecord::tidy_rows)
            .collect::<Vec<_>>();
        let data = match format {
            ExportFormat::Jsonl => json_lines(&rows)?,
            ExportFormat::Csv => csv(&rows)?,
        };
        let mut manifest = lineage(&records, ids);
        let mut destination_path = None;
        if let Some(destination) = destination {
            let filename = destination
                .file_name()
                .filter(|name| !name.is_empty())
                .ok_or(ExportError::InvalidDestination)?;
            let parent = destination
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let parent = fs::canonicalize(parent)?;
            let target = parent.join(filename);
            manifest
                .files
                .insert(format.filename().into(), hex(&Sha256::digest(&data)));
            let stage = tempfile::Builder::new()
                .prefix(".metewand-export-")
                .tempdir_in(&parent)?;
            fs::write(stage.path().join(format.filename()), &data)?;
            fs::write(
                stage.path().join("export.json"),
                canonical_bytes(&manifest)?,
            )?;
            rustix::fs::renameat_with(
                rustix::fs::CWD,
                stage.path(),
                rustix::fs::CWD,
                &target,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(io::Error::from)?;
            destination_path = Some(target.to_string_lossy().into_owned());
        }
        let index_path = if rebuild_index {
            Some(self.rebuild_index()?.to_string_lossy().into_owned())
        } else {
            None
        };
        Ok(ExportReport {
            command: "export",
            schema_version: EXPORT_SCHEMA_VERSION,
            records: records.len(),
            rows: rows.len(),
            destination: destination_path,
            index_path,
            manifest,
        })
    }
}

fn project(
    source: &InspectionRecord,
    graph: &BTreeMap<String, InspectionRecord>,
) -> Result<ExportRecord, ExportError> {
    let row = &source.record;
    let record_type = match source.kind.as_str() {
        "applicability" => ExportRecordType::Applicability,
        "attempt" => ExportRecordType::Attempt,
        _ => ExportRecordType::Observation,
    };
    let attempt_id = if record_type == ExportRecordType::Attempt {
        Some(source.id.clone())
    } else {
        text(row, "attempt_id")
    };
    let attempt = lookup(graph, attempt_id.as_deref());
    let slot_id = if record_type == ExportRecordType::Observation {
        text(row, "attempt_slot_id")
    } else {
        text(row, "slot_id")
    };
    let slot = lookup(graph, slot_id.as_deref());
    let run_id = text(&slot, "resolved_run_id");
    let run = lookup(graph, run_id.as_deref());
    let candidate_id = text(row, "candidate_id")
        .or_else(|| text(&run, "candidate_id"))
        .ok_or(ExportError::InvalidEvidence("missing candidate identity"))?;
    let document = &row["document"];
    if record_type == ExportRecordType::Observation
        && (document["id"] != row["id"]
            || document["slot_id"] != row["slot_id"]
            || document["attempt_id"] != row["attempt_id"]
            || document["validity"]["valid"] != row["valid"])
    {
        return Err(ExportError::InvalidEvidence(
            "observation document disagrees with committed coordination",
        ));
    }
    let observation_slot_id = if record_type == ExportRecordType::Observation {
        text(row, "slot_id")
    } else {
        None
    };
    let observation_slot = lookup(graph, observation_slot_id.as_deref());
    let provenance = document.get("provenance").cloned().unwrap_or(Value::Null);
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
    let mut compatibility = CompatibilityClass {
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
        accounting: json!({"cpu_time_scope": measurements["cpu_time_scope"], "peak_rss_scope": measurements["peak_rss_scope"], "cpu_time": coverage(&measurements["cpu_time_ns"]), "peak_rss": coverage(&measurements["peak_rss_bytes"])}),
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
        compatibility.unclassified_record = Some(source.id.clone());
    }
    let compatibility_id = format!(
        "export-v1:{}",
        hex(&Sha256::digest(canonical_bytes(&compatibility)?))
    );
    let state = text(&attempt, "state");
    let failure_code = text(&attempt, "failure_code");
    let outcome = if record_type == ExportRecordType::Applicability {
        Some(
            if row["applicable"] == true {
                "applicable"
            } else {
                "not_applicable"
            }
            .into(),
        )
    } else if failure_code.as_deref() == Some("invalid_result") {
        Some("invalid".into())
    } else {
        state.clone()
    };
    let metrics = lookup(graph, document["metrics"]["id"].as_str());
    let result = lookup(graph, document["result"]["id"].as_str());
    Ok(ExportRecord {
        schema_version: EXPORT_SCHEMA_VERSION,
        record_type,
        id: source.id.clone(),
        candidate_id,
        logical_run_id: text(&run, "logical_specification_id"),
        resolved_run_id: run_id,
        logical_slot_id: text(&slot, "logical_slot_id"),
        resolved_slot_id: slot_id,
        logical_observation_slot_id: text(&observation_slot, "logical_slot_id"),
        resolved_observation_slot_id: observation_slot_id,
        attempt_id,
        retry_index: attempt["retry_index"].as_u64(),
        attempt_state: state,
        outcome,
        valid: if record_type == ExportRecordType::Observation {
            row["valid"].as_bool()
        } else if failure_code.as_deref() == Some("invalid_result") {
            Some(false)
        } else {
            None
        },
        selected: row["selected"].as_bool(),
        completion: text(row, "completion"),
        failure_code,
        warmup: slot["role"].as_str().map(|role| role == "warmup"),
        compatibility_id,
        compatibility,
        timing: if document["timing"].is_object() {
            document["timing"].clone()
        } else {
            measurements.clone()
        },
        metrics: metrics["document"]
            .get("data")
            .cloned()
            .unwrap_or(Value::Null),
        metric_schema: text(&metrics["document"], "schema"),
        result_schema: text(&result["document"], "schema"),
        provenance,
        evidence: json!({"source": row, "attempt": attempt, "attempt_slot": slot, "observation_slot": observation_slot, "resolved_run": run, "metrics": metrics, "result": result}),
    })
}

fn coverage(value: &Value) -> Value {
    json!({"status": value["status"], "reason": value["reason"]})
}
fn text(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}
fn lookup(graph: &BTreeMap<String, InspectionRecord>, id: Option<&str>) -> Value {
    id.and_then(|id| graph.get(id))
        .map(|record| record.record.clone())
        .unwrap_or(Value::Null)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn selection_ids(record: &ExportRecord) -> Vec<&str> {
    [
        Some(record.id.as_str()),
        Some(record.candidate_id.as_str()),
        record.logical_run_id.as_deref(),
        record.resolved_run_id.as_deref(),
        record.logical_slot_id.as_deref(),
        record.resolved_slot_id.as_deref(),
        record.logical_observation_slot_id.as_deref(),
        record.resolved_observation_slot_id.as_deref(),
        record.attempt_id.as_deref(),
        record.evidence["source"]["slot_id"].as_str(),
        record.evidence["resolved_run"]["applicability_id"].as_str(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ExportError> {
    Ok(CanonicalValue::try_from(serde_json::to_value(value)?)?.to_canonical_bytes()?)
}
fn json_lines<T: Serialize>(values: &[T]) -> Result<Vec<u8>, ExportError> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(canonical_bytes(value)?);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn lineage(records: &[ExportRecord], ids: &[String]) -> ExportManifest {
    fn ordered(values: impl Iterator<Item = String>) -> Vec<String> {
        values.collect::<BTreeSet<_>>().into_iter().collect()
    }
    ExportManifest {
        schema_version: EXPORT_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        selector: ordered(ids.iter().cloned()),
        candidate_ids: ordered(records.iter().map(|r| r.candidate_id.clone())),
        attempt_ids: records
            .iter()
            .filter(|r| r.record_type == ExportRecordType::Attempt)
            .map(|r| r.id.clone())
            .collect(),
        observation_ids: records
            .iter()
            .filter(|r| r.record_type == ExportRecordType::Observation)
            .map(|r| r.id.clone())
            .collect(),
        result_schemas: ordered(records.iter().filter_map(|r| r.result_schema.clone())),
        metric_schemas: ordered(records.iter().filter_map(|r| r.metric_schema.clone())),
        compatibility_classes: records
            .iter()
            .map(|r| (r.compatibility_id.clone(), r.compatibility.clone()))
            .collect(),
        files: BTreeMap::new(),
    }
}

fn csv(rows: &[TidyRow]) -> Result<Vec<u8>, ExportError> {
    // Nested values use canonical JSON cells, preserving types and explicit nulls.
    const COLUMNS: &[&str] = &[
        "schema_version",
        "record_type",
        "id",
        "candidate_id",
        "logical_run_id",
        "resolved_run_id",
        "logical_slot_id",
        "resolved_slot_id",
        "logical_observation_slot_id",
        "resolved_observation_slot_id",
        "attempt_id",
        "retry_index",
        "attempt_state",
        "outcome",
        "valid",
        "selected",
        "completion",
        "failure_code",
        "warmup",
        "compatibility_id",
        "timing_scope",
        "primary_time",
        "environment_class",
        "control_class",
        "control_evidence",
        "metric_name",
        "metric_value",
        "metric_unit",
        "metric_direction",
        "timed_wall_time_ns",
        "timed_wall_time_status",
        "implementation_time_ns",
        "evaluation_time_ns",
        "cpu_time_ns",
        "cpu_time_status",
        "peak_rss_bytes",
        "peak_rss_status",
        "metric_schema",
        "result_schema",
        "timing",
        "compatibility",
        "provenance",
        "evidence",
    ];
    let mut output = COLUMNS.join(",");
    output.push_str("\r\n");
    for row in rows {
        let r = serde_json::to_value(row)?;
        let cells = COLUMNS
            .iter()
            .map(|column| {
                let value = if matches!(
                    *column,
                    "timing_scope"
                        | "primary_time"
                        | "environment_class"
                        | "control_class"
                        | "control_evidence"
                ) {
                    &r["compatibility"][column]
                } else {
                    &r[column]
                };
                let cell = if *column == "metric_value" {
                    String::from_utf8(canonical_bytes(value)?).expect("JSON is UTF-8")
                } else if value.is_null() {
                    String::new()
                } else if let Some(value) = value.as_str() {
                    value.to_owned()
                } else {
                    String::from_utf8(canonical_bytes(value)?).expect("JSON is UTF-8")
                };
                Ok(csv_cell(&cell))
            })
            .collect::<Result<Vec<_>, ExportError>>()?;
        output.push_str(&cells.join(","));
        output.push_str("\r\n");
    }
    Ok(output.into_bytes())
}

#[derive(Debug, Error)]
pub enum ExportError {
    #[error(transparent)]
    Inspection(#[from] InspectionError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error("identity `{0}` has no results in the selected run store")]
    UnknownIdentity(String),
    #[error("invalid export evidence: {0}")]
    InvalidEvidence(&'static str),
    #[error("index.jsonl must be absent or a regular file")]
    UnsafeIndex,
    #[error("export destination must name a new directory with an existing parent")]
    InvalidDestination,
}
