//! Embedded Metewand-owned JSON Schemas.
//!
//! Checked-in schema documents are the public contract. Embedding those exact
//! documents gives the CLI and other Rust consumers a single source of truth
//! without allowing schema validation to retrieve network resources.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use thiserror::Error;

use crate::{
    canonical::{CanonicalJsonError, CanonicalValue},
    schema::{SchemaCatalog, SchemaCatalogError},
};

const COMMON_PATH: &str = "schemas/v1/common.schema.json";
const COMMON_SOURCE: &str = include_str!("../../../schemas/v1/common.schema.json");
static CATALOG: Mutex<Option<SchemaCatalog>> = Mutex::new(None);

/// A public version-1 schema entry point.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PublicSchema {
    /// Build-time inputs bound to an executable hash.
    BuildEvidence,
    ProfileObservationPolicy,
    BufferedManifest,
    BufferedTracePoint,
    ProfileObservation,

    /// Realized worker settings with redacted secret channels.
    WorkerEnvironment,
    /// Envelope used for command events, results, and diagnostics.
    MachineOutput,
    /// Lifecycle events and terminal reports for the run command.
    RunOutput,
    /// Read-only status reports and stable identity explanations.
    InspectionOutput,
    /// Rebuildable result index records.
    ExportRecord,
    /// Tidy metric rows retaining attempts and observations.
    ExportRow,
    /// Export receipts and adjacent lineage manifests.
    ExportOutput,
    /// Exact cache verification, root, and garbage collection receipts.
    CacheOutput,
    /// Typed benchmark manifest.
    Manifest,
    /// Problem and fairness contract.
    ProblemContract,
    /// Gate-1 one-shot observation policy.
    OneShotObservationPolicy,
    /// Hash-complete artifact directory manifest.
    ArtifactManifest,
    /// Completed content-addressed tree inventory.
    ArtifactCacheObject,
    /// Dataset construction mapped to its verified output.
    DatasetCacheEntry,
    /// Process-held artifact lease.
    ArtifactCacheLease,
    /// Canonical result envelope.
    ResultManifest,
    /// Independently finalized run observation.
    Observation,
    /// Terminal execution attempt.
    Attempt,
    /// Evaluator-owned metrics envelope.
    Metrics,
    /// Construction inputs and separately observed dataset output hash.
    DatasetMaterialization,
    /// Versioned request passed to a contract-owned binary dataset checker.
    DatasetValidationRequest,
    /// Immutable Datamonger registry, dataset, and verification selection.
    DatamongerSelector,
    /// Versioned command request to the optional Datamonger bridge.
    DatamongerAcquisitionRequest,
    /// Verified provider evidence and exported raw artifact inventory.
    DatamongerAcquisitionResponse,
    /// Scientific values and unhashed file declarations finalized by the runtime.
    ManagedOutputDraft,
    /// Worker session handshake or one-shot exchange message.
    WorkerProtocolMessage,
}

/// Every public schema entry point in stable presentation order.
pub const PUBLIC_SCHEMAS: [PublicSchema; 31] = [
    PublicSchema::BuildEvidence,
    PublicSchema::ProfileObservationPolicy,
    PublicSchema::BufferedManifest,
    PublicSchema::BufferedTracePoint,
    PublicSchema::ProfileObservation,
    PublicSchema::MachineOutput,
    PublicSchema::RunOutput,
    PublicSchema::InspectionOutput,
    PublicSchema::ExportRecord,
    PublicSchema::ExportRow,
    PublicSchema::ExportOutput,
    PublicSchema::CacheOutput,
    PublicSchema::Manifest,
    PublicSchema::ProblemContract,
    PublicSchema::OneShotObservationPolicy,
    PublicSchema::ArtifactManifest,
    PublicSchema::ArtifactCacheObject,
    PublicSchema::DatasetCacheEntry,
    PublicSchema::ArtifactCacheLease,
    PublicSchema::ResultManifest,
    PublicSchema::Observation,
    PublicSchema::Attempt,
    PublicSchema::Metrics,
    PublicSchema::DatasetMaterialization,
    PublicSchema::DatasetValidationRequest,
    PublicSchema::DatamongerSelector,
    PublicSchema::DatamongerAcquisitionRequest,
    PublicSchema::DatamongerAcquisitionResponse,
    PublicSchema::ManagedOutputDraft,
    PublicSchema::WorkerProtocolMessage,
    PublicSchema::WorkerEnvironment,
];

