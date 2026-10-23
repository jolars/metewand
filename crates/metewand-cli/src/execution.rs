use std::{ffi::OsString, path::PathBuf};

#[cfg(unix)]
use std::{io, io::Write as _};

use crate::output::{CommandError, CommandResult, OutputFormat};

const HELP: &str = "Usage: metewand [--output FORMAT] run [--manifest PATH] --output-dir PATH [--cache-dir PATH] [--select ID] [--max-attempts N] [--resume] [--retry]\n\nExecute the selected supported logical plan using trusted local workers, fixed or unit datasets, and contract-authorized cold_end_to_end, prepare_and_execute, or execute_only wall timing. Publish results in a new directory whose parent exists. Use --resume to reopen a durable run and fill unsatisfied slots. Failed or active slots require --retry to authorize one new immutable attempt per slot. Repeated --select flags select the union of stable candidate, run, observation, slot, and recorded attempt IDs. --max-attempts bounds new attempts in this invocation. Accepted slots are always skipped. Optional --cache-dir assigns a separate Metewand-owned directory for verified dataset construction reuse; its parent must exist. Execution policies default to max_concurrency: 1; a larger positive bound opts into concurrent attempts. JSON Lines output streams versioned events; JSON output includes the same events in one terminal document.\n";

pub fn command(
    arguments: &[OsString],
    output_format: OutputFormat,
) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(HELP));
    }
    let mut manifest = None;
    let mut output_directory = None;
    let mut cache_directory = None;
    let mut options = metewand_core::run_output::RunOptions::default();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--resume" || argument == "--retry" {
            let flag = if argument == "--resume" {
                &mut options.resume
            } else {
                &mut options.retry
            };
            if *flag {
                return Err(CommandError::usage(
                    "duplicate_run_flag",
                    format!("`{}` may be supplied once", argument.to_string_lossy()),
                ));
            }
            *flag = true;
            continue;
        }
        if argument == "--select" || argument == "--max-attempts" {
            let value = arguments
                .next()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    CommandError::usage(
                        "missing_run_selection",
                        format!("`{}` requires a value", argument.to_string_lossy()),
                    )
                })?;
            if argument == "--select" {
                options.selected_ids.push(value.to_owned());
            } else {
                let bound = value
                    .parse::<u64>()
                    .ok()
                    .filter(|bound| {
                        *bound > 0 && *bound <= metewand_core::canonical::MAX_SAFE_INTEGER
                    })
                    .ok_or_else(|| {
                        CommandError::usage(
                            "invalid_max_attempts",
                            "`--max-attempts` requires a positive safe integer",
                        )
                    })?;
                if options.max_attempts.replace(bound).is_some() {
                    return Err(CommandError::usage(
                        "duplicate_max_attempts",
                        "`--max-attempts` may be supplied once",
                    ));
                }
            }
            continue;
        }
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
        } else if argument == "--cache-dir" {
            (
                &mut cache_directory,
                "missing_cache_directory",
                "empty_cache_directory",
                "duplicate_cache_directory",
            )
        } else if argument == "--locked" {
            return Err(CommandError::unsupported(
                "locked execution is not available yet",
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
            "`run` requires an explicit `--output-dir PATH`",
        )
    })?;
    let manifest = manifest.unwrap_or_else(|| PathBuf::from("metewand.yaml"));
    execute(
        &manifest,
        &output_directory,
        cache_directory.as_deref(),
        &options,
        output_format,
    )
}

#[cfg(unix)]
fn execute(
    manifest: &std::path::Path,
    output_directory: &std::path::Path,
    cache_directory: Option<&std::path::Path>,
    options: &metewand_core::run_output::RunOptions,
    output_format: OutputFormat,
) -> Result<CommandResult, CommandError> {
    let repository = metewand_runtime::repository::check_repository(manifest)
        .map_err(|error| CommandError::repository("run", &error))?;
    let plan = repository
        .logical_plan()
        .map_err(|error| CommandError::planning("run", &error))?;
    let report = metewand_runtime::run::run_repository_with_options(
        &repository,
        &plan,
        output_directory,
        cache_directory,
        options,
        |event| {
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
        },
    )
    .map_err(|error| CommandError::execution(&error))?;
    Ok(CommandResult::Run(Box::new(report)))
}

#[cfg(not(unix))]
fn execute(
    _: &std::path::Path,
    _: &std::path::Path,
    _: Option<&std::path::Path>,
    _: &metewand_core::run_output::RunOptions,
    _: OutputFormat,
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "Gate 1 execution requires POSIX protocol pipes",
        "Run this benchmark on a supported POSIX host.",
    ))
}
