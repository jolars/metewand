//! Version-1 dataset derivation identities, independent of observed output bytes.

use crate::{
    canonical::CanonicalValue,
    identity::{IdentityKind, IdentityRecord},
    records::{
        ContentDigest, DatasetDefinitionRecord, DerivedSeedRecord, RecordId, ResolvedWorkerRecord,
    },
};
use serde_json::{Map, Value, json};

/// Verified source semantics, excluding host paths and acquisition timestamps.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedSourceRecord {
    /// Exact source selection, including external provider selectors.
    pub selection: CanonicalValue,
    /// Compatibility version of the source tree transcript.
    pub tree_hash_version: u32,
    /// Independently verified versioned local-tree digest.
    pub tree_digest: ContentDigest,
    /// Resolved version, decoding recipe, and provider verification claim.
    pub decoding: Option<CanonicalValue>,
}

/// Inputs that determine one dataset construction, before observing its output.
#[derive(Clone, Debug, PartialEq)]
pub struct DatasetDerivationRecord {
    /// Authored dataset definition, including its schema and worker definition.
    pub definition: RecordId<DatasetDefinitionRecord>,
    /// Verified source inputs in their declared order.
    pub sources: Vec<VerifiedSourceRecord>,
    /// Resolved materializer code, environment, and normalized launch identity.
    pub materializer: ResolvedWorkerRecord,
    /// Complete canonical dataset parameters.
    pub parameters: CanonicalValue,
    /// Dataset seed and full derivation digest.
    pub seed: DerivedSeedRecord,
}

impl IdentityKind for DatasetDerivationRecord {
    const KIND: &'static str = "dataset-derivation";
}

impl IdentityRecord for DatasetDerivationRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        json!({
            "definition": self.definition,
            "sources": self.sources.iter().map(|source| json!({
                "selection": source.selection.as_json(),
                "tree_hash_version": source.tree_hash_version,
                "tree_sha256": hex(source.tree_digest),
                "decoding": source.decoding.as_ref().map(CanonicalValue::as_json),
            })).collect::<Vec<_>>(),
            "materializer": {
                "source_bundle": self.materializer.source_bundle,
                "environment": self.materializer.environment,
                "launch": self.materializer.launch,
            },
            "parameters": self.parameters.as_json(),
            "seed": {"value": self.seed.value, "derivation_digest": hex(self.seed.derivation_digest)},
        }).as_object().expect("explicit identity object").clone()
    }
}

/// Lowercase SHA-256 spelling used in materialization receipts.
#[must_use]
pub fn hex(digest: ContentDigest) -> String {
    digest
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
