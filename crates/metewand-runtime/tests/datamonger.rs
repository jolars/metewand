#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
use metewand_core::{canonical::CanonicalValue, manifest::Name};
use metewand_runtime::{
    datamonger::DatamongerAcquirer,
    dataset_materialization::verified_source_record,
    source_resolution::{SourceRequest, resolve_source},
};
use serde_json::{Value, json};

fn selector(verification: &str) -> Value {
    json!({
        "version": 1,
        "registry": {
            "schema_version": 1,
            "release": "metewand-test-1",
            "index_sha256": "11".repeat(32),
            "index_url": "https://example.invalid/index.json",
        },
        "dataset": {"source": "test", "name": "numbers", "version": "1"},
        "verification": verification,
    })
}

fn python() -> String {
    let output = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn canonical_standalone_vectors_match_the_public_schemas() {
    let schemas = public_schema_catalog().unwrap();
    for (schema, bytes) in [
        (
            PublicSchema::DatamongerSelector,
            include_bytes!("../../../fixtures/datamonger/v1/selector.json").as_slice(),
        ),
        (
            PublicSchema::DatamongerAcquisitionRequest,
            include_bytes!("../../../fixtures/datamonger/v1/request.json").as_slice(),
        ),
        (
            PublicSchema::DatamongerAcquisitionResponse,
            include_bytes!("../../../fixtures/datamonger/v1/response.json").as_slice(),
        ),
    ] {
        let value = CanonicalValue::from_slice(bytes).unwrap();
        assert_eq!(value.to_canonical_bytes().unwrap(), bytes);
        schemas
            .validate(schema.repository_path(), value.as_json())
            .unwrap();
    }
}

// The fixture supplies only Datamonger's public API. The production bridge still
// runs in a separate process, so these tests cover its actual calls and exports.
fn client(root: &Path, behavior: &str) -> std::path::PathBuf {
    let path = root.join("python");
    let script = include_str!("../../../fixtures/datamonger/v1/client.py")
        .replace("@PYTHON@", &python())
        .replace("@ROOT@", &serde_json::to_string(root).unwrap())
        .replace("@BEHAVIOR@", &serde_json::to_string(behavior).unwrap());
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn acquire(
    root: &Path,
    python: &Path,
    selection: Value,
) -> Result<
    metewand_runtime::source_resolution::ResolvedSource,
    metewand_runtime::source_resolution::SourceResolutionError,
> {
    let acquirer = DatamongerAcquirer::new(
        python,
        &root.join("cache"),
        Duration::from_secs(2),
        1024,
        false,
    )?;
    let selection = CanonicalValue::try_from(selection).unwrap();
    let provider: Name = serde_json::from_value(json!("datamonger")).unwrap();
    resolve_source(
        &SourceRequest::External {
            provider: &provider,
            selector: &selection,
        },
        Some(&acquirer),
        root,
    )
}

#[test]
fn exports_independent_bytes_and_keeps_verification_claims_distinct() {
    let root = tempfile::tempdir().unwrap();
    let python = client(root.path(), "ok");
    for level in ["artifact", "decoded"] {
        let source = acquire(root.path(), &python, selector(level)).unwrap();
        assert_eq!(
            fs::read(source.directory().join("artifacts/data")).unwrap(),
            b"value\n1\n2\n3\n"
        );
        assert_eq!(source.files().len(), 1);
        let record = verified_source_record(&source).unwrap();
        let evidence = record.decoding.unwrap();
        assert_eq!(evidence.as_json()["resolved_dataset_version"], "1");
        assert_eq!(
            evidence.as_json()["decoded_values"],
            if level == "decoded" {
                "provider_verified"
            } else {
                "not_verified"
            }
        );
        let receipt = source.provenance().acquisition.as_ref().unwrap().as_json();
        assert_eq!(receipt["datamonger"]["verification"], level);
        assert_eq!(
            receipt["datamonger"]["registry"],
            selector(level)["registry"]
        );
        fs::write(root.path().join("cache/data"), b"changed provider cache").unwrap();
        source.verify().unwrap();
    }
}

#[test]
fn rejects_missing_floating_and_malformed_pins_before_launch() {
    let root = tempfile::tempdir().unwrap();
    for change in [
        "missing_digest",
        "missing_version",
        "floating_release",
        "bad_digest",
        "qualified_name",
        "unknown_field",
        "future_version",
    ] {
        let mut selection = selector("decoded");
        match change {
            "missing_digest" => {
                selection["registry"]
                    .as_object_mut()
                    .unwrap()
                    .remove("index_sha256");
            }
            "missing_version" => {
                selection["dataset"]
                    .as_object_mut()
                    .unwrap()
                    .remove("version");
            }
            "floating_release" => selection["registry"]["release"] = json!("latest"),
            "bad_digest" => selection["registry"]["index_sha256"] = json!("abc"),
            "qualified_name" => selection["dataset"]["name"] = json!("test:numbers@1"),
            "unknown_field" => selection["default"] = json!(true),
            "future_version" => selection["version"] = json!(2),
            _ => unreachable!(),
        }
        let error = acquire(root.path(), Path::new("/unavailable/python"), selection).unwrap_err();
        assert!(error.to_string().contains("selector"), "{change}: {error}");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn rejects_provider_pin_and_verification_failures_without_partial_exports() {
    let root = tempfile::tempdir().unwrap();
    for (behavior, message) in [
        ("registry_mismatch", "pinned selector"),
        ("dataset_mismatch", "pinned selector"),
        ("artifact_mismatch", "artifact pin"),
        ("decoded_failure", "decoded digest mismatch"),
        ("verification_downgrade", "did not verify"),
        ("extra_export", "complete expected inventory"),
        ("receipt_wrong_selector", "differs from the pinned selector"),
        ("receipt_wrong_digest", "registry recipe"),
        ("receipt_wrong_path", "artifact name"),
        ("receipt_future_version", "schema"),
        ("receipt_missing_decoded_digest", "schema"),
        ("receipt_duplicate_artifact", "complete expected inventory"),
        ("receipt_oversized", "at most 1 MiB"),
    ] {
        let python = client(root.path(), behavior);
        let error = acquire(root.path(), &python, selector("decoded")).unwrap_err();
        assert!(error.to_string().contains(message), "{behavior}: {error}");
        assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("metewand-source-")
        }));
    }
}

#[test]
fn rejects_oversized_artifacts_before_fetching() {
    let root = tempfile::tempdir().unwrap();
    let python = client(root.path(), "ok");
    let acquirer = DatamongerAcquirer::new(
        &python,
        &root.path().join("cache"),
        Duration::from_secs(2),
        1,
        false,
    )
    .unwrap();
    let selection = CanonicalValue::try_from(selector("decoded")).unwrap();
    let provider: Name = serde_json::from_value(json!("datamonger")).unwrap();
    let error = resolve_source(
        &SourceRequest::External {
            provider: &provider,
            selector: &selection,
        },
        Some(&acquirer),
        root.path(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("byte budget"), "{error}");
    assert!(!root.path().join("cache").exists());
}

#[test]
fn bounds_acquisition_and_requires_the_selected_provider() {
    let root = tempfile::tempdir().unwrap();
    let python = client(root.path(), "timeout");
    let acquirer = DatamongerAcquirer::new(
        &python,
        &root.path().join("cache"),
        Duration::from_millis(50),
        1024,
        true,
    )
    .unwrap();
    let selection = CanonicalValue::try_from(selector("decoded")).unwrap();
    let provider: Name = serde_json::from_value(json!("datamonger")).unwrap();
    let error = resolve_source(
        &SourceRequest::External {
            provider: &provider,
            selector: &selection,
        },
        Some(&acquirer),
        root.path(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error}");
    let provider: Name = serde_json::from_value(json!("other")).unwrap();
    let error = resolve_source(
        &SourceRequest::External {
            provider: &provider,
            selector: &selection,
        },
        Some(&acquirer),
        root.path(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("datamonger"), "{error}");
}
