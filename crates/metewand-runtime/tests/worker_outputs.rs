use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::OnceLock,
};

use metewand_core::schema::SchemaCatalog;
use metewand_protocol::{ExecuteResponse, MaterializeResponse, RequestId};
use metewand_runtime::worker_output::{WorkerOutputError, WorkerOutputValidator};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const DATASET_SCHEMA: &str = "schemas/dataset.json";
const RESULT_SCHEMA: &str = "schemas/result.json";
const METRIC_SCHEMA: &str = "schemas/metrics.json";

#[test]
fn validates_a_complete_materialized_dataset() {
    let output = TempDir::new().unwrap();
    write(output.path(), "arrays/data.bin", b"dataset");
    write_json(
        output.path(),
        "dataset-manifest.json",
        &artifact_manifest(vec![file_entry("arrays/data.bin", b"dataset")]),
    );

    let validator = validator();
    let validated = validator
        .validate_materialized_dataset(
            output.path(),
            &materialize_response("dataset-manifest.json"),
            Path::new(DATASET_SCHEMA),
        )
        .unwrap();

    assert_eq!(
        validated.manifest_path(),
        Path::new("dataset-manifest.json")
    );
    assert_eq!(validated.files().len(), 1);
    assert_eq!(validated.files()[0].path(), Path::new("arrays/data.bin"));
    assert_eq!(validated.files()[0].size_bytes(), 7);
}

