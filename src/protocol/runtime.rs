//! Explicit host download-cache bindings, independent of scientific inputs.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, de};

/// Compatibility version of explicitly supplied local runtime configuration.
pub const RUNTIME_CONFIG_VERSION: u32 = 1;

/// Named download caches supplied to a materializer and retained as provenance.
/// Cache paths locate provider-owned files whose scientific content the provider
/// must verify. These settings never define worker or dataset identities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    #[serde(deserialize_with = "deserialize_version")]
    version: u32,
    #[serde(deserialize_with = "deserialize_caches")]
    pub(crate) caches: BTreeMap<String, String>,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            version: RUNTIME_CONFIG_VERSION,
            caches: BTreeMap::new(),
        }
    }
}

impl RuntimeSettings {
    /// Returns named cache directories, resolved to absolute paths by the runner.
    #[must_use]
    pub const fn caches(&self) -> &BTreeMap<String, String> {
        &self.caches
    }

    /// Whether the invocation supplies any host download caches.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.caches.is_empty()
    }
}

fn deserialize_version<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != RUNTIME_CONFIG_VERSION {
        return Err(de::Error::custom("runtime configuration version must be 1"));
    }
    Ok(version)
}

fn deserialize_caches<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    let caches = BTreeMap::<String, String>::deserialize(deserializer)?;
    for (name, path) in &caches {
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Err(de::Error::custom(
                "cache names must contain only ASCII letters, digits, underscores, or hyphens",
            ));
        }
        if path.is_empty() || path.contains('\0') {
            return Err(de::Error::custom(
                "cache paths must be nonempty strings without NUL",
            ));
        }
    }
    Ok(caches)
}
