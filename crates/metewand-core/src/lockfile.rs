//! Versioned environment and launch evidence, independent of host filesystem access.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canonical::CanonicalValue;

/// Compatibility version of lockfiles and their nested evidence.
pub const LOCKFILE_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    pub schema_version: u32,
    pub source: LockedSource,
    pub environments: BTreeMap<String, LockedEnvironment>,
    pub workers: BTreeMap<String, LockedWorker>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedSource {
    pub git: bool,
    pub files: BTreeMap<String, SourceEntry>,
}

/// A null digest denotes a directory; files retain their executable bit.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEntry {
    pub sha256: Option<String>,
    pub executable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorePath {
    pub nar_hash: String,
    pub references: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedEnvironment {
    pub definition: String,
    pub system: String,
    pub source_sha256: String,
    pub native_lock_sha256: Option<String>,
    pub outputs: Vec<String>,
    pub app_program: Option<String>,
    pub sources: BTreeMap<String, StorePath>,
    pub closure: BTreeMap<String, StorePath>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LockedWorker {
    pub environment: String,
    pub program: String,
    pub executable_sha256: String,
    /// Canonical identity document, with stable source and directory templates.
    pub launch: Value,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid lockfile: {0}")]
pub struct LockfileError(pub String);

impl Lockfile {
    /// Parses strict, versioned evidence without accessing an environment.
    pub fn parse(bytes: &[u8]) -> Result<Self, LockfileError> {
        let value = CanonicalValue::from_slice(bytes).map_err(|e| LockfileError(e.to_string()))?;
        let lock: Self = serde_json::from_value(value.as_json().clone())
            .map_err(|e| LockfileError(e.to_string()))?;
        if lock.schema_version != LOCKFILE_VERSION {
            return Err(LockfileError(format!(
                "unsupported version {}",
                lock.schema_version
            )));
        }
        crate::public_schemas::public_schema_catalog()
            .map_err(|e| LockfileError(e.to_string()))?
            .validate(
                crate::public_schemas::PublicSchema::Lockfile.repository_path(),
                value.as_json(),
            )
            .map_err(|e| LockfileError(e.to_string()))?;
        Ok(lock)
    }

    /// Returns deterministic RFC 8785 bytes without a trailing newline.
    pub fn to_bytes(&self) -> Result<Vec<u8>, LockfileError> {
        CanonicalValue::try_from(
            serde_json::to_value(self).map_err(|e| LockfileError(e.to_string()))?,
        )
        .and_then(|value| value.to_canonical_bytes())
        .map_err(|e| LockfileError(e.to_string()))
    }
}
