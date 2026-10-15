//! Durable, observation-first publication for the Gate-1 local execution path.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use metewand_core::{
    EXECUTION_SEMANTICS_VERSION, PUBLIC_SCHEMA_VERSION,
    canonical::{CanonicalJsonError, CanonicalValue},
    identity::{IdentityError, identify_canonical, identify_record, record_id},
    manifest::{PrimaryTime, RepositoryPath},
    public_schemas::{PublicSchema, PublicSchemaCatalogError, public_schema_catalog},
    records::{
        ArtifactReference, AttemptDiagnostic, CompletionStatus, ContentDigest, IdentifiedRecord,
        MetricsArtifact, ObservationTimingRecord, ProvenanceRecord, RecordId,
        ResolvedAttemptSlotRecord, ResolvedObservationSlotRecord, ResultArtifact,
        RunAttemptOutcome, RunAttemptRecord, RunObservationRecord,
    },
    run_documents::{RunDocumentError, attempt_document, observation_document},
    schema::SchemaValidationError,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};
use thiserror::Error;

use crate::{
    LOCAL_TREE_HASH_VERSION,
    fixed_dataset_run::{FixedDatasetResult, hex_digest},
    local_tree::{LocalTreeHashError, hash_local_tree},
    worker_process::{CapturedWorkerLog, WorkerProcessOutput},
};

/// Resolved identities and contextual provenance for one durable publication.
#[derive(Debug)]
pub struct PublicationRequest<'a> {
    /// Existing absolute directory that will own `observations` and `attempts`.
    pub root: &'a Path,
    /// Resolved measured attempt slot selected by the caller.
    pub attempt_slot: RecordId<ResolvedAttemptSlotRecord>,
    /// Resolved observation slot selected by the caller for the same run.
    pub observation_slot: RecordId<ResolvedObservationSlotRecord>,
    /// Zero-based retry index reserved for this attempt.
    pub retry_index: u64,
    /// Caller-observed repository, build, environment, and host context.
    ///
    /// Must be an object. Unavailable optional evidence should be explicit nulls.
    /// Runtime-owned launch and log evidence is attached separately.
    pub provenance_data: CanonicalValue,
}

/// Independently published observation and terminal attempt.
#[derive(Debug)]
pub struct PublishedOneShot {
    /// Complete typed observation, with its canonical identity.
    pub observation: IdentifiedRecord<RunObservationRecord>,
    /// Complete typed attempt, returned only after durable publication.
    pub attempt: IdentifiedRecord<RunAttemptRecord>,
    /// Immutable directory containing the observation document and artifacts.
    pub observation_directory: PathBuf,
    /// Immutable directory containing the attempt document and scoped logs.
    pub attempt_directory: PathBuf,
}

/// Publishes a technical failure that produced no valid observation.
///
/// The caller supplies the resolved slot, retry identity, diagnostic, and
/// observed provenance. This uses the same hash-complete, no-replace publication
/// boundary as successful attempts, without manufacturing an observation.
///
/// # Errors
///
/// Rejects a non-failure record, an observation reference, an invalid root,
/// conflicting existing contents, or a publication failure.
pub fn finalize_failed_attempt(
    root: &Path,
    record: RunAttemptRecord,
) -> Result<(IdentifiedRecord<RunAttemptRecord>, PathBuf), PublicationError> {
    if !root.is_absolute() || !fs::symlink_metadata(root)?.is_dir() {
        return Err(PublicationError::InvalidRoot);
    }
    if !matches!(
        record.outcome,
        RunAttemptOutcome::Failed {
            observation: None,
            ..
        }
    ) {
        return Err(PublicationError::InvalidFailedAttempt);
    }
    let attempt = identify_record(record)?;
    let attempts = durable_child(root, "attempts")?;
    let stage = staging(&attempts)?;
    let document = attempt_document(&attempt.record)?;
    write_document(
        stage.path(),
        "attempt.json",
        PublicSchema::Attempt,
        &document,
    )?;
    let directory = publish(stage, &attempts.join(attempt.id.to_string()))?;
    Ok((attempt, directory))
}

