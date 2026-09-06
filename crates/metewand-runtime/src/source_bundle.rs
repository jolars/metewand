//! Deterministic expansion of repository-owned source bundles.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use globset::GlobBuilder;
use metewand_core::manifest::RepositoryPath;
use thiserror::Error;

/// A source bundle expanded to normalized logical repository paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedSourceBundle {
    paths: Vec<PathBuf>,
}

impl ExpandedSourceBundle {
    /// Returns the expanded repository-relative paths in UTF-8 byte order.
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

/// Expands repository-relative file and glob declarations without following
/// symlinks while walking the repository tree.
///
/// Every declaration must match at least one filesystem entry. The returned
/// paths are relative to the canonical repository root, deduplicated, and
/// sorted by their UTF-8 bytes. A matched directory also establishes a tree
/// boundary: every entry below it is checked, even when only the directory is
/// returned. Symlinks may point within the repository, but broken links and
/// links whose resolved targets escape it are rejected.
///
/// # Errors
///
/// Returns an error when the repository root cannot be established, a glob is
/// invalid or unmatched, a selected path is not UTF-8, the selected tree cannot
/// be read, or containment cannot be proved.
pub fn expand_source_bundle(
    repository_root: &Path,
    patterns: &[RepositoryPath],
) -> Result<ExpandedSourceBundle, SourceBundleError> {
    if patterns.is_empty() {
        return Err(SourceBundleError::Empty);
    }

    let repository_root =
        fs::canonicalize(repository_root).map_err(|source| SourceBundleError::RepositoryRoot {
            path: repository_root.to_path_buf(),
            source,
        })?;
    if !repository_root.is_dir() {
        return Err(SourceBundleError::RepositoryRootNotDirectory {
            path: repository_root,
        });
    }

    let mut selected = BTreeSet::new();
    for pattern in patterns {
        let pattern_path = pattern.as_path();
        let pattern_text = pattern_path
            .to_str()
            .expect("RepositoryPath values are valid UTF-8");
        let matcher = GlobBuilder::new(pattern_text)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .map_err(|error| SourceBundleError::InvalidPattern {
                pattern: pattern_path.to_path_buf(),
                message: error.to_string(),
            })?
            .compile_matcher();

        let (prefix, has_meta) = literal_prefix(pattern_text);
        let candidates = pattern_candidates(&repository_root, &prefix, has_meta)?;
        let matches = candidates
            .into_iter()
            .filter(|candidate| matcher.is_match(candidate))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Err(SourceBundleError::UnmatchedPattern {
                pattern: pattern_path.to_path_buf(),
            });
        }
        selected.extend(matches);
    }

    let mut paths = selected.into_iter().collect::<Vec<_>>();
    for path in &paths {
        utf8_path(path)?;
    }
    paths.sort_by(|left, right| {
        utf8_path(left)
            .expect("matched paths were checked above")
            .as_bytes()
            .cmp(
                utf8_path(right)
                    .expect("matched paths were checked above")
                    .as_bytes(),
            )
    });

    for path in &paths {
        validate_selected_tree(&repository_root, path)?;
    }

    Ok(ExpandedSourceBundle { paths })
}

fn literal_prefix(pattern: &str) -> (PathBuf, bool) {
    let mut prefix = PathBuf::new();
    for component in pattern.split('/') {
        if component.bytes().any(is_glob_metacharacter) {
            return (prefix, true);
        }
        prefix.push(component);
    }
    (prefix, false)
}

fn is_glob_metacharacter(byte: u8) -> bool {
    matches!(byte, b'*' | b'?' | b'[' | b']' | b'{' | b'}')
}

fn pattern_candidates(
    repository_root: &Path,
    prefix: &Path,
    has_meta: bool,
) -> Result<Vec<PathBuf>, SourceBundleError> {
    let absolute_prefix = repository_root.join(prefix);
    let metadata = match fs::symlink_metadata(&absolute_prefix) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(SourceBundleError::PathAccess {
                path: prefix.to_path_buf(),
                source,
            });
        }
    };
    ensure_contained(repository_root, prefix, metadata.file_type().is_symlink())?;

    if !has_meta {
        return Ok(vec![prefix.to_path_buf()]);
    }

    if !metadata.is_dir() {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::new();
    collect_descendants(&absolute_prefix, prefix, &mut candidates)?;
    Ok(candidates)
}

