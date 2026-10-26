//! Artifact, environment, executor, and provenance support for Metewand.

/// Compatibility version of the deterministic local-tree hash transcript.
pub const LOCAL_TREE_HASH_VERSION: u32 = 1;

/// Compatibility version of the source-resolution boundary and its receipts.
pub const SOURCE_RESOLUTION_VERSION: u32 = 1;

#[cfg(unix)]
mod durability;
#[cfg(all(test, unix))]
mod fault_injection;

#[cfg(unix)]
pub mod artifact_store;
pub mod authoring;
#[cfg(unix)]
pub mod buffered;
pub mod build_evidence;
#[cfg(unix)]
pub mod datamonger;
#[cfg(unix)]
pub mod dataset_materialization;
#[cfg(unix)]
pub mod export;
#[cfg(unix)]
pub mod fixed_dataset_run;
#[cfg(unix)]
pub mod inspection;
pub mod local_tree;
#[cfg(unix)]
pub mod local_worker;
#[cfg(unix)]
pub mod publication;
#[cfg(unix)]
mod redaction;
pub mod repository;
#[cfg(unix)]
mod resource_accounting;
#[cfg(unix)]
pub mod run;
pub mod run_store;
pub mod source_acquisition;
pub mod source_bundle;
pub mod source_resolution;
#[cfg(unix)]
pub mod worker_environment;
pub mod worker_output;
#[cfg(unix)]
pub mod worker_process;
#[cfg(unix)]
pub mod worker_sdk;
#[cfg(unix)]
pub mod worker_session;

#[cfg(unix)]
pub mod executor;

#[cfg(unix)]
pub mod worker_reuse;
