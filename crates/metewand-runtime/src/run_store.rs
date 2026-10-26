//! Versioned SQLite layout for durable run coordination.
//!
//! Store initialization verifies SQLite settings and filesystem capabilities.
//! Attempt writers fence transactions by lease ownership and revision. See
//! `docs/reference/run-store-schema.md` for the transaction and validation requirements.

#[cfg(unix)]
mod attempts;
#[cfg(unix)]
mod coordination;
#[cfg(unix)]
mod durable;
#[cfg(unix)]
pub use coordination::SlotProgress;
#[cfg(all(test, unix))]
mod fault_tests;
#[cfg(unix)]
mod publisher;
#[cfg(unix)]
pub(crate) mod reader;
#[cfg(unix)]
mod recovery;
#[cfg(unix)]
pub use attempts::{
    ArtifactRegistration, AttemptPhase, ClaimDisposition, Finalization, LeaseRequest, OwnedAttempt,
    RetryAuthorization, TerminalAttempt, TerminalOutcome,
};
#[cfg(unix)]
pub use durable::{RunStore, RunStoreError};
#[cfg(unix)]
pub use publisher::{ArtifactKind, PublishedArtifact, PublishedFinalization};
#[cfg(unix)]
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
