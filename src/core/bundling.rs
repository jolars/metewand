//! Portable whole-benchmark build contexts and generation receipts.

use serde::{Deserialize, Serialize};

pub const BUNDLE_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub runner_version: String,
    pub builder_image: String,
    pub base_image: String,
    pub manifest: String,
    /// The inventory excludes itself to avoid a hash cycle.
    pub entries: Vec<BundleEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BundleEntry {
    Directory {
        path: String,
    },
    File {
        path: String,
        sha256: String,
        size_bytes: u64,
        executable: bool,
    },
}

#[derive(Debug, Serialize)]
pub struct BundleReport {
    pub command: &'static str,
    pub dry_run: bool,
    pub destination: String,
    pub bundle: BundleManifest,
    pub next_steps: Vec<String>,
}
