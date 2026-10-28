use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self},
    io::Read as _,
    path::Path,
};

use metewand_core::{
    cache::{
        CACHE_MAINTENANCE_VERSION, CacheOperation, CacheReport, CacheTargetReport, CacheTargetState,
    },
    materialization::hex,
    records::ContentDigest,
};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

use super::{
    ArtifactStore, ArtifactStoreError, StoredArtifact,
    catalog::{Catalog, Snapshot},
    missing, real_directory, regular_file, rename_no_replace, verify_object,
};

fn parse_target(id: &str) -> Result<ContentDigest, ArtifactStoreError> {
    let text = id
        .strip_prefix("tree-v1-")
        .filter(|text| {
            text.len() == 64
                && text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(|| ArtifactStoreError::InvalidTarget(id.to_owned()))?;
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&text[2 * index..2 * index + 2], 16).expect("validated ASCII hex");
    }
    Ok(ContentDigest::new(bytes))
}

fn targets(ids: &[String]) -> Result<BTreeMap<String, ContentDigest>, ArtifactStoreError> {
    ids.iter()
        .map(|id| Ok((id.clone(), parse_target(id)?)))
        .collect()
}

fn marker_hash(path: &Path) -> Result<String, ArtifactStoreError> {
    let mut marker = regular_file(&path.join("manifest.json"))?;
    let mut bytes = Vec::new();
    marker.read_to_end(&mut bytes)?;
    Ok(hex(ContentDigest::new(Sha256::digest(bytes).into())))
}

impl ArtifactStore {
    // Publication has synchronized the object before this reference can commit.
    pub(super) fn register_object(
        &self,
        digest: ContentDigest,
    ) -> Result<Catalog, ArtifactStoreError> {
        let catalog = Catalog::open(self)?;
        catalog.register(&hex(digest), &marker_hash(&self.object_path(digest))?)?;
        Ok(catalog)
    }

    fn report(&self, operation: CacheOperation, dry_run: bool) -> CacheReport {
        CacheReport {
            command: "cache".into(),
            schema_version: CACHE_MAINTENANCE_VERSION,
            operation,
            cache_directory: self.root.to_string_lossy().into_owned(),
            dry_run,
            targets: Vec::new(),
        }
    }

    fn target_report(
        &self,
        id: String,
        digest: ContentDigest,
        snapshot: &Snapshot,
        leases: &BTreeMap<String, Vec<String>>,
    ) -> CacheTargetReport {
        let sha = hex(digest);
        CacheTargetReport {
            id,
            state: CacheTargetState::Retained,
            roots: snapshot.roots.get(&sha).cloned().unwrap_or_default(),
            leases: leases.get(&sha).cloned().unwrap_or_default(),
            detail: None,
        }
    }

    fn leases_by_digest(&self) -> Result<BTreeMap<String, Vec<String>>, ArtifactStoreError> {
        let mut leases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for lease in self.active_leases_locked()? {
            leases
                .entry(lease.tree_sha256)
                .or_default()
                .insert(lease.purpose);
        }
        Ok(leases
            .into_iter()
            .map(|(sha, purposes)| (sha, purposes.into_iter().collect()))
            .collect())
    }

    /// Verifies only the requested complete identities. Corruption is reported
    /// without quarantine, catalog changes, or source SQLite sidecars.
    pub fn verify_targets(&self, ids: &[String]) -> Result<CacheReport, ArtifactStoreError> {
        if ids.is_empty() {
            return Err(ArtifactStoreError::InvalidTarget(String::new()));
        }
        let targets = targets(ids)?;
        let _guard = self.lock()?;
        let snapshot = Catalog::read(self)?;
        let leases = self.leases_by_digest()?;
        let mut report = self.report(CacheOperation::Verify, false);
        for (id, digest) in targets {
            let mut target = self.target_report(id, digest, &snapshot, &leases);
            let path = self.object_path(digest);
            if missing(&path)? {
                target.state = CacheTargetState::Missing;
            } else {
                match self.verify_registered(&path, digest, &snapshot) {
                    Ok(()) => target.state = CacheTargetState::Verified,
                    Err(error) => {
                        target.state = CacheTargetState::Corrupt;
                        target.detail = Some(error.to_string());
                    }
                }
            }
            report.targets.push(target);
        }
        Ok(report)
    }

    /// Lists durable named roots from one committed catalog snapshot.
    pub fn roots(&self) -> Result<CacheReport, ArtifactStoreError> {
        let _guard = self.lock()?;
        let snapshot = Catalog::read(self)?;
        let leases = self.leases_by_digest()?;
        let mut report = self.report(CacheOperation::Roots, false);
        for sha in snapshot.roots.keys() {
            let id = format!("tree-v1-{sha}");
            let digest = parse_target(&id)?;
            report
                .targets
                .push(self.target_report(id, digest, &snapshot, &leases));
        }
        Ok(report)
    }

