//! Private construction and validation of dataset artifacts through processes.

use crate::{
    LOCAL_TREE_HASH_VERSION,
    local_tree::{LocalTreeHashError, hash_local_tree},
    local_worker::{LocalWorkerLaunchError, launch_trusted_local_worker},
    source_resolution::{ResolvedSource, SourceOrigin, SourceResolutionError},
    worker_output::{ValidatedDataset, WorkerOutputError, WorkerOutputValidator},
    worker_process::{WorkerLogLimits, WorkerProcessError, WorkerProcessOutput},
    worker_session::{PhaseTimeouts, WorkerSession, WorkerSessionError},
};
use metewand_core::{
    canonical::{CanonicalJsonError, CanonicalValue},
    identity::{IdentityError, canonical_identity_bytes, identify_record, record_id},
    materialization::{DatasetDerivationRecord, VerifiedSourceRecord, hex},
    measurements::Measurement,
    public_schemas::PublicSchemaCatalogError,
    records::{
        ContentDigest, DatasetConfigurationRecord, IdentifiedRecord, ProblemContractResource,
        RecordId, ResolvedLaunchRecord, ResolvedWorkerRecord,
    },
    schema::SchemaCatalog,
};
use metewand_protocol::{WorkerIdentity, WorkerRole};
use serde_json::json;
use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};
use tempfile::{Builder, TempDir};
use thiserror::Error;

/// Compatibility version of the standalone contract dataset validation command.
pub const DATASET_VALIDATION_COMMAND_VERSION: u32 = 1;

/// A contract-owned command that checks opaque payloads after schema validation.
///
/// The command receives `--metewand-dataset-validation-v1` and one canonical JSON
/// argument containing `version`, `contract_id`, `dataset_dir`, and `manifest`.
/// Exit zero accepts the dataset; every other status rejects it. Output is logs.
/// The launch and resolved identity must describe the same checked worker.
pub struct ContractDatasetValidator<'a> {
    pub contract: RecordId<ProblemContractResource>,
    pub launch: &'a ResolvedLaunchRecord,
    pub worker: &'a ResolvedWorkerRecord,
    pub timeout: Duration,
}

/// Fully resolved construction inputs for one dataset configuration.
pub struct DatasetMaterialization<'a> {
    pub configuration: &'a DatasetConfigurationRecord,
    /// Verified input to transform, or no input for a pure generator.
    pub source: Option<&'a ResolvedSource>,
    pub launch: &'a ResolvedLaunchRecord,
    pub worker: &'a ResolvedWorkerRecord,
    pub output_schema: &'a Path,
    pub schemas: &'a SchemaCatalog,
    pub validator: Option<ContractDatasetValidator<'a>>,
    pub private_root: &'a Path,
    pub timeouts: PhaseTimeouts,
    pub log_limits: WorkerLogLimits,
}

/// A private dataset whose output hash is recorded separately from its recipe.
#[derive(Debug)]
pub struct MaterializedDataset {
    /// Complete generation or transformation request, excluding worker startup.
    pub materialization_time_ns: Measurement,
    directory: TempDir,
    pub derivation: IdentifiedRecord<DatasetDerivationRecord>,
    pub output_digest: ContentDigest,
    pub manifest: ValidatedDataset,
    pub provenance: CanonicalValue,
    pub materializer_output: WorkerProcessOutput,
    pub validator_output: Option<WorkerProcessOutput>,
}

impl MaterializedDataset {
    #[must_use]
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
    /// Rechecks all output bytes before a later consumer uses this dataset.
    pub fn verify(&self) -> Result<(), MaterializationError> {
        if hash_local_tree(self.directory())? != self.output_digest {
            return Err(MaterializationError::ChangedInput);
        }
        Ok(())
    }
    #[must_use]
    pub fn into_directory(self) -> TempDir {
        self.directory
    }
}

