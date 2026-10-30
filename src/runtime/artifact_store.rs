//! Durable content-addressed trees, construction mappings, and leased copies.

mod catalog;
mod maintenance;

pub use catalog::CACHE_CATALOG_VERSION;

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read},
    path::{Path, PathBuf},
};

use crate::core::{
    canonical::{CanonicalJsonError, CanonicalValue},
    identity::{IdentityError, canonical_identity_bytes, record_id},
    manifest::RepositoryPath,
    materialization::{DatasetDerivationRecord, hex},
    public_schemas::{PublicSchema, PublicSchemaCatalogError, public_schema_catalog},
    records::{ContentDigest, IdentifiedRecord},
    schema::SchemaValidationError,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};
use thiserror::Error;

use crate::runtime::{
    LOCAL_TREE_HASH_VERSION,
    dataset_materialization::MaterializedDataset,
    local_tree::{LocalTreeHashError, hash_local_tree},
    publication::{
        PublicationError, publish, rename_no_replace, sync_directories, verify_finalized_directory,
        write_file,
    },
};

/// Compatibility version of cache objects, mappings, and lease receipts.
pub const ARTIFACT_STORE_VERSION: u32 = 1;

/// An explicitly selected Metewand-owned cache directory.
#[derive(Clone, Debug)]
pub struct ArtifactStore {
    root: PathBuf,
}

/// An object protected from reclamation for the lifetime of this handle.
#[derive(Debug)]
pub struct StoredArtifact {
    store: ArtifactStore,
    digest: ContentDigest,
    directory: PathBuf,
    _lease: Lease,
}

/// A dataset construction recovered through its verified derivation mapping.
#[derive(Debug)]
pub struct CachedDataset {
    pub artifact: StoredArtifact,
    pub manifest: RepositoryPath,
    pub provenance: CanonicalValue,
}

/// An independent per-attempt copy; dropping it removes the private directory.
/// Verification also checks its canonical object, including quarantine on failure.
#[derive(Debug)]
pub struct DatasetView {
    directory: TempDir,
    digest: ContentDigest,
    _lease: Lease,
}

/// A process-held lease. Kernel locks, rather than PID or wall-clock guesses,
/// establish whether the owner is still active.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheLeaseRecord {
    pub schema_version: u32,
    pub purpose: String,
    pub owner_pid: u32,
    pub tree_sha256: String,
}

#[derive(Debug)]
struct Lease {
    directory: Option<TempDir>,
    lock: Option<File>,
    store: ArtifactStore,
}

impl Lease {
    fn directory(&self) -> &Path {
        self.directory
            .as_ref()
            .expect("live lease directory")
            .path()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Serialize removal with lease snapshots and abandoned-owner recovery.
        let _guard = self.store.lock();
        // Closing the handle first lets Windows remove the entire directory.
        drop(self.lock.take());
        drop(self.directory.take());
    }
}

impl ArtifactStore {
    /// Creates or opens a cache beneath an existing parent. Symlink roots and
    /// symlink-owned subdirectories are rejected. No provider directory should
    /// be passed here: the caller grants Metewand ownership of this directory.
    pub fn open(root: &Path) -> Result<Self, ArtifactStoreError> {
        if !root.is_absolute() {
            return Err(ArtifactStoreError::InvalidRoot);
        }
        create_directory(root)?;
        crate::runtime::platform_fs::check_storage_root(root)?;
        #[cfg(windows)]
        crate::runtime::platform_fs::set_private_directory(root)?;
        let root = fs::canonicalize(root)?;
        for child in ["objects", "derivations", "leases", "quarantine", "garbage"] {
            create_directory(&root.join(child))?;
        }
        let store = Self { root };
        let _guard = store.lock_file(true)?;
        catalog::Catalog::initialize(&store)?;
        Ok(store)
    }

