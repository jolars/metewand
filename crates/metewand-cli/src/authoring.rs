use std::{ffi::OsString, path::PathBuf};

use metewand_runtime::authoring::init_benchmark;

use crate::output::{CommandError, CommandResult};

const INIT_HELP: &str = "Usage: metewand [--output FORMAT] init [PATH] [--dry-run]\n\nCreate only PATH/metewand.toml (default PATH: .), containing a version and a neutral benchmark name. Report every created path, including missing directories. Refuse an existing manifest, including symlinks. Use --dry-run to validate and report proposed paths without writing. Use -- before a path beginning with a hyphen.\n";

pub fn init_command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(INIT_HELP));
    }
    let mut directory = None;
    let mut dry_run = false;
    let mut positional_only = false;
    for argument in arguments {
        if !positional_only {
            if argument == "--" {
                positional_only = true;
                continue;
            }
            if argument == "--dry-run" {
                if dry_run {
                    return Err(CommandError::usage(
                        "duplicate_init_flag",
                        "`--dry-run` may be supplied at most once",
                    ));
                }
                dry_run = true;
                continue;
            }
            if argument.to_string_lossy().starts_with('-') {
                return Err(CommandError::usage(
                    "unexpected_argument",
                    format!("unexpected init option `{}`", argument.to_string_lossy()),
                ));
            }
        }
        if argument.is_empty() || argument.to_str().is_none() {
            return Err(CommandError::usage(
                "invalid_init_path",
                "`init` requires a nonempty UTF-8 directory path",
            ));
        }
        if directory.replace(PathBuf::from(argument)).is_some() {
            return Err(CommandError::usage(
                "unexpected_argument",
                "`init` accepts at most one destination directory",
            ));
        }
    }
    init_benchmark(&directory.unwrap_or_else(|| PathBuf::from(".")), dry_run)
        .map(CommandResult::Init)
        .map_err(|error| CommandError::init(&error))
}
