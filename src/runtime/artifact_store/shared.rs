//! Portable retention catalog. The cache lock fences each immutable snapshot
//! publication through object detachment and cleanup, across all cooperating hosts.

use std::{
    cell::RefCell,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::catalog::Snapshot;
use super::{
    ArtifactStore, ArtifactStoreError, canonical, create_directory, missing, real_directory,
    regular_file, validate,
};
use crate::core::{
    canonical::CanonicalValue, materialization::hex, public_schemas::PublicSchema,
    records::ContentDigest,
};

const DIRECTORY: &str = "shared-catalog";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    sequence: u64,
    previous_sha256: Option<String>,
    snapshot: Snapshot,
}

struct State {
    next: u64,
    previous: Option<String>,
    snapshot: Snapshot,
    failed: bool,
}

pub(super) struct SharedCatalog {
    directory: PathBuf,
    state: RefCell<State>,
}

pub(super) fn is_shared(root: &Path) -> bool {
    fs::symlink_metadata(root.join(DIRECTORY)).is_ok()
}

impl ArtifactStore {
    /// Initializes a shared cache explicitly. Existing local catalogs cannot be
    /// converted. All hosts must provide coherent kernel locks and atomic rename.
    pub fn open_shared(root: &Path) -> Result<Self, ArtifactStoreError> {
        if !root.is_absolute() {
            return Err(ArtifactStoreError::InvalidRoot);
        }
        create_directory(root)?;
        crate::runtime::platform_fs::check_storage_root(root)?;
        let store = Self {
            root: fs::canonicalize(root)?,
        };
        let _guard = store.lock_file(true)?;
        reject_sqlite(&store)?;
        let directory = store.root.join(DIRECTORY);
        if missing(&directory)? {
            store.require_uninitialized()?;
        }
        for child in ["objects", "derivations", "leases", "quarantine", "garbage"] {
            create_directory(&store.root.join(child))?;
        }
        if missing(&directory)? {
            create_directory(&directory)?;
            let catalog = SharedCatalog {
                directory,
                state: RefCell::new(State {
                    next: 0,
                    previous: None,
                    snapshot: Snapshot::default(),
                    failed: false,
                }),
            };
            catalog.publish(Snapshot::default())?;
            crate::runtime::platform_fs::complete_namespace_change(&store.root)?;
        } else {
            SharedCatalog::open(&store, true)?;
        }
        Ok(store)
    }

    pub fn is_shared(&self) -> bool {
        is_shared(&self.root)
    }
}

fn reject_sqlite(store: &ArtifactStore) -> Result<(), ArtifactStoreError> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        if !missing(&store.root.join(format!("cache.sqlite3{suffix}")))? {
            return Err(ArtifactStoreError::Catalog(
                "shared caches cannot contain a SQLite catalog",
            ));
        }
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex(ContentDigest::new(Sha256::digest(bytes).into()))
}

fn filename(sequence: u64, hash: &str) -> String {
    format!("{sequence:016}-{hash}.json")
}

