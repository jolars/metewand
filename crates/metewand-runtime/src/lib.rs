//! Artifact, environment, executor, and provenance support for Metewand.

/// Compatibility version of the deterministic local-tree hash transcript.
pub const LOCAL_TREE_HASH_VERSION: u32 = 1;

#[cfg(unix)]
pub mod fixed_dataset_run;
pub mod local_tree;
#[cfg(unix)]
pub mod local_worker;
#[cfg(unix)]
pub mod publication;
pub mod repository;
#[cfg(unix)]
pub mod run;
pub mod source_bundle;
pub mod worker_output;
#[cfg(unix)]
pub mod worker_process;
#[cfg(unix)]
pub mod worker_session;
