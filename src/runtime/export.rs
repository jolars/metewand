//! Rebuildable result indexes and standalone tables from verified store snapshots.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    core::{
        canonical::{CanonicalJsonError, CanonicalValue},
        export::{
            EXPORT_SCHEMA_VERSION, ExportFormat, ExportManifest, ExportRecord, ExportRecordType,
            ExportReport, analysis::AnalysisSnapshot, metric_row_count,
        },
    },
    runtime::{
        evidence::{EvidenceSnapshot, document_profile, validate_observation_document},
        inspection::{Inspection, InspectionError, calibration_slots},
    },
};

mod analysis;
mod record;
mod table;
mod trace;

use record::Record;
use trace::Trace;

/// A deterministic projection of one committed snapshot and its verified artifacts.
pub struct ResultSet {
    snapshot: EvidenceSnapshot,
    inspection: Inspection,
    pilot_slots: BTreeSet<String>,
    traces: BTreeMap<String, Trace>,
}

impl ResultSet {
    /// Reads authoritative evidence directly, ignoring any existing index.
    pub fn read(root: &Path) -> Result<Self, ExportError> {
        let snapshot = EvidenceSnapshot::read(root)?;
        let mut inspection = Inspection::default();
        inspection.add_snapshot(&snapshot)?;
        let pilot_slots = calibration_slots(&snapshot.tables);
        let mut traces = BTreeMap::new();
        for source in inspection.records().values() {
            let row = &source.record;
            match source.kind.as_str() {
                "observation" => validate_observation_document(row)?,
                "attempt"
                    if !row["slot_id"]
                        .as_str()
                        .is_some_and(|id| pilot_slots.contains(id)) =>
                {
                    if let Some(profile) = document_profile(&row["document"])
                        && let Some(trace) = Trace::read(&snapshot.data_root, &source.id, profile)?
                    {
                        traces.insert(source.id.clone(), trace);
                    }
                }
                _ => (),
            }
        }
        Ok(Self {
            snapshot,
            inspection,
            pilot_slots,
            traces,
        })
    }