fn collect_descendants(
    absolute_directory: &Path,
    relative_directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), SourceBundleError> {
    let mut entries = fs::read_dir(absolute_directory)
        .map_err(|source| SourceBundleError::PathAccess {
            path: relative_directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SourceBundleError::PathAccess {
            path: relative_directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let relative_path = relative_directory.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|source| SourceBundleError::PathAccess {
                path: relative_path.clone(),
                source,
            })?;
        paths.push(relative_path.clone());
        if file_type.is_dir() {
            collect_descendants(&entry.path(), &relative_path, paths)?;
        }
    }
    Ok(())
}

fn validate_selected_tree(
    repository_root: &Path,
    selected: &Path,
) -> Result<(), SourceBundleError> {
    let absolute = repository_root.join(selected);
    let metadata =
        fs::symlink_metadata(&absolute).map_err(|source| SourceBundleError::PathAccess {
            path: selected.to_path_buf(),
            source,
        })?;
    ensure_contained(repository_root, selected, metadata.file_type().is_symlink())?;

    if metadata.is_dir() {
        let mut descendants = Vec::new();
        collect_descendants(&absolute, selected, &mut descendants)?;
        for descendant in descendants {
            utf8_path(&descendant)?;
            let file_type = fs::symlink_metadata(repository_root.join(&descendant))
                .map_err(|source| SourceBundleError::PathAccess {
                    path: descendant.clone(),
                    source,
                })?
                .file_type();
            ensure_contained(repository_root, &descendant, file_type.is_symlink())?;
        }
    }
    Ok(())
}

fn ensure_contained(
    repository_root: &Path,
    relative_path: &Path,
    is_symlink: bool,
) -> Result<(), SourceBundleError> {
    let resolved = fs::canonicalize(repository_root.join(relative_path)).map_err(|source| {
        if is_symlink {
            SourceBundleError::SymlinkResolution {
                path: relative_path.to_path_buf(),
                source,
            }
        } else {
            SourceBundleError::PathResolution {
                path: relative_path.to_path_buf(),
                source,
            }
        }
    })?;
    if resolved.starts_with(repository_root) {
        Ok(())
    } else {
        Err(SourceBundleError::SymlinkEscape {
            path: relative_path.to_path_buf(),
            target: resolved,
        })
    }
}

fn utf8_path(path: &Path) -> Result<&str, SourceBundleError> {
    path.to_str().ok_or_else(|| SourceBundleError::NonUtf8Path {
        path: path.to_path_buf(),
    })
}

/// A failure to establish a deterministic, repository-contained source bundle.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SourceBundleError {
    /// No source declarations were supplied.
    #[error("source bundle cannot be empty")]
    Empty,

    /// The repository root could not be resolved.
    #[error("failed to resolve repository root `{}`: {source}", path.display())]
    RepositoryRoot {
        /// Supplied repository root.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// The resolved repository root is not a directory.
    #[error("repository root `{}` is not a directory", path.display())]
    RepositoryRootNotDirectory {
        /// Resolved repository root.
        path: PathBuf,
    },

    /// A source declaration is not a valid glob.
    #[error("source pattern `{}` is invalid: {message}", pattern.display())]
    InvalidPattern {
        /// Invalid logical pattern.
        pattern: PathBuf,
        /// Glob parser diagnostic.
        message: String,
    },

    /// A source declaration matched no entry.
    #[error("source pattern `{}` did not match any repository entry", pattern.display())]
    UnmatchedPattern {
        /// Unmatched logical pattern.
        pattern: PathBuf,
    },

    /// A selected path could not be inspected.
    #[error("failed to inspect source path `{}`: {source}", path.display())]
    PathAccess {
        /// Logical repository path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A non-symlink path could not be resolved during a containment check.
    #[error("failed to resolve source path `{}`: {source}", path.display())]
    PathResolution {
        /// Logical repository path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A selected path is not representable by the public path contract.
    #[error("source path `{}` is not valid UTF-8", path.display())]
    NonUtf8Path {
        /// Logical repository path.
        path: PathBuf,
    },

    /// A symlink target could not be resolved for containment checking.
    #[error("failed to resolve source symlink `{}`: {source}", path.display())]
    SymlinkResolution {
        /// Logical repository path of the symlink.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A source path resolves outside the repository tree.
    #[error(
        "source path `{}` resolves outside the repository to `{}`",
        path.display(),
        target.display()
    )]
    SymlinkEscape {
        /// Logical repository path that crossed the tree boundary.
        path: PathBuf,
        /// Resolved path outside the repository.
        target: PathBuf,
    },
}
