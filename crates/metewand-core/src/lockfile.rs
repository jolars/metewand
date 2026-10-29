//! Versioned environment and launch evidence, independent of host filesystem access.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::canonical::CanonicalValue;

/// Compatibility version of lockfiles and their nested evidence.
pub const LOCKFILE_VERSION: u32 = 2;

#[derive(Clone, Debug)]
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
pub struct LockedNixEnvironment {
    pub definition: String,
    pub system: String,
    pub source_sha256: String,
    pub native_lock_sha256: Option<String>,
    pub outputs: Vec<String>,
    pub app_program: Option<String>,
    pub sources: BTreeMap<String, StorePath>,
    pub closure: BTreeMap<String, StorePath>,
}

/// Backend-specific evidence; version-1 documents serialize Nix evidence directly.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "backend",
    content = "evidence",
    rename_all = "lowercase",
    deny_unknown_fields
)]
pub enum LockedEnvironment {
    Nix(LockedNixEnvironment),
    Uv(LockedUvEnvironment),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedUvEnvironment {
    pub definition: String,
    pub system: String,
    pub source_sha256: String,
    pub native_lock_sha256: String,
    pub fingerprint: UvFingerprint,
    /// Relative generation locator beneath the local environment data directory.
    pub preparation: String,
    /// Hash of the exact local integrity receipt, excluded from scientific identity.
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UvFingerprint {
    pub python: LockedPython,
    pub packages_sha256: String,
    pub distributions: BTreeMap<String, String>,
    pub groups: Vec<String>,
    pub extras: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPython {
    pub implementation: String,
    pub version: String,
    pub abi: String,
    pub platform: String,
    pub executable_sha256: String,
    pub runtime_sha256: String,
}

impl LockedEnvironment {
    pub fn nix(&self) -> Option<&LockedNixEnvironment> {
        match self {
            Self::Nix(e) => Some(e),
            Self::Uv(_) => None,
        }
    }

    pub fn definition(&self) -> &str {
        match self {
            Self::Nix(e) => &e.definition,
            Self::Uv(e) => &e.definition,
        }
    }

    pub fn source_sha256(&self) -> &str {
        match self {
            Self::Nix(e) => &e.source_sha256,
            Self::Uv(e) => &e.source_sha256,
        }
    }

    pub fn system(&self) -> &str {
        match self {
            Self::Nix(e) => &e.system,
            Self::Uv(e) => &e.system,
        }
    }

    /// Keeps historical Nix identity documents independent of their lock wrapper.
    pub fn fingerprint(&self) -> Value {
        match self {
            Self::Nix(e) => serde_json::json!({"class":"locked", "backend":"nix", "evidence":e}),
            Self::Uv(e) => serde_json::json!({"class":"locked", "backend":"uv", "evidence":{
                "definition":e.definition, "system":e.system,
                "source_sha256":e.source_sha256, "native_lock_sha256":e.native_lock_sha256,
                "fingerprint":e.fingerprint
            }}),
        }
    }
}

impl Serialize for Lockfile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let environments = self
            .environments
            .iter()
            .map(|(name, evidence)| {
                let value = if self.schema_version == 1 {
                    let nix = evidence.nix().ok_or_else(|| {
                        serde::ser::Error::custom("version 1 requires Nix evidence")
                    })?;
                    serde_json::to_value(nix)
                } else {
                    serde_json::to_value(evidence)
                }
                .map_err(serde::ser::Error::custom)?;
                Ok((name.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>, S::Error>>()?;
        serde_json::json!({"schema_version":self.schema_version, "source":self.source,
            "environments":environments, "workers":self.workers})
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Lockfile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Document {
            schema_version: u32,
            source: LockedSource,
            environments: BTreeMap<String, LockedEnvironment>,
            workers: BTreeMap<String, LockedWorker>,
        }
        let mut value = Value::deserialize(deserializer)?;
        if value["schema_version"] == 1
            && let Some(entries) = value["environments"].as_object_mut()
        {
            for evidence in entries.values_mut() {
                *evidence = serde_json::json!({"backend":"nix", "evidence":evidence.take()});
            }
        }
        let d: Document = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            schema_version: d.schema_version,
            source: d.source,
            environments: d.environments,
            workers: d.workers,
        })
    }
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
        if !matches!(lock.schema_version, 1 | LOCKFILE_VERSION) {
            return Err(LockfileError(format!(
                "unsupported version {}",
                lock.schema_version
            )));
        }
        crate::public_schemas::public_schema_catalog()
            .map_err(|e| LockfileError(e.to_string()))?
            .validate(
                if lock.schema_version == 1 {
                    crate::public_schemas::PublicSchema::LockfileV1.repository_path()
                } else {
                    crate::public_schemas::PublicSchema::Lockfile.repository_path()
                },
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