    /// Adds or releases one exact root-to-content edge. Removing a root never
    /// removes data; a subsequent explicit collection evaluates protection again.
    pub fn update_root(
        &self,
        id: &str,
        root: &str,
        add: bool,
        dry_run: bool,
    ) -> Result<CacheReport, ArtifactStoreError> {
        let digest = parse_target(id)?;
        if root.is_empty() || root.chars().any(char::is_control) {
            return Err(ArtifactStoreError::InvalidRootName);
        }
        let _guard = self.lock()?;
        let snapshot = Catalog::read(self)?;
        let leases = self.leases_by_digest()?;
        if add {
            self.verify_registered(&self.object_path(digest), digest, &snapshot)?;
        } else if !snapshot
            .roots
            .get(&hex(digest))
            .is_some_and(|roots| roots.iter().any(|name| name == root))
        {
            return Err(ArtifactStoreError::UnknownRoot(root.to_owned()));
        }
        if !dry_run {
            let catalog = if add {
                self.register_object(digest)?
            } else {
                Catalog::open(self)?
            };
            catalog.root(&hex(digest), root, add)?;
        }
        let mut report = self.report(
            if add {
                CacheOperation::RootAdd
            } else {
                CacheOperation::RootRemove
            },
            dry_run,
        );
        let mut target = self.target_report(id.to_owned(), digest, &snapshot, &leases);
        if add && !target.roots.iter().any(|name| name == root) {
            target.roots.push(root.into());
            target.roots.sort();
        } else if !add {
            target.roots.retain(|name| name != root);
        }
        target.state = match (add, dry_run) {
            (true, false) => CacheTargetState::RootAdded,
            (true, true) => CacheTargetState::WouldAddRoot,
            (false, false) => CacheTargetState::RootRemoved,
            (false, true) => CacheTargetState::WouldRemoveRoot,
        };
        report.targets.push(target);
        Ok(report)
    }

    fn verify_registered(
        &self,
        path: &Path,
        digest: ContentDigest,
        snapshot: &Snapshot,
    ) -> Result<(), ArtifactStoreError> {
        verify_object(path, digest)?;
        if let Some(expected) = snapshot.objects.get(&hex(digest))
            && marker_hash(path)? != *expected
        {
            return Err(ArtifactStoreError::ChangedContent);
        }
        Ok(())
    }

    /// Collects only cataloged content identities unprotected by a durable root
    /// or live kernel lease. The cache lock fences the snapshot through cleanup.
    /// Dry runs never open the source catalog for writing.
    pub fn collect(
        &self,
        ids: &[String],
        dry_run: bool,
    ) -> Result<CacheReport, ArtifactStoreError> {
        let mut targets = targets(ids)?;
        let _guard = self.lock()?;
        let snapshot = Catalog::read(self)?;
        let leases = self.leases_by_digest()?;
        if ids.is_empty() {
            targets = snapshot
                .objects
                .keys()
                .map(|sha| {
                    let id = format!("tree-v1-{sha}");
                    let digest = parse_target(&id)?;
                    Ok((id, digest))
                })
                .collect::<Result<_, ArtifactStoreError>>()?;
        }
        let mut report = self.report(CacheOperation::Gc, dry_run);
        // Validate the entire selection before detaching any directory.
        for (id, digest) in &targets {
            let sha = hex(*digest);
            if !snapshot.objects.contains_key(&sha) {
                return Err(ArtifactStoreError::UnknownTarget(id.clone()));
            }
            let mut target = self.target_report(id.clone(), *digest, &snapshot, &leases);
            if target.roots.is_empty() && target.leases.is_empty() {
                let live = self.object_path(*digest);
                let garbage = self.root.join("garbage").join(id);
                let live_exists = !missing(&live)?;
                let garbage_exists = !missing(&garbage)?;
                if live_exists {
                    self.verify_registered(&live, *digest, &snapshot)?;
                }
                if garbage_exists {
                    real_directory(&garbage)?;
                    match snapshot.pending.get(&sha) {
                        Some(true) => (),
                        Some(false) => self.verify_registered(&garbage, *digest, &snapshot)?,
                        None => return Err(ArtifactStoreError::Catalog("unclaimed garbage path")),
                    }
                }
                target.state =
                    if live_exists || garbage_exists || snapshot.pending.contains_key(&sha) {
                        if dry_run {
                            CacheTargetState::WouldDelete
                        } else {
                            CacheTargetState::Deleted
                        }
                    } else {
                        CacheTargetState::Missing
                    };
            }
            report.targets.push(target);
        }
        if dry_run
            || !report
                .targets
                .iter()
                .any(|target| target.state == CacheTargetState::Deleted)
        {
            return Ok(report);
        }
        let catalog = Catalog::open(self)?;
        for target in &report.targets {
            if target.state != CacheTargetState::Deleted {
                continue;
            }
            let digest = targets[&target.id];
            let sha = hex(digest);
            let live = self.object_path(digest);
            let garbage = self.root.join("garbage").join(&target.id);
            if !missing(&garbage)? {
                crate::platform_fs::complete_namespace_change(&self.root.join("objects"))?;
                crate::platform_fs::complete_namespace_change(&self.root.join("garbage"))?;
                catalog.pending(&sha, true)?;
                #[cfg(test)]
                crate::fault_injection::hit("cache.cleanup.before");
                fs::remove_dir_all(&garbage)?;
                #[cfg(test)]
                crate::fault_injection::hit("cache.cleanup.after");
                crate::platform_fs::complete_namespace_change(&self.root.join("garbage"))?;
            }
            if !missing(&live)? {
                catalog.pending(&sha, false)?;
                rename_no_replace(&live, &garbage)?;
                crate::platform_fs::complete_namespace_change(&self.root.join("objects"))?;
                crate::platform_fs::complete_namespace_change(&self.root.join("garbage"))?;
                catalog.pending(&sha, true)?;
                #[cfg(test)]
                crate::fault_injection::hit("cache.cleanup.before");
                fs::remove_dir_all(&garbage)?;
                #[cfg(test)]
                crate::fault_injection::hit("cache.cleanup.after");
                crate::platform_fs::complete_namespace_change(&self.root.join("garbage"))?;
            }
            catalog.forget(&sha)?;
        }
        Ok(report)
    }
}

impl StoredArtifact {
    /// Retains a verified dataset for a durable run or an explicitly named pin.
    pub fn add_root(&self, name: &str) -> Result<(), ArtifactStoreError> {
        self.store
            .update_root(&format!("tree-v1-{}", hex(self.digest)), name, true, false)?;
        Ok(())
    }
}
