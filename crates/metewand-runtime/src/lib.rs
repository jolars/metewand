//! Artifact, environment, executor, and provenance support for Metewand.

/// Compatibility version of the deterministic local-tree hash transcript.
pub const LOCAL_TREE_HASH_VERSION: u32 = 1;

/// Compatibility version of the source-resolution boundary and its receipts.
pub const SOURCE_RESOLUTION_VERSION: u32 = 1;

#[cfg(any(unix, windows))]
mod durability;
#[cfg(all(test, any(unix, windows)))]
mod fault_injection;
#[cfg(any(unix, windows))]
pub(crate) mod platform_fs;

/// Portable result archive creation, verification, and read-only import.
#[cfg(any(unix, windows))]
pub mod archive;
#[cfg(any(unix, windows))]
pub mod artifact_store;
pub mod authoring;
#[cfg(any(unix, windows))]
pub mod buffered;
pub mod build_evidence;
#[cfg(any(unix, windows))]
pub mod datamonger;
#[cfg(any(unix, windows))]
pub mod dataset_materialization;
#[cfg(any(unix, windows))]
mod evidence;
#[cfg(any(unix, windows))]
pub mod export;
#[cfg(any(unix, windows))]
pub mod fixed_dataset_run;
#[cfg(any(unix, windows))]
pub mod inspection;
#[cfg(any(unix, windows))]
pub mod language_adapter;
pub mod local_tree;
#[cfg(any(unix, windows))]
pub mod local_worker;
#[cfg(any(unix, windows))]
mod managed_output;
#[cfg(any(unix, windows))]
pub mod publication;
#[cfg(any(unix, windows))]
mod redaction;
pub mod repository;
#[cfg(any(unix, windows))]
mod resource_accounting;
#[cfg(any(unix, windows))]
pub mod run;
pub mod run_store;
pub mod source_acquisition;
pub mod source_bundle;
pub mod source_resolution;
#[cfg(any(unix, windows))]
pub mod streaming;
#[cfg(any(unix, windows))]
pub mod worker_environment;
pub mod worker_output;
#[cfg(any(unix, windows))]
pub mod worker_process;
#[cfg(any(unix, windows))]
pub mod worker_sdk;
#[cfg(any(unix, windows))]
pub mod worker_session;

#[cfg(any(unix, windows))]
pub mod executor;

#[cfg(any(unix, windows))]
pub mod worker_reuse;
