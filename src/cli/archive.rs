use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use crate::core::archive::ArchiveOperation;

use crate::cli::output::{CommandError, CommandResult};

const HELP: &str = "Usage: metewand [--output FORMAT] archive <operation> [OPTIONS]\n\nOperations:\n  create --output-dir PATH --destination FILE.tar.gz\n  verify --archive PATH [--max-entries N] [--max-bytes N]\n  import --archive FILE.tar.gz --destination PATH [--max-entries N] [--max-bytes N]\n  restore --archive FILE.tar.gz --destination PATH [--max-entries N] [--max-bytes N]\n\nCreate a deterministic whole-run archive from committed evidence after all started attempts are terminal. Pending work and failures are preserved. Verification accepts a file or an imported collection, works offline, and never runs benchmark code. Import creates a read-only collection for status, explain, and export. Restore rebuilds a resumable run on supported local storage, preserving accepted measurements, failures, retries, pending work, and profile budgets. It does not execute workers or restore ownership leases. Resume requires the original benchmark sources, inputs, and compatible prepared environments. Destinations must be new and have existing parents. Source code and input data outside registered artifacts are not gathered. Verification, import, and restore default to 1000000 entries and 1099511627776 expanded bytes.\n";

pub fn command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["--help"]
        || arguments == ["-h"]
        || arguments
            .get(1)
            .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        return Ok(CommandResult::help(HELP));
    }
    let (operation, rest) = match arguments {
        [op, rest @ ..] if op == "create" => (ArchiveOperation::Create, rest),
        [op, rest @ ..] if op == "verify" => (ArchiveOperation::Verify, rest),
        [op, rest @ ..] if op == "import" => (ArchiveOperation::Import, rest),
        [op, rest @ ..] if op == "restore" => (ArchiveOperation::Restore, rest),
        _ => {
            return Err(CommandError::usage(
                "invalid_archive_operation",
                "expected `archive create`, `archive verify`, `archive import`, or `archive restore`",
            ));
        }
    };
    let mut options = BTreeMap::new();
    let mut args = rest.iter();
    while let Some(flag) = args.next() {
        let name = flag.to_str().unwrap_or("");
        if !matches!(
            name,
            "--output-dir" | "--archive" | "--destination" | "--max-entries" | "--max-bytes"
        ) {
            return Err(CommandError::usage(
                "unexpected_argument",
                format!("unexpected archive argument `{}`", flag.to_string_lossy()),
            ));
        }
        let value = args
            .next()
            .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
            .ok_or_else(|| {
                CommandError::usage(
                    "missing_archive_value",
                    format!("`{name}` requires a nonempty value"),
                )
            })?;
        if options.insert(name, value.clone()).is_some() {
            return Err(CommandError::usage(
                "duplicate_archive_option",
                format!("`{name}` may be supplied once"),
            ));
        }
    }
    let input_flag = if matches!(operation, ArchiveOperation::Create) {
        "--output-dir"
    } else {
        "--archive"
    };
    let input = options
        .remove(input_flag)
        .map(PathBuf::from)
        .ok_or_else(|| {
            CommandError::usage(
                "missing_archive_input",
                format!("archive operation requires `{input_flag} PATH`"),
            )
        })?;
    let destination = options.remove("--destination").map(PathBuf::from);
    if matches!(operation, ArchiveOperation::Verify) == destination.is_some() {
        return Err(CommandError::usage(
            "invalid_archive_destination",
            "create, import, and restore require --destination; verify does not accept a destination",
        ));
    }
    let mut limits = (1_000_000, 1_u64 << 40);
    if !matches!(operation, ArchiveOperation::Create) {
        for (flag, target) in [
            ("--max-entries", &mut limits.0),
            ("--max-bytes", &mut limits.1),
        ] {
            if let Some(value) = options.remove(flag) {
                *target = value
                    .to_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .filter(|n| *n > 0 && *n <= crate::core::canonical::MAX_SAFE_INTEGER)
                    .ok_or_else(|| {
                        CommandError::usage(
                            "invalid_archive_limit",
                            format!(
                                "`{flag}` requires a positive integer in the canonical JSON range"
                            ),
                        )
                    })?;
            }
        }
    }
    if !options.is_empty() {
        return Err(CommandError::usage(
            "invalid_archive_options",
            "these options do not apply to the selected archive operation",
        ));
    }
    execute(operation, input, destination, limits)
}

#[cfg(any(unix, windows))]
fn execute(
    operation: ArchiveOperation,
    input: PathBuf,
    destination: Option<PathBuf>,
    limits: (u64, u64),
) -> Result<CommandResult, CommandError> {
    use crate::runtime::archive::{
        ArchiveLimits, create_archive, import_archive, restore_archive, verify_archive,
    };
    let limits = ArchiveLimits {
        max_entries: limits.0,
        max_expanded_bytes: limits.1,
    };
    let report = match operation {
        ArchiveOperation::Create => {
            create_archive(&input, destination.as_deref().expect("checked destination"))
        }
        ArchiveOperation::Verify => verify_archive(&input, limits),
        ArchiveOperation::Import => import_archive(
            &input,
            destination.as_deref().expect("checked destination"),
            limits,
        ),
        ArchiveOperation::Restore => restore_archive(
            &input,
            destination.as_deref().expect("checked destination"),
            limits,
        ),
    }
    .map_err(|error| CommandError::archive(&error))?;
    Ok(CommandResult::Archive(Box::new(report)))
}

#[cfg(not(any(unix, windows)))]
fn execute(
    _: ArchiveOperation,
    _: PathBuf,
    _: Option<PathBuf>,
    _: (u64, u64),
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "result archives require a supported host",
        "Use Linux, macOS, or Windows.",
    ))
}
