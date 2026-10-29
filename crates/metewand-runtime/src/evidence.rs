//! Shared committed evidence for SQLite runs and imported archive collections.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use metewand_core::{
    archive::{ArchiveEvidence, ArchiveFile, ArchiveRecord, RESULT_ARCHIVE_VERSION},
    records::ContentDigest,
};
use serde_json::{Value, json};

use crate::{
    inspection::InspectionError,
    local_tree::{LocalTreeHashError, hash_local_tree, hash_tree},
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
        if crate::archive::is_collection(root) {
            crate::archive::collection_snapshot(
                root,
                crate::archive::ArchiveLimits {
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