    /// Opens an initialized cache without creating directories, catalogs, locks,
    /// or SQLite sidecars. Maintenance reads use a private committed snapshot.
    pub fn open_existing(root: &Path) -> Result<Self, ArtifactStoreError> {
        if !root.is_absolute() {
            return Err(ArtifactStoreError::InvalidRoot);
        }
        real_directory(root)?;
        let store = Self {
            root: fs::canonicalize(root)?,
        };
        for child in ["objects", "derivations", "leases", "quarantine", "garbage"] {
            real_directory(&store.root.join(child))?;
        }
        let _guard = store.lock()?;
        catalog::Catalog::read(&store)?;
        Ok(store)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Copies and independently verifies an export without changing its files,
    /// modes, ownership, links, or containing provider cache. Hard links are
    /// never used for storage or worker views.
    pub fn import_tree(
        &self,
        source: &Path,
        expected: ContentDigest,
    ) -> Result<StoredArtifact, ArtifactStoreError> {
        real_directory(source)?;
        if self.root.starts_with(fs::canonicalize(source)?) {
            return Err(ArtifactStoreError::OverlappingTrees);
        }
        let lease = self.lease("publication", expected)?;
        let stage = lease.directory().join("object");
        fs::create_dir(&stage)?;
        let data = stage.join("data");
        copy_verified(source, &data, expected)?;
        let entries = inventory(&data)?;
        sync_directories(&data)?;
        let marker = canonical(json!({
            "schema_version": ARTIFACT_STORE_VERSION, "complete": true,
            "tree_hash_version": LOCAL_TREE_HASH_VERSION, "tree_sha256": hex(expected),
            "entries": entries,
        }))?;
        validate(PublicSchema::ArtifactCacheObject, &marker)?;
        write_file(&stage.join("manifest.json"), &marker.to_canonical_bytes()?)?;
        crate::runtime::platform_fs::complete_namespace_change(&stage)?;
        verify_object(&stage, expected)?;
        let destination = self.object_path(expected);
        let _guard = self.lock()?;
        match rename_no_replace(&stage, &destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.verify_or_quarantine(expected)?;
                let compared = same_bytes(&data, &destination.join("data"));
                self.verify_or_quarantine(expected)?;
                if !compared? {
                    return Err(ArtifactStoreError::ChangedContent);
                }
            }
            Err(error) => return Err(error.into()),
        }
        crate::runtime::platform_fs::complete_namespace_change(lease.directory())?;
        crate::runtime::platform_fs::complete_namespace_change(&self.root.join("objects"))?;
        self.register_object(expected)?;
        Ok(StoredArtifact {
            store: self.clone(),
            digest: expected,
            directory: destination.join("data"),
            _lease: lease,
        })
    }

    /// Opens only a completed object whose full inventory and content address
    /// agree. A missing object returns `None`; an incomplete object is an error.
    pub fn get(&self, digest: ContentDigest) -> Result<Option<StoredArtifact>, ArtifactStoreError> {
        let lease = self.lease("read", digest)?;
        let path = self.object_path(digest);
        let _guard = self.lock()?;
        if missing(&path)? {
            return Ok(None);
        }
        self.verify_or_quarantine(digest)?;
        self.register_object(digest)?;
        Ok(Some(StoredArtifact {
            store: self.clone(),
            digest,
            directory: path.join("data"),
            _lease: lease,
        }))
    }

    /// Imports a resolved provider or repository export through an independently
    /// verified copy, retaining ownership of the resolver and provider storage.
    pub fn import_source(
        &self,
        source: &crate::runtime::source_resolution::ResolvedSource,
    ) -> Result<StoredArtifact, ArtifactStoreError> {
        source.verify()?;
        let artifact = self.import_tree(source.directory(), source.tree_digest())?;
        source.verify()?;
        Ok(artifact)
    }

