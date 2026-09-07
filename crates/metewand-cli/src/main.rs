mod output;

use std::{env, ffi::OsString, io, io::Write as _, path::PathBuf, process::ExitCode};

use metewand_core::public_schemas::{PUBLIC_SCHEMAS, PublicSchema};
use metewand_runtime::repository::check_repository;

use crate::output::{
    CommandError, CommandResult, EXIT_SUCCESS, OutputFormat, render_diagnostic, render_result,
};

const HELP: &str = "\
Metewand reproducible benchmark runner

Usage:
  metewand [--output FORMAT] schema [SCHEMA]
  metewand [--output FORMAT] check [--manifest PATH]
  metewand [--output FORMAT] plan [--manifest PATH]

Commands:
  schema  List public schemas, or print one exact versioned document.
  check   Validate a benchmark repository without launching workers.
  plan    Print the expanded logical plan without side effects.

Options:
  --output FORMAT  Write human, json, or jsonl output (default: human).
  --manifest PATH  Use PATH instead of ./metewand.toml.
  -h, --help       Print help.
  -V, --version    Print the package version.
";

const CHECK_HELP: &str = "Usage: metewand [--output FORMAT] check [--manifest PATH]\n\nValidate the manifest, contracts, schemas, source bundles, and logical compatibility without launching workers or modifying the repository.\n";
const PLAN_HELP: &str = "Usage: metewand [--output FORMAT] plan [--manifest PATH]\n\nPrint the deterministic unresolved logical plan without downloads, builds, worker launches, or filesystem writes.\n";
const SCHEMA_HELP: &str = "Usage: metewand [--output FORMAT] schema [SCHEMA]\n\nWithout SCHEMA, list the stable public schema names and identifiers.\n";

fn main() -> ExitCode {
    let raw_arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let output_format = output_format_hint(&raw_arguments);
    let result = extract_output_format(raw_arguments).and_then(|arguments| run(&arguments));

    match result {
        Ok(result) => match render_result(&result, output_format) {
            Ok(output) => write_success(&output),
            Err(error) => write_failure(
                &CommandError::internal(format!("failed to serialize command output: {error}")),
                output_format,
            ),
        },
        Err(error) => write_failure(&error, output_format),
    }
}

fn write_success(output: &str) -> ExitCode {
    match io::stdout().write_all(output.as_bytes()) {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::from(EXIT_SUCCESS),
        Err(error) => {
            let diagnostic =
                CommandError::output_write(format!("failed to write command output: {error}"));
            let _ = io::stderr().write_all(diagnostic.diagnostic.render_human().as_bytes());
            ExitCode::from(diagnostic.exit_code)
        }
    }
}

fn write_failure(error: &CommandError, output_format: OutputFormat) -> ExitCode {
    if output_format.is_machine() {
        match render_diagnostic(&error.diagnostic, output_format) {
            Ok(output) => {
                if let Err(write_error) = io::stdout().write_all(output.as_bytes())
                    && write_error.kind() != io::ErrorKind::BrokenPipe
                {
                    let diagnostic = CommandError::output_write(format!(
                        "failed to write structured diagnostic: {write_error}"
                    ));
                    let _ = io::stderr().write_all(diagnostic.diagnostic.render_human().as_bytes());
                    return ExitCode::from(diagnostic.exit_code);
                }
            }
            Err(serialization_error) => {
                let diagnostic = CommandError::internal(format!(
                    "failed to serialize structured diagnostic: {serialization_error}"
                ));
                let _ = io::stderr().write_all(diagnostic.diagnostic.render_human().as_bytes());
                return ExitCode::from(diagnostic.exit_code);
            }
        }
    }
    let _ = io::stderr().write_all(error.diagnostic.render_human().as_bytes());
    ExitCode::from(error.exit_code)
}

fn output_format_hint(arguments: &[OsString]) -> OutputFormat {
    arguments
        .windows(2)
        .filter(|pair| pair[0] == "--output")
        .filter_map(|pair| pair[1].to_str().and_then(OutputFormat::parse))
        .next_back()
        .unwrap_or(OutputFormat::Human)
}

