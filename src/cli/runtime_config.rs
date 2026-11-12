use std::{ffi::OsString, path::PathBuf};

use crate::{
    cli::output::CommandError,
    protocol::RuntimeSettings,
    runtime::runtime_config::{RuntimeConfigError, load_runtime_config},
};

pub(super) fn arguments(
    arguments: &[OsString],
) -> Result<(Vec<OsString>, RuntimeSettings), CommandError> {
    let mut remaining = Vec::new();
    let mut selected = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--runtime-config" {
            let path = arguments
                .next()
                .filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"-"))
                .ok_or_else(|| {
                    CommandError::usage(
                        "missing_runtime_config",
                        "`--runtime-config` requires a nonempty path",
                    )
                })?;
            if selected.replace(PathBuf::from(path)).is_some() {
                return Err(CommandError::usage(
                    "duplicate_runtime_config",
                    "`--runtime-config` may be supplied once",
                ));
            }
        } else {
            remaining.push(argument.clone());
            if matches!(
                argument.to_str(),
                Some(
                    "--manifest"
                        | "--output-dir"
                        | "--cache-dir"
                        | "--experiment"
                        | "--implementation"
                        | "--dataset"
                        | "--problem"
                        | "--select"
                        | "--max-attempts"
                )
            ) {
                remaining.extend(arguments.next().cloned());
            }
        }
    }
    let settings = selected
        .as_deref()
        .map(load_runtime_config)
        .transpose()
        .map_err(error)?
        .unwrap_or_default();
    Ok((remaining, settings))
}

pub(super) fn error(error: RuntimeConfigError) -> CommandError {
    CommandError::runtime_config(&error)
}