    /// Publishes the output first, then atomically binds the construction to
    /// that output. A recipe already bound to different bytes always fails.
    pub fn publish_dataset(
        &self,
        dataset: &MaterializedDataset,
    ) -> Result<StoredArtifact, ArtifactStoreError> {
        let artifact = self.import_tree(dataset.directory(), dataset.output_digest)?;
        let lease = self.lease("mapping", artifact.digest)?;
        let stage = Builder::new()
            .prefix("mapping-")
            .tempdir_in(lease.directory())?;
        let document = canonical(json!({
            "schema_version": ARTIFACT_STORE_VERSION,
            "manifest": dataset.manifest.manifest_path(),
            "receipt": dataset.provenance.as_json(),
        }))?;
        validate(PublicSchema::DatasetCacheEntry, &document)?;
        if verify_receipt(&document, &dataset.derivation)? != artifact.digest {
            return Err(ArtifactStoreError::InvalidMapping);
        }
        let destination = self
            .root
            .join("derivations")
            .join(dataset.derivation.id.to_string());
        // Serialize mapping claims so identical outputs may retain the first
        // receipt even when operational acquisition evidence differs.
        let _guard = self.lock()?;
        self.verify_or_quarantine(artifact.digest)?;
        if !missing(&destination)? {
            verify_finalized_directory(&destination)?;
            let existing =
                CanonicalValue::from_slice(&fs::read(destination.join("dataset.json"))?)?;
            validate(PublicSchema::DatasetCacheEntry, &existing)?;
            if verify_receipt(&existing, &dataset.derivation)? != artifact.digest {
                return Err(ArtifactStoreError::DerivationConflict);
            }
            return Ok(artifact);
        }
        write_file(
            &stage.path().join("dataset.json"),
            &document.to_canonical_bytes()?,
        )?;
        publish(stage, &destination)?;
        crate::runtime::platform_fs::complete_namespace_change(lease.directory())?;
        Ok(artifact)
    }

    /// Resolves a recipe through its completion marker, exact identity
    /// transcript, construction receipt, and separately verified output.
    pub fn lookup_dataset(
        &self,
        derivation: &IdentifiedRecord<DatasetDerivationRecord>,
    ) -> Result<Option<CachedDataset>, ArtifactStoreError> {
        let path = self
            .root
            .join("derivations")
            .join(derivation.id.to_string());
        if missing(&path)? {
            return Ok(None);
        }
        verify_finalized_directory(&path)?;
        let document = CanonicalValue::from_slice(&fs::read(path.join("dataset.json"))?)?;
        validate(PublicSchema::DatasetCacheEntry, &document)?;
        let digest = verify_receipt(&document, derivation)?;
        let Some(artifact) = self.get(digest)? else {
            // Retain the recipe's expected hash while allowing verified reconstruction
            // after quarantine removed its output from the live address space.
            return Ok(None);
        };
        let manifest = serde_json::from_value(document.as_json()["manifest"].clone())?;
        Ok(Some(CachedDataset {
            artifact,
            manifest,
            provenance: canonical(document.as_json()["receipt"].clone())?,
        }))
    }

    /// Reports only leases whose owner still holds its kernel lock.
    pub fn active_leases(&self) -> Result<Vec<CacheLeaseRecord>, ArtifactStoreError> {
        let _guard = self.lock()?;
        self.active_leases_locked()
    }

