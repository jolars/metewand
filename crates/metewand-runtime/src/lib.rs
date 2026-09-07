//! Artifact, environment, executor, and provenance support for Metewand.

/// Compatibility version of the deterministic local-tree hash transcript.
pub const LOCAL_TREE_HASH_VERSION: u32 = 1;

pub mod local_tree;
#[cfg(unix)]
pub mod local_worker;
pub mod repository;
pub mod source_bundle;
pub mod worker_output;
#[cfg(unix)]
pub mod worker_process;
#[cfg(unix)]
pub mod worker_session;
