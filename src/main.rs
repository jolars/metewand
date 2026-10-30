#[cfg(not(test))]
include!("modules.rs");

#[cfg(not(test))]
mod cli;

#[cfg(not(test))]
fn main() -> std::process::ExitCode {
    cli::main()
}