impl SharedCatalog {
    // Every caller holds the cache lock. Reads never settle or repair publication.
    pub fn open(store: &ArtifactStore, writable: bool) -> Result<Self, ArtifactStoreError> {
        reject_sqlite(store)?;
        let directory = store.root.join(DIRECTORY);
        real_directory(&directory)?;
        if writable {
            // Settle complete renames left by a crashed publisher before their
            // retention decisions can authorize further mutation.
            crate::runtime::platform_fs::complete_namespace_change(&store.root)?;
            crate::runtime::platform_fs::complete_namespace_change(&directory)?;
        }
        let mut paths = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".pending-"))
            {
                continue;
            }
            paths.push(entry.path());
        }
        paths.sort();
        let mut state = State {
            next: 0,
            previous: None,
            snapshot: Snapshot::default(),
            failed: false,
        };
        for path in paths {
            let mut file = regular_file(&path)?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut bytes)?;
            let document = CanonicalValue::from_slice(&bytes)?;
            validate(PublicSchema::SharedCacheRecord, &document)?;
            let record: Record = serde_json::from_value(document.as_json().clone())?;
            let hash = digest(&bytes);
            if document.to_canonical_bytes()? != bytes
                || record.sequence != state.next
                || record.previous_sha256 != state.previous
                || path.file_name().and_then(|name| name.to_str())
                    != Some(filename(state.next, &hash).as_str())
            {
                return Err(ArtifactStoreError::Catalog(
                    "invalid shared cache record chain",
                ));
            }
            validate_snapshot(&record.snapshot)?;
            state.next += 1;
            state.previous = Some(hash);
            state.snapshot = record.snapshot;
        }
        if state.next == 0 {
            return Err(ArtifactStoreError::Catalog(
                "shared cache has no initialization record",
            ));
        }
        Ok(Self {
            directory,
            state: RefCell::new(state),
        })
    }

    pub fn snapshot(&self) -> Result<Snapshot, ArtifactStoreError> {
        Ok(self.state.borrow().snapshot.clone())
    }

    pub fn update(
        &self,
        change: impl FnOnce(&mut Snapshot) -> Result<(), ArtifactStoreError>,
    ) -> Result<(), ArtifactStoreError> {
        let mut snapshot = self.state.borrow().snapshot.clone();
        change(&mut snapshot)?;
        if snapshot != self.state.borrow().snapshot {
            self.publish(snapshot)?;
        }
        Ok(())
    }

    fn publish(&self, snapshot: Snapshot) -> Result<(), ArtifactStoreError> {
        let mut state = self.state.borrow_mut();
        if state.failed {
            return Err(ArtifactStoreError::Catalog(
                "reopen cache after failed publication",
            ));
        }
        validate_snapshot(&snapshot)?;
        let document = canonical(serde_json::to_value(Record {
            schema_version: 1,
            sequence: state.next,
            previous_sha256: state.previous.clone(),
            snapshot: snapshot.clone(),
        })?)?;
        validate(PublicSchema::SharedCacheRecord, &document)?;
        let bytes = document.to_canonical_bytes()?;
        let hash = digest(&bytes);
        // Any publication error invalidates this writer, including ambiguous
        // errors after rename. A new open replays the visible complete prefix.
        state.failed = true;
        let mut stage = tempfile::Builder::new()
            .prefix(".pending-")
            .tempfile_in(&self.directory)?;
        stage.write_all(&bytes)?;
        crate::runtime::durability::sync_all(stage.as_file())?;
        crate::runtime::publication::rename_no_replace(
            stage.path(),
            &self.directory.join(filename(state.next, &hash)),
        )?;
        crate::runtime::platform_fs::complete_namespace_change(&self.directory)?;
        state.snapshot = snapshot;
        state.previous = Some(hash);
        state.next += 1;
        state.failed = false;
        Ok(())
    }
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<(), ArtifactStoreError> {
    for (sha, roots) in &snapshot.roots {
        if !snapshot.objects.contains_key(sha)
            || roots.is_empty()
            || roots.windows(2).any(|pair| pair[0] >= pair[1])
            || roots
                .iter()
                .any(|name| name.is_empty() || name.chars().any(char::is_control))
        {
            return Err(ArtifactStoreError::Catalog(
                "invalid shared cache retention roots",
            ));
        }
    }
    if snapshot
        .pending
        .keys()
        .any(|sha| !snapshot.objects.contains_key(sha))
    {
        return Err(ArtifactStoreError::Catalog(
            "unregistered shared cache collection claim",
        ));
    }
    Ok(())
}

/// This random operational owner distinguishes independent run homes without
/// binding mount points, host names, or scientific identities to retention.
pub(super) fn run_owner(output: &Path) -> Result<String, ArtifactStoreError> {
    let path = output.join(".cache-owner.json");
    if missing(&path)? {
        let mut nonce = [0; 32];
        getrandom::fill(&mut nonce).map_err(std::io::Error::other)?;
        let document = canonical(serde_json::json!({
            "schema_version": 1, "root": format!("run-v1:{}", hex(ContentDigest::new(nonce)))
        }))?;
        let mut stage = tempfile::Builder::new()
            .prefix(".pending-cache-owner-")
            .tempfile_in(output)?;
        stage.write_all(&document.to_canonical_bytes()?)?;
        crate::runtime::durability::sync_all(stage.as_file())?;
        match crate::runtime::publication::rename_no_replace(stage.path(), &path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
    }
    let mut file = regular_file(&path)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)?;
    let document = CanonicalValue::from_slice(&bytes)?;
    validate(PublicSchema::SharedCacheOwner, &document)?;
    crate::runtime::platform_fs::sync_owned_file(&path)?;
    crate::runtime::platform_fs::complete_namespace_change(output)?;
    Ok(document.as_json()["root"]
        .as_str()
        .expect("validated owner")
        .into())
}