fn extract_output_format(arguments: Vec<OsString>) -> Result<Vec<OsString>, CommandError> {
    let mut command_arguments = Vec::with_capacity(arguments.len());
    let mut selected = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument != "--output" {
            command_arguments.push(argument);
            continue;
        }
        let value = arguments.next().ok_or_else(|| {
            CommandError::usage(
                "missing_output_format",
                "`--output` requires one of `human`, `json`, or `jsonl`.",
            )
        })?;
        let value = value.to_str().ok_or_else(|| {
            CommandError::usage(
                "invalid_output_format",
                "the `--output` value is not valid UTF-8",
            )
        })?;
        let format = OutputFormat::parse(value).ok_or_else(|| {
            CommandError::usage(
                "invalid_output_format",
                format!("unknown output format `{value}`; expected `human`, `json`, or `jsonl`"),
            )
        })?;
        if selected.replace(format).is_some() {
            return Err(CommandError::usage(
                "duplicate_output_format",
                "`--output` may be supplied at most once",
            ));
        }
    }
    Ok(command_arguments)
}

fn run(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    let Some(command) = arguments.first().and_then(|argument| argument.to_str()) else {
        if arguments.is_empty() {
            return Ok(CommandResult::help(HELP));
        }
        return Err(CommandError::usage(
            "invalid_command_name",
            "command name is not valid UTF-8",
        ));
    };

    match command {
        "-h" | "--help" | "help" => {
            no_arguments(&arguments[1..])?;
            Ok(CommandResult::help(HELP))
        }
        "-V" | "--version" => {
            no_arguments(&arguments[1..])?;
            Ok(CommandResult::version(env!("CARGO_PKG_VERSION")))
        }
        "schema" => schema_command(&arguments[1..]),
        "check" => check_command(&arguments[1..]),
        "plan" => plan_command(&arguments[1..]),
        _ => Err(CommandError::usage(
            "unknown_command",
            format!("unknown command `{command}`"),
        )),
    }
}

fn schema_command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    match arguments {
        [] => Ok(CommandResult::schema_list()),
        [argument] if argument == "-h" || argument == "--help" => {
            Ok(CommandResult::help(SCHEMA_HELP))
        }
        [argument] => {
            let slug = argument.to_str().ok_or_else(|| {
                CommandError::usage("invalid_schema_name", "schema name is not valid UTF-8")
            })?;
            PublicSchema::from_slug(slug)
                .map(CommandResult::schema_document)
                .ok_or_else(|| {
                    let names = PUBLIC_SCHEMAS
                        .iter()
                        .map(|schema| schema.slug())
                        .collect::<Vec<_>>()
                        .join(", ");
                    CommandError::usage(
                        "unknown_schema",
                        format!("unknown public schema `{slug}`; expected one of {names}"),
                    )
                })
        }
        _ => Err(CommandError::usage(
            "unexpected_argument",
            "`metewand schema` accepts at most one schema name",
        )),
    }
}

fn check_command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(CHECK_HELP));
    }
    if arguments == ["--workers"] {
        return Err(CommandError::unsupported(
            "`check --workers` is not available until the worker protocol is implemented; no workers were launched",
            "Omit `--workers` to run the side-effect-free repository check.",
        ));
    }
    let manifest = manifest_argument(arguments)?;
    let repository =
        check_repository(&manifest).map_err(|error| CommandError::repository("check", &error))?;
    Ok(CommandResult::check(&repository))
}

fn plan_command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(PLAN_HELP));
    }
    let manifest = manifest_argument(arguments)?;
    let repository =
        check_repository(&manifest).map_err(|error| CommandError::repository("plan", &error))?;
    let plan = repository
        .logical_plan()
        .map_err(|error| CommandError::planning(&error))?;
    Ok(CommandResult::plan(&repository, &plan))
}

fn manifest_argument(arguments: &[OsString]) -> Result<PathBuf, CommandError> {
    match arguments {
        [] => Ok(PathBuf::from("metewand.toml")),
        [flag, path] if flag == "--manifest" => {
            if path.is_empty() {
                Err(CommandError::usage(
                    "empty_manifest_path",
                    "`--manifest` requires a nonempty path",
                ))
            } else {
                Ok(PathBuf::from(path))
            }
        }
        [flag] if flag == "--manifest" => Err(CommandError::usage(
            "missing_manifest_path",
            "`--manifest` requires a path argument",
        )),
        [argument] => Err(CommandError::usage(
            "unexpected_argument",
            format!(
                "unexpected argument `{}`; use `--manifest PATH` to select a manifest",
                argument.to_string_lossy()
            ),
        )),
        _ => Err(CommandError::usage(
            "unexpected_argument",
            "expected no arguments or exactly `--manifest PATH`",
        )),
    }
}

fn no_arguments(arguments: &[OsString]) -> Result<(), CommandError> {
    if arguments.is_empty() {
        Ok(())
    } else {
        Err(CommandError::usage(
            "unexpected_argument",
            "this option accepts no arguments",
        ))
    }
}
