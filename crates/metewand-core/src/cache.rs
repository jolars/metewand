//! Versioned receipts for exact cache verification, roots, and collection.

use serde::{Deserialize, Serialize};

/// Compatibility version of cache maintenance receipts.
pub const CACHE_MAINTENANCE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheOperation {
    Verify,
    Gc,
    Roots,
    RootAdd,
    RootRemove,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheTargetState {
    Verified,
    Missing,
    Corrupt,
    Retained,
    WouldDelete,
    Deleted,
    RootAdded,
    RootRemoved,
    WouldAddRoot,
    WouldRemoveRoot,
}

/// One exact content identity and the protection observed in the same snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheTargetReport {
    pub id: String,
    pub state: CacheTargetState,
    pub roots: Vec<String>,
    pub leases: Vec<String>,
    pub detail: Option<String>,
}

/// Human and machine renderers consume the same deterministic receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheReport {
    pub command: String,
    pub schema_version: u32,
    pub operation: CacheOperation,
    pub cache_directory: String,
    pub dry_run: bool,
    pub targets: Vec<CacheTargetReport>,
}
