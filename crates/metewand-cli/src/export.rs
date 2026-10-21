use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use metewand_core::export::ExportFormat;

use crate::output::{CommandError, CommandResult};

const HELP: &str = "Usage: metewand [--output FORMAT] export --output-dir PATH [--format jsonl|csv] [--destination PATH] [--id ID ...] [--rebuild-index]\n\nExport all attempts, applicability decisions, and observations to a new directory containing results.jsonl or results.csv and export.json. --destination explicitly authorizes creating that directory; existing paths are never replaced. --rebuild-index explicitly replaces only the run's rebuildable index.jsonl with the complete verified projection, regardless of --id filters. Supply --destination, --rebuild-index, or both. Reads never use an existing index, launch workers, or recover the store.\n";

pub fn command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(HELP));
    }
    let mut options = BTreeMap::new();
    let mut ids = Vec::new();
    let mut rebuild_index = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let flag = argument.to_str().unwrap_or("");
        if flag == "--rebuild-index" {
            if rebuild_index {
                return Err(CommandError::usage(
                    "duplicate_export_option",
                    "`--rebuild-index` may be supplied once",
                ));
            }
            rebuild_index = true;
        } else if matches!(flag, "--output-dir" | "--destination" | "--format" | "--id") {
            let value = arguments
                .next()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    CommandError::usage(
                        "missing_export_value",
                        format!("`{flag}` requires a nonempty value"),
                    )
                })?;
            if flag == "--id" {
                ids.push(
                    value
                        .to_str()
                        .ok_or_else(|| {
                            CommandError::usage(
                                "invalid_export_id",
                                "export identities must be UTF-8",
                            )
                        })?
                        .to_owned(),
                );
            } else if options.insert(flag.to_owned(), value.clone()).is_some() {
                return Err(CommandError::usage(
                    "duplicate_export_option",
                    format!("`{flag}` may be supplied once"),
                ));
            }
        } else {
            return Err(CommandError::usage(
                "unexpected_argument",
                format!(
                    "unexpected export argument `{}`",
                    argument.to_string_lossy()
                ),
            ));
        }
    }
    let root = options
        .get("--output-dir")
        .map(PathBuf::from)
        .ok_or_else(|| {
            CommandError::usage(
                "missing_output_directory",
                "`export` requires `--output-dir PATH`",
            )
        })?;
    let destination = options.get("--destination").map(PathBuf::from);
    if destination.is_none() && !rebuild_index {
        return Err(CommandError::usage(
            "missing_export_destination",
            "supply `--destination PATH`, `--rebuild-index`, or both",
        ));
    }
    let format = match options
        .get("--format")
        .map(|value| value.to_str())
        .unwrap_or(Some("jsonl"))
    {
        Some("jsonl") => ExportFormat::Jsonl,
        Some("csv") => ExportFormat::Csv,
        _ => {
            return Err(CommandError::usage(
                "invalid_export_format",
                "`--format` must be `jsonl` or `csv`",
            ));
        }
    };
    ids.sort();
    ids.dedup();
    export(root, destination, format, ids, rebuild_index)
}

#[cfg(unix)]
fn export(
    root: PathBuf,
    destination: Option<PathBuf>,
    format: ExportFormat,
    ids: Vec<String>,
    rebuild_index: bool,
) -> Result<CommandResult, CommandError> {
    let results = metewand_runtime::export::ResultSet::read(&root)
        .map_err(|error| CommandError::export(&error))?;
    let report = results
        .export(destination.as_deref(), format, &ids, rebuild_index)
        .map_err(|error| CommandError::export(&error))?;
    Ok(CommandResult::Export(Box::new(report)))
}

#[cfg(not(unix))]
fn export(
    _: PathBuf,
    _: Option<PathBuf>,
    _: ExportFormat,
    _: Vec<String>,
    _: bool,
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "run export currently requires a POSIX host",
        "Export this benchmark on a supported POSIX host.",
    ))
}
