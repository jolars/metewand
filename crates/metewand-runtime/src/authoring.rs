//! Explicit, no-overwrite repository authoring operations.

use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
};

use metewand_core::MANIFEST_VERSION;
use serde::Serialize;
use thiserror::Error;

/// A path created by an authoring operation, or proposed by its dry run.
#[derive(Clone, Debug, Serialize)]
pub struct CreatedPath {
    /// Path as addressed by the operation.
    pub path: PathBuf,
    /// Type of entry created at this path.
    pub kind: CreatedPathKind,
}

/// Filesystem entry types created by authoring commands.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatedPathKind {
    /// A directory required to contain the manifest.
    Directory,
    /// The minimal benchmark manifest.
    File,
}

/// The ordered changes made or proposed by benchmark initialization.
#[derive(Debug, Serialize)]
pub struct InitReport {
    /// Command discriminator within the versioned machine-output envelope.
    pub command: &'static str,
    /// Whether paths were only proposed.
    pub dry_run: bool,
    /// Destination of the minimal manifest.
    pub manifest: PathBuf,
    /// Created paths in creation order, or proposed paths for a dry run.
    pub paths: Vec<CreatedPath>,
}

/// A failed initialization, including any paths already created.
#[derive(Debug, Error)]
#[error("cannot initialize benchmark at `{path}`: {source}")]
pub struct InitError {
    /// Path whose validation or creation failed.
    pub path: PathBuf,
    /// Filesystem failure; `AlreadyExists` also identifies preflight conflicts.
    #[source]
    pub source: io::Error,
    /// Paths created before this failure, retained for explicit inspection.
    pub created_paths: Vec<CreatedPath>,
}

/// Creates only a minimal versioned manifest, with no scientific definitions.
///
/// Missing destination directories are created in order and reported. Existing
/// directories are reused, while the manifest must be absent, even for a dry
/// run. File creation uses an exclusive open so competing invocations cannot
/// overwrite a manifest that appears after preflight.
///
/// # Errors
///
/// Returns path-aware errors for conflicts, invalid paths, or filesystem
/// failures. Paths must be nonempty UTF-8 so reports can always be serialized
/// before any mutation. A failed write may leave a partial file; the error
/// reports every created path and does not remove potentially changed files.
pub fn init_benchmark(directory: &Path, dry_run: bool) -> Result<InitReport, InitError> {
    let fail = |path: &Path, source| InitError {
        path: path.to_path_buf(),
        source,
        created_paths: Vec::new(),
    };
    if directory.as_os_str().is_empty() || directory.to_str().is_none() {
        return Err(fail(
            directory,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a nonempty UTF-8 directory path",
            ),
        ));
    }

    let manifest = directory.join("metewand.toml");
    let mut paths = Vec::new();
    let resolved_directory = plan_directory(directory, &mut paths)?;
    match fs::symlink_metadata(resolved_directory.join("metewand.toml")) {
        Ok(_) => {
            return Err(fail(
                &manifest,
                io::Error::new(io::ErrorKind::AlreadyExists, "manifest path already exists"),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(fail(&manifest, error)),
    }
    paths.push(CreatedPath {
        path: manifest.clone(),
        kind: CreatedPathKind::File,
    });
    if dry_run {
        return Ok(InitReport {
            command: "init",
            dry_run,
            manifest,
            paths,
        });
    }

    let created_paths = create_paths(paths)?;
    Ok(InitReport {
        command: "init",
        dry_run,
        manifest,
        paths: created_paths,
    })
}

fn plan_directory(directory: &Path, paths: &mut Vec<CreatedPath>) -> Result<PathBuf, InitError> {
    let mut addressed = PathBuf::new();
    let mut resolved = if directory.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().map_err(|source| InitError {
            path: directory.to_path_buf(),
            source,
            created_paths: Vec::new(),
        })?
    };
    let mut planned = BTreeSet::new();
    for component in directory.components() {
        addressed.push(component);
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
                continue;
            }
            _ => resolved.push(component),
        }
        if planned.contains(&resolved) {
            continue;
        }
        let fail = |source| InitError {
            path: addressed.clone(),
            source,
            created_paths: Vec::new(),
        };
        match fs::metadata(&resolved) {
            Ok(metadata) if metadata.is_dir() => {
                resolved = fs::canonicalize(&resolved).map_err(fail)?;
                continue;
            }
            Ok(_) => {
                return Err(fail(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "expected a directory",
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(fail(error)),
        }
        // A dangling symlink is a conflict, not a directory to create.
        match fs::symlink_metadata(&resolved) {
            Ok(_) => {
                return Err(fail(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "path already exists",
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(fail(error)),
        }
        planned.insert(resolved.clone());
        paths.push(CreatedPath {
            path: addressed.clone(),
            kind: CreatedPathKind::Directory,
        });
    }
    Ok(resolved)
}

fn create_paths(paths: Vec<CreatedPath>) -> Result<Vec<CreatedPath>, InitError> {
    let mut created_paths = Vec::new();
    for entry in paths {
        let result = match entry.kind {
            CreatedPathKind::Directory => fs::create_dir(&entry.path),
            CreatedPathKind::File => {
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&entry.path)
                {
                    Ok(mut file) => {
                        // Record creation before writing so a partial write remains visible.
                        created_paths.push(entry.clone());
                        file.write_all(
                            format!("version = {MANIFEST_VERSION}\nname = \"benchmark\"\n")
                                .as_bytes(),
                        )
                    }
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(source) = result {
            return Err(InitError {
                path: entry.path,
                source,
                created_paths,
            });
        }
        if matches!(entry.kind, CreatedPathKind::Directory) {
            created_paths.push(entry);
        }
    }
    Ok(created_paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_conflicts_preserve_existing_files_and_report_prior_creations() {
        let root = tempfile::TempDir::new().unwrap();
        let directory = root.path().join("created");
        let manifest = root.path().join("metewand.toml");
        fs::write(&manifest, "preserve\n").unwrap();
        let error = create_paths(vec![
            CreatedPath {
                path: directory.clone(),
                kind: CreatedPathKind::Directory,
            },
            CreatedPath {
                path: manifest.clone(),
                kind: CreatedPathKind::File,
            },
        ])
        .unwrap_err();
        assert_eq!(error.source.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(error.path, manifest);
        assert_eq!(error.created_paths.len(), 1);
        assert_eq!(error.created_paths[0].path, directory);
        assert!(directory.is_dir());
        assert_eq!(fs::read_to_string(manifest).unwrap(), "preserve\n");
    }
}
