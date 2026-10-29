use std::{ffi::OsString, path::PathBuf};

use crate::output::{CommandError, CommandResult};

const HELP: &str = "Usage: metewand [--output FORMAT] lock [--manifest PATH] [--check]\n\nPrepare all planned Nix and uv environments and atomically create or refresh metewand.lock beside the manifest. Native dependency locks must already be complete. uv environments use installed Python interpreters and persistent user data storage (override with METEWAND_DATA_DIR). --check verifies existing sources, closures, and launches without fetching, building, or updating the lockfile. Run prepared benchmarks with run --locked.\n";

pub fn command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["--help"] || arguments == ["-h"] {
        return Ok(CommandResult::help(HELP));
    }
    let mut manifest = None;
    let mut check = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--check" && !check {
            check = true;
        } else if argument == "--manifest" && manifest.is_none() {
            manifest = Some(PathBuf::from(
                arguments.next().filter(|v| !v.is_empty()).ok_or_else(|| {
                    CommandError::usage(
                        "missing_manifest_path",
                        "--manifest requires a nonempty path",
                    )
                })?,
            ));
        } else {
            return Err(CommandError::usage(
                "unexpected_argument",
                format!("unexpected lock argument `{}`", argument.to_string_lossy()),
            ));
        }
    }
    #[cfg(any(unix, windows))]
    {
        let repository = metewand_runtime::repository::check_repository(
            &manifest.unwrap_or_else(|| "metewand.yaml".into()),
        )
        .map_err(|e| CommandError::repository("lock", &e))?;
        let plan = repository
            .logical_plan()
            .map_err(|e| CommandError::planning("lock", &e))?;
        let report = metewand_runtime::lockfile::lock_repository(&repository, &plan, check)
            .map_err(|e| CommandError::locking(&e))?;
        Ok(CommandResult::Lock(report))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (manifest, check);
        Err(CommandError::unsupported(
            "Locked execution requires a supported host",
            "Use Linux, macOS, or Windows.",
        ))
    }
}
