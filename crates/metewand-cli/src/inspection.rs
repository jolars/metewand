use std::{ffi::OsString, path::PathBuf};

use crate::output::{CommandError, CommandResult};

const STATUS_HELP: &str = "Usage: metewand [--output FORMAT] status [--manifest PATH] [--output-dir PATH]\n\nInspect measured slots, immutable retry history, selected observations, and failure diagnostics without running workers or recovering a store. Without --output-dir, inspect ./metewand.yaml. With --output-dir, inspect committed run history independently of repository inputs; add --manifest to compare it with the current logical plan.\n";
const EXPLAIN_HELP: &str = "Usage: metewand [--output FORMAT] explain <id> [--manifest PATH] [--output-dir PATH]\n\nExplain an exact stable definition, configuration, run, slot, attempt, observation, artifact, applicability, or diagnostic ID. Inspect ./metewand.yaml by default, or committed history with --output-dir. Add --manifest to inspect both. Lookup never downloads inputs, launches workers, initializes a store, or performs recovery.\n";

pub fn command(
    command: &'static str,
    arguments: &[OsString],
) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(if command == "status" {
            STATUS_HELP
        } else {
            EXPLAIN_HELP
        }));
    }
    let mut manifest = None;
    let mut output_directory = None;
    let mut id = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--manifest" || argument == "--output-dir" {
            let manifest_flag = argument == "--manifest";
            let (target, missing, empty, duplicate) = if manifest_flag {
                (
                    &mut manifest,
                    "missing_manifest_path",
                    "empty_manifest_path",
                    "duplicate_manifest_path",
                )
            } else {
                (
                    &mut output_directory,
                    "missing_output_directory",
                    "empty_output_directory",
                    "duplicate_output_directory",
                )
            };
            let value = arguments.next().ok_or_else(|| {
                CommandError::usage(
                    missing,
                    format!("`{}` requires a path", argument.to_string_lossy()),
                )
            })?;
            if value.is_empty() {
                return Err(CommandError::usage(
                    empty,
                    "inspection requires a nonempty path",
                ));
            }
            if target.replace(PathBuf::from(value)).is_some() {
                return Err(CommandError::usage(
                    duplicate,
                    "an inspection path may be supplied once",
                ));
            }
        } else if command == "explain"
            && id.is_none()
            && !argument.as_encoded_bytes().starts_with(b"-")
        {
            let value = argument
                .to_str()
                .filter(|value| metewand_core::inspection::is_inspection_id(value))
                .ok_or_else(|| {
                    CommandError::usage(
                        "invalid_inspection_id",
                        "`explain` requires a complete version-1 stable identity",
                    )
                })?;
            id = Some(value.to_owned());
        } else {
            return Err(CommandError::usage(
                "unexpected_argument",
                format!(
                    "unexpected `{command}` argument `{}`",
                    argument.to_string_lossy()
                ),
            ));
        }
    }
    if command == "explain" && id.is_none() {
        return Err(CommandError::usage(
            "missing_inspection_id",
            "`explain` requires a stable identity",
        ));
    }
    if manifest.is_none() && output_directory.is_none() {
        manifest = Some(PathBuf::from("metewand.yaml"));
    }
    inspect(command, manifest, output_directory, id)
}

#[cfg(unix)]
fn inspect(
    command: &'static str,
    manifest: Option<PathBuf>,
    output_directory: Option<PathBuf>,
    id: Option<String>,
) -> Result<CommandResult, CommandError> {
    let mut inspection = metewand_runtime::inspection::Inspection::default();
    if let Some(manifest) = manifest {
        let repository = metewand_runtime::repository::check_repository(&manifest)
            .map_err(|error| CommandError::repository(command, &error))?;
        let plan = repository
            .logical_plan()
            .map_err(|error| CommandError::planning(command, &error))?;
        inspection
            .add_repository(&repository, &plan)
            .map_err(|error| CommandError::inspection(&error))?;
    }
    if let Some(output_directory) = output_directory {
        inspection
            .add_store(&output_directory)
            .map_err(|error| CommandError::inspection(&error))?;
    }
    if let Some(id) = id {
        Ok(CommandResult::Explain(Box::new(
            inspection
                .explain(&id)
                .map_err(|error| CommandError::inspection(&error))?,
        )))
    } else {
        Ok(CommandResult::Status(Box::new(inspection.status())))
    }
}

#[cfg(not(unix))]
fn inspect(
    _: &'static str,
    _: Option<PathBuf>,
    _: Option<PathBuf>,
    _: Option<String>,
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "run inspection currently requires a POSIX host",
        "Inspect this benchmark on a supported POSIX host.",
    ))
}