    fn active_leases_locked(&self) -> Result<Vec<CacheLeaseRecord>, ArtifactStoreError> {
        let mut active: Vec<CacheLeaseRecord> = Vec::new();
        for entry in fs::read_dir(self.root.join("leases"))? {
            let entry = entry?;
            real_directory(&entry.path())?;
            let path = entry.path().join("lease.json");
            if missing(&path)? {
                continue;
            }
            let file = regular_file(&lease_lock_path(&entry.path()))?;
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    let document = CanonicalValue::from_slice(&fs::read(path)?)?;
                    validate(PublicSchema::ArtifactCacheLease, &document)?;
                    active.push(serde_json::from_value(document.as_json().clone())?);
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        active.sort_by(|a, b| {
            (&a.tree_sha256, &a.purpose, a.owner_pid).cmp(&(
                &b.tree_sha256,
                &b.purpose,
                b.owner_pid,
            ))
        });
        Ok(active)
    }

    /// Quarantines exact abandoned lease directories with their unfinished
    /// staging for diagnosis. Active owners cannot lose a lease due to a long
    /// operation or clock changes. Completed objects and mappings stay intact.
    pub fn quarantine_abandoned_leases(&self) -> Result<usize, ArtifactStoreError> {
        let _guard = self.lock()?;
        let mut removed = 0;
        for entry in fs::read_dir(self.root.join("leases"))? {
            let entry = entry?;
            real_directory(&entry.path())?;
            let path = lease_lock_path(&entry.path());
            let file = if missing(&path)? {
                None
            } else {
                Some(regular_file(&path)?)
            };
            if let Some(file) = &file {
                match file.try_lock() {
                    Ok(()) => {}
                    Err(TryLockError::WouldBlock) => continue,
                    Err(TryLockError::Error(error)) => return Err(error.into()),
                }
            }
            // The cache lock excludes new owners after the abandoned lease is
            // identified. Close its handle so Windows can move the directory.
            #[cfg(windows)]
            drop(file);
            rename_no_replace(
                &entry.path(),
                &self.root.join("quarantine").join(entry.file_name()),
            )?;
            removed += 1;
        }
        crate::runtime::platform_fs::complete_namespace_change(&self.root.join("leases"))?;
        crate::runtime::platform_fs::complete_namespace_change(&self.root.join("quarantine"))?;
        Ok(removed)
    }

    fn object_path(&self, digest: ContentDigest) -> PathBuf {
        self.root
            .join("objects")
            .join(format!("tree-v{LOCAL_TREE_HASH_VERSION}-{}", hex(digest)))
    }

    // Callers hold the store lock through verification, quarantine, and any copy
    // into a worker view, so other readers cannot consume a known-bad address.
    fn verify_or_quarantine(&self, digest: ContentDigest) -> Result<(), ArtifactStoreError> {
        let object = self.object_path(digest);
        if missing(&object)? {
            return Err(ArtifactStoreError::ChangedContent);
        }
        if let Err(source) = verify_object(&object, digest) {
            let quarantine = Builder::new()
                .prefix(&format!("tree-v{LOCAL_TREE_HASH_VERSION}-{}-", hex(digest)))
                .tempdir_in(self.root.join("quarantine"))?;
            rename_no_replace(&object, &quarantine.path().join("object"))?;
            // Retain the evidence even if synchronizing the move fails.
            let quarantine = quarantine.keep();
            crate::runtime::platform_fs::complete_namespace_change(&quarantine)?;
            crate::runtime::platform_fs::complete_namespace_change(&self.root.join("quarantine"))?;
            crate::runtime::platform_fs::complete_namespace_change(&self.root.join("objects"))?;
            return Err(ArtifactStoreError::Quarantined {
                tree_sha256: hex(digest),
                source: Box::new(source),
            });
        }
        Ok(())
    }

    fn lock(&self) -> Result<File, ArtifactStoreError> {
        self.lock_file(false)
    }

    fn lock_file(&self, create: bool) -> Result<File, ArtifactStoreError> {
        let lock = crate::runtime::platform_fs::open_regular(
            &self.root.join(".lock"),
            OpenOptions::new()
                .create(create)
                .truncate(false)
                .read(true)
                .write(true),
        )?;
        if !lock.metadata()?.is_file() {
            return Err(ArtifactStoreError::UnsafeEntry(self.root.join(".lock")));
        }
        lock.lock()?;
        Ok(lock)
    }

    fn lease(&self, purpose: &str, digest: ContentDigest) -> Result<Lease, ArtifactStoreError> {
        let _guard = self.lock()?;
        let directory = Builder::new()
            .prefix("lease-")
            .tempdir_in(self.root.join("leases"))?;
        let file = crate::runtime::platform_fs::open_regular(
            &lease_lock_path(directory.path()),
            OpenOptions::new().read(true).write(true).create_new(true),
        )?;
        file.lock()?;
        let document = canonical(json!(CacheLeaseRecord {
            schema_version: ARTIFACT_STORE_VERSION,
            purpose: purpose.to_owned(),
            owner_pid: std::process::id(),
            tree_sha256: hex(digest)
        }))?;
        #[cfg(unix)]
        std::io::Write::write_all(&mut &file, &document.to_canonical_bytes()?)?;
        // Windows byte-range locks prevent reads through another handle. Keep
        // the public lease receipt separate so other processes can inspect it.
        #[cfg(windows)]
        write_file(
            &directory.path().join("lease.json"),
            &document.to_canonical_bytes()?,
        )?;
        crate::runtime::durability::sync_all(&file)?;
        crate::runtime::platform_fs::complete_namespace_change(directory.path())?;
        crate::runtime::platform_fs::complete_namespace_change(&self.root.join("leases"))?;
        Ok(Lease {
            directory: Some(directory),
            lock: Some(file),
            store: self.clone(),
        })
    }
}

fn lease_lock_path(directory: &Path) -> PathBuf {
    directory.join(if cfg!(windows) {
        "lease.lock"
    } else {
        "lease.json"
    })
}

impl StoredArtifact {
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    #[must_use]
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }
    pub fn verify(&self) -> Result<(), ArtifactStoreError> {
        let _guard = self.store.lock()?;
        self.store.verify_or_quarantine(self.digest)
    }
    /// Copies into a unique private directory and verifies both sides of the
    /// boundary. The worker never receives the canonical cache path.
    pub fn private_view(&self, private_parent: &Path) -> Result<DatasetView, ArtifactStoreError> {
        let parent = fs::canonicalize(private_parent)?;
        if parent.starts_with(&self.store.root) {
            return Err(ArtifactStoreError::OverlappingTrees);
        }
        let lease = self.store.lease("view", self.digest)?;
        let _guard = self.store.lock()?;
        self.store.verify_or_quarantine(self.digest)?;
        let directory = crate::runtime::platform_fs::private_tempdir_in(&parent, "metewand-dataset-view-")?;
        let copied = copy_verified(self.directory(), directory.path(), self.digest);
        self.store.verify_or_quarantine(self.digest)?;
        copied?;
        Ok(DatasetView {
            directory,
            digest: self.digest,
            _lease: lease,
        })
    }
}

