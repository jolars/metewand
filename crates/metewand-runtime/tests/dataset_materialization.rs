#![cfg(unix)]
use metewand_core::{
    canonical::CanonicalValue,
    records::{
        ContentDigest, DatasetConfigurationRecord, DerivedSeedRecord, RecordId,
        ResolvedLaunchRecord, ResolvedWorkerRecord, WorkerWorkingDirectory,
    },
    schema::SchemaCatalog,
};
use metewand_runtime::{
    dataset_materialization::{
        ContractDatasetValidator, DatasetMaterialization, MaterializationError, materialize_dataset,
    },
    worker_process::WorkerLogLimits,
    worker_session::PhaseTimeouts,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/worker-protocol/v1")
        .join(name)
}
fn timeouts() -> PhaseTimeouts {
    let d = Duration::from_secs(2);
    PhaseTimeouts {
        hello: d,
        materialize: d,
        prepare: d,
        execute: d,
        reset: d,
        evaluate: d,
        shutdown: d,
    }
}
fn worker() -> ResolvedWorkerRecord {
    ResolvedWorkerRecord {
        source_bundle: Some(RecordId::from_digest([1; 32])),
        environment: RecordId::from_digest([2; 32]),
        launch: RecordId::from_digest([3; 32]),
    }
}
fn launch() -> ResolvedLaunchRecord {
    ResolvedLaunchRecord {
        environment: RecordId::from_digest([2; 32]),
        program: fixture("dataset-materializer").to_str().unwrap().to_owned(),
        args: vec![],
        environment_variables: BTreeMap::from([("PATH".to_owned(), env::var("PATH").unwrap())]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    }
}
fn configuration() -> DatasetConfigurationRecord {
    DatasetConfigurationRecord {
        definition: RecordId::from_digest([4; 32]),
        parameters: CanonicalValue::try_from(json!({"values":[1,2,3]})).unwrap(),
        seed: DerivedSeedRecord {
            value: 17,
            derivation_digest: ContentDigest::new([5; 32]),
        },
    }
}
fn schemas(valid: bool) -> SchemaCatalog {
    SchemaCatalog::try_new([(
        PathBuf::from("schemas/dataset.json"),
        if valid {
            serde_json::from_slice(&fs::read(fixture("schemas/dataset.json")).unwrap()).unwrap()
        } else {
            json!({"$schema":"https://json-schema.org/draft/2020-12/schema","not":{}})
        },
    )])
    .unwrap()
}

#[test]
fn generator_receives_dataset_seed_and_canonical_parameters() {
    let root = tempfile::tempdir().unwrap();
    let config = configuration();
    let launch = launch();
    let worker = worker();
    let schemas = schemas(true);
    let run = || {
        materialize_dataset(DatasetMaterialization {
            configuration: &config,
            source: None,
            launch: &launch,
            worker: &worker,
            output_schema: Path::new("schemas/dataset.json"),
            schemas: &schemas,
            validator: None,
            private_root: root.path(),
            timeouts: timeouts(),
            log_limits: WorkerLogLimits::new(1024, 1024),
        })
        .unwrap()
    };
    let first = run();
    metewand_core::public_schemas::public_schema_catalog()
        .unwrap()
        .validate(
            metewand_core::public_schemas::PublicSchema::DatasetMaterialization.repository_path(),
            first.provenance.as_json(),
        )
        .unwrap();
    let second = run();
    assert_eq!(first.derivation.id, second.derivation.id);
    assert_eq!(first.output_digest, second.output_digest);
    let data: serde_json::Value =
        serde_json::from_slice(&fs::read(first.directory().join("data.json")).unwrap()).unwrap();
    assert_eq!(data, json!({"dataset_seed":17,"values":[1,2,3]}));
    assert_eq!(
        fs::read(first.directory().join(first.manifest.manifest_path())).unwrap(),
        first.manifest.manifest().to_canonical_bytes().unwrap()
    );
    first.verify().unwrap();
    drop(first);
    drop(second);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn schema_failure_removes_output() {
    let root = tempfile::tempdir().unwrap();
    let config = configuration();
    let launch = launch();
    let worker = worker();
    let schemas = schemas(false);
    assert!(matches!(
        materialize_dataset(DatasetMaterialization {
            configuration: &config,
            source: None,
            launch: &launch,
            worker: &worker,
            output_schema: Path::new("schemas/dataset.json"),
            schemas: &schemas,
            validator: None,
            private_root: root.path(),
            timeouts: timeouts(),
            log_limits: WorkerLogLimits::new(1024, 1024)
        }),
        Err(MaterializationError::Output(_))
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn contract_validator_accepts_rejects_times_out_and_cannot_mutate_output() {
    for mode in ["accept", "reject", "timeout", "mutate"] {
        let root = tempfile::tempdir().unwrap();
        let config = configuration();
        let launch = launch();
        let worker = worker();
        let schemas = schemas(true);
        let mut check = launch.clone();
        check.program = "python3".to_owned();
        check.args = vec![
            "-c".to_owned(),
            format!(
                r#"import json,sys,time,pathlib
assert sys.argv[1] == '--metewand-dataset-validation-v1'
r=json.loads(sys.argv[2]); assert r['version']==1
p=pathlib.Path(r['dataset_dir'])
assert json.loads((p/'data.json').read_text())['dataset_seed']==17
mode={mode:?}
if mode=='reject': sys.exit(7)
if mode=='timeout': time.sleep(5)
if mode=='mutate': (p/'data.json').write_text('changed')
"#
            ),
        ];
        let outcome = materialize_dataset(DatasetMaterialization {
            configuration: &config,
            source: None,
            launch: &launch,
            worker: &worker,
            output_schema: Path::new("schemas/dataset.json"),
            schemas: &schemas,
            validator: Some(ContractDatasetValidator {
                contract: RecordId::from_digest([6; 32]),
                launch: &check,
                worker: &worker,
                timeout: Duration::from_millis(200),
            }),
            private_root: root.path(),
            timeouts: timeouts(),
            log_limits: WorkerLogLimits::new(1024, 1024),
        });
        match mode {
            "accept" => {
                let output = outcome.unwrap();
                assert_eq!(
                    output.provenance.as_json()["binary_validation"]["status"],
                    "accepted"
                );
            }
            "reject" => assert!(matches!(outcome, Err(MaterializationError::BinaryRejected))),
            "timeout" => assert!(matches!(outcome, Err(MaterializationError::BinaryTimeout))),
            _ => assert!(matches!(outcome, Err(MaterializationError::ChangedInput))),
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn external_transformation_records_selection_decoding_and_independent_bytes() {
    use metewand_runtime::dataset_materialization::verified_source_record;
    use metewand_runtime::source_resolution::{
        AcquiredSource, SourceAcquirer, SourceFile, SourceRequest, SourceResolutionError,
        resolve_source,
    };
    use sha2::{Digest, Sha256};
    struct Provider {
        receipt: CanonicalValue,
    }
    impl SourceAcquirer for Provider {
        fn acquire(
            &self,
            _: &SourceRequest<'_>,
            output: &Path,
        ) -> Result<AcquiredSource, SourceResolutionError> {
            let bytes = br#"{"values":[4,5]}"#;
            fs::write(output.join("source"), bytes)?;
            Ok(AcquiredSource {
                files: vec![SourceFile {
                    path: serde_json::from_value(json!("source")).unwrap(),
                    size_bytes: bytes.len() as u64,
                    sha256: ContentDigest::new(Sha256::digest(bytes).into()),
                }],
                provenance: self.receipt.clone(),
            })
        }
    }
    let root = tempfile::tempdir().unwrap();
    let provider = serde_json::from_value(json!("registry")).unwrap();
    let selector = CanonicalValue::try_from(json!({"dataset":"integers","version":"v7"})).unwrap();
    let request = SourceRequest::External {
        provider: &provider,
        selector: &selector,
    };
    for status in ["not_verified", "provider_verified"] {
        let receipt = CanonicalValue::try_from(json!({"external_dataset":{"resolved_dataset_version":"v7","decoding_recipe":{"format":"json","decoder_version":1},"decoded_values":status},"provider_receipt":"opaque"})).unwrap();
        let source = resolve_source(&request, Some(&Provider { receipt }), root.path()).unwrap();
        let config = configuration();
        let worker = worker();
        let schemas = schemas(true);
        let mut launch = launch();
        launch.program = "python3".to_owned();
        launch.args = vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/generated-dataset/worker.py")
                .to_str()
                .unwrap()
                .to_owned(),
            "dataset_materializer".to_owned(),
        ];
        let output = materialize_dataset(DatasetMaterialization {
            configuration: &config,
            source: Some(&source),
            launch: &launch,
            worker: &worker,
            output_schema: Path::new("schemas/dataset.json"),
            schemas: &schemas,
            validator: None,
            private_root: root.path(),
            timeouts: timeouts(),
            log_limits: WorkerLogLimits::new(1024, 1024),
        })
        .unwrap();
        let data: serde_json::Value =
            serde_json::from_slice(&fs::read(output.directory().join("data.json")).unwrap())
                .unwrap();
        assert_eq!(data["values"], json!([4, 5]));
        let input = &output.derivation.record.sources[0];
        assert_eq!(
            input.selection.as_json()["selector"],
            selector.as_json().clone()
        );
        assert_eq!(
            input.decoding.as_ref().unwrap().as_json()["resolved_dataset_version"],
            "v7"
        );
        assert_eq!(
            input.decoding.as_ref().unwrap().as_json()["decoding_recipe"],
            json!({"format":"json", "decoder_version":1})
        );
        assert_eq!(
            input.decoding.as_ref().unwrap().as_json()["decoded_values"],
            status
        );
        assert_eq!(
            output.provenance.as_json()["source"]["raw_bytes_verified"],
            true
        );
        assert_eq!(
            output.provenance.as_json()["source"]["files"][0]["sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        source.verify().unwrap();
    }
    let bad = resolve_source(
        &request,
        Some(&Provider {
            receipt: CanonicalValue::try_from(json!({})).unwrap(),
        }),
        root.path(),
    )
    .unwrap();
    assert!(matches!(
        verified_source_record(&bad),
        Err(MaterializationError::ExternalEvidence)
    ));
}

#[test]
fn opaque_binary_payloads_need_the_contract_checker() {
    let root = tempfile::tempdir().unwrap();
    let example =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/generated-dataset/worker.py");
    let script = fs::read_to_string(example).unwrap()
        .replace("import hashlib", "import struct\nimport hashlib")
        .replace("write_json(output / \"data.json\", {\"values\": values, \"dataset_seed\": request[\"dataset_seed\"]})", "(output / 'data.json').write_bytes(struct.pack('>I', request['dataset_seed']))")
        .replace("data.json", "data.bin")
        .replace("application/json", "application/octet-stream");
    let code = root.path().join("binary-worker.py");
    fs::write(&code, script).unwrap();
    let config = configuration();
    let worker = worker();
    let mut launch = launch();
    launch.program = "python3".to_owned();
    launch.args = vec![
        code.to_str().unwrap().to_owned(),
        "dataset_materializer".to_owned(),
    ];
    let schemas = SchemaCatalog::try_new([(
        PathBuf::from("schemas/dataset.json"),
        json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object"}),
    )])
    .unwrap();
    for expected in [17, 18] {
        let mut checker = launch.clone();
        checker.args = vec![
            "-c".to_owned(),
            format!(
                "import json,pathlib,struct,sys; r=json.loads(sys.argv[2]); p=pathlib.Path(r['dataset_dir'])/'data.bin'; assert struct.unpack('>I',p.read_bytes())[0]=={expected}"
            ),
        ];
        let output = materialize_dataset(DatasetMaterialization {
            configuration: &config,
            source: None,
            launch: &launch,
            worker: &worker,
            output_schema: Path::new("schemas/dataset.json"),
            schemas: &schemas,
            validator: Some(ContractDatasetValidator {
                contract: RecordId::from_digest([6; 32]),
                launch: &checker,
                worker: &worker,
                timeout: Duration::from_secs(2),
            }),
            private_root: root.path(),
            timeouts: timeouts(),
            log_limits: WorkerLogLimits::new(1024, 1024),
        });
        if expected == 17 {
            let output = output.unwrap();
            assert_eq!(
                fs::read(output.directory().join("data.bin")).unwrap(),
                17_u32.to_be_bytes()
            );
        } else {
            assert!(matches!(output, Err(MaterializationError::BinaryRejected)));
        }
    }
}