#[test]
fn rejects_absolute_and_traversing_worker_manifest_paths() {
    let output = TempDir::new().unwrap();
    let validator = validator();

    for path in [
        "/tmp/manifest.json",
        "C:/tmp/manifest.json",
        "../manifest.json",
        "nested\\manifest.json",
    ] {
        let error = validator
            .validate_materialized_dataset(
                output.path(),
                &materialize_response(path),
                Path::new(DATASET_SCHEMA),
            )
            .unwrap_err();
        assert!(
            matches!(error, WorkerOutputError::InvalidRelativePath { .. }),
            "unexpected error for {path:?}: {error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn rejects_a_manifest_path_through_an_escaping_symlink() {
    use std::os::unix::fs::symlink;

    let output = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    write_json(outside.path(), "manifest.json", &artifact_manifest(vec![]));
    symlink(outside.path(), output.path().join("escape")).unwrap();

    let error = validator()
        .validate_materialized_dataset(
            output.path(),
            &materialize_response("escape/manifest.json"),
            Path::new(DATASET_SCHEMA),
        )
        .unwrap_err();
    assert!(
        matches!(error, WorkerOutputError::UnsupportedEntryType { ref path } if path == Path::new("escape"))
    );
}

#[test]
fn rejects_absolute_and_traversing_declared_file_paths() {
    for declared_path in ["/tmp/data.bin", "../data.bin"] {
        let output = TempDir::new().unwrap();
        let entry = json!({
            "path": declared_path,
            "sha256": sha256(b"dataset"),
            "size_bytes": 7,
            "media_type": "application/octet-stream"
        });
        write_json(
            output.path(),
            "dataset-manifest.json",
            &artifact_manifest(vec![entry]),
        );

        let error = validator()
            .validate_materialized_dataset(
                output.path(),
                &materialize_response("dataset-manifest.json"),
                Path::new(DATASET_SCHEMA),
            )
            .unwrap_err();
        assert!(matches!(error, WorkerOutputError::PublicSchema { .. }));
    }
}

#[cfg(unix)]
#[test]
fn rejects_special_files_and_symlinks_anywhere_in_the_output() {
    use std::os::unix::{fs::symlink, net::UnixListener};

    for special in ["socket", "symlink"] {
        let output = TempDir::new().unwrap();
        write_json(
            output.path(),
            "dataset-manifest.json",
            &artifact_manifest(vec![]),
        );
        let special_path = output.path().join(special);
        let _socket = if special == "socket" {
            Some(UnixListener::bind(&special_path).unwrap())
        } else {
            symlink("dataset-manifest.json", &special_path).unwrap();
            None
        };

        let error = validator()
            .validate_materialized_dataset(
                output.path(),
                &materialize_response("dataset-manifest.json"),
                Path::new(DATASET_SCHEMA),
            )
            .unwrap_err();
        assert!(
            matches!(error, WorkerOutputError::UnsupportedEntryType { ref path } if path == Path::new(special)),
            "unexpected error for {special}: {error}"
        );
    }
}

#[test]
fn rejects_undeclared_files_and_directories() {
    for undeclared in ["extra.bin", "empty"] {
        let output = TempDir::new().unwrap();
        write_json(
            output.path(),
            "dataset-manifest.json",
            &artifact_manifest(vec![]),
        );
        if undeclared == "empty" {
            fs::create_dir(output.path().join(undeclared)).unwrap();
        } else {
            write(output.path(), undeclared, b"extra");
        }

        let error = validator()
            .validate_materialized_dataset(
                output.path(),
                &materialize_response("dataset-manifest.json"),
                Path::new(DATASET_SCHEMA),
            )
            .unwrap_err();
        assert!(
            matches!(error, WorkerOutputError::UndeclaredEntry { ref path } if path == Path::new(undeclared)),
            "unexpected error for {undeclared}: {error}"
        );
    }
}

#[test]
fn rejects_an_incomplete_manifest_and_missing_or_incomplete_files() {
    let output = TempDir::new().unwrap();
    write_json(
        output.path(),
        "dataset-manifest.json",
        &json!({"schema_version": 1, "complete": false, "files": []}),
    );
    assert!(matches!(
        validate_dataset(output.path()).unwrap_err(),
        WorkerOutputError::PublicSchema { .. }
    ));

    let output = TempDir::new().unwrap();
    write_json(
        output.path(),
        "dataset-manifest.json",
        &artifact_manifest(vec![file_entry("missing.bin", b"missing")]),
    );
    assert!(matches!(
        validate_dataset(output.path()).unwrap_err(),
        WorkerOutputError::MissingDeclaredFile { ref path } if path == Path::new("missing.bin")
    ));

    let output = TempDir::new().unwrap();
    write(output.path(), "data.bin", b"short");
    let mut wrong_size = file_entry("data.bin", b"short");
    wrong_size["size_bytes"] = json!(9);
    write_json(
        output.path(),
        "dataset-manifest.json",
        &artifact_manifest(vec![wrong_size]),
    );
    assert!(matches!(
        validate_dataset(output.path()).unwrap_err(),
        WorkerOutputError::FileSizeMismatch { ref path, .. } if path == Path::new("data.bin")
    ));

    let output = TempDir::new().unwrap();
    write(output.path(), "data.bin", b"actual");
    write_json(
        output.path(),
        "dataset-manifest.json",
        &artifact_manifest(vec![file_entry("data.bin", b"expect")]),
    );
    assert!(matches!(
        validate_dataset(output.path()).unwrap_err(),
        WorkerOutputError::FileHashMismatch { ref path, .. } if path == Path::new("data.bin")
    ));
}

#[test]
fn rejects_duplicate_paths_and_returns_files_in_bytewise_order() {
    let duplicate = TempDir::new().unwrap();
    write(duplicate.path(), "data.bin", b"data");
    let mut second = file_entry("data.bin", b"data");
    second["media_type"] = json!("application/x-data");
    write_json(
        duplicate.path(),
        "dataset-manifest.json",
        &artifact_manifest(vec![file_entry("data.bin", b"data"), second]),
    );
    assert!(matches!(
        validate_dataset(duplicate.path()).unwrap_err(),
        WorkerOutputError::DuplicateDeclaredPath { ref path } if path == Path::new("data.bin")
    ));

    let output = TempDir::new().unwrap();
    write(output.path(), "a.bin", b"a");
    write(output.path(), "b.bin", b"b");
    write_json(
        output.path(),
        "dataset-manifest.json",
        &artifact_manifest(vec![file_entry("b.bin", b"b"), file_entry("a.bin", b"a")]),
    );
    let schemas = SchemaCatalog::try_new([(
        PathBuf::from(DATASET_SCHEMA),
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object"
        }),
    )])
    .unwrap();
    let validator = WorkerOutputValidator::new(&schemas).unwrap();
    let validated = validator
        .validate_materialized_dataset(
            output.path(),
            &materialize_response("dataset-manifest.json"),
            Path::new(DATASET_SCHEMA),
        )
        .unwrap();
    assert_eq!(validated.files()[0].path(), Path::new("a.bin"));
    assert_eq!(validated.files()[1].path(), Path::new("b.bin"));
}