impl DatasetView {
    #[must_use]
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
    #[must_use]
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }
    pub fn verify(&self) -> Result<(), ArtifactStoreError> {
        let _guard = self._lease.store.lock()?;
        self._lease.store.verify_or_quarantine(self.digest)?;
        verify_dataset_tree(self.directory(), self.digest)
    }
}

pub(crate) fn verify_dataset_tree(
    directory: &Path,
    expected: ContentDigest,
) -> Result<(), ArtifactStoreError> {
    real_directory(directory)?;
    inventory(directory)?;
    if hash_local_tree(directory)? != expected {
        return Err(ArtifactStoreError::ChangedContent);
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Entry {
    Directory {
        path: String,
    },
    File {
        path: String,
        size_bytes: u64,
        sha256: String,
        executable: bool,
    },
}

impl Entry {
    fn path(&self) -> &str {
        match self {
            Self::Directory { path } | Self::File { path, .. } => path,
        }
    }
}

fn inventory(root: &Path) -> Result<Vec<Entry>, ArtifactStoreError> {
    fn visit(root: &Path, dir: &Path, entries: &mut Vec<Entry>) -> Result<(), ArtifactStoreError> {
        real_directory(dir)?;
        crate::runtime::platform_fs::validate_directory_names(dir)?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let absolute = entry.path();
            let path = crate::runtime::platform_fs::relative_path(
                absolute.strip_prefix(root).expect("tree-relative entry"),
            )
            .map_err(|_| ArtifactStoreError::UnsafeEntry(absolute.clone()))?;
            serde_json::from_value::<RepositoryPath>(json!(path))
                .map_err(|_| ArtifactStoreError::UnsafeEntry(absolute.clone()))?;
            let metadata = fs::symlink_metadata(&absolute)?;
            if crate::runtime::platform_fs::is_real_directory(&metadata) {
                entries.push(Entry::Directory { path });
                visit(root, &absolute, entries)?;
            } else if crate::runtime::platform_fs::is_regular_file(&metadata) {
                let mut file = regular_file(&absolute)?;
                let mut hasher = Sha256::new();
                let mut size_bytes = 0;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let count = match file.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => count,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error.into()),
                    };
                    hasher.update(&buffer[..count]);
                    size_bytes += count as u64;
                }
                entries.push(Entry::File {
                    path,
                    size_bytes,
                    sha256: hex(ContentDigest::new(hasher.finalize().into())),
                    executable: crate::runtime::platform_fs::is_executable(&metadata),
                });
            } else {
                return Err(ArtifactStoreError::UnsafeEntry(absolute));
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries)?;
    entries.sort_by(|a, b| a.path().as_bytes().cmp(b.path().as_bytes()));
    Ok(entries)
}

