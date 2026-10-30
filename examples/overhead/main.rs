#[cfg(not(test))]
include!("../../src/modules.rs");

#[cfg(not(test))]
mod worker;

#[cfg(not(test))]
fn main() -> impl std::process::Termination {
    worker::main()
}
