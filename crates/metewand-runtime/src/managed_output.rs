//! Finalization of explicitly negotiated version-1 managed output drafts.

use crate::language_adapter::hex;
use metewand_core::canonical::CanonicalValue;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Instant,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    schema_version: u32,
    kind: String,
    data: Option<Value>,
    files: Vec<Artifact>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    path: String,
    media_type: String,
    #[serde(default)]
    representation: Option<Value>,
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
fn relative(path: &str) -> io::Result<PathBuf> {
    crate::worker_output::validate_relative_path(path).map_err(invalid)
}
fn check_deadline(deadline: Option<Instant>) -> io::Result<()> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "managed output deadline expired",
        ))
    } else {
        Ok(())
    }
}

fn document_bytes(value: Value, deadline: Option<Instant>) -> io::Result<Vec<u8>> {
    check_deadline(deadline)?;
    let value = CanonicalValue::try_from(value).map_err(invalid)?;
    check_deadline(deadline)?;
    let bytes = value.to_canonical_bytes().map_err(invalid)?;
    check_deadline(deadline)?;
    Ok(bytes)
}

struct PreparedOutput {
    root: PathBuf,
    draft_path: PathBuf,
    expected: &'static str,
    manifest: String,
    documents: Vec<(String, Vec<u8>)>,
}

struct PublishedDocuments(Vec<PathBuf>);

impl Drop for PublishedDocuments {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

impl PreparedOutput {
    fn publish(self, response: &mut Value, deadline: Option<Instant>) -> io::Result<()> {
        let mut staged = Vec::new();
        for (name, bytes) in self.documents {
            check_deadline(deadline)?;
            let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
            for chunk in bytes.chunks(65536) {
                check_deadline(deadline)?;
                file.write_all(chunk)?;
            }
            staged.push((file, self.root.join(name)));
        }
        let mut published = PublishedDocuments(Vec::new());
        for (file, path) in staged {
            check_deadline(deadline)?;
            file.persist_noclobber(&path).map_err(|error| error.error)?;
            published.0.push(path);
        }
        check_deadline(deadline)?;
        fs::remove_file(self.root.join(self.draft_path))?;
        check_deadline(deadline)?;
        response
            .as_object_mut()
            .expect("response object")
            .remove("draft");
        match self.expected {
            "dataset" => response["dataset"] = json!({"manifest":self.manifest}),
            "result" => response["result"] = json!({"manifest":self.manifest}),
            _ => {}
        }
        published.0.clear();
        Ok(())
    }
}

pub(crate) fn finalize(
    request: &Value,
    response: &mut Value,
    deadline: Option<Instant>,
) -> io::Result<()> {
    check_deadline(deadline)?;
    if response["ok"] != true || response.get("draft").is_none() {
        return Ok(());
    }
    let prepared = if let Some(deadline) = deadline {
        let request = request.clone();
        let response = response.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        // Library calls may outlive the deadline. Preparation stays read-only
        // so the exchange can stop waiting without allowing late publication.
        thread::Builder::new()
            .name("metewand-output".to_owned())
            .spawn(move || {
                let _ = sender.send(prepare(&request, &response, Some(deadline)));
            })?;
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result?,
            Err(RecvTimeoutError::Timeout) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "managed output deadline expired",
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("managed output preparation stopped"));
            }
        }
    } else {
        prepare(request, response, None)?
    };
    prepared.publish(response, deadline)
}

