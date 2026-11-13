//! Immutable coordination transactions, independent of node-local database files.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DURABLE_RECORD_VERSION: u32 = 1;

/// Cells use decimal text for integers so nanosecond timestamps retain all bits.
/// JSON-valued cells retain their original text. Null remains distinct from text.
pub type DurableRow = BTreeMap<String, Option<String>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableChange {
    pub collection: String,
    pub before: Option<DurableRow>,
    pub after: Option<DurableRow>,
}

/// One atomically published transaction. Sequence zero establishes an empty run.
/// Every later record binds the SHA-256 of its predecessor's canonical bytes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRecord {
    pub schema_version: u32,
    pub sequence: u64,
    pub previous_sha256: Option<String>,
    pub changes: Vec<DurableChange>,
}
