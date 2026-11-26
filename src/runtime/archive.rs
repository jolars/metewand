//! Portable result evidence with offline verification and atomic publication.

#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};

use crate::core::{
    archive::{
        ArchiveEvidence, ArchiveFile, ArchiveManifest, ArchiveOperation, ArchiveReport,
        RESULT_ARCHIVE_VERSION,
    },
    canonical::{CanonicalJsonError, CanonicalValue},
    public_schemas::{PublicSchema, public_schema_catalog},
};
use flate2::{Compression, GzBuilder, bufread::GzDecoder};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};
use thiserror::Error;

use crate::runtime::{
    evidence::{
        EvidenceSnapshot, buffered_trace_path, document_profile, validate_observation_document,
        visit_buffered_trace,
    },
    inspection::Inspection,
    local_tree::hash_reader,
    platform_fs,
    publication::rename_no_replace,
};

/// Bounds apply to actual uncompressed entries, including archive metadata.
#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    pub max_entries: u64,
    pub max_expanded_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 1_000_000,
            max_expanded_bytes: 1 << 40,
        }
    }
}

/// Marker detection deliberately does not validate or modify the collection.
pub(crate) fn is_collection(root: &Path) -> bool {
    fs::symlink_metadata(root.join("archive.json")).is_ok()
}

/// Creates a deterministic gzip-compressed archive from one committed snapshot.
/// Every started attempt must already be terminal; creation never recovers it.
pub fn create_archive(root: &Path, destination: &Path) -> Result<ArchiveReport, ArchiveError> {
    let destination = new_destination(destination)?;
    let snapshot = EvidenceSnapshot::read(root).map_err(evidence_error)?;
    if destination.starts_with(&snapshot.root) {
        return Err(invalid(
            "the archive destination must be outside its source collection or run",
        ));
    }
    let evidence = snapshot.archive_evidence()?;
    validate_evidence(&evidence)?;
    let parent = destination.parent().expect("destination parent exists");
    let evidence_bytes = canonical_bytes(&evidence)?;
    validate_document(&evidence_bytes, PublicSchema::ArchiveEvidence)?;
    let evidence_file = metadata_member("evidence.json", &evidence_bytes);
    let mut files = BTreeMap::from([(evidence_file.path.clone(), evidence_file)]);
    for artifact in &snapshot.tables["stored-artifact"] {
        let relative = artifact["path"]
            .as_str()
            .ok_or_else(|| invalid("artifact path is missing"))?;
        let source = contained(&snapshot.data_root, relative)?;
        let payloads = match artifact["storage_kind"].as_str() {
            Some("directory") => walk_files(&source)?
                .into_values()
                .map(|mut file| {
                    file.path = format!("data/{relative}/{}", file.path);
                    file
                })
                .collect(),
            Some("file") => vec![inventory_file(
                &source,
                format!("data/{relative}"),
                &mut 0,
                &mut 0,
                UNLIMITED,
            )?],
            _ => return Err(invalid("unsupported artifact storage kind")),
        };
        validate_registered_members(artifact, &source, relative, &payloads)?;
        for mut file in payloads {
            if let Some(original) = snapshot
                .files
                .as_ref()
                .and_then(|files| files.get(&file.path))
            {
                check_member(original, file.size_bytes, &file.sha256)?;
                file.executable = original.executable;
            }
            if files.insert(file.path.clone(), file).is_some() {
                return Err(invalid("registered artifact paths overlap"));
            }
        }
    }
    let manifest = ArchiveManifest {
        schema_version: RESULT_ARCHIVE_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        files: files.into_values().collect(),
    };
    let manifest_bytes = canonical_bytes(&manifest)?;
    validate_document(&manifest_bytes, PublicSchema::ArchiveManifest)?;
    let manifest_file = metadata_member("archive.json", &manifest_bytes);
    let mut verified = EvidenceSnapshot::archived(snapshot.root, evidence, manifest.files.clone());
    verified.data_root = snapshot.data_root;
    validate_payload_closure(&verified)?;
    validate_snapshot(&verified)?;
    validate_documents(&verified, UNLIMITED)?;

    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    {
        let encoder = GzBuilder::new()
            .mtime(0)
            .write(output.as_file_mut(), Compression::new(6));
        let mut tar = tar::Builder::new(encoder);
        for member in std::iter::once(&manifest_file).chain(&manifest.files) {
            let mut header = tar::Header::new_gnu();
            header.set_size(member.size_bytes);
            header.set_mode(0o600);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            match member.path.as_str() {
                "archive.json" => {
                    tar.append_data(&mut header, &member.path, manifest_bytes.as_slice())?
                }
                "evidence.json" => {
                    tar.append_data(&mut header, &member.path, evidence_bytes.as_slice())?
                }
                name => {
                    let relative = name.strip_prefix("data/").expect("payload prefix");
                    let source = contained(&verified.data_root, relative)?;
                    let input = regular_file(&source)?;
                    if input.metadata()?.len() != member.size_bytes {
                        return Err(invalid("archive file changed while it was read"));
                    }
                    tar.append_data(&mut header, name, input.take(member.size_bytes))?;
                }
            }
        }
        tar.into_inner()?.finish()?;
    }
    // Read back the completed stream before publication, without extracting it.
    verify_created_archive(output.path(), &manifest_file, &manifest.files)?;
    platform_fs::sync_owned_file(output.path())?;
    #[cfg(test)]
    crate::runtime::fault_injection::hit("rename.before");
    // File persistence can use a hard link when no-replace rename is unsupported.
    output
        .persist_noclobber(&destination)
        .map_err(|error| error.error)?;
    #[cfg(test)]
    crate::runtime::fault_injection::hit("rename.after");
    platform_fs::complete_namespace_change(parent)?;
    report(
        ArchiveOperation::Create,
        &destination,
        Some(&destination),
        &manifest,
        &verified,
    )
}