    fn records(&self) -> Result<Vec<Record<'_>>, ExportError> {
        let graph = self.inspection.records();
        let mut records = Vec::new();
        for source in graph.values().filter(|record| {
            matches!(
                record.kind.as_str(),
                "applicability" | "attempt" | "observation"
            ) || (record.kind == "resolved-attempt-slot"
                && record.record["fresh_sequence"]["disposition"].is_string())
        }) {
            let record = Record::new(source, graph)?;
            if !record
                .slot_id
                .is_some_and(|id| self.pilot_slots.contains(id))
            {
                records.push(record);
            }
        }
        records.sort_by(|a, b| a.order_key().cmp(&b.order_key()));
        Ok(records)
    }

    /// Selects exact identities and retains the producing attempts and applicability.
    fn select<'a>(
        &self,
        records: &'a [Record<'a>],
        ids: &[String],
    ) -> Result<Vec<&'a Record<'a>>, ExportError> {
        if ids.is_empty() {
            return Ok(records.iter().collect());
        }
        let mut selected = BTreeSet::new();
        let mut attempts = BTreeSet::new();
        let mut candidates = BTreeSet::new();
        for id in ids {
            // A point selector is equivalent to selecting its producing attempt.
            // The suffix must be the exact ordinal used in the v1 trace ID.
            let trace_attempt = id.rsplit_once(":check:").and_then(|(attempt, suffix)| {
                let trace = self.traces.get(attempt)?;
                let index = suffix.parse::<usize>().ok()?;
                (index < trace.len && suffix == index.to_string()).then_some(attempt)
            });
            let mut matches = records
                .iter()
                .filter(|record| record.matches(id) || Some(record.id) == trace_attempt)
                .peekable();
            if matches.peek().is_none() {
                return Err(ExportError::UnknownIdentity(id.clone()));
            }
            for record in matches {
                selected.insert(record.id);
                attempts.extend(record.attempt_id);
                candidates.insert(record.candidate_id);
            }
        }
        Ok(records
            .iter()
            .filter(|record| {
                selected.contains(record.id)
                    || record.attempt_id.is_some_and(|id| attempts.contains(id))
                    || (record.kind == ExportRecordType::Applicability
                        && candidates.contains(record.candidate_id))
            })
            .collect())
    }

    /// Writes a new directory with one table and its adjacent lineage manifest.
    /// Existing destinations, including symlinks, are never replaced.
    pub fn export_checked(
        &self,
        destination: Option<&Path>,
        format: ExportFormat,
        ids: &[String],
        rebuild_index: bool,
        require_complete: bool,
    ) -> Result<ExportReport, ExportError> {
        let records = self.records()?;
        let selected = self.select(&records, ids)?;
        let analysis = if format == ExportFormat::Json || require_complete {
            let analysis = analysis::project(&records, &self.inspection, &self.snapshot.tables)?;
            Some(if ids.is_empty() {
                analysis
            } else {
                analysis::select(analysis, &selected)
            })
        } else {
            None
        };
        if require_complete
            && let Some(analysis) = &analysis
            && !analysis.completeness.complete
        {
            let messages = analysis
                .completeness
                .issues
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<BTreeSet<_>>();
            return Err(ExportError::IncompleteResults(
                messages.into_iter().collect::<Vec<_>>().join(" "),
            ));
        }
        if rebuild_index {
            self.index_target()?;
        }
        let mut manifest = lineage(&selected, ids);
        let analysis = analysis.as_ref().filter(|_| format == ExportFormat::Json);
        let rows = match analysis {
            Some(analysis) => analysis.observations.len(),
            None => selected
                .iter()
                .map(|record| {
                    metric_row_count(record.metric_values())
                        + self.traces.get(record.id).map_or(0, |trace| trace.rows)
                })
                .sum(),
        };
        let destination = destination
            .map(|path| self.write_destination(path, format, &selected, analysis, &mut manifest))
            .transpose()?;
        let index_path = rebuild_index
            .then(|| self.write_index(&records))
            .transpose()?;
        Ok(ExportReport {
            command: "export",
            schema_version: EXPORT_SCHEMA_VERSION,
            records: selected.len()
                + selected
                    .iter()
                    .filter_map(|record| self.traces.get(record.id))
                    .map(|trace| trace.len)
                    .sum::<usize>(),
            rows,
            destination: destination.map(|path| path.to_string_lossy().into_owned()),
            index_path: index_path.map(|path| path.to_string_lossy().into_owned()),
            manifest,
        })
    }

    fn write_destination(
        &self,
        destination: &Path,
        format: ExportFormat,
        records: &[&Record<'_>],
        analysis: Option<&AnalysisSnapshot>,
        manifest: &mut ExportManifest,
    ) -> Result<PathBuf, ExportError> {
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
        if self.snapshot.files.is_some() && target.starts_with(&self.snapshot.root) {
            return Err(ExportError::ReadOnlyCollection);
        }
        let stage = tempfile::Builder::new()
            .prefix(".metewand-export-")
            .tempdir_in(&parent)?;
        let mut output = table::Table::create(&stage.path().join(format.filename()), format)?;
        if let Some(analysis) = analysis {
            output.write(&canonical_bytes(analysis)?)?;
        } else {
            self.visit_records(records.iter().copied(), |record| {
                record.visit_tidy_rows(|row| output.write_row(&row))
            })?;
        }
        manifest
            .files
            .insert(format.filename().into(), output.finish()?);
        fs::write(stage.path().join("export.json"), canonical_bytes(manifest)?)?;
        crate::runtime::publication::rename_no_replace(stage.path(), &target)?;
        Ok(target)
    }

    fn index_target(&self) -> Result<PathBuf, ExportError> {
        if self.snapshot.files.is_some() {
            return Err(ExportError::ReadOnlyCollection);
        }
        if crate::runtime::run_store::shared::is_shared(&self.snapshot.root) {
            return Err(ExportError::SharedIndex);
        }
        let target = self.snapshot.root.join("index.jsonl");
        match fs::symlink_metadata(&target) {
            Ok(metadata) if !metadata.is_file() => return Err(ExportError::UnsafeIndex),
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        Ok(target)
    }

    /// Atomically replaces only the explicitly requested, nonauthoritative index.
    fn write_index(&self, records: &[Record<'_>]) -> Result<PathBuf, ExportError> {
        let target = self.index_target()?;
        let mut stage = tempfile::NamedTempFile::new_in(&self.snapshot.root)?;
        {
            let mut writer = BufWriter::new(stage.as_file_mut());
            self.visit_records(records, |record| table::write_line(&mut writer, record))?;
            writer.flush()?;
        }
        stage.persist(&target).map_err(|error| error.error)?;
        Ok(target)
    }

    fn visit_records<'a>(
        &self,
        records: impl IntoIterator<Item = &'a Record<'a>>,
        mut visit: impl FnMut(&mut ExportRecord) -> Result<(), ExportError>,
    ) -> Result<(), ExportError> {
        for source in records {
            let mut record = source.index_record();
            visit(&mut record)?;
            if let Some(trace) = self.traces.get(source.id) {
                // Trace rows share the attempt's sort key up to the ID, so
                // their :check: suffixes place them directly after the attempt.
                let profile = record.profile.take().expect("registered trace profile");
                let manifest = &profile["buffer"]["manifest"];
                record.record_type = ExportRecordType::Trace;
                record.metrics = Value::Null;
                record.valid = None;
                record.selected = None;
                record.completion = None;
                trace.visit(source.id, |index, point| {
                    record.id = format!("{}:check:{index}", source.id);
                    record.profile = Some(json!({
                        "delivery": "buffered_profile",
                        "measurement_basis": "instrumented",
                        "instrumented_scope": manifest["instrumented_scope"],
                        "clock": manifest["clock"],
                        "instrumented_time_ns": point.instrumented_time_ns,
                        "instrumented_total_time_ns": manifest["instrumented_total_time_ns"],
                        "trace_index": index,
                        "check_index": point.check_index,
                        "work": point.work,
                        "terminal": point.terminal,
                        "provisional_metrics": point.provisional_metrics,
                        "independently_validated": false,
                    }));
                    visit(&mut record)
                })?;
            }
        }
        Ok(())
    }
}

fn lineage(records: &[&Record<'_>], ids: &[String]) -> ExportManifest {
    fn ordered<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
        values
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
    ExportManifest {
        schema_version: EXPORT_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        selector: ordered(ids.iter().map(String::as_str)),
        candidate_ids: ordered(records.iter().map(|r| r.candidate_id)),
        attempt_ids: records
            .iter()
            .filter(|r| r.kind == ExportRecordType::Attempt)
            .map(|r| r.id.to_owned())
            .collect(),
        observation_ids: records
            .iter()
            .filter(|r| r.kind == ExportRecordType::Observation)
            .map(|r| r.id.to_owned())
            .collect(),
        result_schemas: ordered(records.iter().filter_map(|r| r.result_schema())),
        metric_schemas: ordered(records.iter().filter_map(|r| r.metric_schema())),
        compatibility_classes: records
            .iter()
            .map(|r| (r.compatibility_id.clone(), r.compatibility.clone()))
            .collect(),
        files: BTreeMap::new(),
    }
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ExportError> {
    Ok(CanonicalValue::try_from(serde_json::to_value(value)?)?.to_canonical_bytes()?)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("the selected comparison is incomplete: {0}")]
    IncompleteResults(String),
    #[error(
        "archive collections are read-only; export to a new destination without --rebuild-index"
    )]
    ReadOnlyCollection,
    #[error(
        "shared runs use disposable local views; export to a new destination without --rebuild-index"
    )]
    SharedIndex,
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