/// Publishes a validated one-shot observation before its terminal attempt.
///
/// Publication uses private staging directories on the destination filesystem,
/// synchronizes all contents, writes the completion manifest last, and uses an
/// atomic no-replace rename. Existing identical objects are verified byte for
/// byte. An attempt publication error may leave a complete orphan observation;
/// callers must not infer acceptance from that observation alone.
///
/// # Errors
///
/// Rejects modified private outputs, unsafe entries, invalid public documents,
/// conflicting existing objects, unsupported atomic publication, and I/O errors.
pub fn finalize_fixed_dataset(
    result: &FixedDatasetResult,
    request: PublicationRequest<'_>,
) -> Result<PublishedOneShot, PublicationError> {
    if !request.root.is_absolute() || !fs::symlink_metadata(request.root)?.is_dir() {
        return Err(PublicationError::InvalidRoot);
    }
    if !request.provenance_data.as_json().is_object() {
        return Err(PublicationError::InvalidProvenance);
    }
    let provenance = ProvenanceRecord {
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        wire_protocol_version: metewand_protocol::WIRE_PROTOCOL_VERSION,
        execution_semantics_version: EXECUTION_SEMANTICS_VERSION,
        data: CanonicalValue::try_from(json!({
            "context": request.provenance_data.as_json(),
            "execution": result.execution_provenance,
            "result_manifest": Path::new("result").join(result.validated_result().manifest_path()),
            "implementation_statistics": result.statistics,
            "logs": {
                "implementation": log_metadata(&result.implementation_output),
                "problem-evaluator": log_metadata(&result.evaluator_output),
            },
        }))?,
    };
    let mut attempt = RunAttemptRecord {
        slot: request.attempt_slot,
        retry_index: request.retry_index,
        outcome: RunAttemptOutcome::Failed {
            diagnostic: AttemptDiagnostic {
                code: "publication_incomplete".to_owned(),
                message: "Publication has not completed.".to_owned(),
                details: None,
            },
            observation: None,
        },
        started_at: result.started_at,
        finished_at: result.finished_at,
        provenance: provenance.clone(),
    };
    // The attempt ID binds only the slot and retry, so it is available before
    // the observation without creating an identity cycle.
    let attempt_id = record_id(&attempt)?;
    let observations = durable_child(request.root, "observations")?;
    let observation_stage = staging(&observations)?;
    let result_copy = observation_stage.path().join("result");
    copy_result(result, &result_copy)?;
    let metrics_path = result.metrics_document();
    if !fs::symlink_metadata(&metrics_path)?.is_file()
        || !fs::symlink_metadata(
            metrics_path
                .parent()
                .expect("private metrics have a parent"),
        )?
        .is_dir()
    {
        return Err(PublicationError::UnsafeEntry(metrics_path));
    }
    if fs::read(&metrics_path)? != result.metrics_bytes {
        return Err(PublicationError::ModifiedOutput);
    }
    write_file(
        &observation_stage.path().join("metrics.json"),
        &result.metrics_bytes,
    )?;

    let result_digest = hash_local_tree(&result_copy)?;
    let metrics_digest = sha256(&result.metrics_bytes);
    let observation = identify_record(RunObservationRecord {
        slot: request.observation_slot,
        producing_attempt: attempt_id,
        result: ArtifactReference {
            id: result_artifact_id(result_digest)?,
            path: serde_json::from_value(json!("result")).expect("static normalized path"),
            sha256: result_digest,
        },
        metrics: ArtifactReference {
            id: metrics_artifact_id(metrics_digest)?,
            path: serde_json::from_value(json!("metrics.json")).expect("static normalized path"),
            sha256: metrics_digest,
        },
        timing: ObservationTimingRecord {
            primary_time: PrimaryTime::TimedWallTime,
            timed_wall_time: result.timed_wall_time,
            implementation_time: result.implementation_time_ns.map(Duration::from_nanos),
            evaluation_time: result.evaluation_time,
            cpu_time: None,
            peak_rss_bytes: None,
        },
        completion: result.completion,
        provenance,
    })?;
    let document = observation_document(&observation.record)?;
    write_document(
        observation_stage.path(),
        "observation.json",
        PublicSchema::Observation,
        &document,
    )?;
    let observation_directory = publish(
        observation_stage,
        &observations.join(observation.id.to_string()),
    )?;

    attempt.outcome = if result.completion == CompletionStatus::NotReached {
        RunAttemptOutcome::Failed {
            diagnostic: AttemptDiagnostic {
                code: "completion_target_not_reached".to_owned(),
                message: "The valid result did not reach the problem's completion target."
                    .to_owned(),
                details: None,
            },
            observation: Some(observation.id),
        }
    } else {
        RunAttemptOutcome::Accepted {
            observation: observation.id,
        }
    };
    attempt.finished_at = SystemTime::now();
    let attempt = identify_record(attempt)?;
    let attempts = durable_child(request.root, "attempts")?;
    let attempt_stage = staging(&attempts)?;
    write_logs(
        attempt_stage.path(),
        "implementation",
        &result.implementation_output,
    )?;
    write_logs(
        attempt_stage.path(),
        "problem-evaluator",
        &result.evaluator_output,
    )?;
    let document = attempt_document(&attempt.record)?;
    write_document(
        attempt_stage.path(),
        "attempt.json",
        PublicSchema::Attempt,
        &document,
    )?;
    let attempt_directory = publish(attempt_stage, &attempts.join(attempt.id.to_string()))?;
    Ok(PublishedOneShot {
        observation,
        attempt,
        observation_directory,
        attempt_directory,
    })
}

