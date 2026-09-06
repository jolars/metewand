//! Deterministic component-seed derivation.

use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    canonical::{CanonicalJsonError, CanonicalValue},
    manifest::Name,
    records::{
        ContentDigest, DatasetConfigurationRecord, DatasetDefinitionRecord, DerivedSeedRecord,
        ProblemConfigurationRecord, RecordId,
    },
};

/// Domain separator for every version-1 component-seed transcript.
pub const SEED_DERIVATION_DOMAIN: &str = "metewand-seed-v1";

const DATASET_ROLE: &str = "dataset";
const IMPLEMENTATION_ROLE: &str = "implementation";
const SCHEDULING_ROLE: &str = "scheduling";

/// Derives the seed used to acquire or materialize one dataset configuration.
///
/// The definition identity and resolved parameters make every data-affecting
/// input explicit. Expansion position and unrelated manifest members do not
/// participate.
///
/// # Errors
///
/// Returns an error when an integer is outside the version-1 canonical JSON
/// domain or canonical serialization unexpectedly fails.
pub fn derive_dataset_seed(
    experiment_seed: u64,
    dataset_definition: RecordId<DatasetDefinitionRecord>,
    dataset_parameters: &CanonicalValue,
) -> Result<DerivedSeedRecord, SeedDerivationError> {
    Ok(derive_seed(
        DATASET_ROLE,
        &canonical_dataset_seed_fields(experiment_seed, dataset_definition, dataset_parameters)?,
    ))
}

/// Returns the exact canonical field bytes hashed for a dataset seed.
///
/// # Errors
///
/// Returns an error when an integer is outside the version-1 canonical JSON
/// domain or canonical serialization unexpectedly fails.
pub fn canonical_dataset_seed_fields(
    experiment_seed: u64,
    dataset_definition: RecordId<DatasetDefinitionRecord>,
    dataset_parameters: &CanonicalValue,
) -> Result<Vec<u8>, SeedDerivationError> {
    canonical_fields(json!({
        "experiment_seed": experiment_seed,
        "dataset_definition": dataset_definition.to_string(),
        "dataset_parameters": dataset_parameters.as_json(),
    }))
}

/// Derives the seed shared by implementations in one replication block.
///
/// The implementation identity is deliberately absent, so implementations
/// solving the same configured problem receive the same numeric seed.
///
/// # Errors
///
/// Returns an error when an integer is outside the version-1 canonical JSON
/// domain or canonical serialization unexpectedly fails.
pub fn derive_implementation_seed(
    experiment_seed: u64,
    dataset_configuration: RecordId<DatasetConfigurationRecord>,
    problem_configuration: RecordId<ProblemConfigurationRecord>,
    implementation_repetition: u64,
) -> Result<DerivedSeedRecord, SeedDerivationError> {
    Ok(derive_seed(
        IMPLEMENTATION_ROLE,
        &canonical_implementation_seed_fields(
            experiment_seed,
            dataset_configuration,
            problem_configuration,
            implementation_repetition,
        )?,
    ))
}

/// Returns the exact canonical field bytes hashed for an implementation seed.
///
/// # Errors
///
/// Returns an error when an integer is outside the version-1 canonical JSON
/// domain or canonical serialization unexpectedly fails.
pub fn canonical_implementation_seed_fields(
    experiment_seed: u64,
    dataset_configuration: RecordId<DatasetConfigurationRecord>,
    problem_configuration: RecordId<ProblemConfigurationRecord>,
    implementation_repetition: u64,
) -> Result<Vec<u8>, SeedDerivationError> {
    canonical_fields(json!({
        "experiment_seed": experiment_seed,
        "dataset_configuration": dataset_configuration.to_string(),
        "problem_configuration": problem_configuration.to_string(),
        "implementation_repetition": implementation_repetition,
    }))
}

/// Derives the seed used to order one experiment's expanded run plan.
///
/// The expanded member set is deliberately absent, so inserting an unrelated
/// member cannot perturb the scheduling seed.
///
/// # Errors
///
/// Returns an error when an integer is outside the version-1 canonical JSON
/// domain or canonical serialization unexpectedly fails.
pub fn derive_scheduling_seed(
    experiment_seed: u64,
    benchmark_name: &Name,
    experiment_name: &Name,
    scheduling_policy_version: u32,
) -> Result<DerivedSeedRecord, SeedDerivationError> {
    Ok(derive_seed(
        SCHEDULING_ROLE,
        &canonical_scheduling_seed_fields(
            experiment_seed,
            benchmark_name,
            experiment_name,
            scheduling_policy_version,
        )?,
    ))
}

/// Returns the exact canonical field bytes hashed for a scheduling seed.
///
/// # Errors
///
/// Returns an error when an integer is outside the version-1 canonical JSON
/// domain or canonical serialization unexpectedly fails.
pub fn canonical_scheduling_seed_fields(
    experiment_seed: u64,
    benchmark_name: &Name,
    experiment_name: &Name,
    scheduling_policy_version: u32,
) -> Result<Vec<u8>, SeedDerivationError> {
    canonical_fields(json!({
        "experiment_seed": experiment_seed,
        "benchmark_name": benchmark_name.as_str(),
        "experiment_name": experiment_name.as_str(),
        "scheduling_policy_version": scheduling_policy_version,
    }))
}

fn canonical_fields(fields: serde_json::Value) -> Result<Vec<u8>, SeedDerivationError> {
    Ok(CanonicalValue::try_from(fields)?.to_canonical_bytes()?)
}

fn derive_seed(role: &str, canonical_fields: &[u8]) -> DerivedSeedRecord {
    let mut hasher = Sha256::new();
    hasher.update(SEED_DERIVATION_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(role.as_bytes());
    hasher.update([0]);
    hasher.update(canonical_fields);
    let digest: [u8; 32] = hasher.finalize().into();
    let leading_bits = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("a SHA-256 digest always contains eight leading bytes"),
    );

    DerivedSeedRecord {
        value: leading_bits >> 11,
        derivation_digest: ContentDigest::new(digest),
    }
}

/// A failure to encode seed fields in the version-1 canonical domain.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SeedDerivationError {
    /// A field violated canonical JSON or failed canonical serialization.
    #[error("seed fields are not canonical-domain JSON: {0}")]
    Canonical(#[from] CanonicalJsonError),
}