fn copy_verified(
    source: &Path,
    target: &Path,
    expected: ContentDigest,
) -> Result<(), ArtifactStoreError> {
    let entries = inventory(source)?;
    if hash_local_tree(source)? != expected {
        return Err(ArtifactStoreError::ChangedContent);
    }
    if !target.exists() {
        fs::create_dir(target)?;
    }
    for entry in &entries {
        let destination = target.join(entry.path());
        match entry {
            Entry::Directory { .. } => fs::create_dir(&destination)?,
            Entry::File { executable, .. } => {
                let mut input = regular_file(&source.join(entry.path()))?;
                let mut options = OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(if *executable { 0o700 } else { 0o600 });
                }
                let mut output = options.open(&destination)?;
                crate::runtime::platform_fs::set_executable(&destination, *executable)?;
                io::copy(&mut input, &mut output)?;
                crate::runtime::durability::sync_all(&output)?;
            }
        }
    }
    if inventory(source)? != entries
        || inventory(target)? != entries
        || hash_local_tree(source)? != expected
        || hash_local_tree(target)? != expected
    {
        return Err(ArtifactStoreError::ChangedContent);
    }
    Ok(())
}

fn verify_object(object: &Path, digest: ContentDigest) -> Result<(), ArtifactStoreError> {
    real_directory(object)?;
    let mut names = fs::read_dir(object)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    if names != ["data", "manifest.json"] {
        return Err(ArtifactStoreError::ChangedContent);
    }
    let mut bytes = Vec::new();
    regular_file(&object.join("manifest.json"))?.read_to_end(&mut bytes)?;
    let marker = CanonicalValue::from_slice(&bytes)?;
    validate(PublicSchema::ArtifactCacheObject, &marker)?;
    if marker.to_canonical_bytes()? != bytes
        || marker.as_json()["tree_sha256"] != hex(digest)
        || serde_json::from_value::<Vec<Entry>>(marker.as_json()["entries"].clone())?
            != inventory(&object.join("data"))?
        || hash_local_tree(object.join("data"))? != digest
    {
        return Err(ArtifactStoreError::ChangedContent);
    }
    Ok(())
}

fn same_bytes(left: &Path, right: &Path) -> Result<bool, ArtifactStoreError> {
    let entries = inventory(left)?;
    if entries != inventory(right)? {
        return Ok(false);
    }
    for entry in entries {
        if let Entry::File { path, .. } = entry {
            let mut a = regular_file(&left.join(&path))?;
            let mut b = regular_file(&right.join(path))?;
            let mut x = [0; 65536];
            let mut y = [0; 65536];
            loop {
                let n = a.read(&mut x)?;
                b.read_exact(&mut y[..n])?;
                if x[..n] != y[..n] {
                    return Ok(false);
                }
                if n == 0 {
                    if b.read(&mut y[..1])? != 0 {
                        return Ok(false);
                    }
                    break;
                }
            }
        }
    }
    Ok(true)
}

