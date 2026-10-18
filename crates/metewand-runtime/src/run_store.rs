//! Versioned SQLite layout for durable run coordination.
//!
//! This module exposes the schema without opening a store or claiming durable
//! execution. Connection initialization, filesystem checks, transactional writer
//! operations, and recovery are separate implementation slices. See
//! `docs/run-store-schema.md` for the transaction and validation requirements.

/// Internal SQLite layout version, independent of public record compatibility.
pub const RUN_STORE_SCHEMA_VERSION: u32 = 1;

/// Initial schema for a new run store.
///
/// Enable SQLite foreign keys and recursive triggers before executing this SQL
/// in a transaction. Never apply it to an existing store without first checking
/// its `user_version`. This schema does not configure WAL or synchronization.
pub const RUN_STORE_SCHEMA_SQL: &str = include_str!("run_store/v1.sql");