// Bind the inventory to committed artifact digests before opening the stream.
fn validate_registered_members(
    artifact: &serde_json::Value,
    source: &Path,
    relative: &str,
    members: &[ArchiveFile],
) -> Result<(), ArchiveError> {
    if artifact["storage_kind"] == "file" {
        if artifact["sha256"] != members[0].sha256 {
            return Err(invalid(
                "archive member differs from its registered artifact",
            ));
        }
        return Ok(());
    }
    let prefix = format!("data/{relative}/");
    let mut members = members
        .iter()
        .map(|file| {
            (
                file.path.strip_prefix(&prefix).expect("artifact prefix"),
                file,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let marker = members
        .remove("manifest.json")
        .ok_or_else(|| invalid("registered artifact is missing its manifest"))?;
    let mut bytes = Vec::new();
    regular_file(&source.join("manifest.json"))?
        .take(marker.size_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    check_member(marker, bytes.len() as u64, &digest(&bytes))?;
    if artifact["manifest_sha256"] != marker.sha256 {
        return Err(invalid(
            "archive member differs from its registered artifact",
        ));
    }
    let document = validate_document(&bytes, PublicSchema::ArtifactManifest)?;
    let declared = document.as_json()["files"]
        .as_array()
        .expect("validated inventory");
    for item in declared {
        let path = item["path"].as_str().expect("validated member path");
        let file = members
            .remove(path)
            .ok_or_else(|| invalid("artifact manifest lists a missing archive member"))?;
        if item["sha256"] != file.sha256 || item["size_bytes"] != file.size_bytes {
            return Err(invalid("archive member differs from its artifact manifest"));
        }
    }
    if !members.is_empty() {
        return Err(invalid(
            "archive member is missing from its artifact manifest",
        ));
    }
    Ok(())
}

/// Verifies an archive or an imported collection without modifying its input.
pub fn verify_archive(path: &Path, limits: ArchiveLimits) -> Result<ArchiveReport, ArchiveError> {
    let (manifest, snapshot) = if platform_fs::is_real_directory(&fs::symlink_metadata(path)?) {
        let snapshot = collection_snapshot(path, limits)?;
        let manifest = read_document(
            &path.join("archive.json"),
            PublicSchema::ArchiveManifest,
            limits,
        )?;
        (manifest, snapshot)
    } else {
        let stage = private_stage(None)?;
        extract(path, stage.path(), limits)?;
        let snapshot = collection_snapshot(stage.path(), limits)?;
        let manifest = read_document(
            &stage.path().join("archive.json"),
            PublicSchema::ArchiveManifest,
            limits,
        )?;
        return report(ArchiveOperation::Verify, path, None, &manifest, &snapshot);
    };
    report(ArchiveOperation::Verify, path, None, &manifest, &snapshot)
}

/// Imports only fully verified evidence into a new read-only collection.
pub fn import_archive(
    path: &Path,
    destination: &Path,
    limits: ArchiveLimits,
) -> Result<ArchiveReport, ArchiveError> {
    let destination = new_destination(destination)?;
    let parent = destination.parent().expect("destination parent");
    let stage = private_stage(Some(parent))?;
    extract(path, stage.path(), limits)?;
    let snapshot = collection_snapshot(stage.path(), limits)?;
    let manifest: ArchiveManifest = read_document(
        &stage.path().join("archive.json"),
        PublicSchema::ArchiveManifest,
        limits,
    )?;
    reuse_verified_payloads(&stage.path().join("data"), &manifest.files)?;
    for name in walk_files(stage.path())?.keys() {
        platform_fs::sync_owned_file(&stage.path().join(name))?;
    }
    crate::runtime::publication::sync_directories(stage.path()).map_err(evidence_error)?;
    rename_no_replace(stage.path(), &destination)?;
    let _ = stage.keep();
    platform_fs::complete_namespace_change(parent)?;
    report(
        ArchiveOperation::Import,
        path,
        Some(&destination),
        &manifest,
        &snapshot,
    )
}

/// Rebuilds a resumable local run from a verified archive without executing workers.
/// The destination is published only after its database and artifacts agree
/// with the portable evidence. Original ownership leases are not restored.
pub fn restore_archive(
    path: &Path,
    destination: &Path,
    limits: ArchiveLimits,
) -> Result<ArchiveReport, ArchiveError> {
    let destination = new_destination(destination)?;
    let parent = destination.parent().expect("destination parent");
    let source = private_stage(Some(parent))?;
    extract(path, source.path(), limits)?;
    let snapshot = collection_snapshot(source.path(), limits)?;
    let manifest: ArchiveManifest = read_document(
        &source.path().join("archive.json"),
        PublicSchema::ArchiveManifest,
        limits,
    )?;
    let evidence = snapshot.archive_evidence()?;
    // Reuse the privately extracted payloads so restoration needs only one
    // uncompressed copy, even for large profile archives on local scratch.
    let stage = source.path().join("data");
    if !stage.exists() {
        platform_fs::create_private_directory(&stage)?;
    }
    platform_fs::set_private_directory(&stage)?;
    crate::runtime::run_store::RunStore::restore(&stage, &evidence).map_err(evidence_error)?;
    for file in &manifest.files {
        let Some(relative) = file.path.strip_prefix("data/") else {
            continue;
        };
        let target = stage.join(relative);
        platform_fs::set_executable(&target, file.executable)?;
        if platform_fs::is_executable(&fs::symlink_metadata(&target)?) != file.executable {
            return Err(invalid(
                "the destination cannot preserve archived executable bits for a resumable run",
            ));
        }
    }
    reuse_verified_payloads(&stage, &manifest.files)?;
    let restored = EvidenceSnapshot::read(&stage).map_err(evidence_error)?;
    let records = |evidence: &ArchiveEvidence| -> Result<Vec<Vec<u8>>, ArchiveError> {
        let mut records = evidence
            .records
            .iter()
            .map(canonical_bytes)
            .collect::<Result<Vec<_>, _>>()?;
        records.sort();
        Ok(records)
    };
    if records(&evidence)? != records(&restored.archive_evidence()?)? {
        return Err(invalid(
            "restored coordination state differs from portable evidence",
        ));
    }
    validate_snapshot(&restored)?;
    for name in walk_files(&stage)?.keys() {
        platform_fs::sync_owned_file(&stage.join(name))?;
    }
    crate::runtime::publication::sync_directories(&stage).map_err(evidence_error)?;
    rename_no_replace(&stage, &destination)?;
    platform_fs::complete_namespace_change(source.path())?;
    platform_fs::complete_namespace_change(parent)?;
    report(
        ArchiveOperation::Restore,
        path,
        Some(&destination),
        &manifest,
        &snapshot,
    )
}

pub(crate) fn collection_snapshot(
    root: &Path,
    limits: ArchiveLimits,
) -> Result<EvidenceSnapshot, ArchiveError> {
    real_directory(root)?;
    let root = fs::canonicalize(root)?;
    let manifest: ArchiveManifest = read_document(
        &root.join("archive.json"),
        PublicSchema::ArchiveManifest,
        limits,
    )?;
    let actual = walk_files_limited(&root, limits)?;
    if manifest
        .files
        .windows(2)
        .any(|pair| pair[0].path >= pair[1].path)
        || actual.len() != manifest.files.len() + 1
    {
        return Err(invalid(
            "archive inventory is unsorted, duplicated, or incomplete",
        ));
    }
    for file in &manifest.files {
        safe_name(&file.path)?;
        if file.path != "evidence.json" && !file.path.starts_with("data/") {
            return Err(invalid(
                "archive inventory contains an unsupported payload path",
            ));
        }
        let current = actual
            .get(&file.path)
            .ok_or_else(|| invalid("an inventoried file is missing"))?;
        check_member(file, current.size_bytes, &current.sha256)?;
        if file.path == "evidence.json" && file.executable {
            return Err(invalid("evidence metadata cannot have an executable bit"));
        }
    }
    let evidence: ArchiveEvidence = read_document(
        &root.join("evidence.json"),
        PublicSchema::ArchiveEvidence,
        limits,
    )?;
    validate_evidence(&evidence)?;
    let snapshot = EvidenceSnapshot::archived(root, evidence, manifest.files);
    validate_payload_closure(&snapshot)?;
    validate_snapshot(&snapshot)?;
    validate_documents(&snapshot, limits)?;
    Ok(snapshot)
}

// Portable archives retain every logical path. The private extraction has
// already been verified, so identical immutable files can share storage.
fn reuse_verified_payloads(root: &Path, files: &[ArchiveFile]) -> io::Result<()> {
    let mut contents = BTreeMap::new();
    for file in files {
        let Some(relative) = file.path.strip_prefix("data/") else {
            continue;
        };
        let id = (&file.sha256, file.size_bytes, file.executable);
        let path = root.join(relative);
        if let Some(existing) = contents.get(&id) {
            fs::remove_file(&path)?;
            if fs::hard_link(existing, &path).is_err() {
                // A portable archive can also be extracted onto a filesystem
                // that cannot create hard links.
                fs::copy(existing, &path)?;
            }
        } else {
            contents.insert(id, path);
        }
    }
    Ok(())
}

fn validate_snapshot(snapshot: &EvidenceSnapshot) -> Result<(), ArchiveError> {
    let mut inspection = Inspection::default();
    inspection.add_snapshot(snapshot).map_err(evidence_error)?;
    for record in inspection.into_records().into_values() {
        match record.kind.as_str() {
            "observation" => {
                validate_observation_document(&record.record).map_err(evidence_error)?
            }
            "attempt" => {
                if let Some(profile) = document_profile(&record.record["document"]) {
                    if let Some(files) = &snapshot.files
                        && let Some(path) = buffered_trace_path(&snapshot.data_root, profile)
                            .map_err(evidence_error)?
                    {
                        let relative = path
                            .strip_prefix(&snapshot.data_root)
                            .expect("buffered trace is relative to the data root");
                        let name = format!("data/{}", platform_fs::relative_path(relative)?);
                        if !files.contains_key(&name) {
                            return Err(invalid(
                                "buffered trace is missing from the archive inventory",
                            ));
                        }
                    }
                    visit_buffered_trace(&snapshot.data_root, &record.id, profile, |_, _| {})
                        .map_err(evidence_error)?;
                }
            }
            _ => (),
        }
    }
    Ok(())
}

fn validate_payload_closure(snapshot: &EvidenceSnapshot) -> Result<(), ArchiveError> {
    let files = snapshot.files.as_ref().expect("archive inventory");
    let mut required = BTreeSet::from(["evidence.json".to_owned()]);
    for artifact in &snapshot.tables["stored-artifact"] {
        let path = artifact["path"]
            .as_str()
            .ok_or_else(|| invalid("artifact lacks a path"))?;
        safe_name(path)?;
        let full = format!("data/{path}");
        let prefix = format!("{full}/");
        let matches = if artifact["storage_kind"] == "file" {
            files
                .get_key_value(&full)
                .map(|(name, _)| name)
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            files
                .range(prefix.clone()..)
                .take_while(|(name, _)| name.starts_with(&prefix))
                .map(|(name, _)| name)
                .collect()
        };
        if matches.is_empty() || matches.iter().any(|name| !required.insert((*name).clone())) {
            return Err(invalid(
                "registered artifact payloads are missing or overlap",
            ));
        }
    }
    if required.len() != files.len() {
        return Err(invalid(
            "archive contains payloads with no registered artifact",
        ));
    }
    Ok(())
}

fn fresh_repetition_key(row: &serde_json::Value) -> Result<(&str, u64), ArchiveError> {
    row["resolved_run_id"]
        .as_str()
        .zip(row["measurement_index"].as_u64())
        .ok_or_else(|| invalid("invalid fresh repetition identity"))
}

struct FreshMembership {
    document: serde_json::Value,
    digest: String,
    path: String,
}

fn fresh_membership_documents(
    snapshot: &EvidenceSnapshot,
) -> Result<BTreeMap<String, FreshMembership>, ArchiveError> {
    use crate::runtime::run_store::fresh::{membership_address, membership_document};
    use serde_json::{Value, json};

    let mut members = BTreeMap::<_, Vec<Value>>::new();
    for row in snapshot.tables.get("fresh-member").into_iter().flatten() {
        members
            .entry(fresh_repetition_key(row)?)
            .or_default()
            .push(json!({
                "slot_id": row["slot_id"],
                "schedule_index": row["schedule_index"],
                "control": row["control"],
            }));
    }
    let mut documents = BTreeMap::new();
    for row in snapshot.tables.get("fresh-policy").into_iter().flatten() {
        let key = fresh_repetition_key(row)?;
        let mut members = members.remove(&key).unwrap_or_default();
        members.sort_by_key(|row| row["schedule_index"].as_u64());
        let document = membership_document(key.0, key.1, row["policy"].clone(), members);
        let digest = digest(&canonical_bytes(&document)?);
        let (id, path) = membership_address(key.0, key.1);
        documents.insert(
            id,
            FreshMembership {
                document,
                digest,
                path,
            },
        );
    }
    Ok(documents)
}

fn validate_documents(
    snapshot: &EvidenceSnapshot,
    limits: ArchiveLimits,
) -> Result<(), ArchiveError> {
    use crate::runtime::{
        evidence::index_rows,
        run_store::fresh::{MEMBERSHIP_ROLE, membership_address},
    };

    let attempts = index_rows(&snapshot.tables["attempt"], "artifact_id");
    let observations = index_rows(&snapshot.tables["observation"], "artifact_id");
    let observations_by_attempt = index_rows(&snapshot.tables["observation"], "attempt_id");
    let budgets = index_rows(
        snapshot
            .tables
            .get("fresh-budget")
            .map_or(&[], Vec::as_slice),
        "attempt_id",
    );
    let fresh_documents = fresh_membership_documents(snapshot)?;
    let fresh_members = index_rows(
        snapshot
            .tables
            .get("fresh-member")
            .map_or(&[], Vec::as_slice),
        "slot_id",
    );
    let mut stored_memberships = BTreeSet::new();
    for artifact in &snapshot.tables["stored-artifact"] {
        let id = artifact["id"].as_str().expect("validated artifact id");
        if id.starts_with("fresh-membership-v1:") {
            let expected = fresh_documents
                .get(id)
                .ok_or_else(|| invalid("stored fresh membership has no repetition"))?;
            if artifact["path"] != expected.path
                || artifact["sha256"] != expected.digest
                || artifact["kind"] != "other"
                || artifact["storage_kind"] != "file"
                || !artifact["manifest_sha256"].is_null()
            {
                return Err(invalid(
                    "stored fresh membership disagrees with its repetition",
                ));
            }
            stored_memberships.insert(id);
        }
    }
    let attempt_artifacts = index_rows(&snapshot.tables["attempt-artifact"], "attempt_id");
    for artifact in &snapshot.tables["stored-artifact"] {
        let relative = artifact["path"]
            .as_str()
            .ok_or_else(|| invalid("missing artifact path"))?;
        let directory = snapshot.data_root.join(relative);
        let document = match artifact["kind"].as_str() {
            Some("attempt") => Some(("attempt.json", PublicSchema::Attempt)),
            Some("observation") => Some(("observation.json", PublicSchema::Observation)),
            _ => None,
        };
        if let Some((name, schema)) = document {
            let path = directory.join(name);
            if path.exists() {
                let document: serde_json::Value = read_document(&path, schema, limits)?;
                let is_attempt = schema == PublicSchema::Attempt;
                let index = if is_attempt { &attempts } else { &observations };
                if let Some(row) = artifact["id"]
                    .as_str()
                    .and_then(|id| index.get(id))
                    .and_then(|rows| rows.first())
                {
                    if document["id"] != row["id"] || document["slot_id"] != row["slot_id"] {
                        return Err(invalid(
                            "artifact document disagrees with its registered subject",
                        ));
                    }
                    if !is_attempt {
                        if document != row["definition"] {
                            return Err(invalid(
                                "observation artifact disagrees with its committed document",
                            ));
                        }
                        validate_scientific_payloads(snapshot, relative, &document)?;
                    }
                    if is_attempt
                        && (document["retry_index"] != row["retry_index"]
                            || document["outcome"]["kind"] != row["state"])
                    {
                        return Err(invalid(
                            "attempt document disagrees with its terminal coordination",
                        ));
                    }
                    if is_attempt {
                        let declared = document["observations"]
                            .as_array()
                            .expect("validated observation list")
                            .iter()
                            .map(|id| id.as_str().expect("validated observation identity"))
                            .collect::<BTreeSet<_>>();
                        let committed = row["id"]
                            .as_str()
                            .and_then(|id| observations_by_attempt.get(id))
                            .into_iter()
                            .flatten()
                            .map(|observation| {
                                observation["id"]
                                    .as_str()
                                    .expect("typed observation identity")
                            })
                            .collect::<BTreeSet<_>>();
                        if declared != committed {
                            return Err(invalid(
                                "attempt observation list disagrees with committed observations",
                            ));
                        }
                    }
                }
            }
        }
        if artifact["kind"] == "attempt" && snapshot.tables.contains_key("fresh-budget") {
            let outcome = CanonicalValue::from_slice(&fs::read(directory.join("outcome.json"))?)?;
            let outcome = outcome.as_json();
            let attempt = artifact["id"]
                .as_str()
                .and_then(|id| attempts.get(id))
                .and_then(|rows| rows.first())
                .ok_or_else(|| invalid("missing terminal attempt for coordination outcome"))?;
            if outcome["attempt_id"] != attempt["id"] || outcome["slot_id"] != attempt["slot_id"] {
                return Err(invalid("coordination outcome disagrees with its attempt"));
            }
            let fresh = &outcome["coordination"]["fresh"];
            let references = outcome["references"]
                .as_array()
                .ok_or_else(|| invalid("missing outcome dependencies"))?;
            let mut references = references
                .iter()
                .filter(|row| row["role"] == MEMBERSHIP_ROLE);
            let reference = references.next();
            let member = attempt["slot_id"]
                .as_str()
                .and_then(|slot| fresh_members.get(slot))
                .and_then(|rows| rows.first());
            if let Some(member) = member {
                let key = fresh_repetition_key(member)?;
                let (id, _) = membership_address(key.0, key.1);
                let FreshMembership {
                    document,
                    digest,
                    path,
                } = fresh_documents
                    .get(&id)
                    .ok_or_else(|| invalid("missing fresh repetition policy"))?;
                if fresh_repetition_key(fresh)? != key {
                    return Err(invalid("fresh outcome disagrees with its repetition"));
                }
                if let Some(reference) = reference {
                    let committed = attempt["id"]
                        .as_str()
                        .and_then(|id| attempt_artifacts.get(id))
                        .is_some_and(|rows| {
                            rows.iter().any(|row| {
                                row["role"] == MEMBERSHIP_ROLE && row["artifact_id"] == id
                            })
                        });
                    if references.next().is_some()
                        || fresh.get("members").is_some()
                        || fresh.get("policy").is_some()
                        || !committed
                        || reference
                            != &serde_json::json!({
                                "role": MEMBERSHIP_ROLE, "id": id, "relative_path": path,
                                "kind": "other", "storage_kind": "file",
                                "content_sha256": digest, "manifest_sha256": null,
                            })
                        || !stored_memberships.contains(id.as_str())
                    {
                        return Err(invalid("invalid durable fresh membership reference"));
                    }
                } else if fresh["members"] != document["members"]
                    || fresh["policy"] != document["policy"]
                {
                    return Err(invalid(
                        "inline fresh membership disagrees with coordination records",
                    ));
                }
            } else if reference.is_some() || !fresh.is_null() {
                return Err(invalid("fresh membership requires a fresh attempt"));
            }
            let budget = outcome["attempt_id"]
                .as_str()
                .and_then(|id| budgets.get(id))
                .and_then(|rows| rows.first())
                .map_or(serde_json::Value::Null, |budget| {
                    serde_json::json!({
                        "reserved_ns": budget["reserved_ns"],
                        "charged_ns": budget["charged_ns"],
                    })
                });
            if budget != fresh["budget"] {
                return Err(invalid(
                    "fresh budget disagrees with its immutable outcome snapshot",
                ));
            }
        }
    }
    Ok(())
}

fn validate_scientific_payloads(
    snapshot: &EvidenceSnapshot,
    directory: &str,
    document: &serde_json::Value,
) -> Result<(), ArchiveError> {
    let files = snapshot.files.as_ref().expect("archive inventory");
    for role in ["result", "metrics"] {
        let reference = &document[role];
        let path = reference["path"]
            .as_str()
            .ok_or_else(|| invalid("missing observation payload path"))?;
        let relative = format!("{directory}/{path}");
        safe_name(&relative)?;
        let sha256 = if role == "result" {
            hex(snapshot
                .tree_hash(&relative)
                .map_err(evidence_error)?
                .bytes())
        } else {
            files
                .get(&format!("data/{relative}"))
                .ok_or_else(|| invalid("missing observation metrics payload"))?
                .sha256
                .clone()
        };
        if reference["sha256"] != sha256 {
            return Err(invalid(format!(
                "observation {role} digest disagrees with its included payload"
            )));
        }
    }
    Ok(())
}

fn extract(path: &Path, destination: &Path, limits: ArchiveLimits) -> Result<(), ArchiveError> {
    visit_archive(path, limits, |name, size, entry| {
        let target = destination.join(name);
        fs::create_dir_all(target.parent().expect("member parent"))?;
        let mut file = File::create_new(target)?;
        if io::copy(entry, &mut file)? != size {
            return Err(invalid("truncated archive member"));
        }
        Ok(())
    })
}

fn verify_created_archive(
    path: &Path,
    manifest: &ArchiveFile,
    files: &[ArchiveFile],
) -> Result<(), ArchiveError> {
    let mut expected = std::iter::once(manifest).chain(files);
    visit_archive(path, UNLIMITED, |name, size, entry| {
        let file = expected
            .next()
            .filter(|file| file.path == name && file.size_bytes == size)
            .ok_or_else(|| invalid("streamed archive member differs from its inventory"))?;
        let (digest, size_bytes) = hash_reader(entry)?;
        check_member(file, size_bytes, &hex(digest.bytes()))
    })?;
    if expected.next().is_some() {
        return Err(invalid("streamed archive is missing an inventoried member"));
    }
    Ok(())
}

fn visit_archive(
    path: &Path,
    limits: ArchiveLimits,
    mut visit: impl FnMut(&str, u64, &mut dyn Read) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let decoder = GzDecoder::new(BufReader::new(regular_file(path)?));
    let mut archive = tar::Archive::new(decoder);
    let mut names = BTreeSet::new();
    let mut count = 0;
    let mut total = 0_u64;
    let mut long_name = None;
    // Raw iteration lets us bound GNU filename metadata before allocating it.
    for entry in archive.entries()?.raw(true) {
        let mut entry = entry?;
        if entry.header().entry_type().is_gnu_longname() {
            if long_name.is_some() || entry.size() > 65536 {
                return Err(invalid("oversized or repeated GNU filename metadata"));
            }
            total = total.checked_add(entry.size()).ok_or(ArchiveError::Limit)?;
            if total > limits.max_expanded_bytes {
                return Err(ArchiveError::Limit);
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            if bytes.pop() != Some(0) {
                return Err(invalid("GNU filename metadata lacks its terminator"));
            }
            long_name =
                Some(String::from_utf8(bytes).map_err(|_| invalid("archive paths must be UTF-8"))?);
            continue;
        }
        let name = if let Some(name) = long_name.take() {
            name
        } else {
            entry
                .path()?
                .to_str()
                .ok_or_else(|| invalid("archive paths must be UTF-8"))?
                .to_owned()
        };
        safe_name(&name)?;
        if !entry.header().entry_type().is_file() || !names.insert(name.clone()) {
            return Err(invalid(
                "archive links, special entries, directories, and duplicate paths are unsupported",
            ));
        }
        if name != "archive.json" && name != "evidence.json" && !name.starts_with("data/") {
            return Err(invalid("unsupported archive member"));
        }
        charge(&mut count, &mut total, entry.size(), limits)?;
        visit(&name, entry.size(), &mut entry)?;
    }
    if long_name.is_some() {
        return Err(invalid("GNU filename metadata has no following member"));
    }
    let mut decoder = archive.into_inner();
    let mut tail = [0; 8192];
    let mut padding = 0;
    loop {
        let count = decoder.read(&mut tail)?;
        if count == 0 {
            break;
        }
        padding += count;
        if padding > 1024 || tail[..count].iter().any(|byte| *byte != 0) {
            return Err(invalid("unexpected data after the tar terminator"));
        }
    }
    let mut input = decoder.into_inner();
    if input.read(&mut [0])? != 0 {
        return Err(invalid("unexpected data after the gzip member"));
    }
    Ok(())
}

fn report(
    operation: ArchiveOperation,
    path: &Path,
    destination: Option<&Path>,
    manifest: &ArchiveManifest,
    snapshot: &EvidenceSnapshot,
) -> Result<ArchiveReport, ArchiveError> {
    Ok(ArchiveReport {
        command: "archive".to_owned(),
        schema_version: RESULT_ARCHIVE_VERSION,
        operation,
        archive_path: path.to_string_lossy().into_owned(),
        destination: destination.map(|p| p.to_string_lossy().into_owned()),
        manifest_sha256: digest(&canonical_bytes(manifest)?),
        files: manifest.files.len() as u64,
        size_bytes: manifest.files.iter().map(|f| f.size_bytes).sum(),
        records: snapshot.tables.values().map(|rows| rows.len() as u64).sum(),
    })
}

fn read_document<T: DeserializeOwned>(
    path: &Path,
    schema: PublicSchema,
    limits: ArchiveLimits,
) -> Result<T, ArchiveError> {
    let file = regular_file(path)?;
    if file.metadata()?.len() > limits.max_expanded_bytes {
        return Err(ArchiveError::Limit);
    }
    let mut bytes = Vec::new();
    file.take(limits.max_expanded_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limits.max_expanded_bytes {
        return Err(ArchiveError::Limit);
    }
    let value = validate_document(&bytes, schema)?;
    Ok(serde_json::from_value(value.as_json().clone())?)
}

fn validate_document(bytes: &[u8], schema: PublicSchema) -> Result<CanonicalValue, ArchiveError> {
    let value = CanonicalValue::from_slice(bytes)?;
    if value
        .as_json()
        .get("schema_version")
        .and_then(|v| v.as_u64())
        != Some(RESULT_ARCHIVE_VERSION.into())
        && matches!(
            schema,
            PublicSchema::ArchiveManifest | PublicSchema::ArchiveEvidence
        )
    {
        return Err(ArchiveError::UnsupportedVersion);
    }
    if matches!(
        schema,
        PublicSchema::ArchiveManifest | PublicSchema::ArchiveEvidence
    ) && value.to_canonical_bytes()? != bytes
    {
        return Err(invalid("archive metadata must use canonical JSON bytes"));
    }
    public_schema_catalog()
        .map_err(evidence_error)?
        .validate(schema.repository_path(), value.as_json())
        .map_err(evidence_error)?;
    Ok(value)
}

const UNLIMITED: ArchiveLimits = ArchiveLimits {
    max_entries: u64::MAX,
    max_expanded_bytes: u64::MAX,
};

fn walk_files(root: &Path) -> Result<BTreeMap<String, ArchiveFile>, ArchiveError> {
    walk_files_limited(root, UNLIMITED)
}

fn walk_files_limited(
    root: &Path,
    limits: ArchiveLimits,
) -> Result<BTreeMap<String, ArchiveFile>, ArchiveError> {
    fn walk(
        root: &Path,
        relative: &str,
        output: &mut BTreeMap<String, ArchiveFile>,
        total: &mut u64,
        limits: ArchiveLimits,
    ) -> Result<(), ArchiveError> {
        let directory = root.join(relative);
        real_directory(&directory)?;
        platform_fs::validate_directory_names(&directory)?;
        let mut entries = 0;
        for entry in fs::read_dir(&directory)? {
            entries += 1;
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("archive paths must be UTF-8"))?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            safe_name(&path)?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if platform_fs::is_real_directory(&metadata) {
                walk(root, &path, output, total, limits)?;
            } else {
                let mut count = output.len() as u64;
                let file = inventory_file(&entry.path(), path.clone(), &mut count, total, limits)?;
                output.insert(path, file);
            }
        }
        if entries == 0 && !relative.is_empty() {
            return Err(invalid("empty artifact directories are unsupported"));
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(root, "", &mut files, &mut 0, limits)?;
    Ok(files)
}

fn inventory_file(
    path: &Path,
    name: String,
    count: &mut u64,
    total: &mut u64,
    limits: ArchiveLimits,
) -> Result<ArchiveFile, ArchiveError> {
    let file = regular_file(path)?;
    let metadata = file.metadata()?;
    let expected_size = metadata.len();
    charge(count, total, expected_size, limits)?;
    let (digest, size_bytes) = hash_reader(file.take(expected_size.saturating_add(1)))?;
    if size_bytes != expected_size {
        return Err(invalid("archive file changed while it was read"));
    }
    Ok(ArchiveFile {
        path: name,
        sha256: hex(digest.bytes()),
        size_bytes,
        executable: platform_fs::is_executable(&metadata),
    })
}

fn metadata_member(name: &str, bytes: &[u8]) -> ArchiveFile {
    ArchiveFile {
        path: name.to_owned(),
        sha256: digest(bytes),
        size_bytes: bytes.len() as u64,
        executable: false,
    }
}

fn check_member(file: &ArchiveFile, size_bytes: u64, sha256: &str) -> Result<(), ArchiveError> {
    if file.size_bytes != size_bytes || file.sha256 != sha256 {
        return Err(invalid(
            "archive file does not match its recorded hash or size",
        ));
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<File, ArchiveError> {
    if !platform_fs::is_regular_file(&fs::symlink_metadata(path)?) {
        return Err(invalid(
            "archive payload must be a regular file without links",
        ));
    }
    Ok(platform_fs::open_regular(
        path,
        OpenOptions::new().read(true),
    )?)
}

fn real_directory(path: &Path) -> Result<(), ArchiveError> {
    if !platform_fs::is_real_directory(&fs::symlink_metadata(path)?) {
        return Err(invalid("archive root and parents must be real directories"));
    }
    Ok(())
}

fn safe_name(name: &str) -> Result<(), ArchiveError> {
    if name.is_empty()
        || name.contains(['\\', ':', '\0'])
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid("unsafe or nonnormalized archive path"));
    }
    Ok(())
}

fn contained(root: &Path, name: &str) -> Result<PathBuf, ArchiveError> {
    safe_name(name)?;
    let mut path = root.to_owned();
    for part in name.split('/') {
        path.push(part);
        let metadata = fs::symlink_metadata(&path)?;
        if !platform_fs::is_real_directory(&metadata) && !platform_fs::is_regular_file(&metadata) {
            return Err(invalid("artifact path traverses a link or special entry"));
        }
    }
    Ok(path)
}

fn new_destination(path: &Path) -> Result<PathBuf, ArchiveError> {
    let name = path
        .file_name()
        .ok_or_else(|| invalid("destination must name a new file or directory"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let target = fs::canonicalize(parent)?.join(name);
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "archive destination already exists",
            )
            .into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(target)
}

fn private_stage(parent: Option<&Path>) -> Result<TempDir, ArchiveError> {
    let mut builder = Builder::new();
    builder.prefix(".metewand-archive-");
    let stage = if let Some(parent) = parent {
        builder.tempdir_in(parent)?
    } else {
        builder.tempdir()?
    };
    platform_fs::set_private_directory(stage.path())?;
    Ok(stage)
}

fn charge(
    count: &mut u64,
    total: &mut u64,
    size: u64,
    limits: ArchiveLimits,
) -> Result<(), ArchiveError> {
    *count = count.checked_add(1).ok_or(ArchiveError::Limit)?;
    *total = total.checked_add(size).ok_or(ArchiveError::Limit)?;
    if *count > limits.max_entries || *total > limits.max_expanded_bytes {
        return Err(ArchiveError::Limit);
    }
    Ok(())
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, ArchiveError> {
    Ok(CanonicalValue::try_from(serde_json::to_value(value)?)?.to_canonical_bytes()?)
}
fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn invalid(message: impl Into<String>) -> ArchiveError {
    ArchiveError::Invalid(message.into())
}
fn evidence_error(error: impl std::fmt::Display) -> ArchiveError {
    invalid(error.to_string())
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("unsupported result archive compatibility version")]
    UnsupportedVersion,
    #[error(
        "archive contains an unfinished attempt; run `metewand recover --output-dir PATH` before archiving"
    )]
    Unsettled,
    #[error("archive exceeds --max-entries or --max-bytes")]
    Limit,
    #[error("invalid result archive evidence: {0}")]
    Invalid(String),
    #[error("archive I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

fn validate_evidence(evidence: &ArchiveEvidence) -> Result<(), ArchiveError> {
    evidence.validate().map_err(|error| match error {
        crate::core::archive::ArchiveEvidenceError::UnsupportedVersion => {
            ArchiveError::UnsupportedVersion
        }
        crate::core::archive::ArchiveEvidenceError::Unsettled => ArchiveError::Unsettled,
        error => evidence_error(error),
    })
}
