//! Versioned SQLite layout for durable run coordination.
//!
//! Store initialization verifies SQLite settings and filesystem capabilities.
//! Attempt writers fence transactions by lease ownership and revision. See
//! `CONTRIBUTING.md#durability-and-recovery` for the transaction and validation requirements.

#[cfg(any(unix, windows))]
mod attempts;
#[cfg(any(unix, windows))]
mod coordination;
#[cfg(any(unix, windows))]
mod durable;
#[cfg(any(unix, windows))]
pub(crate) mod fresh;
#[cfg(any(unix, windows))]
pub use coordination::SlotProgress;
#[cfg(all(test, any(unix, windows)))]
mod fault_tests;
#[cfg(any(unix, windows))]
mod publisher;
#[cfg(any(unix, windows))]
pub(crate) mod reader;
#[cfg(any(unix, windows))]
mod recovery;
#[cfg(any(unix, windows))]
pub use attempts::{
    ArtifactRegistration, AttemptPhase, ClaimDisposition, Finalization, LeaseRequest, OwnedAttempt,
    RetryAuthorization, TerminalAttempt, TerminalOutcome,
};
#[cfg(any(unix, windows))]
pub use durable::{RunStore, RunStoreError};
#[cfg(any(unix, windows))]
pub use publisher::{ArtifactKind, PublishedArtifact, PublishedFinalization};
#[cfg(any(unix, windows))]
pub use recovery::{RecoveryReport, RejectedArtifact};

/// Internal SQLite layout version, independent of public record compatibility.
pub const RUN_STORE_SCHEMA_VERSION: u32 = 1;
/// Buffered execution requires the version-2 membership and acceptance layout.
pub const BUFFERED_RUN_STORE_SCHEMA_VERSION: u32 = 2;
pub const BUFFERED_RUN_STORE_SCHEMA_SQL: &str = include_str!("run_store/v2.sql");

/// Initial schema for a new run store.
///
/// Enable SQLite foreign keys and recursive triggers before executing this SQL
/// in a transaction. Never apply it to an existing store without first checking
/// its `user_version`. This schema does not configure WAL or synchronization.
pub const RUN_STORE_SCHEMA_SQL: &str = include_str!("run_store/v1.sql");

/// Fresh-sequence membership, budget accounting, and terminal slot dispositions.
pub const FRESH_RUN_STORE_SCHEMA_VERSION: u32 = 3;
pub const FRESH_RUN_STORE_SCHEMA_SQL: &str = include_str!("run_store/v3.sql");
