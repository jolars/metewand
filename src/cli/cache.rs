use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use crate::core::cache::CacheOperation;

use crate::cli::output::{CommandError, CommandResult};

const HELP: &str = "Usage: metewand [--output FORMAT] cache <operation> --cache-dir PATH [OPTIONS]\n\nOperations:\n  verify --id tree-v1-SHA256 ...    Verify exact objects without changing the cache.\n  gc [--id tree-v1-SHA256 ...] [--dry-run]\n                                  Collect unrooted, unleased cataloged objects.\n  roots                           List durable named roots.\n  root add|remove --root NAME --id tree-v1-SHA256 [--dry-run]\n                                  Add or release one exact root-to-object edge.\n\nThe cache must already exist. Verification and dry runs create no cache files or SQLite sidecars. Collection uses a fresh transactional catalog snapshot and live kernel leases; indexes never authorize deletion. Run roots persist until explicitly released, even if the run directory disappears.\n";

pub fn command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(HELP));
    }
    let (operation, rest) = match arguments {
        [op, rest @ ..] if op == "verify" => (CacheOperation::Verify, rest),
        [op, rest @ ..] if op == "gc" => (CacheOperation::Gc, rest),
        [op, rest @ ..] if op == "roots" => (CacheOperation::Roots, rest),
        [op, action, rest @ ..] if op == "root" && action == "add" => {
            (CacheOperation::RootAdd, rest)
        }
        [op, action, rest @ ..] if op == "root" && action == "remove" => {
            (CacheOperation::RootRemove, rest)
        }
        _ => {
            return Err(CommandError::usage(
                "invalid_cache_operation",
                "expected `cache verify`, `cache gc`, `cache roots`, or `cache root add|remove`",
            ));
        }
    };
    let mut options = BTreeMap::new();
    let mut ids = Vec::new();
    let mut dry_run = false;
    let mut arguments = rest.iter();
    while let Some(argument) = arguments.next() {
        let flag = argument.to_str().unwrap_or("");
        if flag == "--dry-run" && !dry_run {
            dry_run = true;
        } else if matches!(flag, "--cache-dir" | "--id" | "--root") {
            let value = arguments
                .next()
                .filter(|v| !v.is_empty() && !v.to_string_lossy().starts_with("--"))
                .ok_or_else(|| {
                    CommandError::usage(
                        "missing_cache_value",
                        format!("`{flag}` requires a nonempty value"),
                    )
                })?;
            if flag == "--id" {
                ids.push(
                    value
                        .to_str()
                        .ok_or_else(|| {
                            CommandError::usage(
                                "invalid_cache_id",
                                "cache identities must be UTF-8",
                            )
                        })?
                        .to_owned(),
                );
            } else if options.insert(flag, value.clone()).is_some() {
                return Err(CommandError::usage(
                    "duplicate_cache_option",
                    format!("`{flag}` may be supplied once"),
                ));
            }
        } else {
            return Err(CommandError::usage(
                "unexpected_argument",
                format!("unexpected cache argument `{}`", argument.to_string_lossy()),
            ));
        }
    }
    let path = options
        .get("--cache-dir")
        .map(PathBuf::from)
        .ok_or_else(|| {
            CommandError::usage(
                "missing_cache_directory",
                "`cache` requires `--cache-dir PATH`",
            )
        })?;
    let root = options
        .get("--root")
        .map(|value| {
            value.to_str().ok_or_else(|| {
                CommandError::usage("invalid_cache_root", "root names must be UTF-8")
            })
        })
        .transpose()?;
    let root_operation = matches!(
        operation,
        CacheOperation::RootAdd | CacheOperation::RootRemove
    );
    if root_operation && (root.is_none() || ids.len() != 1) {
        return Err(CommandError::usage(
            "missing_cache_root_target",
            "`cache root add|remove` requires `--root NAME` and exactly one `--id tree-v1-SHA256`",
        ));
    }
    if (!root_operation && root.is_some())
        || (operation == CacheOperation::Roots && !ids.is_empty())
        || (matches!(operation, CacheOperation::Verify | CacheOperation::Roots) && dry_run)
    {
        return Err(CommandError::usage(
            "invalid_cache_options",
            "these options do not apply to the selected cache operation",
        ));
    }
    if operation == CacheOperation::Verify && ids.is_empty() {
        return Err(CommandError::usage(
            "missing_cache_id",
            "`cache verify` requires at least one exact `--id tree-v1-SHA256`",
        ));
    }
    maintain(path, operation, ids, root, dry_run)
}

#[cfg(any(unix, windows))]
fn maintain(
    path: PathBuf,
    operation: CacheOperation,
    ids: Vec<String>,
    root: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, CommandError> {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| CommandError::usage("cache_directory_unavailable", error.to_string()))?
            .join(path)
    };
    let store = crate::runtime::artifact_store::ArtifactStore::open_existing(&path)
        .map_err(|error| CommandError::cache(&error))?;
    let result = match operation {
        CacheOperation::Verify => store.verify_targets(&ids),
        CacheOperation::Gc => store.collect(&ids, dry_run),
        CacheOperation::Roots => store.roots(),
        CacheOperation::RootAdd | CacheOperation::RootRemove => store.update_root(
            &ids[0],
            root.expect("validated root"),
            operation == CacheOperation::RootAdd,
            dry_run,
        ),
    }
    .map_err(|error| CommandError::cache(&error))?;
    Ok(CommandResult::Cache(Box::new(result)))
}

#[cfg(not(any(unix, windows)))]
fn maintain(
    _: PathBuf,
    _: CacheOperation,
    _: Vec<String>,
    _: Option<&str>,
    _: bool,
) -> Result<CommandResult, CommandError> {
    Err(CommandError::unsupported(
        "cache maintenance requires a POSIX host",
        "Use a supported POSIX host.",
    ))
}
