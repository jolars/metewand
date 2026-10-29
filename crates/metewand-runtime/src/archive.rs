//! Portable result evidence with offline verification and atomic publication.

#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
};

use flate2::{Compression, GzBuilder, bufread::GzDecoder};
use metewand_core::{
    archive::{
        ArchiveEvidence, ArchiveFile, ArchiveManifest, ArchiveOperation, ArchiveReport,
        RESULT_ARCHIVE_VERSION,
    },
    canonical::{CanonicalJsonError, CanonicalValue},
    public_schemas::{PublicSchema, public_schema_catalog},
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};
use thiserror::Error;

use crate::{
    evidence::EvidenceSnapshot, export::ResultSet, platform_fs, publication::rename_no_replace,
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
    ResultSet::from_snapshot(&snapshot).map_err(evidence_error)?;
    let parent = destination.parent().expect("destination parent exists");
    let stage = private_stage(Some(parent))?;
    write_new(
        &stage.path().join("evidence.json"),
        &canonical_bytes(&evidence)?,
    )?;
    let mut executable = BTreeMap::new();
    for artifact in &snapshot.tables["stored-artifact"] {
        let relative = artifact["path"]
            .as_str()
            .ok_or_else(|| invalid("artifact path is missing"))?;
        let source = contained(&snapshot.data_root, relative)?;
        let paths = match artifact["storage_kind"].as_str() {
            Some("directory") => walk_files(&source)?
                .into_keys()
                .map(|path| format!("{relative}/{path}"))
                .collect::<Vec<_>>(),
            Some("file") => vec![relative.to_owned()],
            _ => return Err(invalid("unsupported artifact storage kind")),
        };
        for path in paths {
            let source = contained(&snapshot.data_root, &path)?;
            let name = format!("data/{path}");
            let target = stage.path().join(&name);
            fs::create_dir_all(target.parent().expect("payload parent"))?;
            let mode = snapshot
                .files
                .as_ref()
                .and_then(|files| files.get(&name))
                .map_or_else(
                    || fs::symlink_metadata(&source).map(|m| platform_fs::is_executable(&m)),
                    |file| Ok(file.executable),
                )?;
            if executable.insert(name, mode).is_some() {
                return Err(invalid("registered artifact paths overlap"));
            }
            let mut input = regular_file(&source)?;
            let mut output = File::create_new(target)?;
            io::copy(&mut input, &mut output)?;
        }
    }
    let mut files = walk_files(stage.path())?;
    for (path, file) in &mut files {
        file.executable = executable.get(path).copied().unwrap_or(false);
    }
    let manifest = ArchiveManifest {
        schema_version: RESULT_ARCHIVE_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        files: files.into_values().collect(),
    };
    write_new(
        &stage.path().join("archive.json"),
        &canonical_bytes(&manifest)?,
    )?;
    let limits = ArchiveLimits {
        max_entries: u64::MAX,
        max_expanded_bytes: u64::MAX,
    };
    let verified = collection_snapshot(stage.path(), limits)?;
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    {
        let encoder = GzBuilder::new()
            .mtime(0)
            .write(output.as_file_mut(), Compression::new(6));
        let mut tar = tar::Builder::new(encoder);
        for name in std::iter::once("archive.json")
            .chain(manifest.files.iter().map(|file| file.path.as_str()))
        {
            let mut file = regular_file(&stage.path().join(name))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(file.metadata()?.len());
            header.set_mode(0o600);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            tar.append_data(&mut header, name, &mut file)?;
        }
        tar.into_inner()?.finish()?;
    }
    platform_fs::sync_owned_file(output.path())?;
    rename_no_replace(output.path(), &destination)?;
    platform_fs::complete_namespace_change(parent)?;
    report(
        ArchiveOperation::Create,
        &destination,
        Some(&destination),
        &manifest,
        &verified,
    )
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
    let manifest = read_document(
        &stage.path().join("archive.json"),
        PublicSchema::ArchiveManifest,
        limits,
    )?;
    for name in walk_files(stage.path())?.keys() {
        platform_fs::sync_owned_file(&stage.path().join(name))?;
    }
    crate::publication::sync_directories(stage.path()).map_err(evidence_error)?;
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
        if current.sha256 != file.sha256 || current.size_bytes != file.size_bytes {
            return Err(invalid(
                "archive file does not match its recorded hash or size",
            ));
        }
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
    ResultSet::from_snapshot(&snapshot).map_err(evidence_error)?;
    validate_documents(&snapshot, limits)?;
    Ok(snapshot)
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
        let matches = files
            .keys()
            .filter(|name| {
                if artifact["storage_kind"] == "file" {
                    **name == full
                } else {
                    name.starts_with(&format!("{full}/"))
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        if matches.is_empty() || matches.iter().any(|name| !required.insert(name.clone())) {
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

fn validate_documents(
    snapshot: &EvidenceSnapshot,
    limits: ArchiveLimits,
) -> Result<(), ArchiveError> {
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
                let kind = if schema == PublicSchema::Attempt {
                    "attempt"
                } else {
                    "observation"
                };
                if let Some(row) = snapshot.tables[kind]
                    .iter()
                    .find(|row| row["artifact_id"] == artifact["id"])
                {
                    if document["id"] != row["id"] || document["slot_id"] != row["slot_id"] {
                        return Err(invalid(
                            "artifact document disagrees with its registered subject",
                        ));
                    }
                    if kind == "observation" {
                        if document != row["definition"] {
                            return Err(invalid(
                                "observation artifact disagrees with its committed document",
                            ));
                        }
                        validate_scientific_payloads(snapshot, relative, &document)?;
                    }
                    if kind == "attempt"
                        && (document["retry_index"] != row["retry_index"]
                            || document["outcome"]["kind"] != row["state"])
                    {
                        return Err(invalid(
                            "attempt document disagrees with its terminal coordination",
                        ));
                    }
                    if kind == "attempt" {
                        let declared = document["observations"]
                            .as_array()
                            .expect("validated observation list")
                            .iter()
                            .map(|id| id.as_str().expect("validated observation identity"))
                            .collect::<BTreeSet<_>>();
                        let committed = snapshot.tables["observation"]
                            .iter()
                            .filter(|observation| observation["attempt_id"] == row["id"])
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
            let budget = snapshot.tables["fresh-budget"]
                .iter()
                .find(|budget| budget["attempt_id"] == outcome.as_json()["attempt_id"])
                .map_or(serde_json::Value::Null, |budget| {
                    serde_json::json!({
                        "reserved_ns": budget["reserved_ns"],
                        "charged_ns": budget["charged_ns"],
                    })
                });
            if budget != outcome.as_json()["coordination"]["fresh"]["budget"] {
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
        let target = destination.join(&name);
        fs::create_dir_all(target.parent().expect("member parent"))?;
        let mut file = File::create_new(target)?;
        if io::copy(&mut entry, &mut file)? != entry.size() {
            return Err(invalid("truncated archive member"));
        }
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
    let value = CanonicalValue::from_slice(&bytes)?;
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
    Ok(serde_json::from_value(value.as_json().clone())?)
}

fn walk_files(root: &Path) -> Result<BTreeMap<String, ArchiveFile>, ArchiveError> {
    walk_files_limited(
        root,
        ArchiveLimits {
            max_entries: u64::MAX,
            max_expanded_bytes: u64::MAX,
        },
    )
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
                let file = regular_file(&entry.path())?;
                let expected_size = file.metadata()?.len();
                let mut count = output.len() as u64;
                charge(&mut count, total, expected_size, limits)?;
                let mut hasher = Sha256::new();
                let size_bytes = io::copy(
                    &mut file.take(expected_size.saturating_add(1)),
                    &mut HashWriter(&mut hasher),
                )?;
                if size_bytes != expected_size {
                    return Err(invalid("archive file changed while it was read"));
                }
                output.insert(
                    path.clone(),
                    ArchiveFile {
                        path,
                        sha256: hex(&hasher.finalize()),
                        size_bytes,
                        executable: platform_fs::is_executable(&metadata),
                    },
                );
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

struct HashWriter<'a>(&'a mut Sha256);
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ArchiveError> {
    File::create_new(path)?.write_all(bytes)?;
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
    #[error("archive contains an unfinished attempt; explicitly recover the run before archiving")]
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
        metewand_core::archive::ArchiveEvidenceError::UnsupportedVersion => {
            ArchiveError::UnsupportedVersion
        }
        metewand_core::archive::ArchiveEvidenceError::Unsettled => ArchiveError::Unsettled,
        error => evidence_error(error),
    })
}