/// Verifies the completion marker and the exact hash and size inventory.
///
/// This rejects missing or undeclared files, duplicate or unsorted paths,
/// symbolic links, special files, and superfluous empty directories. It does
/// not by itself prove scientific validity or the existence of an enclosing
/// attempt or metadata database reference.
///
/// # Errors
///
/// Returns an error for an incomplete, unsafe, corrupted, or unreadable object.
pub fn verify_finalized_directory(directory: &Path) -> Result<(), PublicationError> {
    let marker = read_marker(directory)?;
    let mut actual = inventory(directory)?;
    actual.remove("manifest.json");
    if marker.files != actual.into_values().collect::<Vec<_>>() {
        return Err(PublicationError::InventoryMismatch(directory.to_path_buf()));
    }
    Ok(())
}

fn copy_result(result: &FixedDatasetResult, destination: &Path) -> Result<(), PublicationError> {
    if hash_local_tree(result.result_directory())? != result.result_tree_hash {
        return Err(PublicationError::ModifiedOutput);
    }
    let files = inventory(result.result_directory())?;
    fs::create_dir(destination)?;
    for entry in files.values() {
        let target = destination.join(&entry.path);
        fs::create_dir_all(target.parent().expect("a destination file has a parent"))?;
        fs::copy(result.result_directory().join(&entry.path), target)?;
    }
    // Validate the copy rather than trusting a pre-copy hash if the source
    // changed while files were being copied.
    if hash_local_tree(destination)? != result.result_tree_hash {
        return Err(PublicationError::ModifiedOutput);
    }
    Ok(())
}

fn log_metadata(output: &WorkerProcessOutput) -> serde_json::Value {
    fn stream(log: &CapturedWorkerLog) -> serde_json::Value {
        json!({"retained_bytes": log.bytes().len(), "total_bytes": log.total_bytes(), "truncated": log.truncated()})
    }
    json!({"stdout": stream(&output.stdout), "stderr": stream(&output.stderr)})
}

fn write_logs(
    root: &Path,
    role: &str,
    output: &WorkerProcessOutput,
) -> Result<(), PublicationError> {
    let directory = root.join("logs").join(role);
    fs::create_dir_all(&directory)?;
    write_file(&directory.join("stdout.log"), output.stdout.bytes())?;
    write_file(&directory.join("stderr.log"), output.stderr.bytes())?;
    Ok(())
}

