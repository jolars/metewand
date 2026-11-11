use std::{ffi::OsString, path::PathBuf};

#[cfg(any(unix, windows))]
use std::{io, io::Write as _};

use crate::cli::output::{CommandError, CommandResult, OutputFormat};

const HELP: &str = "Usage: metewand [--output FORMAT] run [EXPERIMENT]... [--manifest PATH] [--output-dir PATH] [--cache-dir PATH] [--experiment NAME] [--select ID] [--max-attempts N] [--resume] [--retry] [--locked] [--verbose]\n\nWith --locked, verify prepared Nix, uv, and Linux Docker OCI and Apptainer SIF environments and worker launches against metewand.lock without building or fetching software. Execute the selected supported logical plan using trusted local workers, fixed or unit datasets, and contract-authorized cold_end_to_end, prepare_and_execute, or execute_only wall timing. Create a run or resume an existing durable run in .metewand beside the manifest by default. Use --output-dir PATH to select another directory; relative paths resolve from the working directory, and the parent must exist. Matching accepted measurements are always skipped. Interrupted attempts restart automatically after recovery, once their ownership leases expire. Recorded failures or active slots require --retry to authorize one new immutable attempt per slot. Use a new output directory for an independent run. --resume remains accepted as a compatibility alias for the default behavior. Positional names and repeated --experiment flags select named comparisons in manifest order, overriding default-experiment. Without a selection or default, select every experiment. Repeat the selection when resuming. Repeated --select flags select the union of stable candidate, run, observation, slot, and recorded attempt IDs. --max-attempts bounds new attempts in this invocation. Optional --cache-dir assigns a separate Metewand-owned directory for verified dataset construction reuse; its parent must exist. Execution policies default to max-concurrency: 1; a larger positive bound opts into concurrent attempts. Human output reports preparation and live execution progress on stderr, with spinners, progress bars, and completed sections that fold into summaries on interactive terminals (disable color with NO_COLOR). Redirected stdout or stderr uses plain progress lines and periodic heartbeats. JSON Lines output streams versioned events after preparation; JSON output includes the same events in one terminal document. Human output ends with a short summary. Use --verbose for individual measurements, settings, metrics, and profile details. Use status to inspect saved measurements in the default run, or status --output-dir PATH for another directory, without rerunning. Machine formats disable human progress and always include complete records.\n";

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
    let mut verbose = false;
    let mut resume = false;
    let mut options = crate::core::run_output::RunOptions::default();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if let Some(name) = super::positional_experiment(argument)? {
            options.experiments.push(name);
            continue;
        }
        if argument == "--verbose" {
            if verbose {
                return Err(CommandError::usage(
                    "duplicate_verbose_flag",
                    "`--verbose` may be supplied once",
                ));
            }
            verbose = true;
            continue;
        }
        if argument == "--resume" || argument == "--retry" {
            let flag = if argument == "--resume" {
                &mut resume
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
        if argument == "--select" || argument == "--max-attempts" || argument == "--experiment" {
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
            if argument == "--experiment" {
                options.experiments.push(value.to_owned());
            } else if argument == "--select" {
                options.selected_ids.push(value.to_owned());
            } else {
                let bound = value
                    .parse::<u64>()
                    .ok()
                    .filter(|bound| {
                        *bound > 0 && *bound <= crate::core::canonical::MAX_SAFE_INTEGER
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
            if options.locked {
                return Err(CommandError::usage(
                    "duplicate_run_flag",
                    "--locked may be supplied once",
                ));
            }
            options.locked = true;
            continue;
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
    let manifest = manifest.unwrap_or_else(|| PathBuf::from("metewand.yaml"));
    execute(
        &manifest,
        output_directory.as_deref(),
        cache_directory.as_deref(),
        &options,
        output_format,
        verbose,
    )
}

#[cfg(any(unix, windows))]
fn execute(
    manifest: &std::path::Path,
    output_directory: Option<&std::path::Path>,
    cache_directory: Option<&std::path::Path>,
    options: &crate::core::run_output::RunOptions,
    output_format: OutputFormat,
    verbose: bool,
) -> Result<CommandResult, CommandError> {
    use crate::cli::progress::Progress;
    let progress = Progress::new(output_format == OutputFormat::Human, options.max_attempts);
    progress
        .stage("Checking manifest", manifest.display().to_string())
        .map_err(|e| CommandError::output_write(e.to_string()))?;
    let repository = crate::runtime::repository::check_repository(manifest)
        .map_err(|error| CommandError::repository("run", &error))?;
    let default_output_directory = repository.root.join(super::DEFAULT_RUN_DIRECTORY);
    let output_directory = output_directory.unwrap_or(&default_output_directory);
    progress
        .stage("Planning benchmark", repository.manifest.name.to_string())
        .map_err(|e| CommandError::output_write(e.to_string()))?;
    let plan = repository
        .logical_plan()
        .map_err(|error| CommandError::planning("run", &error))?;
    progress
        .labels(&repository, &plan)
        .map_err(|e| CommandError::output_write(e.to_string()))?;
    let report = crate::runtime::run::run_repository_with_progress(
        &repository,
        &plan,
        output_directory,
        cache_directory,
        options,
        |event| match output_format {
            OutputFormat::JsonLines => {
                let rendered = crate::cli::output::render_event(event, output_format)
                    .map_err(io::Error::other)?;
                let mut stdout = io::stdout().lock();
                stdout.write_all(rendered.as_bytes())?;
                stdout.flush()
            }
            OutputFormat::Human => progress.event(event),
            OutputFormat::Json => Ok(()),
        },
        |preparation| progress.runtime(preparation),
    )
    .map_err(|error| CommandError::execution(&error))?;
    progress
        .finish()
        .map_err(|e| CommandError::output_write(e.to_string()))?;
    Ok(CommandResult::Run {
        report: Box::new(report),
        verbose,
    })
}

#[cfg(not(any(unix, windows)))]
fn execute(
    _: &std::path::Path,
    _: Option<&std::path::Path>,
    _: Option<&std::path::Path>,
    _: &crate::core::run_output::RunOptions,
    _: OutputFormat,
    _: bool,
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "Gate 1 execution requires POSIX protocol pipes",
        "Run this benchmark on a supported POSIX host.",
    ))
}
