use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use crate::core::export::ExportFormat;

use crate::cli::output::{CommandError, CommandResult};

const HELP: &str = "Usage: metewand [--output FORMAT] export --output-dir PATH [--format json|jsonl|csv] [--destination PATH] [--id ID ...] [--rebuild-index] [--require-complete]\n\nExport verified evidence from a run or imported archive collection. JSON writes a standalone analysis snapshot to results.json with full observation metrics, settings, progress, and recorded provenance. JSONL and CSV write tidy metric rows. Each destination also contains export.json with content hashes and lineage. --require-complete checks the registered measurement scope of selected candidates before writing; accepted retries and target-based skips satisfy it. --destination explicitly authorizes creating a new directory; existing paths are never replaced. --rebuild-index explicitly replaces only a writable run's disposable index.jsonl, regardless of --id filters. Imported collections require a destination outside the collection and reject --rebuild-index. Supply --destination, --rebuild-index, or both. Reads never launch workers or recover the store.\n";

pub fn command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(HELP));
    }
    let mut options = BTreeMap::new();
    let mut ids = Vec::new();
    let mut rebuild_index = false;
    let mut require_complete = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let flag = argument.to_str().unwrap_or("");
        if matches!(flag, "--rebuild-index" | "--require-complete") {
            let target = if flag == "--rebuild-index" {
                &mut rebuild_index
            } else {
                &mut require_complete
            };
            if *target {
                return Err(CommandError::usage(
                    "duplicate_export_option",
                    format!("`{flag}` may be supplied once"),
                ));
            }
            *target = true;
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
        Some("json") => ExportFormat::Json,
        Some("jsonl") => ExportFormat::Jsonl,
        Some("csv") => ExportFormat::Csv,
        _ => {
            return Err(CommandError::usage(
                "invalid_export_format",
                "`--format` must be `json`, `jsonl`, or `csv`",
            ));
        }
    };
    ids.sort();
    ids.dedup();
    export(
        root,
        destination,
        format,
        ids,
        rebuild_index,
        require_complete,
    )
}

#[cfg(any(unix, windows))]
fn export(
    root: PathBuf,
    destination: Option<PathBuf>,
    format: ExportFormat,
    ids: Vec<String>,
    rebuild_index: bool,
    require_complete: bool,
) -> Result<CommandResult, CommandError> {
    let results = crate::runtime::export::ResultSet::read(&root)
        .map_err(|error| CommandError::export(&error))?;
    let report = results
        .export_checked(
            destination.as_deref(),
            format,
            &ids,
            rebuild_index,
            require_complete,
        )
        .map_err(|error| CommandError::export(&error))?;
    Ok(CommandResult::Export(Box::new(report)))
}

#[cfg(not(any(unix, windows)))]
fn export(
    _: PathBuf,
    _: Option<PathBuf>,
    _: ExportFormat,
    _: Vec<String>,
    _: bool,
    _: bool,
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "run export currently requires a POSIX host",
        "Export this benchmark on a supported POSIX host.",
    ))
}