#[test]
fn validates_canonical_result_data_and_its_complete_file_inventory() {
    let output = TempDir::new().unwrap();
    write(output.path(), "coefficients.bin", b"coefficients");
    write_json(
        output.path(),
        "result.json",
        &result_manifest(
            RESULT_SCHEMA,
            json!({"answer": 42.0}),
            vec![file_entry("coefficients.bin", b"coefficients")],
        ),
    );

    let validated = validator()
        .validate_execution_result(
            output.path(),
            &execute_response("result.json"),
            Path::new(RESULT_SCHEMA),
        )
        .unwrap();
    assert_eq!(validated.data().as_json(), &json!({"answer": 42}));
    assert_eq!(validated.files().len(), 1);
}

#[test]
fn rejects_result_schema_mismatches_and_invalid_result_data() {
    for (schema, data, expected_schema_error) in [
        ("schemas/other.json", json!({"answer": 42.0}), true),
        (RESULT_SCHEMA, json!({"answer": "forty-two"}), false),
    ] {
        let output = TempDir::new().unwrap();
        write_json(
            output.path(),
            "result.json",
            &result_manifest(schema, data, vec![]),
        );
        let error = validator()
            .validate_execution_result(
                output.path(),
                &execute_response("result.json"),
                Path::new(RESULT_SCHEMA),
            )
            .unwrap_err();
        assert_eq!(
            matches!(error, WorkerOutputError::SchemaReferenceMismatch { .. }),
            expected_schema_error,
            "unexpected error: {error}"
        );
        if !expected_schema_error {
            assert!(matches!(error, WorkerOutputError::ScientificSchema { .. }));
        }
    }

    let output = TempDir::new().unwrap();
    fs::write(
        output.path().join("result.json"),
        br#"{"schema_version":1,"schema":"schemas/result.json","data":{"answer":42,"answer":43},"files":[]}"#,
    )
    .unwrap();
    assert!(matches!(
        validator()
            .validate_execution_result(
                output.path(),
                &execute_response("result.json"),
                Path::new(RESULT_SCHEMA),
            )
            .unwrap_err(),
        WorkerOutputError::InvalidCanonicalJson { .. }
    ));
}

#[test]
fn validates_metrics_and_rejects_invalid_or_noncanonical_metric_documents() {
    let output = TempDir::new().unwrap();
    write_json(
        output.path(),
        "metrics.json",
        &json!({
            "schema_version": 1,
            "schema": METRIC_SCHEMA,
            "data": {"loss": 0.25}
        }),
    );
    let validated = validator()
        .validate_metrics(
            output.path(),
            Path::new("metrics.json"),
            Path::new(METRIC_SCHEMA),
        )
        .unwrap();
    assert_eq!(validated.data().as_json(), &json!({"loss": 0.25}));

    write_json(
        output.path(),
        "metrics.json",
        &json!({
            "schema_version": 1,
            "schema": METRIC_SCHEMA,
            "data": {"loss": "low"}
        }),
    );
    assert!(matches!(
        validator()
            .validate_metrics(
                output.path(),
                Path::new("metrics.json"),
                Path::new(METRIC_SCHEMA)
            )
            .unwrap_err(),
        WorkerOutputError::ScientificSchema { .. }
    ));

    fs::write(
        output.path().join("metrics.json"),
        br#"{"schema_version":1,"schema":"schemas/metrics.json","data":{"loss":0.25,"loss":0.5}}"#,
    )
    .unwrap();
    assert!(matches!(
        validator()
            .validate_metrics(
                output.path(),
                Path::new("metrics.json"),
                Path::new(METRIC_SCHEMA)
            )
            .unwrap_err(),
        WorkerOutputError::InvalidCanonicalJson { .. }
    ));
}