fn durable_child(parent: &Path, name: &str) -> Result<PathBuf, PublicationError> {
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => File::open(parent)?.sync_all()?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&path)?.is_dir() {
                return Err(PublicationError::UnsafeEntry(path));
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

fn staging(parent: &Path) -> Result<TempDir, PublicationError> {
    Ok(Builder::new().prefix(".staging-").tempdir_in(parent)?)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), PublicationError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_document(
    root: &Path,
    name: &str,
    schema: PublicSchema,
    document: &CanonicalValue,
) -> Result<(), PublicationError> {
    public_schema_catalog()?.validate(schema.repository_path(), document.as_json())?;
    write_file(&root.join(name), &document.to_canonical_bytes()?)
}

fn publish(stage: TempDir, destination: &Path) -> Result<PathBuf, PublicationError> {
    let files = inventory(stage.path())?;
    for entry in files.values() {
        File::open(stage.path().join(&entry.path))?.sync_all()?;
    }
    sync_directories(stage.path())?;
    let marker = CanonicalValue::try_from(json!({
        "schema_version": PUBLIC_SCHEMA_VERSION,
        "complete": true,
        "files": files.into_values().collect::<Vec<_>>(),
    }))?;
    write_document(
        stage.path(),
        "manifest.json",
        PublicSchema::ArtifactManifest,
        &marker,
    )?;
    File::open(stage.path())?.sync_all()?;
    verify_finalized_directory(stage.path())?;
    match rename_no_replace(stage.path(), destination) {
        Ok(()) => {
            // Disarm TempDir only after rename succeeds. No incomplete staging
            // directory can be mistaken for a published identity.
            let _ = stage.keep();
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            verify_finalized_directory(destination)?;
            if !same_contents(stage.path(), destination)? {
                return Err(PublicationError::Conflict(destination.to_path_buf()));
            }
        }
        Err(error) => return Err(error.into()),
    }
    File::open(
        destination
            .parent()
            .expect("published directories have a parent"),
    )?
    .sync_all()?;
    Ok(destination.to_path_buf())
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "redox"
))]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(Into::into)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "redox"
)))]
fn rename_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace directory publication is unavailable",
    ))
}

fn sync_directories(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directories(&entry.path())?;
        }
    }
    File::open(root)?.sync_all()
}

#[derive(Debug, Deserialize)]
struct Marker {
    files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FileEntry {
    path: String,
    sha256: String,
    size_bytes: u64,
    media_type: String,
}

fn read_marker(directory: &Path) -> Result<Marker, PublicationError> {
    if !fs::symlink_metadata(directory)?.is_dir()
        || !fs::symlink_metadata(directory.join("manifest.json"))?.is_file()
    {
        return Err(PublicationError::UnsafeEntry(directory.to_path_buf()));
    }
    let document = CanonicalValue::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    public_schema_catalog()?.validate(
        PublicSchema::ArtifactManifest.repository_path(),
        document.as_json(),
    )?;
    Ok(serde_json::from_value(document.as_json().clone())?)
}

fn inventory(root: &Path) -> Result<BTreeMap<String, FileEntry>, PublicationError> {
    fn walk(
        root: &Path,
        relative: &Path,
        files: &mut BTreeMap<String, FileEntry>,
    ) -> Result<(), PublicationError> {
        let mut count = 0;
        for entry in fs::read_dir(root.join(relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            let text = path
                .to_str()
                .ok_or_else(|| PublicationError::UnsafeEntry(path.clone()))?;
            serde_json::from_value::<RepositoryPath>(json!(text))
                .map_err(|_| PublicationError::UnsafeEntry(path.clone()))?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(root, &path, files)?;
            } else if kind.is_file() {
                let mut file = File::open(entry.path())?;
                let mut hasher = Sha256::new();
                let size_bytes = io::copy(&mut file, &mut hasher)?;
                files.insert(
                    text.to_owned(),
                    FileEntry {
                        path: text.to_owned(),
                        sha256: hex_digest(ContentDigest::new(hasher.finalize().into())),
                        size_bytes,
                        media_type: if text.ends_with(".json") {
                            "application/json"
                        } else {
                            "application/octet-stream"
                        }
                        .to_owned(),
                    },
                );
            } else {
                return Err(PublicationError::UnsafeEntry(path));
            }
            count += 1;
        }
        if count == 0 && !relative.as_os_str().is_empty() {
            return Err(PublicationError::UnsafeEntry(relative.to_path_buf()));
        }
        Ok(())
    }
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(PublicationError::UnsafeEntry(root.to_path_buf()));
    }
    let mut files = BTreeMap::new();
    walk(root, Path::new(""), &mut files)?;
    Ok(files)
}

