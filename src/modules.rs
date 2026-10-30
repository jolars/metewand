// Repository tests and native examples compile these modules directly because
// the package intentionally has no library target.
#[path = "core/mod.rs"]
pub mod core;
#[path = "protocol/mod.rs"]
pub mod protocol;
#[path = "runtime/mod.rs"]
pub mod runtime;