/// Captures the source selection and semantic decoding evidence without host paths.
///
/// External receipts must contain an `external_dataset` object with a nonempty
/// `resolved_dataset_version`, object-valued `decoding_recipe`, and a
/// `decoded_values` status of `not_verified` or `provider_verified`.
pub fn verified_source_record(
    source: &ResolvedSource,
) -> Result<VerifiedSourceRecord, MaterializationError> {
    source.verify()?;
    let selection = match &source.provenance().origin {
        SourceOrigin::Repository { source_bundle } => {
            json!({"kind":"repository", "source_bundle":source_bundle})
        }
        SourceOrigin::PinnedUrl { url, sha256 } => {
            json!({"kind":"pinned_url", "url":url, "sha256":sha256.as_str()})
        }
        SourceOrigin::External { provider, selector } => {
            json!({"kind":"external", "provider":provider.as_str(), "selector":selector.as_json()})
        }
    };
    let decoding = if matches!(source.provenance().origin, SourceOrigin::External { .. }) {
        let value = source
            .provenance()
            .acquisition
            .as_ref()
            .and_then(|p| p.as_json().get("external_dataset"))
            .ok_or(MaterializationError::ExternalEvidence)?;
        let version = value
            .get("resolved_dataset_version")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .ok_or(MaterializationError::ExternalEvidence)?;
        let recipe = value
            .get("decoding_recipe")
            .filter(|v| v.is_object())
            .ok_or(MaterializationError::ExternalEvidence)?;
        let status = value
            .get("decoded_values")
            .and_then(|v| v.as_str())
            .filter(|v| matches!(*v, "not_verified" | "provider_verified"))
            .ok_or(MaterializationError::ExternalEvidence)?;
        Some(CanonicalValue::try_from(
            json!({"resolved_dataset_version":version,"decoding_recipe":recipe,"decoded_values":status}),
        )?)
    } else {
        None
    };
    Ok(VerifiedSourceRecord {
        selection: CanonicalValue::try_from(selection)?,
        tree_hash_version: LOCAL_TREE_HASH_VERSION,
        tree_digest: source.tree_digest(),
        decoding,
    })
}

/// Identifies verified construction inputs before deciding whether to reuse a
/// completed cache entry or execute the materializer.
pub fn identify_dataset_derivation(
    configuration: &DatasetConfigurationRecord,
    source: Option<&ResolvedSource>,
    worker: &ResolvedWorkerRecord,
) -> Result<IdentifiedRecord<DatasetDerivationRecord>, MaterializationError> {
    if !configuration.parameters.as_json().is_object() {
        return Err(MaterializationError::InvalidParameters);
    }
    Ok(identify_record(DatasetDerivationRecord {
        definition: configuration.definition,
        sources: source
            .map(verified_source_record)
            .transpose()?
            .into_iter()
            .collect(),
        materializer: worker.clone(),
        parameters: configuration.parameters.clone(),
        seed: configuration.seed,
    })?)
}

/// Executes a generator or transformation, closes its process, canonicalizes the
/// manifest, checks its complete inventory and schema, and optionally validates
/// binary payloads with a contract-owned command. Errors remove private output.
/// No durable cache publication occurs here.
pub fn materialize_dataset(
    input: DatasetMaterialization<'_>,
) -> Result<MaterializedDataset, MaterializationError> {
    text(input.private_root)?;
    if !input.configuration.parameters.as_json().is_object() {
        return Err(MaterializationError::InvalidParameters);
    }
    let derivation = identify_dataset_derivation(input.configuration, input.source, input.worker)?;
    let empty = Builder::new()
        .prefix("metewand-generator-input-")
        .tempdir_in(input.private_root)?;
    let source_dir = input
        .source
        .map(ResolvedSource::directory)
        .unwrap_or(empty.path());
    let source_digest = hash_local_tree(source_dir)?;
    let directory = Builder::new()
        .prefix("metewand-dataset-")
        .tempdir_in(input.private_root)?;
    let identity: WorkerIdentity = record_id(input.worker)?
        .to_string()
        .parse()
        .expect("resolved worker identity");
    let process = launch_trusted_local_worker(input.launch, input.private_root, input.log_limits)?;
    let mut session = WorkerSession::connect(
        process,
        WorkerRole::DatasetMaterializer,
        identity,
        &[],
        input.timeouts,
    )?;
    let sdk = session
        .negotiated()
        .sdk()
        .map(|sdk| json!({"name": sdk.name(), "version": sdk.version()}));
    let started = Instant::now();
    let response = session.materialize(
        text(source_dir)?,
        input.configuration.parameters.as_json().clone(),
        input.configuration.seed.value,
        text(directory.path())?,
    )?;
    let materialization_time_ns = Measurement::elapsed(started.elapsed(), true);
    let materializer_output = session.shutdown()?;
    if hash_local_tree(source_dir)? != source_digest {
        return Err(MaterializationError::ChangedInput);
    }
    let validator = WorkerOutputValidator::new(input.schemas)?;
    let validated = validator.validate_materialized_dataset(
        directory.path(),
        &response,
        input.output_schema,
    )?;
    let canonical_manifest = validated.manifest().to_canonical_bytes()?;
    fs::write(
        directory.path().join(validated.manifest_path()),
        canonical_manifest,
    )?;
    let before_validation = hash_local_tree(directory.path())?;
    let validator_output = input
        .validator
        .as_ref()
        .map(|hook| {
            validate_binary(
                hook,
                directory.path(),
                validated.manifest_path(),
                input.private_root,
                input.log_limits,
            )
        })
        .transpose()?;
    if hash_local_tree(directory.path())? != before_validation {
        return Err(MaterializationError::ChangedInput);
    }
    if let Some(source) = input.source {
        source.verify()?;
    }
    let manifest = validator.validate_materialized_dataset(
        directory.path(),
        &response,
        input.output_schema,
    )?;
    let output_digest = hash_local_tree(directory.path())?;
    if output_digest != before_validation {
        return Err(MaterializationError::ChangedInput);
    }
    let provenance = CanonicalValue::try_from(json!({
        "version":1,
        "materialization_time_ns": materialization_time_ns,
        "derivation_id": derivation.id,
        "derivation": serde_json::from_slice::<serde_json::Value>(&canonical_identity_bytes(&derivation.record)?).expect("canonical identity JSON"),
        "output": {"tree_hash_version": LOCAL_TREE_HASH_VERSION,"sha256":hex(output_digest)},
        "source": input.source.map(|source| json!({
            "raw_bytes_verified":true,
            "files":source.files().iter().map(|f| json!({"path":f.path.as_path(),"size_bytes":f.size_bytes,"sha256":hex(f.sha256)})).collect::<Vec<_>>(),
            "acquisition":source.provenance().acquisition.as_ref().map(CanonicalValue::as_json),
        })),
        "materializer_sdk":sdk,
        "worker_environment": materializer_output.environment,
        "validator_environment": validator_output.as_ref().and_then(|output| output.environment.as_ref()),
        "binary_validation": input.validator.as_ref().map(|hook| json!({"command_version":DATASET_VALIDATION_COMMAND_VERSION,"contract":hook.contract,"worker":record_id(hook.worker).expect("validated identity"),"status":"accepted"})),
    }))?;
    Ok(MaterializedDataset {
        materialization_time_ns,
        directory,
        derivation,
        output_digest,
        manifest,
        provenance,
        materializer_output,
        validator_output,
    })
}