impl PublicSchema {
    /// Returns the stable command-facing name of this schema.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::BuildEvidence => "build-evidence",
            Self::ProfileObservationPolicy => "profile-observation-policy",
            Self::BufferedManifest => "buffered-manifest",
            Self::BufferedTracePoint => "buffered-trace-point",
            Self::ProfileObservation => "profile-observation",
            Self::WorkerEnvironment => "worker-environment",
            Self::ArtifactCacheObject => "artifact-cache-object",
            Self::DatasetCacheEntry => "dataset-cache-entry",
            Self::ArtifactCacheLease => "artifact-cache-lease",
            Self::MachineOutput => "machine-output",
            Self::RunOutput => "run-output",
            Self::InspectionOutput => "inspection-output",
            Self::ExportRecord => "export-record",
            Self::ExportRow => "export-row",
            Self::ExportOutput => "export-output",
            Self::CacheOutput => "cache-output",
            Self::Manifest => "manifest",
            Self::ProblemContract => "problem-contract",
            Self::OneShotObservationPolicy => "one-shot-observation-policy",
            Self::ArtifactManifest => "artifact-manifest",
            Self::ResultManifest => "result-manifest",
            Self::Observation => "observation",
            Self::Attempt => "attempt",
            Self::Metrics => "metrics",
            Self::DatasetMaterialization => "dataset-materialization",
            Self::DatasetValidationRequest => "dataset-validation-request",
            Self::DatamongerSelector => "datamonger-selector",
            Self::DatamongerAcquisitionRequest => "datamonger-acquisition-request",
            Self::DatamongerAcquisitionResponse => "datamonger-acquisition-response",
            Self::ManagedOutputDraft => "managed-output-draft",
            Self::WorkerProtocolMessage => "worker-protocol-message",
        }
    }

    /// Finds a public schema by its stable command-facing name.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        PUBLIC_SCHEMAS
            .into_iter()
            .find(|schema| schema.slug() == slug)
    }

    /// Returns the checked-in repository path of this schema.
    #[must_use]
    pub fn repository_path(self) -> &'static Path {
        Path::new(match self {
            Self::BuildEvidence => "schemas/v1/build-evidence.schema.json",
            Self::ProfileObservationPolicy => "schemas/v1/profile-observation-policy.schema.json",
            Self::BufferedManifest => "schemas/v1/buffered-manifest.schema.json",
            Self::BufferedTracePoint => "schemas/v1/buffered-trace-point.schema.json",
            Self::ProfileObservation => "schemas/v1/profile-observation.schema.json",
            Self::WorkerEnvironment => "schemas/v1/worker-environment.schema.json",
            Self::ArtifactCacheObject => "schemas/v1/artifact-cache-object.schema.json",
            Self::DatasetCacheEntry => "schemas/v1/dataset-cache-entry.schema.json",
            Self::ArtifactCacheLease => "schemas/v1/artifact-cache-lease.schema.json",
            Self::MachineOutput => "schemas/v1/machine-output.schema.json",
            Self::RunOutput => "schemas/v1/run-output.schema.json",
            Self::InspectionOutput => "schemas/v1/inspection-output.schema.json",
            Self::ExportRecord => "schemas/v1/export-record.schema.json",
            Self::ExportRow => "schemas/v1/export-row.schema.json",
            Self::ExportOutput => "schemas/v1/export-output.schema.json",
            Self::CacheOutput => "schemas/v1/cache-output.schema.json",
            Self::Manifest => "schemas/v1/manifest.schema.json",
            Self::ProblemContract => "schemas/v1/problem-contract.schema.json",
            Self::OneShotObservationPolicy => "schemas/v1/one-shot-observation-policy.schema.json",
            Self::ArtifactManifest => "schemas/v1/artifact-manifest.schema.json",
            Self::ResultManifest => "schemas/v1/result-manifest.schema.json",
            Self::Observation => "schemas/v1/observation.schema.json",
            Self::Attempt => "schemas/v1/attempt.schema.json",
            Self::Metrics => "schemas/v1/metrics.schema.json",
            Self::DatasetMaterialization => "schemas/v1/dataset-materialization.schema.json",
            Self::DatasetValidationRequest => "schemas/v1/dataset-validation-request.schema.json",
            Self::DatamongerSelector => "schemas/v1/datamonger-selector.schema.json",
            Self::DatamongerAcquisitionRequest => {
                "schemas/v1/datamonger-acquisition-request.schema.json"
            }
            Self::DatamongerAcquisitionResponse => {
                "schemas/v1/datamonger-acquisition-response.schema.json"
            }
            Self::ManagedOutputDraft => "schemas/v1/managed-output-draft.schema.json",
            Self::WorkerProtocolMessage => "schemas/v1/worker-protocol-message.schema.json",
        })
    }

    /// Returns the stable logical identifier declared by this schema.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::BuildEvidence => "metewand://schemas/v1/build-evidence.schema.json",
            Self::ProfileObservationPolicy => {
                "metewand://schemas/v1/profile-observation-policy.schema.json"
            }
            Self::BufferedManifest => "metewand://schemas/v1/buffered-manifest.schema.json",
            Self::BufferedTracePoint => "metewand://schemas/v1/buffered-trace-point.schema.json",
            Self::ProfileObservation => "metewand://schemas/v1/profile-observation.schema.json",
            Self::WorkerEnvironment => "metewand://schemas/v1/worker-environment.schema.json",
            Self::ArtifactCacheObject => "metewand://schemas/v1/artifact-cache-object.schema.json",
            Self::DatasetCacheEntry => "metewand://schemas/v1/dataset-cache-entry.schema.json",
            Self::ArtifactCacheLease => "metewand://schemas/v1/artifact-cache-lease.schema.json",
            Self::MachineOutput => "metewand://schemas/v1/machine-output.schema.json",
            Self::RunOutput => "metewand://schemas/v1/run-output.schema.json",
            Self::InspectionOutput => "metewand://schemas/v1/inspection-output.schema.json",
            Self::ExportRecord => "metewand://schemas/v1/export-record.schema.json",
            Self::ExportRow => "metewand://schemas/v1/export-row.schema.json",
            Self::ExportOutput => "metewand://schemas/v1/export-output.schema.json",
            Self::CacheOutput => "metewand://schemas/v1/cache-output.schema.json",
            Self::Manifest => "metewand://schemas/v1/manifest.schema.json",
            Self::ProblemContract => "metewand://schemas/v1/problem-contract.schema.json",
            Self::OneShotObservationPolicy => {
                "metewand://schemas/v1/one-shot-observation-policy.schema.json"
            }
            Self::ArtifactManifest => "metewand://schemas/v1/artifact-manifest.schema.json",
            Self::ResultManifest => "metewand://schemas/v1/result-manifest.schema.json",
            Self::Observation => "metewand://schemas/v1/observation.schema.json",
            Self::Attempt => "metewand://schemas/v1/attempt.schema.json",
            Self::Metrics => "metewand://schemas/v1/metrics.schema.json",
            Self::DatasetMaterialization => {
                "metewand://schemas/v1/dataset-materialization.schema.json"
            }
            Self::DatasetValidationRequest => {
                "metewand://schemas/v1/dataset-validation-request.schema.json"
            }
            Self::DatamongerSelector => "metewand://schemas/v1/datamonger-selector.schema.json",
            Self::DatamongerAcquisitionRequest => {
                "metewand://schemas/v1/datamonger-acquisition-request.schema.json"
            }
            Self::DatamongerAcquisitionResponse => {
                "metewand://schemas/v1/datamonger-acquisition-response.schema.json"
            }
            Self::ManagedOutputDraft => "metewand://schemas/v1/managed-output-draft.schema.json",
            Self::WorkerProtocolMessage => {
                "metewand://schemas/v1/worker-protocol-message.schema.json"
            }
        }
    }

    /// Returns the exact checked-in JSON Schema document.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::BuildEvidence => include_str!("../../../schemas/v1/build-evidence.schema.json"),
            Self::ProfileObservationPolicy => {
                include_str!("../../../schemas/v1/profile-observation-policy.schema.json")
            }
            Self::BufferedManifest => {
                include_str!("../../../schemas/v1/buffered-manifest.schema.json")
            }
            Self::BufferedTracePoint => {
                include_str!("../../../schemas/v1/buffered-trace-point.schema.json")
            }
            Self::ProfileObservation => {
                include_str!("../../../schemas/v1/profile-observation.schema.json")
            }

            Self::WorkerEnvironment => {
                include_str!("../../../schemas/v1/worker-environment.schema.json")
            }
            Self::ArtifactCacheObject => {
                include_str!("../../../schemas/v1/artifact-cache-object.schema.json")
            }
            Self::DatasetCacheEntry => {
                include_str!("../../../schemas/v1/dataset-cache-entry.schema.json")
            }
            Self::ArtifactCacheLease => {
                include_str!("../../../schemas/v1/artifact-cache-lease.schema.json")
            }
            Self::MachineOutput => {
                include_str!("../../../schemas/v1/machine-output.schema.json")
            }
            Self::RunOutput => include_str!("../../../schemas/v1/run-output.schema.json"),
            Self::InspectionOutput => {
                include_str!("../../../schemas/v1/inspection-output.schema.json")
            }
            Self::ExportRecord => include_str!("../../../schemas/v1/export-record.schema.json"),
            Self::ExportRow => include_str!("../../../schemas/v1/export-row.schema.json"),
            Self::ExportOutput => include_str!("../../../schemas/v1/export-output.schema.json"),
            Self::CacheOutput => include_str!("../../../schemas/v1/cache-output.schema.json"),
            Self::Manifest => include_str!("../../../schemas/v1/manifest.schema.json"),
            Self::ProblemContract => {
                include_str!("../../../schemas/v1/problem-contract.schema.json")
            }
            Self::OneShotObservationPolicy => {
                include_str!("../../../schemas/v1/one-shot-observation-policy.schema.json")
            }
            Self::ArtifactManifest => {
                include_str!("../../../schemas/v1/artifact-manifest.schema.json")
            }
            Self::ResultManifest => {
                include_str!("../../../schemas/v1/result-manifest.schema.json")
            }
            Self::Observation => include_str!("../../../schemas/v1/observation.schema.json"),
            Self::Attempt => include_str!("../../../schemas/v1/attempt.schema.json"),
            Self::Metrics => include_str!("../../../schemas/v1/metrics.schema.json"),
            Self::DatasetMaterialization => {
                include_str!("../../../schemas/v1/dataset-materialization.schema.json")
            }
            Self::DatasetValidationRequest => {
                include_str!("../../../schemas/v1/dataset-validation-request.schema.json")
            }
            Self::DatamongerSelector => {
                include_str!("../../../schemas/v1/datamonger-selector.schema.json")
            }
            Self::DatamongerAcquisitionRequest => {
                include_str!("../../../schemas/v1/datamonger-acquisition-request.schema.json")
            }
            Self::DatamongerAcquisitionResponse => {
                include_str!("../../../schemas/v1/datamonger-acquisition-response.schema.json")
            }
            Self::ManagedOutputDraft => {
                include_str!("../../../schemas/v1/managed-output-draft.schema.json")
            }
            Self::WorkerProtocolMessage => {
                include_str!("../../../schemas/v1/worker-protocol-message.schema.json")
            }
        }
    }
}

