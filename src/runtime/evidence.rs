//! Shared committed evidence for SQLite runs and imported archive collections.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Seek, SeekFrom},
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
    run_store::reader::{SnapshotPurpose, StoreSnapshot},
};

pub(crate) fn index_rows<'a>(rows: &'a [Value], field: &str) -> BTreeMap<&'a str, Vec<&'a Value>> {
    let mut index = BTreeMap::<_, Vec<_>>::new();
    for row in rows {
        if let Some(key) = row[field].as_str() {
            index.entry(key).or_default().push(row);
        }
    }
    index
}

pub(crate) struct EvidenceSnapshot {
    pub root: PathBuf,
    pub data_root: PathBuf,
    pub tables: BTreeMap<&'static str, Vec<Value>>,
    pub files: Option<BTreeMap<String, ArchiveFile>>,
}

impl EvidenceSnapshot {
    pub fn read(root: &Path) -> Result<Self, InspectionError> {
        Self::read_for(root, SnapshotPurpose::Archive)
    }

    pub fn read_for(root: &Path, purpose: SnapshotPurpose) -> Result<Self, InspectionError> {
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
            let snapshot = match purpose {
                SnapshotPurpose::Archive => StoreSnapshot::read(root)?,
                _ => StoreSnapshot::read_for(root, purpose)?,
            };
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
    let Some(path) = buffered_trace_path(data_root, profile)? else {
        return Ok(());
    };
    let mut reader = TraceReader::open(&path, attempt_id, TracePosition::default())?;
    let mut index = 0;
    while let Some((_, point)) = reader.next_point()? {
        visit(index, point);
        index += 1;
    }
    Ok(())
}

pub(crate) fn buffered_trace_path(
    data_root: &Path,
    profile: &Value,
) -> Result<Option<PathBuf>, InspectionError> {
    let buffer = &profile["buffer"];
    let (Some(directory), Some(trace)) = (
        buffer["artifact"]["directory"].as_str(),
        buffer["manifest"]["trace"].as_str(),
    ) else {
        return Ok(None);
    };
    let directory = safe_profile_path(data_root, directory)?;
    safe_profile_path(&directory, trace).map(Some)
}

#[derive(Clone, Copy, Default)]
pub(crate) struct TracePosition {
    offset: u64,
    lines_read: usize,
}

pub(crate) struct TraceReader<'a> {
    path: &'a Path,
    attempt_id: &'a str,
    reader: BufReader<File>,
    line: Vec<u8>,
    position: TracePosition,
}

impl<'a> TraceReader<'a> {
    pub(crate) fn open(
        path: &'a Path,
        attempt_id: &'a str,
        position: TracePosition,
    ) -> Result<Self, InspectionError> {
        let mut file = platform_fs::open_regular(path, OpenOptions::new().read(true))?;
        if position.offset != 0 {
            file.seek(SeekFrom::Start(position.offset))?;
        }
        Ok(Self {
            path,
            attempt_id,
            reader: BufReader::new(file),
            line: Vec::new(),
            position,
        })
    }

    pub(crate) fn next_point(
        &mut self,
    ) -> Result<Option<(TracePosition, TracePoint)>, InspectionError> {
        loop {
            self.line.clear();
            let bytes_read = self.reader.read_until(b'\n', &mut self.line)?;
            if bytes_read == 0 {
                return Ok(None);
            }
            let start = self.position;
            self.position.offset += bytes_read as u64;
            self.position.lines_read += 1;
            let row = self.line.strip_suffix(b"\n").unwrap_or(&self.line);
            if row.is_empty() {
                continue;
            }
            let invalid_row = |reason: &dyn std::fmt::Display| InspectionError::InvalidTrace {
                attempt_id: self.attempt_id.to_owned(),
                path: self.path.to_owned(),
                line: self.position.lines_read,
                reason: reason.to_string(),
            };
            let value = CanonicalValue::from_slice(row).map_err(|error| invalid_row(&error))?;
            let point =
                TracePoint::deserialize(value.as_json()).map_err(|error| invalid_row(&error))?;
            return Ok(Some((start, point)));
        }
    }
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