fn text(path: &Path) -> Result<&str, MaterializationError> {
    path.to_str()
        .filter(|_| path.is_absolute())
        .ok_or(MaterializationError::InvalidPath)
}

fn validate_binary(
    hook: &ContractDatasetValidator<'_>,
    directory: &Path,
    manifest: &Path,
    parent: &Path,
    logs: WorkerLogLimits,
) -> Result<WorkerProcessOutput, MaterializationError> {
    let request = CanonicalValue::try_from(json!({"version":DATASET_VALIDATION_COMMAND_VERSION,"contract_id":hook.contract,"dataset_dir":text(directory)?,"manifest":manifest}))?.to_canonical_bytes()?;
    let mut launch = hook.launch.clone();
    launch
        .args
        .push("--metewand-dataset-validation-v1".to_owned());
    launch
        .args
        .push(String::from_utf8(request).expect("canonical UTF-8"));
    let mut process = launch_trusted_local_worker(&launch, parent, logs)?;
    let started = Instant::now();
    loop {
        if let Some(status) = process.try_wait()? {
            let output = process.wait()?;
            return if status.success() {
                Ok(output)
            } else {
                Err(MaterializationError::BinaryRejected)
            };
        }
        if started.elapsed() >= hook.timeout {
            process.kill()?;
            let _ = process.wait();
            return Err(MaterializationError::BinaryTimeout);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MaterializationError {
    #[error("dataset parameters must be a canonical object")]
    InvalidParameters,
    #[error("materialization requires absolute UTF-8 paths")]
    InvalidPath,
    #[error(
        "external dataset receipt lacks a resolved version, decoding recipe, or verification status"
    )]
    ExternalEvidence,
    #[error("dataset source or output changed during materialization or validation")]
    ChangedInput,
    #[error("contract-owned binary dataset validation rejected the dataset")]
    BinaryRejected,
    #[error("contract-owned binary dataset validation timed out")]
    BinaryTimeout,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Source(#[from] SourceResolutionError),
    #[error(transparent)]
    Tree(#[from] LocalTreeHashError),
    #[error(transparent)]
    Launch(#[from] LocalWorkerLaunchError),
    #[error(transparent)]
    Process(#[from] WorkerProcessError),
    #[error(transparent)]
    Session(#[from] WorkerSessionError),
    #[error(transparent)]
    Output(#[from] WorkerOutputError),
    #[error(transparent)]
    PublicSchemas(#[from] PublicSchemaCatalogError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
}