#[test]
fn metrics_must_be_the_only_declared_regular_file_in_their_output() {
    let output = TempDir::new().unwrap();
    write_json(
        output.path(),
        "metrics.json",
        &json!({
            "schema_version": 1,
            "schema": METRIC_SCHEMA,
            "data": {"loss": 0.25}
        }),
    );
    write(output.path(), "extra.bin", b"extra");

    assert!(matches!(
        validator()
            .validate_metrics(output.path(), Path::new("metrics.json"), Path::new(METRIC_SCHEMA))
            .unwrap_err(),
        WorkerOutputError::UndeclaredEntry { ref path } if path == Path::new("extra.bin")
    ));
}

fn validator() -> WorkerOutputValidator<'static> {
    static SCHEMAS: OnceLock<SchemaCatalog> = OnceLock::new();
    let schemas = SCHEMAS.get_or_init(|| {
        SchemaCatalog::try_new([
            (
                PathBuf::from(DATASET_SCHEMA),
                json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {
                        "files": {
                            "type": "array",
                            "contains": {
                                "type": "object",
                                "properties": {"path": {"const": "arrays/data.bin"}}
                            }
                        }
                    }
                }),
            ),
            (
                PathBuf::from(RESULT_SCHEMA),
                json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {"answer": {"type": "number"}},
                    "required": ["answer"],
                    "additionalProperties": false
                }),
            ),
            (
                PathBuf::from(METRIC_SCHEMA),
                json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {"loss": {"type": "number"}},
                    "required": ["loss"],
                    "additionalProperties": false
                }),
            ),
        ])
        .unwrap()
    });
    WorkerOutputValidator::new(schemas).unwrap()
}

fn validate_dataset(
    root: &Path,
) -> Result<metewand_runtime::worker_output::ValidatedDataset, WorkerOutputError> {
    validator().validate_materialized_dataset(
        root,
        &materialize_response("dataset-manifest.json"),
        Path::new(DATASET_SCHEMA),
    )
}

fn materialize_response(manifest: &str) -> MaterializeResponse {
    MaterializeResponse::new(request_id(), manifest)
}

fn execute_response(manifest: &str) -> ExecuteResponse {
    ExecuteResponse::new(request_id(), None, manifest, serde_json::Map::new())
}

fn request_id() -> RequestId {
    RequestId::from_str("1").unwrap()
}

fn artifact_manifest(files: Vec<Value>) -> Value {
    json!({"schema_version": 1, "complete": true, "files": files})
}

fn result_manifest(schema: &str, data: Value, files: Vec<Value>) -> Value {
    json!({"schema_version": 1, "schema": schema, "data": data, "files": files})
}

fn file_entry(path: &str, contents: &[u8]) -> Value {
    json!({
        "path": path,
        "sha256": sha256(contents),
        "size_bytes": contents.len(),
        "media_type": "application/octet-stream"
    })
}

fn sha256(contents: &[u8]) -> String {
    format!("{:x}", Sha256::digest(contents))
}

fn write(root: &Path, path: &str, contents: &[u8]) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn write_json(root: &Path, path: &str, value: &Value) {
    write(root, path, &serde_json::to_vec(value).unwrap());
}
