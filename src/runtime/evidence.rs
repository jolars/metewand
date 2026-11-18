//! Shared committed evidence for SQLite runs and imported archive collections.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use crate::core::{
    archive::{ArchiveEvidence, ArchiveFile, ArchiveRecord, RESULT_ARCHIVE_VERSION},
    canonical::CanonicalValue,
    manifest::RepositoryPath,
    profile::TracePoint,
    records::ContentDigest,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runtime::{
    inspection::InspectionError,
    local_tree::{LocalTreeHashError, hash_local_tree, hash_tree},
    platform_fs,
    run_store::reader::StoreSnapshot,
};

pub(crate) struct EvidenceSnapshot {
    pub root: PathBuf,
    pub data_root: PathBuf,
    pub tables: BTreeMap<&'static str, Vec<Value>>,
    pub files: Option<BTreeMap<String, ArchiveFile>>,
}

impl EvidenceSnapshot {
    pub fn read(root: &Path) -> Result<Self, InspectionError> {
        if crate::runtime::archive::is_collection(root) {
            crate::runtime::archive::collection_snapshot(
                root,
                crate::runtime::archive::ArchiveLimits {
                    max_entries: u64::MAX,
                    max_expanded_bytes: u64::MAX,
                },
            )
            .map_err(|error| InspectionError::Archive(error.to_string()))
        } else {
            let snapshot = StoreSnapshot::read(root)?;
            Ok(Self {
                data_root: snapshot.root.clone(),
                root: snapshot.root,
                tables: snapshot.tables,
                files: None,
            })
        }
    }

    pub fn archived(root: PathBuf, evidence: ArchiveEvidence, files: Vec<ArchiveFile>) -> Self {
        let mut tables = BTreeMap::new();
        // Empty categories remain present so existing projections have the same inputs.
        for kind in [
            "applicability",
            "resolved-specification",
            "resolved-attempt-slot",
            "resolved-observation-slot",
            "slot-membership",
            "attempt",
            "attempt-transition",
            "observation",
            "observation-selection",
            "stored-artifact",
            "attempt-artifact",
            "observation-artifact",
            "event",
        ] {
            tables.insert(kind, Vec::new());
        }
        for record in evidence.records {
            tables
                .entry(record.kind())
                .or_default()
                .push(record.value());
        }
        // Version-3 stores include empty fresh categories even for streaming runs.
        if tables.keys().any(|kind| kind.starts_with("fresh-")) {
            for kind in [
                "fresh-sequence",
                "fresh-policy",
                "fresh-member",
                "fresh-budget",
                "fresh-disposition",
            ] {
                tables.entry(kind).or_default();
            }
        }
        Self {
            data_root: root.join("data"),
            root,
            tables,
            files: Some(
                files
                    .into_iter()
                    .map(|file| (file.path.clone(), file))
                    .collect(),
            ),
        }
    }

    pub fn archive_evidence(&self) -> Result<ArchiveEvidence, serde_json::Error> {
        let records = self
            .tables
            .iter()
            .flat_map(|(kind, rows)| rows.iter().map(move |row| (kind, row)))
            .map(|(kind, row)| {
                serde_json::from_value::<ArchiveRecord>(json!({"kind": kind, "record": row}))
            })
            .collect::<Result<_, _>>()?;
        Ok(ArchiveEvidence {
            schema_version: RESULT_ARCHIVE_VERSION,
            records,
        })
    }

    pub fn tree_hash(&self, relative: &str) -> Result<ContentDigest, LocalTreeHashError> {
        let path = self.data_root.join(relative);
        if let Some(files) = &self.files {
            hash_tree(&path, &|name, _| {
                files
                    .get(&format!("data/{relative}/{name}"))
                    .is_some_and(|file| file.executable)
            })
        } else {
            hash_local_tree(path)
        }
    }

    pub fn reported_path(&self, relative: &str) -> String {
        if self.files.is_some() {
            format!("data/{relative}")
        } else {
            relative.to_owned()
        }
    }
}

pub(crate) fn validate_observation_document(row: &Value) -> Result<(), InspectionError> {
    let document = &row["document"];
    if document["id"] != row["id"]
        || document["slot_id"] != row["slot_id"]
        || document["attempt_id"] != row["attempt_id"]
        || document["validity"]["valid"] != row["valid"]
    {
        return Err(InspectionError::Invalid(
            "observation document disagrees with committed coordination",
        ));
    }
    Ok(())
}

pub(crate) fn document_profile(document: &Value) -> Option<&Value> {
    document
        .get("profile")
        .or_else(|| document["provenance"]["data"].get("profile"))
}

/// Visits decoded points in file order, retaining only the current trace row.
pub(crate) fn visit_buffered_trace(
    data_root: &Path,
    attempt_id: &str,
    profile: &Value,
    mut visit: impl FnMut(usize, TracePoint),
) -> Result<(), InspectionError> {
    let buffer = &profile["buffer"];
    let (Some(directory), Some(trace)) = (
        buffer["artifact"]["directory"].as_str(),
        buffer["manifest"]["trace"].as_str(),
    ) else {
        return Ok(());
    };
    let directory = safe_profile_path(data_root, directory)?;
    let path = safe_profile_path(&directory, trace)?;
    let file = platform_fs::open_regular(&path, OpenOptions::new().read(true))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut index = 0;
    for line_number in 1.. {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let row = line.strip_suffix(b"\n").unwrap_or(&line);
        if row.is_empty() {
            continue;
        }
        let invalid_row = |reason: &dyn std::fmt::Display| InspectionError::InvalidTrace {
            attempt_id: attempt_id.to_owned(),
            path: path.clone(),
            line: line_number,
            reason: reason.to_string(),
        };
        let value = CanonicalValue::from_slice(row).map_err(|error| invalid_row(&error))?;
        let point =
            TracePoint::deserialize(value.as_json()).map_err(|error| invalid_row(&error))?;
        visit(index, point);
        index += 1;
    }
    Ok(())
}

fn safe_profile_path(root: &Path, relative: &str) -> Result<PathBuf, InspectionError> {
    let path: RepositoryPath = serde_json::from_value(json!(relative))
        .map_err(|_| InspectionError::Invalid("unsafe buffered artifact path"))?;
    let target = root.join(path.as_path());
    if !fs::canonicalize(&target)?.starts_with(fs::canonicalize(root)?) {
        return Err(InspectionError::Invalid("buffer path escapes its artifact"));
    }
    Ok(target)
}