/// Returns every resource required to compile the public schema catalog.
pub fn public_schema_resources() -> impl Iterator<Item = (&'static Path, &'static str)> {
    std::iter::once((Path::new(COMMON_PATH), COMMON_SOURCE)).chain(
        PUBLIC_SCHEMAS
            .into_iter()
            .map(|schema| (schema.repository_path(), schema.source())),
    )
}

/// Returns a shared offline catalog containing every version-1 public schema.
///
/// The embedded documents cannot change during a process's lifetime. Compile
/// them once and return inexpensive clones sharing the immutable validators.
///
/// # Errors
///
/// Returns an error if an embedded document is outside Metewand's canonical
/// JSON domain or if the complete catalog cannot be compiled offline.
pub fn public_schema_catalog() -> Result<SchemaCatalog, PublicSchemaCatalogError> {
    let mut cached = CATALOG.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(catalog) = cached.as_ref() {
        return Ok(catalog.clone());
    }

    let documents = public_schema_resources()
        .map(|(path, source)| {
            let canonical = CanonicalValue::from_slice(source.as_bytes()).map_err(|error| {
                PublicSchemaCatalogError::InvalidDocument {
                    path: path.to_path_buf(),
                    error,
                }
            })?;
            Ok((path.to_path_buf(), canonical.as_json().clone()))
        })
        .collect::<Result<Vec<_>, PublicSchemaCatalogError>>()?;

    let catalog = SchemaCatalog::try_new(documents)?;
    *cached = Some(catalog.clone());
    Ok(catalog)
}

/// An error encountered while loading the embedded public schema catalog.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PublicSchemaCatalogError {
    /// A checked-in schema document is outside the canonical JSON domain.
    #[error("public schema `{path}` is not valid canonical-domain JSON: {error}")]
    InvalidDocument {
        /// Repository path of the invalid schema document.
        path: PathBuf,
        /// Canonical JSON parsing failure.
        #[source]
        error: CanonicalJsonError,
    },

    /// The complete offline catalog failed validation or compilation.
    #[error(transparent)]
    Catalog(#[from] SchemaCatalogError),
}
