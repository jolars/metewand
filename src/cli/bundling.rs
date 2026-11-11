use super::output::{CommandError, CommandResult};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

const HELP: &str = "Usage: metewand [--output FORMAT] bundle --destination PATH --base-image IMAGE@sha256:DIGEST [--manifest PATH] [--runner-version VERSION] [--recipe PATH] [--include PATTERN]... [--dry-run]\n\nGenerate a whole-benchmark Docker build context without downloads or execution. Requires local environments and local data. Recipe paths and include patterns are relative to the manifest. The recipe runs with /bin/sh -eu during docker build. Destination must be new, outside the benchmark root, with an existing parent. Defaults to this CLI's exact package version. Run the built image with a host directory mounted at /output; results go to /output/run.\n";

pub fn command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["--help"] || arguments == ["-h"] {
        return Ok(CommandResult::help(HELP));
    }
    let mut options = BTreeMap::new();
    let mut includes = Vec::new();
    let mut dry_run = false;
    let mut args = arguments.iter();
    while let Some(flag) = args.next() {
        let name = flag.to_str().unwrap_or("");
        if name == "--dry-run" && !dry_run {
            dry_run = true;
            continue;
        }
        if !matches!(
            name,
            "--manifest"
                | "--destination"
                | "--base-image"
                | "--runner-version"
                | "--recipe"
                | "--include"
        ) {
            return Err(CommandError::usage(
                "invalid_bundle_option",
                format!("unexpected or duplicate option `{name}`"),
            ));
        }
        let value = args
            .next()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty() && !s.starts_with("--"))
            .ok_or_else(|| {
                CommandError::usage("missing_bundle_value", format!("`{name}` requires a value"))
            })?;
        if name == "--include" {
            includes.push(value.to_owned());
        } else if options.insert(name, value.to_owned()).is_some() {
            return Err(CommandError::usage(
                "duplicate_bundle_option",
                format!("`{name}` may be supplied once"),
            ));
        }
    }
    let required = |name| {
        options.get(name).cloned().ok_or_else(|| {
            CommandError::usage(
                "missing_bundle_option",
                format!("bundling requires `{name}`"),
            )
        })
    };
    let destination = PathBuf::from(required("--destination")?);
    let base_image = required("--base-image")?;
    let manifest = PathBuf::from(
        options
            .get("--manifest")
            .map_or("metewand.yaml", String::as_str),
    );
    let repository = crate::runtime::repository::check_repository(&manifest)
        .map_err(|e| CommandError::repository("bundle", &e))?;
    #[cfg(any(unix, windows))]
    {
        let path = |s: String| {
            serde_json::from_value(serde_json::json!(s))
                .map_err(|e| CommandError::usage("invalid_bundle_path", e.to_string()))
        };
        let options = crate::runtime::bundling::BundleOptions {
            destination,
            base_image,
            runner_version: options
                .remove("--runner-version")
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()),
            recipe: options.remove("--recipe").map(path).transpose()?,
            includes: includes.into_iter().map(path).collect::<Result<_, _>>()?,
            dry_run,
        };
        let report = crate::runtime::bundling::bundle_repository(&repository, &options)
            .map_err(|e| CommandError::bundling(&e))?;
        Ok(CommandResult::Bundle(Box::new(report)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (destination, base_image, repository);
        Err(CommandError::usage(
            "unsupported_bundling",
            "bundling requires Linux, macOS, or Windows",
        ))
    }
}
