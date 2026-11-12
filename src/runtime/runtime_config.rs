//! Read-only loading and path resolution of explicit local runtime settings.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{core::manifest::SourceSpan, protocol::RuntimeSettings};

/// A local configuration could not be read, decoded, or resolved.
#[derive(Debug, Error)]
pub enum RuntimeConfigError {
    #[error("cannot read runtime configuration `{path}`: {source}")]
    Access { path: PathBuf, source: io::Error },
    #[error("invalid runtime configuration: {message}")]
    Parse { message: String, span: SourceSpan },
    #[error("cannot resolve download cache `{name}` at `{path}`: {source}")]
    Cache {
        name: String,
        path: PathBuf,
        source: io::Error,
    },
    #[error("download cache `{name}` at `{path}` overlaps selected worker sources")]
    SourceOverlap { name: String, path: PathBuf },
    #[error(transparent)]
    Sources(#[from] crate::runtime::source_bundle::SourceBundleError),
}

/// Rejects mutable download caches within any declared scientific source bundle.
pub fn validate_runtime_config(
    repository: &crate::runtime::repository::CheckedRepository,
    runtime: &RuntimeSettings,
) -> Result<(), RuntimeConfigError> {
    let patterns = repository
        .manifest
        .datasets
        .values()
        .filter_map(|dataset| match dataset {
            crate::core::manifest::DatasetDefinition::Generated(dataset) => Some(&dataset.worker),
            crate::core::manifest::DatasetDefinition::Fixed(_) => None,
        })
        .chain(
            repository
                .manifest
                .implementations
                .values()
                .map(|implementation| &implementation.worker),
        )
        .chain(repository.manifest.problems.values().flat_map(|problem| {
            std::iter::once(&problem.evaluator).chain(
                problem
                    .preparation
                    .iter()
                    .map(|preparation| &preparation.worker),
            )
        }))
        .filter_map(crate::core::manifest::WorkerDefinition::sources)
        .collect::<Vec<_>>();
    for (name, directory) in runtime.caches() {
        let path = Path::new(directory);
        for patterns in &patterns {
            // A provider cache may write anywhere below its root, including
            // scientific source prefixes nested within that directory.
            if patterns.iter().any(|pattern| {
                let (prefix, _) = crate::runtime::source_bundle::literal_prefix(pattern.as_str());
                repository.root.join(prefix).starts_with(path)
            }) || crate::runtime::source_bundle::source_bundle_overlaps_directory(
                &repository.root,
                patterns,
                path,
            )? {
                return Err(RuntimeConfigError::SourceOverlap {
                    name: name.clone(),
                    path: path.to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Loads only the selected configuration. Relative cache paths resolve beside
/// that file. Resolving a missing cache requires an existing parent and creates
/// no files or directories; the provider owns cache initialization and repair.
pub fn load_runtime_config(path: &Path) -> Result<RuntimeSettings, RuntimeConfigError> {
    let source = fs::read_to_string(path).map_err(|source| RuntimeConfigError::Access {
        path: path.to_owned(),
        source,
    })?;
    let (mut settings, _): (RuntimeSettings, _) =
        crate::core::yaml::parse(path, &source).map_err(|error| RuntimeConfigError::Parse {
            message: error.message,
            span: error.source_span,
        })?;
    let parent = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
    .map_err(|source| RuntimeConfigError::Access {
        path: path.to_owned(),
        source,
    })?;
    for (name, value) in &mut settings.caches {
        let path = parent.join(&*value);
        let resolved =
            resolve_cache_directory(&path).map_err(|source| RuntimeConfigError::Cache {
                name: name.clone(),
                path,
                source,
            })?;
        *value =
            resolved
                .into_os_string()
                .into_string()
                .map_err(|_| RuntimeConfigError::Cache {
                    name: name.clone(),
                    path: PathBuf::from(&*value),
                    source: io::Error::other("cache paths must be UTF-8"),
                })?;
    }
    Ok(settings)
}

fn resolve_cache_directory(path: &Path) -> io::Result<PathBuf> {
    match fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(io::Error::other("download cache must be a directory"));
            }
            fs::canonicalize(path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = fs::canonicalize(
                path.parent()
                    .ok_or_else(|| io::Error::other("download cache needs a parent"))?,
            )?;
            if !parent.is_dir() {
                return Err(io::Error::other(
                    "download cache parent must be a directory",
                ));
            }
            Ok(parent.join(
                path.file_name()
                    .ok_or_else(|| io::Error::other("download cache needs a name"))?,
            ))
        }
        Err(error) => Err(error),
    }
}