fn same_contents(left: &Path, right: &Path) -> Result<bool, PublicationError> {
    let left_files = inventory(left)?;
    if left_files != inventory(right)? {
        return Ok(false);
    }
    for path in left_files.keys() {
        let mut a = File::open(left.join(path))?;
        let mut b = File::open(right.join(path))?;
        let mut a_buffer = [0; 8192];
        let mut b_buffer = [0; 8192];
        loop {
            let n = a.read(&mut a_buffer)?;
            b.read_exact(&mut b_buffer[..n])?;
            if a_buffer[..n] != b_buffer[..n] {
                return Ok(false);
            }
            if n == 0 {
                break;
            }
        }
    }
    Ok(true)
}

fn sha256(bytes: &[u8]) -> ContentDigest {
    ContentDigest::new(Sha256::digest(bytes).into())
}

fn result_artifact_id(digest: ContentDigest) -> Result<RecordId<ResultArtifact>, PublicationError> {
    Ok(identify_canonical(&CanonicalValue::try_from(json!({
        "tree_hash_version": LOCAL_TREE_HASH_VERSION,
        "tree_sha256": hex_digest(digest),
    }))?)?)
}

fn metrics_artifact_id(
    digest: ContentDigest,
) -> Result<RecordId<MetricsArtifact>, PublicationError> {
    Ok(identify_canonical(&CanonicalValue::try_from(json!({
        "sha256": hex_digest(digest),
    }))?)?)
}

