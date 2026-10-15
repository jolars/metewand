use std::{ffi::OsString, path::PathBuf};

#[cfg(unix)]
use std::{io, io::Write as _};

use crate::output::{CommandError, CommandResult, OutputFormat};

const HELP: &str = "Usage: metewand [--output FORMAT] run [--manifest PATH] --output-dir PATH\n\nExecute the complete supported logical plan using trusted local workers, fixed or unit datasets, and prepare_and_execute timing. Publish results in a new directory whose parent exists. JSON Lines output streams versioned events; JSON output includes the same events in one terminal document.\n";

pub fn command(
    arguments: &[OsString],
    output_format: OutputFormat,
) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(HELP));
    }
    let mut manifest = None;
    let mut output_directory = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let (target, missing, empty, duplicate) = if argument == "--manifest" {
            (
                &mut manifest,
                "missing_manifest_path",
                "empty_manifest_path",
                "duplicate_manifest_path",
            )
        } else if argument == "--output-dir" {
            (
                &mut output_directory,
                "missing_output_directory",
                "empty_output_directory",
                "duplicate_output_directory",
            )
        } else if argument == "--locked" || argument == "--resume" {
            return Err(CommandError::unsupported(
                "locked execution and resume are not available in Gate 1",
                "Choose a new `--output-dir PATH` for an exploratory trusted local run.",
            ));
        } else {
            return Err(CommandError::usage(
                "unexpected_argument",
                format!("unexpected `run` argument `{}`", argument.to_string_lossy()),
            ));
        };
        let value = arguments.next().ok_or_else(|| {
            CommandError::usage(
                missing,
                format!("`{}` requires a path argument", argument.to_string_lossy()),
            )
        })?;
        if value.is_empty() {
            return Err(CommandError::usage(
                empty,
                format!("`{}` requires a nonempty path", argument.to_string_lossy()),
            ));
        }
        if target.replace(PathBuf::from(value)).is_some() {
            return Err(CommandError::usage(
                duplicate,
                format!(
                    "`{}` may be supplied at most once",
                    argument.to_string_lossy()
                ),
            ));
        }
    }
    let output_directory = output_directory.ok_or_else(|| {
        CommandError::usage(
            "missing_output_directory",
            "`run` requires an explicit new `--output-dir PATH`",
        )
    })?;
    let manifest = manifest.unwrap_or_else(|| PathBuf::from("metewand.toml"));
    execute(&manifest, &output_directory, output_format)
}

#[cfg(unix)]
fn execute(
    manifest: &std::path::Path,
    output_directory: &std::path::Path,
    output_format: OutputFormat,
) -> Result<CommandResult, CommandError> {
    let repository = metewand_runtime::repository::check_repository(manifest)
        .map_err(|error| CommandError::repository("run", &error))?;
    let plan = repository
        .logical_plan()
        .map_err(|error| CommandError::planning("run", &error))?;
    let report =
        metewand_runtime::run::run_repository(&repository, &plan, output_directory, |event| {
            let rendered =
                crate::output::render_event(event, output_format).map_err(io::Error::other)?;
            match output_format {
                OutputFormat::JsonLines => {
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(rendered.as_bytes())?;
                    stdout.flush()
                }
                OutputFormat::Human => io::stderr().write_all(rendered.as_bytes()),
                OutputFormat::Json => Ok(()),
            }
        })
        .map_err(|error| CommandError::execution(&error))?;
    Ok(CommandResult::Run(Box::new(report)))
}

#[cfg(not(unix))]
fn execute(
    _: &std::path::Path,
    _: &std::path::Path,
    _: OutputFormat,
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "Gate 1 execution requires POSIX protocol pipes",
        "Run this benchmark on a supported POSIX host.",
    ))
}
