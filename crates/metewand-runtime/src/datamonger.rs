//! Optional Datamonger acquisition through an explicitly selected Python process.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use metewand_core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    manifest::{RepositoryPath, Sha256Digest},
    public_schemas::{PublicSchema, public_schema_catalog},
    records::ContentDigest,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    source_resolution::{
        AcquiredSource, SourceAcquirer, SourceFile, SourceRequest, SourceResolutionError,
    },
    worker_process::{PosixWorkerProcess, WorkerLogLimits},
};

/// Compatibility version of the selector and standalone acquisition command.
pub const DATAMONGER_ADAPTER_VERSION: u32 = 1;

const BRIDGE: &str = include_str!("datamonger_bridge.py");
const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;

/// Explicit, bounded acquisition using Datamonger's public Python API.
///
/// No Python package is required until this backend is selected. Each call uses
/// a private working directory, isolated Python, an empty inherited environment,
/// null stdin, and bounded log capture. The caller owns the provider cache.
pub struct DatamongerAcquirer {
    python: PathBuf,
    cache_dir: PathBuf,
    timeout: Duration,
    max_bytes: u64,
    offline: bool,
}

fn failure(error: impl std::fmt::Display) -> SourceResolutionError {
    SourceResolutionError::Acquisition(Box::new(io::Error::other(error.to_string())))
}

impl DatamongerAcquirer {
    /// Selects an absolute interpreter and provider cache without creating either.
    /// `max_bytes` bounds the sum of pinned raw artifact sizes. `offline` forbids
    /// Datamonger from retrieving uncached indexes or artifacts.
    ///
    /// # Errors
    ///
    /// Rejects relative paths, zero deadlines, and invalid byte budgets.
    pub fn new(
        python: &Path,
        cache_dir: &Path,
        timeout: Duration,
        max_bytes: u64,
        offline: bool,
    ) -> Result<Self, SourceResolutionError> {
        if !python.is_absolute() || !cache_dir.is_absolute() {
            return Err(failure(
                "Datamonger interpreter and cache paths must be absolute",
            ));
        }
        if timeout.is_zero() || max_bytes == 0 || max_bytes > MAX_SAFE_INTEGER {
            return Err(failure(
                "Datamonger requires a positive deadline and safe byte budget",
            ));
        }
        Ok(Self {
            python: python.to_owned(),
            cache_dir: cache_dir.to_owned(),
            timeout,
            max_bytes,
            offline,
        })
    }
}

