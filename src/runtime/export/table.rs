use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::core::export::{ExportFormat, TidyRow, csv_cell};

use super::{ExportError, canonical_bytes, hex};

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
    "primary_time_ns",
    "primary_time_status",
    "environment_class",
    "control_class",
    "control_evidence",
    "metric_name",
    "metric_value",
    "metric_unit",
    "metric_direction",
    "instrumented_time_ns",
    "active_time_ns",
    "elapsed_wall_time_ns",
    "checkpoint_sequence",
    "instrumented_total_time_ns",
    "provisional_metric_name",
    "provisional_metric_value",
    "profile",
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

pub(super) struct Table {
    writer: BufWriter<File>,
    digest: Sha256,
    format: ExportFormat,
}

impl Table {
    pub(super) fn create(path: &Path, format: ExportFormat) -> Result<Self, ExportError> {
        let mut table = Self {
            writer: BufWriter::new(File::create(path)?),
            digest: Sha256::new(),
            format,
        };
        if format == ExportFormat::Csv {
            table.write(COLUMNS.join(",").as_bytes())?;
            table.write(b"\r\n")?;
        }
        Ok(table)
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> Result<(), ExportError> {
        self.writer.write_all(bytes)?;
        self.digest.update(bytes);
        Ok(())
    }

    pub(super) fn write_row(&mut self, row: &TidyRow<'_>) -> Result<(), ExportError> {
        if self.format == ExportFormat::Csv {
            self.write(csv_row(row)?.as_bytes())?;
            self.write(b"\r\n")
        } else {
            self.write(&canonical_bytes(row)?)?;
            self.write(b"\n")
        }
    }

    pub(super) fn finish(mut self) -> Result<String, ExportError> {
        self.writer.flush()?;
        Ok(hex(&self.digest.finalize()))
    }
}

pub(super) fn write_line(
    mut writer: impl Write,
    value: &impl Serialize,
) -> Result<(), ExportError> {
    writer.write_all(&canonical_bytes(value)?)?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn csv_row(row: &TidyRow<'_>) -> Result<String, ExportError> {
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
    Ok(cells.join(","))
}
