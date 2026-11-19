//! Portable retention catalog. The cache lock fences each immutable record
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
struct RecordV1 {
    schema_version: u32,
    sequence: u64,
    previous_sha256: Option<String>,
    snapshot: Snapshot,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordV2 {
    schema_version: u32,
    sequence: u64,
    previous_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<Delta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint: Option<Snapshot>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Delta {
    Register {
        digest: String,
        marker: String,
    },
    Root {
        digest: String,
        name: String,
        add: bool,
    },
    Pending {
        digest: String,
        detached: bool,
    },
    Forget {
        digest: String,
    },
}

impl Delta {
    fn apply(&self, snapshot: &mut Snapshot) -> Result<bool, ArtifactStoreError> {
        match self {
            Self::Register { digest, marker } => {
                if let Some(expected) = snapshot.objects.get(digest) {
                    if expected != marker {
                        return Err(ArtifactStoreError::ChangedContent);
                    }
                    return Ok(false);
                }
                snapshot.objects.insert(digest.clone(), marker.clone());
            }
            Self::Root { digest, name, add } => {
                if !snapshot.objects.contains_key(digest) {
                    return Err(ArtifactStoreError::UnknownTarget(digest.clone()));
                }
                if name.is_empty() || name.chars().any(char::is_control) {
                    return Err(ArtifactStoreError::Catalog(
                        "invalid shared cache retention root",
                    ));
                }
                if *add {
                    let roots = snapshot.roots.entry(digest.clone()).or_default();
                    match roots.binary_search(name) {
                        Ok(_) => return Ok(false),
                        Err(index) => roots.insert(index, name.clone()),
                    }
                } else {
                    let Some(roots) = snapshot.roots.get_mut(digest) else {
                        return Err(ArtifactStoreError::UnknownRoot(name.clone()));
                    };
                    let index = roots
                        .binary_search(name)
                        .map_err(|_| ArtifactStoreError::UnknownRoot(name.clone()))?;
                    roots.remove(index);
                    if roots.is_empty() {
                        snapshot.roots.remove(digest);
                    }
                }
            }
            Self::Pending { digest, detached } => {
                if !snapshot.objects.contains_key(digest) {
                    return Err(ArtifactStoreError::Catalog(
                        "unregistered shared cache collection claim",
                    ));
                }
                return Ok(snapshot.pending.insert(digest.clone(), *detached) != Some(*detached));
            }
            Self::Forget { digest } => {
                if snapshot.roots.contains_key(digest) {
                    return Err(ArtifactStoreError::Catalog("cannot forget rooted content"));
                }
                snapshot.pending.remove(digest);
                return Ok(snapshot.objects.remove(digest).is_some());
            }
        }
        Ok(true)
    }
}

impl State {
    fn replay(
        &mut self,
        document: &CanonicalValue,
        hash: &str,
        path: &Path,
    ) -> Result<(), ArtifactStoreError> {
        let value = document.as_json();
        if value["sequence"].as_u64() != Some(self.next)
            || value["previous_sha256"].as_str() != self.previous.as_deref()
            || path.file_name().and_then(|name| name.to_str())
                != Some(filename(self.next, hash).as_str())
        {
            return Err(ArtifactStoreError::Catalog(
                "invalid shared cache record chain",
            ));
        }
        match value["schema_version"].as_u64() {
            Some(1) if !self.version_two => {
                validate(PublicSchema::SharedCacheRecord, document)?;
                let record: RecordV1 = serde_json::from_value(value.clone())?;
                validate_snapshot(&record.snapshot)?;
                self.snapshot = record.snapshot;
            }
            Some(2) => {
                validate(PublicSchema::SharedCacheRecordV2, document)?;
                let record: RecordV2 = serde_json::from_value(value.clone())?;
                if let Some(delta) = record.delta
                    && !delta.apply(&mut self.snapshot)?
                {
                    return Err(ArtifactStoreError::Catalog(
                        "shared cache delta has no effect",
                    ));
                }
                if let Some(checkpoint) = record.checkpoint
                    && checkpoint != self.snapshot
                {
                    return Err(ArtifactStoreError::Catalog(
                        "shared cache checkpoint disagrees with replay",
                    ));
                }
                self.version_two = true;
            }
            _ => {
                return Err(ArtifactStoreError::Catalog(
                    "unsupported shared cache record version",
                ));
            }
        }
        self.next += 1;
        self.previous = Some(hash.to_owned());
        Ok(())
    }
}

struct State {
    next: u64,
    previous: Option<String>,
    snapshot: Snapshot,
    failed: bool,
    version_two: bool,
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
                    version_two: false,
                }),
            };
            catalog.publish(&mut catalog.state.borrow_mut(), None)?;
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
            version_two: false,
        };
        for path in paths {
            let mut file = regular_file(&path)?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut bytes)?;
            let document = CanonicalValue::from_slice(&bytes)?;
            if document.to_canonical_bytes()? != bytes {
                return Err(ArtifactStoreError::Catalog(
                    "noncanonical shared cache record",
                ));
            }
            state.replay(&document, &digest(&bytes), &path)?;
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
        let state = self.state.borrow();
        require_healthy(&state)?;
        Ok(state.snapshot.clone())
    }

    pub fn update(&self, delta: Delta) -> Result<(), ArtifactStoreError> {
        let mut state = self.state.borrow_mut();
        require_healthy(&state)?;
        if delta.apply(&mut state.snapshot)? {
            self.publish(&mut state, Some(delta))?;
        }
        Ok(())
    }

    fn publish(&self, state: &mut State, delta: Option<Delta>) -> Result<(), ArtifactStoreError> {
        // The in-memory change may already be applied. After any failure,
        // reopen and replay the visible complete prefix before trusting it.
        state.failed = true;
        let checkpoint =
            (state.next == 0 || state.next.is_power_of_two()).then(|| state.snapshot.clone());
        let document = canonical(serde_json::to_value(RecordV2 {
            schema_version: 2,
            sequence: state.next,
            previous_sha256: state.previous.clone(),
            delta,
            checkpoint,
        })?)?;
        validate(PublicSchema::SharedCacheRecordV2, &document)?;
        let bytes = document.to_canonical_bytes()?;
        let hash = digest(&bytes);
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
        state.previous = Some(hash);
        state.next += 1;
        state.failed = false;
        Ok(())
    }
}

fn require_healthy(state: &State) -> Result<(), ArtifactStoreError> {
    if state.failed {
        return Err(ArtifactStoreError::Catalog(
            "reopen cache after failed publication",
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests;