impl SourceAcquirer for DatamongerAcquirer {
    fn acquire(
        &self,
        request: &SourceRequest<'_>,
        output_dir: &Path,
    ) -> Result<AcquiredSource, SourceResolutionError> {
        let SourceRequest::External { provider, selector } = request else {
            return Err(failure("Datamonger requires an external datamonger source"));
        };
        if provider.as_str() != "datamonger" {
            return Err(failure("Datamonger requires provider `datamonger`"));
        }
        let schemas = public_schema_catalog().map_err(failure)?;
        schemas
            .validate(
                PublicSchema::DatamongerSelector.repository_path(),
                selector.as_json(),
            )
            .map_err(|error| failure(format!("invalid Datamonger selector: {error}")))?;
        let registry = &selector.as_json()["registry"];
        let url = reqwest::Url::parse(registry["index_url"].as_str().expect("validated URL"))
            .map_err(|error| failure(format!("invalid Datamonger selector URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(failure(
                "Datamonger selector requires an HTTP(S) index URL without credentials or a fragment",
            ));
        }
        let output_dir = fs::canonicalize(output_dir)?;
        let work = tempfile::Builder::new()
            .prefix("metewand-datamonger-")
            .tempdir_in(output_dir.parent().expect("source parent"))?;
        let receipt_path = work.path().join("receipt.json");
        let command_request = CanonicalValue::try_from(json!({
            "version": DATAMONGER_ADAPTER_VERSION,
            "selector": selector.as_json(),
            "cache_dir": self.cache_dir,
            "output_dir": output_dir,
            "receipt_path": receipt_path,
            "offline": self.offline,
            "max_bytes": self.max_bytes,
        }))
        .map_err(failure)?;
        schemas
            .validate(
                PublicSchema::DatamongerAcquisitionRequest.repository_path(),
                command_request.as_json(),
            )
            .map_err(failure)?;
        let mut command = Command::new(&self.python);
        command
            .args(["-I", "-c", BRIDGE])
            .arg(
                String::from_utf8(command_request.to_canonical_bytes().map_err(failure)?)
                    .expect("canonical UTF-8"),
            )
            .env_clear()
            .stdin(Stdio::null())
            .current_dir(work.path());
        let mut process = PosixWorkerProcess::spawn(command, WorkerLogLimits::new(8192, 8192))
            .map_err(failure)?;
        let started = Instant::now();
        loop {
            if let Some(status) = process.try_wait().map_err(failure)? {
                let output = process.wait().map_err(failure)?;
                if !status.success() {
                    return Err(failure(format!(
                        "Datamonger acquisition failed ({status}): {}",
                        String::from_utf8_lossy(output.stderr.bytes())
                    )));
                }
                break;
            }
            if started.elapsed() >= self.timeout {
                process.kill().map_err(failure)?;
                let _ = process.wait();
                return Err(failure("Datamonger acquisition exceeded its deadline"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let metadata = fs::symlink_metadata(&receipt_path)?;
        if !metadata.is_file() || metadata.len() > MAX_RECEIPT_BYTES {
            return Err(failure(
                "Datamonger receipt must be a regular file of at most 1 MiB",
            ));
        }
        let mut bytes = Vec::new();
        File::open(&receipt_path)?
            .take(MAX_RECEIPT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECEIPT_BYTES {
            return Err(failure("Datamonger receipt exceeds 1 MiB"));
        }
        let receipt = CanonicalValue::from_slice(&bytes).map_err(failure)?;
        schemas
            .validate(
                PublicSchema::DatamongerAcquisitionResponse.repository_path(),
                receipt.as_json(),
            )
            .map_err(failure)?;
        validate_receipt(selector.as_json(), receipt.as_json(), self.max_bytes)
    }
}

#[derive(Deserialize)]
struct Artifact {
    name: String,
    path: RepositoryPath,
    size_bytes: u64,
    sha256: Sha256Digest,
}

fn validate_receipt(
    selector: &Value,
    receipt: &Value,
    max_bytes: u64,
) -> Result<AcquiredSource, SourceResolutionError> {
    let dataset = &selector["dataset"];
    let dataset_id = format!(
        "{}:{}@{}",
        dataset["source"].as_str().expect("source"),
        dataset["name"].as_str().expect("name"),
        dataset["version"].as_str().expect("version")
    );
    if receipt["registry"] != selector["registry"]
        || receipt["dataset_id"] != dataset_id
        || receipt["dataset_version"] != dataset["version"]
        || receipt["verification"] != selector["verification"]
    {
        return Err(failure(
            "Datamonger receipt differs from the pinned selector or verification request",
        ));
    }
    if receipt["verification"] == "decoded" {
        let verification = &receipt["representation"]["expect"]["verification"];
        if !verification.as_array().is_some_and(|records| {
            records.iter().any(|record| {
                record["algorithm"] == "sha256"
                    && record["canonical_form"] == receipt["canonical_form"]
                    && record["digest"] == receipt["canonical_digest"]
            })
        }) {
            return Err(failure(
                "Datamonger decoded verification does not match the registry recipe",
            ));
        }
    }
    let artifacts: Vec<Artifact> =
        serde_json::from_value(receipt["artifacts"].clone()).map_err(failure)?;
    let mut total = 0_u64;
    let mut files = Vec::new();
    for artifact in artifacts {
        total = total
            .checked_add(artifact.size_bytes)
            .filter(|total| *total <= max_bytes)
            .ok_or_else(|| failure("Datamonger artifacts exceed the byte budget"))?;
        if artifact.path.as_path() != Path::new("artifacts").join(&artifact.name) {
            return Err(failure(
                "Datamonger export path differs from its artifact name",
            ));
        }
        let mut digest = [0_u8; 32];
        for (byte, pair) in digest
            .iter_mut()
            .zip(artifact.sha256.as_str().as_bytes().as_chunks::<2>().0)
        {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).expect("hex UTF-8"), 16)
                .expect("validated SHA-256");
        }
        files.push(SourceFile {
            path: artifact.path,
            size_bytes: artifact.size_bytes,
            sha256: ContentDigest::new(digest),
        });
    }
    let decoding_artifacts: Vec<_> = receipt["artifacts"].as_array().expect("artifacts").iter()
        .map(|artifact| json!({"name":artifact["name"],"format":artifact["format"],"compression":artifact["compression"]})).collect();
    Ok(AcquiredSource {
        files,
        provenance: CanonicalValue::try_from(json!({
            "datamonger": receipt,
            "external_dataset": {
                "resolved_dataset_version": receipt["dataset_version"],
                "decoding_recipe": {"representation": receipt["representation"], "artifacts": decoding_artifacts, "canonical_form": receipt["canonical_form"], "canonical_digest": receipt["canonical_digest"]},
                "decoded_values": if receipt["verification"] == "decoded" { "provider_verified" } else { "not_verified" },
            },
        })).map_err(failure)?,
    })
}
