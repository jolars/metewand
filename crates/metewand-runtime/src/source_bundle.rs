//! Deterministic expansion of repository-owned source bundles.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use globset::GlobBuilder;
use metewand_core::{
    canonical::{CanonicalJsonError, CanonicalValue},
    identity::{IdentityError, identify_canonical},
    manifest::RepositoryPath,
    records::{RecordId, SourceBundleResource},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::local_tree::{LocalTreeHashError, hash_local_tree};

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

/// Expands and identifies a repository-owned source bundle from its contents.
///
/// The resource representation retains the expanded logical paths, entry
/// kinds, portable executable bit, file-content hashes, directory tree hashes,
/// and exact symbolic-link targets. Host paths and incidental metadata do not
/// contribute.
///
/// # Errors
///
/// Returns an expansion error or a failure to inspect, hash, or canonically
/// identify one of the selected entries.
pub fn identify_source_bundle(
    repository_root: &Path,
    patterns: &[RepositoryPath],
) -> Result<RecordId<SourceBundleResource>, SourceBundleIdentityError> {
    let expanded = expand_source_bundle(repository_root, patterns)?;
    let root = fs::canonicalize(repository_root).map_err(|source| {
        SourceBundleIdentityError::RepositoryRoot {
            path: repository_root.to_path_buf(),
            source,
        }
    })?;
    let entries = expanded
        .paths()
        .iter()
        .map(|path| source_entry(&root, path))
        .collect::<Result<Vec<_>, SourceBundleIdentityError>>()?;
    let value = CanonicalValue::try_from(json!({"entries": entries}))?;
    identify_canonical(&value).map_err(SourceBundleIdentityError::Identity)
}

fn source_entry(root: &Path, path: &Path) -> Result<Value, SourceBundleIdentityError> {
    let absolute = root.join(path);
    let metadata = fs::symlink_metadata(&absolute).map_err(|source| {
        SourceBundleIdentityError::EntryAccess {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let path_text = utf8_path(path).map_err(SourceBundleIdentityError::Expansion)?;

    if metadata.is_dir() {
        let digest = hash_local_tree(&absolute).map_err(|source| {
            SourceBundleIdentityError::DirectoryHash {
                path: path.to_path_buf(),
                source,
            }
        })?;
        Ok(json!({
            "kind": "directory",
            "path": path_text,
            "tree_sha256": hex(digest.bytes()),
        }))
    } else if metadata.is_file() {
        let contents =
            fs::read(&absolute).map_err(|source| SourceBundleIdentityError::EntryRead {
                path: path.to_path_buf(),
                source,
            })?;
        let after =
            fs::metadata(&absolute).map_err(|source| SourceBundleIdentityError::EntryAccess {
                path: path.to_path_buf(),
                source,
            })?;
        if metadata.len() != contents.len() as u64 || metadata.len() != after.len() {
            return Err(SourceBundleIdentityError::ChangedDuringRead {
                path: path.to_path_buf(),
            });
        }
        Ok(json!({
            "executable": is_executable(&metadata),
            "kind": "file",
            "path": path_text,
            "sha256": hex(&Sha256::digest(contents)),
        }))
    } else if metadata.file_type().is_symlink() {
        let target =
            fs::read_link(&absolute).map_err(|source| SourceBundleIdentityError::EntryRead {
                path: path.to_path_buf(),
                source,
            })?;
        let target = target
            .to_str()
            .ok_or_else(|| SourceBundleIdentityError::NonUtf8Target {
                path: path.to_path_buf(),
                target: target.clone(),
            })?;
        Ok(json!({
            "kind": "symlink",
            "path": path_text,
            "target": target,
        }))
    } else {
        Err(SourceBundleIdentityError::UnsupportedEntry {
            path: path.to_path_buf(),
        })
    }
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
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

/// A failure to derive a content identity for a declared source bundle.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SourceBundleIdentityError {
    /// The source declarations could not be expanded safely.
    #[error(transparent)]
    Expansion(#[from] SourceBundleError),

    /// The repository root could not be resolved after expansion.
    #[error("failed to resolve repository root `{}`: {source}", path.display())]
    RepositoryRoot {
        /// Supplied repository root.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A selected entry could not be inspected.
    #[error("failed to inspect source entry `{}`: {source}", path.display())]
    EntryAccess {
        /// Logical repository path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A selected entry could not be read.
    #[error("failed to read source entry `{}`: {source}", path.display())]
    EntryRead {
        /// Logical repository path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A directory could not be hashed with the local-tree contract.
    #[error("failed to hash source directory `{}`: {source}", path.display())]
    DirectoryHash {
        /// Logical repository path.
        path: PathBuf,
        /// Tree-hash failure.
        #[source]
        source: LocalTreeHashError,
    },

    /// A regular file changed size while it was read.
    #[error("source entry `{}` changed while it was read", path.display())]
    ChangedDuringRead {
        /// Logical repository path.
        path: PathBuf,
    },

    /// A symbolic-link target is outside the portable UTF-8 path domain.
    #[error("source symlink `{}` has non-UTF-8 target `{}`", path.display(), target.display())]
    NonUtf8Target {
        /// Logical repository path.
        path: PathBuf,
        /// Native link target.
        target: PathBuf,
    },

    /// A selected path is neither a directory, regular file, nor symbolic link.
    #[error("source entry `{}` has an unsupported file type", path.display())]
    UnsupportedEntry {
        /// Logical repository path.
        path: PathBuf,
    },

    /// The resource inventory was outside the canonical JSON domain.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),

    /// The resource inventory could not receive an identity.
    #[error(transparent)]
    Identity(#[from] IdentityError),
}
