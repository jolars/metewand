//! Resolve Datamonger once, then construct and compare without the provider.

use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use metewand_core::{
    canonical::CanonicalValue,
    identity::{identify_canonical, identify_record, record_id},
    manifest::{Name, RepositoryPath},
    materialization::hex,
    records::{
        ContentDigest, DatasetArtifact, DatasetInstanceRecord, ResolvedEnvironmentRecord,
        ResolvedLaunchRecord, ResolvedWorkerRecord, SourceBundleArtifact, WorkerWorkingDirectory,
    },
};
use metewand_runtime::{
    LOCAL_TREE_HASH_VERSION,
    artifact_store::ArtifactStore,
    datamonger::DatamongerAcquirer,
    dataset_materialization::{DatasetMaterialization, materialize_dataset},
    fixed_dataset_run::{FixedDatasetRun, run_fixed_dataset},
    repository::check_repository,
    source_resolution::{ResolvedSource, SourceRequest, resolve_source},
    worker_process::WorkerLogLimits,
    worker_session::PhaseTimeouts,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn canonical(value: Value) -> Result<CanonicalValue> {
    Ok(CanonicalValue::try_from(value)?)
}

fn deadlines() -> PhaseTimeouts {
    let timeout = Duration::from_secs(10);
    PhaseTimeouts {
        hello: timeout,
        materialize: timeout,
        prepare: timeout,
        execute: timeout,
        reset: timeout,
        evaluate: timeout,
        shutdown: timeout,
    }
}

struct Comparison {
    dataset_instance: String,
    materialization: CanonicalValue,
    attempts: Vec<ComparedImplementation>,
}

struct ComparedImplementation {
    name: &'static str,
    view: PathBuf,
    tree_digest: String,
    result: CanonicalValue,
    metrics: CanonicalValue,
    statistics: Value,
}

fn compare(
    source: &ResolvedSource,
    worker_python: &Path,
    store: &ArtifactStore,
    private_root: &Path,
) -> Result<Comparison> {
    let repository = check_repository(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/datamonger/metewand.yaml"),
    )?;
    let plan = repository.logical_plan()?;
    let candidates = &plan.experiments[0].candidates;
    let local: Name = serde_json::from_value(json!("local"))?;
    let source_paths: Vec<RepositoryPath> = serde_json::from_value(json!(["worker.py"]))?;
    let sources = resolve_source(
        &SourceRequest::Repository {
            root: &repository.root,
            sources: &source_paths,
            expected: repository.resources.source_bundles[&vec![PathBuf::from("worker.py")]],
        },
        None,
        private_root,
    )?;
    let python_digest = hex(ContentDigest::new(
        Sha256::digest(fs::read(worker_python)?).into(),
    ));
    let environment = identify_record(ResolvedEnvironmentRecord {
        definition: repository.planning_catalog.environment_definitions[&local],
        fingerprint: canonical(
            json!({"class":"development","os":env::consts::OS,"architecture":env::consts::ARCH,"program_sha256":python_digest,"dependency_closure":null}),
        )?,
    })?;
    let source_bundle = identify_canonical::<SourceBundleArtifact>(&canonical(
        json!({"tree_hash_version":LOCAL_TREE_HASH_VERSION,"tree_sha256":hex(sources.tree_digest())}),
    )?)?;
    let worker = |role: &str,
                  representation: Option<&str>|
     -> Result<(ResolvedLaunchRecord, ResolvedWorkerRecord)> {
        let mut args = vec!["{sources}/worker.py".to_owned(), role.to_owned()];
        if let Some(representation) = representation {
            args.push(representation.to_owned());
        }
        let mut launch = ResolvedLaunchRecord {
            environment: environment.id,
            program: format!("sha256:{python_digest}"),
            args,
            environment_variables: BTreeMap::from([
                ("LC_ALL".into(), "C".into()),
                ("TZ".into(), "UTC".into()),
            ]),
            working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
        };
        let record = ResolvedWorkerRecord {
            source_bundle: Some(source_bundle),
            environment: environment.id,
            launch: record_id(&launch)?,
        };
        launch.program = worker_python
            .to_str()
            .ok_or("Python path must be UTF-8")?
            .to_owned();
        launch.args[0] = sources
            .directory()
            .join("worker.py")
            .to_str()
            .ok_or("source path must be UTF-8")?
            .to_owned();
        Ok((launch, record))
    };
    let (launch, materializer) = worker("dataset_materializer", None)?;
    let materialized = materialize_dataset(DatasetMaterialization {
        configuration: &candidates[0].dataset_configuration.record,
        source: Some(source),
        launch: &launch,
        worker: &materializer,
        output_schema: Path::new("schemas/dataset.json"),
        schemas: &repository.schemas,
        validator: None,
        private_root,
        timeouts: deadlines(),
        log_limits: WorkerLogLimits::new(1024, 1024),
    })?;
    let artifact = store.publish_dataset(&materialized)?;
    let instance = identify_record(DatasetInstanceRecord {
        configuration: candidates[0].dataset_configuration.id,
        artifact: identify_canonical::<DatasetArtifact>(&canonical(
            json!({"tree_hash_version":LOCAL_TREE_HASH_VERSION,"tree_sha256":hex(artifact.digest())}),
        )?)?,
        output_schema: repository.resources.schemas[Path::new("schemas/dataset.json")],
    })?;
    let sum: Name = serde_json::from_value(json!("sum"))?;
    let contract_id = repository.resources.problem_contracts[&sum].to_string();
    let (evaluator_launch, evaluator) = worker("problem_evaluator", None)?;
    let evaluator_identity = record_id(&evaluator)?.to_string().parse()?;
    // Retaining both views proves that concurrent clients have distinct files,
    // even on systems that aggressively reuse temporary-directory names.
    let views = [
        artifact.private_view(private_root)?,
        artifact.private_view(private_root)?,
    ];
    let mut attempts = Vec::new();
    for ((candidate, view), (name, representation)) in candidates
        .iter()
        .zip(&views)
        .zip([("reference", "list"), ("packed", "packed")])
    {
        let (launch, implementation) = worker("implementation", Some(representation))?;
        let identity = record_id(&implementation)?.to_string().parse()?;
        view.verify()?;
        let result = run_fixed_dataset(FixedDatasetRun {
            dataset_dir: view.directory(),
            problem_contract_id: &contract_id,
            problem_parameters: candidate.problem_configuration.record.parameters.as_json(),
            problem_semantics: repository.problem_contracts[&sum]
                .semantics
                .as_canonical()
                .as_json(),
            implementation_parameters: candidate
                .implementation_configuration
                .record
                .parameters
                .as_json(),
            implementation_seed: candidate.specifications[0]
                .specification
                .record
                .implementation_seed
                .value,
            implementation_launch: &launch,
            implementation_identity: &identity,
            evaluator_launch: &evaluator_launch,
            evaluator_identity: &evaluator_identity,
            result_schema: Path::new("schemas/result.json"),
            metric_schema: Path::new("schemas/metrics.json"),
            schemas: &repository.schemas,
            private_root,
            timeouts: deadlines(),
            timing: metewand_runtime::fixed_dataset_run::ExecutionTimingPolicy::default(),
            log_limits: WorkerLogLimits::new(1024, 1024),
        })?;
        view.verify()?;
        attempts.push(ComparedImplementation {
            name,
            view: view.directory().to_owned(),
            tree_digest: hex(view.digest()),
            result: result.result,
            metrics: result.metrics,
            statistics: result.statistics,
        });
    }
    sources.verify()?;
    source.verify()?;
    Ok(Comparison {
        dataset_instance: instance.id.to_string(),
        materialization: materialized.provenance,
        attempts,
    })
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if !(4..=5).contains(&args.len()) || (args.len() == 5 && args[4] != "--offline") {
        return Err("usage: datamonger DATAMONGER_PYTHON WORKER_PYTHON PROVIDER_CACHE ARTIFACT_STORE [--offline]".into());
    }
    // Preserve a virtual environment's interpreter path: canonicalizing its
    // symlink would discard the environment containing Datamonger.
    let absolute =
        |path: &std::ffi::OsStr| -> Result<PathBuf> { Ok(env::current_dir()?.join(path)) };
    let provider_python = absolute(&args[0])?;
    let worker_python = absolute(&args[1])?;
    let provider_cache = absolute(&args[2])?;
    let store = ArtifactStore::open(&absolute(&args[3])?)?;
    let staging = tempfile::tempdir()?;
    let selector = CanonicalValue::from_slice(include_bytes!("selector.json"))?;
    let provider: Name = serde_json::from_value(json!("datamonger"))?;
    let acquirer = DatamongerAcquirer::new(
        &provider_python,
        &provider_cache,
        Duration::from_secs(60),
        64 * 1024 * 1024,
        args.len() == 5,
    )?;
    let source = resolve_source(
        &SourceRequest::External {
            provider: &provider,
            selector: &selector,
        },
        Some(&acquirer),
        staging.path(),
    )?;
    drop(acquirer);
    let comparison = compare(&source, &worker_python, &store, staging.path())?;
    println!("Dataset instance: {}", comparison.dataset_instance);
    println!(
        "Decoded values: {}",
        comparison.materialization.as_json()["derivation"]["sources"][0]["decoding"]["decoded_values"]
    );
    for attempt in comparison.attempts {
        println!(
            "{}: result {}, metrics {}",
            attempt.name,
            attempt.result.as_json(),
            attempt.metrics.as_json()
        );
        println!(
            "  Private view: {} ({})",
            attempt.view.display(),
            attempt.tree_digest
        );
        println!(
            "  Consumed input SHA-256: {}",
            attempt.statistics["input_sha256"]
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        os::unix::fs::PermissionsExt,
        process::Command,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::{self, JoinHandle},
    };

    fn python() -> PathBuf {
        let output = Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    fn selection() -> Value {
        serde_json::from_slice(include_bytes!("../../fixtures/datamonger/v1/selector.json"))
            .unwrap()
    }

    fn resolve(
        root: &Path,
        interpreter: &Path,
        selection: Value,
        offline: bool,
    ) -> Result<ResolvedSource> {
        let selector = canonical(selection)?;
        let provider: Name = serde_json::from_value(json!("datamonger"))?;
        let acquirer = DatamongerAcquirer::new(
            interpreter,
            &root.join("provider-cache"),
            Duration::from_secs(15),
            1024,
            offline,
        )?;
        Ok(resolve_source(
            &SourceRequest::External {
                provider: &provider,
                selector: &selector,
            },
            Some(&acquirer),
            root,
        )?)
    }

    fn assert_comparison(comparison: &Comparison) {
        assert!(
            comparison
                .dataset_instance
                .starts_with("mw1-dataset-instance-")
        );
        assert_eq!(comparison.attempts.len(), 2);
        let first = &comparison.attempts[0];
        let second = &comparison.attempts[1];
        assert_ne!(first.view, second.view);
        assert_eq!(first.tree_digest, second.tree_digest);
        assert_eq!(
            first.statistics["input_sha256"],
            second.statistics["input_sha256"]
        );
        for attempt in &comparison.attempts {
            assert_eq!(attempt.statistics["dataset_dir"], json!(attempt.view));
            assert_eq!(attempt.result.as_json(), &json!({"answer":60}));
            assert_eq!(attempt.metrics.as_json(), &json!({"absolute_error":0}));
        }
    }

    #[test]
    fn constructs_one_instance_and_compares_private_views_after_removing_the_provider() {
        let root = tempfile::tempdir().unwrap();
        let interpreter = root.path().join("provider-python");
        let worker_python = python();
        let script = include_str!("../../fixtures/datamonger/v1/client.py")
            .replace("@PYTHON@", worker_python.to_str().unwrap())
            .replace("@ROOT@", &serde_json::to_string(root.path()).unwrap())
            .replace("@BEHAVIOR@", "\"ok\"");
        fs::write(&interpreter, &script).unwrap();
        fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o700)).unwrap();
        let decoded = resolve(root.path(), &interpreter, selection(), false).unwrap();
        let mut artifact_only = selection();
        artifact_only["verification"] = json!("artifact");
        let artifact_only = resolve(root.path(), &interpreter, artifact_only, false).unwrap();
        assert_eq!(decoded.tree_digest(), artifact_only.tree_digest());
        fs::remove_file(&interpreter).unwrap();
        fs::remove_dir_all(root.path().join("provider-cache")).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(cache.path()).unwrap();
        let first = compare(&decoded, &worker_python, &store, root.path()).unwrap();
        let second = compare(&artifact_only, &worker_python, &store, root.path()).unwrap();
        assert_comparison(&first);
        assert_comparison(&second);
        assert_ne!(
            first.materialization.as_json()["derivation_id"],
            second.materialization.as_json()["derivation_id"]
        );
        assert_eq!(
            first.materialization.as_json()["output"],
            second.materialization.as_json()["output"]
        );
        assert!(!root.path().join("provider-cache").exists());
        assert!(!interpreter.exists());
    }

    struct RegistryServer {
        address: std::net::SocketAddr,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
        selector: Value,
        bad_artifact: Value,
        bad_decoded: Value,
    }

    impl RegistryServer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let raw = include_str!("../../fixtures/datamonger/v1/index.json")
                .replace("127.0.0.1:PORT", &address.to_string());
            let mut bad_artifact: Value = serde_json::from_str(&raw).unwrap();
            bad_artifact["datasets"][0]["artifacts"][0]["sha256"] = json!("00".repeat(32));
            let mut bad_decoded: Value = serde_json::from_str(&raw).unwrap();
            bad_decoded["datasets"][0]["representation"]["expect"]["verification"][0]["digest"] =
                json!("00".repeat(32));
            let responses = BTreeMap::from([
                ("/index.json", raw.into_bytes()),
                (
                    "/bad-artifact.json",
                    serde_json::to_vec(&bad_artifact).unwrap(),
                ),
                (
                    "/bad-decoded.json",
                    serde_json::to_vec(&bad_decoded).unwrap(),
                ),
                (
                    "/data.csv",
                    include_bytes!("../../fixtures/datamonger/v1/data.csv").to_vec(),
                ),
            ]);
            let select = |path: &str| {
                let mut value = selection();
                value["registry"]["index_url"] = json!(format!("http://{address}{path}"));
                value["registry"]["index_sha256"] = json!(hex(ContentDigest::new(
                    Sha256::digest(&responses[path]).into()
                )));
                value
            };
            let selector = select("/index.json");
            let bad_artifact = select("/bad-artifact.json");
            let bad_decoded = select("/bad-decoded.json");
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = Arc::clone(&stop);
            let handle = thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let path = line.split_whitespace().nth(1).unwrap();
                    let body = &responses[path];
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(body).unwrap();
                }
            });
            Self {
                address,
                stop,
                handle: Some(handle),
                selector,
                bad_artifact,
                bad_decoded,
            }
        }
    }

    impl Drop for RegistryServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.address);
            self.handle.take().unwrap().join().unwrap();
        }
    }

    #[test]
    #[ignore = "requires an installed Datamonger client in METEWAND_DATAMONGER_PYTHON"]
    fn actual_client_enforces_pins_and_verification_then_runs_offline() {
        let interpreter = PathBuf::from(env::var_os("METEWAND_DATAMONGER_PYTHON").expect(
            "set METEWAND_DATAMONGER_PYTHON to an absolute interpreter with Datamonger installed",
        ));
        let root = tempfile::tempdir().unwrap();
        let server = RegistryServer::start();
        let mut bad_pin = server.selector.clone();
        bad_pin["registry"]["index_sha256"] = json!("00".repeat(32));
        let error = resolve(root.path(), &interpreter, bad_pin, false).unwrap_err();
        assert!(error.to_string().contains("SHA-256"), "{error}");
        let mut bad_version = server.selector.clone();
        bad_version["dataset"]["version"] = json!("2");
        let error = resolve(root.path(), &interpreter, bad_version, false).unwrap_err();
        assert!(error.to_string().contains("version"), "{error}");
        let error = resolve(
            root.path(),
            &interpreter,
            server.bad_artifact.clone(),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("SHA-256"), "{error}");
        let error =
            resolve(root.path(), &interpreter, server.bad_decoded.clone(), false).unwrap_err();
        assert!(
            error.to_string().contains("decoded SHA-256 mismatch"),
            "{error}"
        );
        let mut artifact_only = server.bad_decoded.clone();
        artifact_only["verification"] = json!("artifact");
        let source = resolve(root.path(), &interpreter, artifact_only, false).unwrap();
        assert_eq!(
            source.provenance().acquisition.as_ref().unwrap().as_json()["external_dataset"]["decoded_values"],
            "not_verified"
        );
        let source = resolve(root.path(), &interpreter, server.selector.clone(), false).unwrap();
        let selected = server.selector.clone();
        drop(server);
        let offline = resolve(root.path(), &interpreter, selected.clone(), true).unwrap();
        assert_eq!(source.tree_digest(), offline.tree_digest());
        fs::remove_dir_all(root.path().join("provider-cache")).unwrap();
        assert!(resolve(root.path(), &interpreter, selected, true).is_err());
        let cache = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(cache.path()).unwrap();
        let comparison = compare(&source, &python(), &store, root.path()).unwrap();
        assert_comparison(&comparison);
        source.verify().unwrap();
        assert_eq!(source.files()[0].size_bytes, 12);
    }
}