fn prepare(
    request: &Value,
    response: &Value,
    deadline: Option<Instant>,
) -> io::Result<PreparedOutput> {
    check_deadline(deadline)?;
    if response["id"] != request["id"] {
        return Err(invalid("managed output response ID does not match request"));
    }
    let method = request["method"]
        .as_str()
        .ok_or_else(|| invalid("missing operation"))?;
    let (root, expected, manifest) = match method {
        "execute" => (
            PathBuf::from(
                request["result_dir"]
                    .as_str()
                    .ok_or_else(|| invalid("missing result directory"))?,
            ),
            "result",
            "result.json".to_owned(),
        ),
        "materialize" => (
            PathBuf::from(
                request["output_dir"]
                    .as_str()
                    .ok_or_else(|| invalid("missing dataset directory"))?,
            ),
            "dataset",
            "dataset-manifest.json".to_owned(),
        ),
        "evaluate" => {
            let path = Path::new(
                request["metrics_path"]
                    .as_str()
                    .ok_or_else(|| invalid("missing metrics path"))?,
            );
            (
                path.parent()
                    .ok_or_else(|| invalid("missing metrics directory"))?
                    .to_owned(),
                "metrics",
                path.file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| invalid("invalid metrics filename"))?
                    .to_owned(),
            )
        }
        _ => return Err(invalid("operation cannot return a managed output draft")),
    };
    let draft_name = response["draft"]
        .as_str()
        .ok_or_else(|| invalid("invalid draft reference"))?;
    let draft_path = relative(draft_name)?;
    if !crate::platform_fs::is_real_directory(&fs::symlink_metadata(&root)?) {
        return Err(invalid("managed output root must be a real directory"));
    }
    let actual = crate::worker_output::output_files(&root)
        .map_err(invalid)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    check_deadline(deadline)?;
    let document = crate::worker_output::read_stable_file(&root, &draft_path).map_err(invalid)?;
    check_deadline(deadline)?;
    let value = CanonicalValue::from_slice(&document).map_err(invalid)?;
    check_deadline(deadline)?;
    use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
    public_schema_catalog()
        .map_err(invalid)?
        .validate(
            PublicSchema::ManagedOutputDraft.repository_path(),
            value.as_json(),
        )
        .map_err(invalid)?;
    check_deadline(deadline)?;
    let draft: Draft = serde_json::from_value(value.as_json().clone()).map_err(invalid)?;
    check_deadline(deadline)?;
    if draft.schema_version != 1 || draft.kind != expected {
        return Err(invalid("unsupported managed output draft version or kind"));
    }
    if expected == "metrics" && !draft.files.is_empty() {
        return Err(invalid("metrics cannot declare artifact files"));
    }
    let mut declared = BTreeSet::from([draft_path.clone()]);
    for file in &draft.files {
        check_deadline(deadline)?;
        let path = relative(&file.path)?;
        if file.media_type.is_empty() || path == Path::new(&manifest) || !declared.insert(path) {
            return Err(invalid(
                "duplicate, reserved, or invalid artifact declaration",
            ));
        }
    }
    if actual != declared {
        return Err(invalid(
            "managed output contains missing or undeclared files",
        ));
    }
    let mut files = Vec::new();
    for file in draft.files {
        check_deadline(deadline)?;
        let path = root.join(&file.path);
        let mut input = crate::platform_fs::open_regular(&path, OpenOptions::new().read(true))?;
        let signature = crate::platform_fs::file_signature(&input)?;
        let mut hash = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = [0; 65536];
        loop {
            check_deadline(deadline)?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
            size += count as u64;
        }
        if crate::platform_fs::file_signature(&input)? != signature {
            return Err(invalid("artifact changed during hashing"));
        }
        let mut entry = json!({"path":file.path,"media_type":file.media_type,"sha256":hex(hash.finalize()),"size_bytes":size});
        if let Some(representation) = file.representation {
            entry["representation"] = representation;
        }
        files.push(entry);
    }
    let mut documents = Vec::new();
    if expected == "dataset"
        && let Some(data) = draft.data
    {
        if !data.is_object() || declared.contains(Path::new("data.json")) {
            return Err(invalid(
                "dataset JSON data must be an object and cannot replace a declared file",
            ));
        }
        let bytes = document_bytes(data, deadline)?;
        files.push(json!({"path":"data.json","media_type":"application/json","sha256":hex(Sha256::digest(&bytes)),"size_bytes":bytes.len()}));
        documents.push(("data.json".to_owned(), bytes));
    } else if expected != "dataset" {
        let data = draft
            .data
            .ok_or_else(|| invalid("scientific data is required"))?;
        if !data.is_object() {
            return Err(invalid("scientific data must be an object"));
        }
        files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        let document = if expected == "metrics" {
            json!({"schema_version":1,"data":data})
        } else {
            json!({"schema_version":1,"data":data,"files":files})
        };
        documents.push((manifest.clone(), document_bytes(document, deadline)?));
    }
    if expected == "dataset" {
        files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        documents.push((
            manifest.clone(),
            document_bytes(
                json!({"schema_version":1,"complete":true,"files":files}),
                deadline,
            )?,
        ));
    }
    check_deadline(deadline)?;
    Ok(PreparedOutput {
        root,
        draft_path,
        expected,
        manifest,
        documents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_deadlines_prevent_finalization() {
        let root = tempfile::tempdir().unwrap();
        let error = finalize(
            &json!({"id":"1","method":"execute","result_dir":root.path()}),
            &mut json!({"id":"1","ok":true,"draft":"draft.json"}),
            Some(Instant::now()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    fn large_json_drafts_cannot_overrun_the_deadline_or_publish_output() {
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let mut draft = fs::File::create(root.path().join("draft.json")).unwrap();
        draft
            .write_all(br#"{"schema_version":1,"kind":"result","data":{"payload":""#)
            .unwrap();
        let chunk = [b'x'; 65536];
        for _ in 0..400 {
            draft.write_all(&chunk).unwrap();
        }
        draft.write_all(br#""},"files":[]}"#).unwrap();
        drop(draft);
        let mut response = json!({"id":"1","ok":true,"draft":"draft.json"});
        let original = response.clone();
        let started = Instant::now();
        let error = finalize(
            &json!({"id":"1","method":"execute","result_dir":root.path()}),
            &mut response,
            Some(started + Duration::from_millis(20)),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(response, original);
        assert!(root.path().join("draft.json").exists());
        assert!(!root.path().join("result.json").exists());
    }

    #[test]
    fn prepared_outputs_cannot_publish_after_expiration() {
        for (method, kind, manifest) in [
            ("execute", "result", "result.json"),
            ("materialize", "dataset", "dataset-manifest.json"),
            ("evaluate", "metrics", "metrics.json"),
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::write(
                root.path().join("draft.json"),
                json!({"schema_version":1,"kind":kind,"data":{"answer":1},"files":[]}).to_string(),
            )
            .unwrap();
            let request = json!({
                "id":"1", "method":method, "result_dir":root.path(),
                "output_dir":root.path(), "metrics_path":root.path().join(manifest),
            });
            let mut response = json!({"id":"1","ok":true,"draft":"draft.json"});
            let original = response.clone();
            let prepared = prepare(&request, &response, None).unwrap();
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
            let error = prepared
                .publish(&mut response, Some(Instant::now()))
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert_eq!(response, original);
            assert!(root.path().join("draft.json").exists());
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn failed_publication_preserves_existing_files_and_removes_new_documents() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("draft.json"),
            br#"{"schema_version":1,"kind":"dataset","data":{"answer":1},"files":[]}"#,
        )
        .unwrap();
        let request = json!({"id":"1","method":"materialize","output_dir":root.path()});
        let mut response = json!({"id":"1","ok":true,"draft":"draft.json"});
        let original = response.clone();
        let prepared = prepare(&request, &response, None).unwrap();
        let manifest = root.path().join("dataset-manifest.json");
        fs::write(&manifest, "existing manifest").unwrap();
        let error = prepared.publish(&mut response, None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&manifest).unwrap(), b"existing manifest");
        assert!(!root.path().join("data.json").exists());
        assert!(root.path().join("draft.json").exists());
        assert_eq!(response, original);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn result_and_metrics_finalization_preserve_canonical_bytes() {
        for (method, kind, manifest, expected) in [
            (
                "execute",
                "result",
                "result.json",
                r#"{"data":{"a":1,"z":0},"files":[],"schema_version":1}"#,
            ),
            (
                "evaluate",
                "metrics",
                "metrics.json",
                r#"{"data":{"a":1,"z":0},"schema_version":1}"#,
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::write(
                root.path().join("draft.json"),
                format!(
                    r#"{{"schema_version":1,"kind":"{kind}","data":{{"z":-0.0,"a":1}},"files":[]}}"#
                ),
            )
            .unwrap();
            let mut response = json!({"id":"1","ok":true,"draft":"draft.json"});
            finalize(
                &json!({"id":"1","method":method,"result_dir":root.path(),"metrics_path":root.path().join(manifest)}),
                &mut response,
                Some(Instant::now() + std::time::Duration::from_secs(5)),
            )
            .unwrap();
            assert_eq!(
                fs::read(root.path().join(manifest)).unwrap(),
                expected.as_bytes()
            );
            assert!(!root.path().join("draft.json").exists());
            assert!(response.get("draft").is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_output_roots_cannot_finalize_another_directory() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(outside.path(), &linked).unwrap();
        fs::write(
            outside.path().join("draft.json"),
            br#"{"schema_version":1,"kind":"result","data":{},"files":[]}"#,
        )
        .unwrap();
        assert!(
            finalize(
                &json!({"id":"1","method":"execute","result_dir":linked}),
                &mut json!({"id":"1","ok":true,"draft":"draft.json"}),
                None
            )
            .is_err()
        );
        assert!(outside.path().join("draft.json").exists());
        assert!(!outside.path().join("result.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_output_parents_cannot_reach_outside_the_assigned_directory() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "unchanged").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        fs::write(root.path().join("draft.json"), json!({"schema_version":1,"kind":"result","data":{},"files":[{"path":"linked/secret","media_type":"text/plain"}]}).to_string()).unwrap();
        assert!(
            finalize(
                &json!({"id":"1","method":"execute","result_dir":root.path()}),
                &mut json!({"id":"1","ok":true,"draft":"draft.json"}),
                None
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("secret")).unwrap(),
            "unchanged"
        );
        assert!(!root.path().join("result.json").exists());
    }
    #[test]
    fn draft_schema_conformance() {
        use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/fixtures/managed-output-v1.json"
        ))
        .unwrap();
        let catalog = public_schema_catalog().unwrap();
        for (name, valid) in [("valid", true), ("invalid", false)] {
            for value in vectors[name].as_array().unwrap() {
                assert_eq!(
                    catalog
                        .validate(PublicSchema::ManagedOutputDraft.repository_path(), value)
                        .is_ok(),
                    valid,
                    "{value}"
                );
            }
        }
    }
    #[test]
    fn dataset_finalization_is_canonical_and_hash_complete() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("draft.json"),
            br#"{"schema_version":1,"kind":"dataset","data":{"z":-0.0,"a":1},"files":[]}"#,
        )
        .unwrap();
        let mut response = json!({"id":"1","ok":true,"draft":"draft.json"});
        finalize(
            &json!({"id":"1","method":"materialize","output_dir":root.path()}),
            &mut response,
            None,
        )
        .unwrap();
        assert_eq!(
            fs::read(root.path().join("data.json")).unwrap(),
            br#"{"a":1,"z":0}"#
        );
        assert!(!root.path().join("draft.json").exists());
        let manifest: Value =
            serde_json::from_slice(&fs::read(root.path().join("dataset-manifest.json")).unwrap())
                .unwrap();
        assert_eq!(
            manifest["files"][0]["sha256"],
            hex(Sha256::digest(br#"{"a":1,"z":0}"#))
        );
    }
    #[test]
    fn stale_response_cannot_finalize_output() {
        assert!(
            finalize(
                &json!({"id":"1"}),
                &mut json!({"id":"2","ok":true,"draft":"draft.json"}),
                None
            )
            .is_err()
        );
    }
    #[test]
    fn undeclared_and_escaping_files_are_rejected() {
        for files in [
            json!([]),
            json!([{"path":"../outside","media_type":"text/plain"}]),
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::write(
                root.path().join("draft.json"),
                json!({"schema_version":1,"kind":"result","data":{},"files":files}).to_string(),
            )
            .unwrap();
            fs::write(root.path().join("extra"), "unexpected").unwrap();
            assert!(
                finalize(
                    &json!({"id":"1","method":"execute","result_dir":root.path()}),
                    &mut json!({"id":"1","ok":true,"draft":"draft.json"}),
                    None
                )
                .is_err()
            );
            assert!(!root.path().join("result.json").exists());
        }
    }
}