/// A durable publication or verification failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PublicationError {
    /// The store root must already be a real absolute directory.
    #[error("publication root must be an existing absolute directory")]
    InvalidRoot,
    /// Contextual provenance must be a JSON object.
    #[error("publication provenance must be an object")]
    InvalidProvenance,
    /// The failure publisher requires a failed attempt without an observation.
    #[error("failed-attempt publication requires a failure without an observation")]
    InvalidFailedAttempt,
    /// Private output no longer matches its validation snapshot.
    #[error("validated worker output changed before publication")]
    ModifiedOutput,
    /// A path is nonnormalized or has an unsupported filesystem type.
    #[error("unsafe publication entry: {}", .0.display())]
    UnsafeEntry(PathBuf),
    /// The completion inventory no longer describes the directory.
    #[error("completion inventory mismatch: {}", .0.display())]
    InventoryMismatch(PathBuf),
    /// A finalized identity already has different bytes.
    #[error("conflicting finalized object: {}", .0.display())]
    Conflict(PathBuf),
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A canonical document could not be represented.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// A typed identity could not be assigned.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// A local tree could not be hashed.
    #[error(transparent)]
    Tree(#[from] LocalTreeHashError),
    /// A record could not be rendered as a public document.
    #[error(transparent)]
    Document(#[from] RunDocumentError),
    /// The embedded public schema catalog could not be loaded.
    #[error(transparent)]
    PublicSchemas(#[from] PublicSchemaCatalogError),
    /// A public document did not satisfy its schema.
    #[error(transparent)]
    Schema(#[from] SchemaValidationError),
    /// A validated manifest could not be decoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::symlink,
        sync::{Arc, Barrier},
        thread,
    };

    fn object(parent: &Path, bytes: &[u8]) -> TempDir {
        let stage = staging(parent).unwrap();
        write_file(&stage.path().join("payload.bin"), bytes).unwrap();
        stage
    }

    #[test]
    fn artifact_resource_identities_match_version_one_vectors() {
        let digest = ContentDigest::new([0; 32]);
        assert_eq!(
            result_artifact_id(digest).unwrap().to_string(),
            "mw1-artifact-3a7292181f2ee264e9e0ed459026af829d78599d20b006f4a69c500939ee2b5d"
        );
        assert_eq!(
            metrics_artifact_id(digest).unwrap().to_string(),
            "mw1-artifact-037b355951f6be41c4031a064a3ade9d39d5ca40d1a57bd3fe62eb479d6e56f9"
        );
    }

    #[test]
    fn completion_manifest_has_exact_canonical_bytes() {
        let root = tempfile::tempdir().unwrap();
        let stage = object(root.path(), b"abc");
        assert!(!stage.path().join("manifest.json").exists());
        let directory = publish(stage, &root.path().join("object")).unwrap();
        assert_eq!(fs::read(directory.join("manifest.json")).unwrap(),
            br#"{"complete":true,"files":[{"media_type":"application/octet-stream","path":"payload.bin","sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","size_bytes":3}],"schema_version":1}"#);
    }

    #[test]
    fn duplicate_publication_verifies_bytes_and_conflicts_never_replace() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("object");
        publish(object(root.path(), b"abc"), &destination).unwrap();
        publish(object(root.path(), b"abc"), &destination).unwrap();
        assert!(matches!(
            publish(object(root.path(), b"different"), &destination),
            Err(PublicationError::Conflict(_))
        ));
        assert_eq!(fs::read(destination.join("payload.bin")).unwrap(), b"abc");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);

        let empty = root.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(publish(object(root.path(), b"abc"), &empty).is_err());
        assert_eq!(fs::read_dir(empty).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_identical_publishers_share_only_a_complete_object() {
        let root = tempfile::tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let stage = object(root.path(), b"abc");
                let destination = root.path().join("object");
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    publish(stage, &destination)
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            verify_finalized_directory(&handle.join().unwrap().unwrap()).unwrap();
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn corrupted_or_incomplete_objects_never_verify() {
        for corruption in [
            "changed",
            "missing",
            "undeclared",
            "symlink",
            "empty_directory",
            "incomplete",
            "duplicate",
            "traversal",
        ] {
            let root = tempfile::tempdir().unwrap();
            let directory =
                publish(object(root.path(), b"abc"), &root.path().join("object")).unwrap();
            match corruption {
                "changed" => fs::write(directory.join("payload.bin"), b"abd").unwrap(),
                "missing" => fs::remove_file(directory.join("payload.bin")).unwrap(),
                "undeclared" => fs::write(directory.join("extra"), b"extra").unwrap(),
                "symlink" => {
                    fs::remove_file(directory.join("payload.bin")).unwrap();
                    symlink("../object/manifest.json", directory.join("payload.bin")).unwrap();
                }
                "empty_directory" => fs::create_dir(directory.join("extra")).unwrap(),
                "incomplete" => fs::remove_file(directory.join("manifest.json")).unwrap(),
                "duplicate" | "traversal" => {
                    let mut marker: serde_json::Value =
                        serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap())
                            .unwrap();
                    if corruption == "duplicate" {
                        let duplicate = marker["files"][0].clone();
                        marker["files"].as_array_mut().unwrap().push(duplicate);
                    } else {
                        marker["files"][0]["path"] = json!("../payload.bin");
                    }
                    fs::write(
                        directory.join("manifest.json"),
                        serde_json::to_vec(&marker).unwrap(),
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            assert!(
                verify_finalized_directory(&directory).is_err(),
                "{corruption}"
            );
        }
    }

    #[test]
    fn symbolic_links_and_special_files_cannot_enter_staging() {
        let root = tempfile::tempdir().unwrap();
        let stage = object(root.path(), b"abc");
        symlink("payload.bin", stage.path().join("link")).unwrap();
        assert!(publish(stage, &root.path().join("object")).is_err());
        let stage = object(root.path(), b"abc");
        std::os::unix::net::UnixListener::bind(stage.path().join("socket")).unwrap();
        assert!(publish(stage, &root.path().join("object")).is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
