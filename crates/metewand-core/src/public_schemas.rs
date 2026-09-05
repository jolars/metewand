//! Embedded Metewand-owned JSON Schemas.
//!
//! Checked-in schema documents are the public contract. Embedding those exact
//! documents gives the CLI and other Rust consumers a single source of truth
//! without allowing schema validation to retrieve network resources.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{
    canonical::{CanonicalJsonError, CanonicalValue},
    schema::{SchemaCatalog, SchemaCatalogError},
};

const COMMON_PATH: &str = "schemas/v1/common.schema.json";
const COMMON_SOURCE: &str = include_str!("../../../schemas/v1/common.schema.json");

/// A public version-1 schema entry point.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PublicSchema {
    /// Envelope used for command events, results, and diagnostics.
    MachineOutput,
    /// Typed benchmark manifest.
    Manifest,
    /// Problem and fairness contract.
    ProblemContract,
    /// Gate-1 one-shot observation policy.
    OneShotObservationPolicy,
    /// Hash-complete artifact directory manifest.
    ArtifactManifest,
    /// Canonical result envelope.
    ResultManifest,
    /// Independently finalized run observation.
    Observation,
    /// Terminal execution attempt.
    Attempt,
    /// Evaluator-owned metrics envelope.
    Metrics,
}

/// Every public schema entry point in stable presentation order.
pub const PUBLIC_SCHEMAS: [PublicSchema; 9] = [
    PublicSchema::MachineOutput,
    PublicSchema::Manifest,
    PublicSchema::ProblemContract,
    PublicSchema::OneShotObservationPolicy,
    PublicSchema::ArtifactManifest,
    PublicSchema::ResultManifest,
    PublicSchema::Observation,
    PublicSchema::Attempt,
    PublicSchema::Metrics,
];

impl PublicSchema {
    /// Returns the stable command-facing name of this schema.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::MachineOutput => "machine-output",
            Self::Manifest => "manifest",
            Self::ProblemContract => "problem-contract",
            Self::OneShotObservationPolicy => "one-shot-observation-policy",
            Self::ArtifactManifest => "artifact-manifest",
            Self::ResultManifest => "result-manifest",
            Self::Observation => "observation",
            Self::Attempt => "attempt",
            Self::Metrics => "metrics",
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
            Self::MachineOutput => "schemas/v1/machine-output.schema.json",
            Self::Manifest => "schemas/v1/manifest.schema.json",
            Self::ProblemContract => "schemas/v1/problem-contract.schema.json",
            Self::OneShotObservationPolicy => "schemas/v1/one-shot-observation-policy.schema.json",
            Self::ArtifactManifest => "schemas/v1/artifact-manifest.schema.json",
            Self::ResultManifest => "schemas/v1/result-manifest.schema.json",
            Self::Observation => "schemas/v1/observation.schema.json",
            Self::Attempt => "schemas/v1/attempt.schema.json",
            Self::Metrics => "schemas/v1/metrics.schema.json",
        })
    }

    /// Returns the stable logical identifier declared by this schema.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::MachineOutput => "metewand://schemas/v1/machine-output.schema.json",
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
        }
    }

    /// Returns the exact checked-in JSON Schema document.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::MachineOutput => {
                include_str!("../../../schemas/v1/machine-output.schema.json")
            }
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

/// Builds an offline catalog containing every version-1 public schema.
///
/// # Errors
///
/// Returns an error if an embedded document is outside Metewand's canonical
/// JSON domain or if the complete catalog cannot be compiled offline.
pub fn public_schema_catalog() -> Result<SchemaCatalog, PublicSchemaCatalogError> {
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

    SchemaCatalog::try_new(documents).map_err(PublicSchemaCatalogError::Catalog)
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
