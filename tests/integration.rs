include!("../src/modules.rs");

#[path = "../src/cli/mod.rs"]
pub mod cli;

#[path = "cli/mod.rs"]
mod cli_tests;
#[path = "core/mod.rs"]
mod core_tests;
#[path = "protocol/mod.rs"]
mod protocol_tests;
#[path = "runtime/mod.rs"]
mod runtime_tests;

#[cfg(unix)]
#[path = "../examples/datamonger/worker.rs"]
pub mod datamonger_example;

pub mod native;