fn verify_receipt(
    document: &CanonicalValue,
    derivation: &IdentifiedRecord<DatasetDerivationRecord>,
) -> Result<ContentDigest, ArtifactStoreError> {
    let receipt = &document.as_json()["receipt"];
    if record_id(&derivation.record)? != derivation.id
        || receipt["derivation_id"] != derivation.id.to_string()
        || canonical(receipt["derivation"].clone())?.to_canonical_bytes()?
            != canonical_identity_bytes(&derivation.record)?
    {
        return Err(ArtifactStoreError::InvalidMapping);
    }
    let text = receipt["output"]["sha256"]
        .as_str()
        .ok_or(ArtifactStoreError::InvalidMapping)?;
    let mut bytes = [0; 32];
    if text.len() != 64 {
        return Err(ArtifactStoreError::InvalidMapping);
    }
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)
            .map_err(|_| ArtifactStoreError::InvalidMapping)?;
    }
    Ok(ContentDigest::new(bytes))
}

fn create_directory(path: &Path) -> Result<(), ArtifactStoreError> {
    match crate::runtime::platform_fs::create_private_directory(path) {
        Ok(()) => {
            crate::runtime::platform_fs::complete_namespace_change(
                path.parent().ok_or(ArtifactStoreError::InvalidRoot)?,
            )?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => real_directory(path)?,
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
fn real_directory(path: &Path) -> Result<(), ArtifactStoreError> {
    if !crate::runtime::platform_fs::is_real_directory(&fs::symlink_metadata(path)?) {
        return Err(ArtifactStoreError::UnsafeEntry(path.to_owned()));
    }
    Ok(())
}
fn regular_file(path: &Path) -> Result<File, ArtifactStoreError> {
    if !crate::runtime::platform_fs::is_regular_file(&fs::symlink_metadata(path)?) {
        return Err(ArtifactStoreError::UnsafeEntry(path.to_owned()));
    }
    Ok(crate::runtime::platform_fs::open_regular(
        path,
        OpenOptions::new().read(true),
    )?)
}
fn missing(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}
fn canonical(value: serde_json::Value) -> Result<CanonicalValue, ArtifactStoreError> {
    Ok(CanonicalValue::try_from(value)?)
}
fn validate(schema: PublicSchema, document: &CanonicalValue) -> Result<(), ArtifactStoreError> {
    Ok(public_schema_catalog()?.validate(schema.repository_path(), document.as_json())?)
}

/// Cache publication, integrity, mapping, or lease failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ArtifactStoreError {
    #[error("cache target must be one exact lowercase tree-v1 SHA-256 identity: {0}")]
    InvalidTarget(String),
    #[error("cache catalog does not register target: {0}")]
    UnknownTarget(String),
    #[error("invalid cache root name")]
    InvalidRootName,
    #[error("cache root does not protect this target: {0}")]
    UnknownRoot(String),
    #[error("cache catalog: {0}")]
    Catalog(&'static str),
    #[error("artifact cache root must be an absolute directory beneath an existing parent")]
    InvalidRoot,
    #[error("artifact import or view overlaps the canonical store")]
    OverlappingTrees,
    #[error("unsafe artifact cache entry: {}", .0.display())]
    UnsafeEntry(PathBuf),
    #[error("artifact bytes or completion inventory changed")]
    ChangedContent,
    #[error("canonical artifact {tree_sha256} was quarantined: {source}")]
    Quarantined {
        tree_sha256: String,
        #[source]
        source: Box<ArtifactStoreError>,
    },
    #[error("dataset derivation already maps to a different output hash")]
    DerivationConflict,
    #[error("dataset cache mapping does not match its construction identity or output")]
    InvalidMapping,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Tree(#[from] LocalTreeHashError),
    #[error(transparent)]
    Source(#[from] crate::runtime::source_resolution::SourceResolutionError),
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    PublicSchemas(#[from] PublicSchemaCatalogError),
    #[error(transparent)]
    Schema(#[from] SchemaValidationError),
    #[error(transparent)]
    Publication(#[from] PublicationError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Snapshot(#[from] crate::runtime::run_store::RunStoreError),
}
