//! Versioned, deterministic hashing of local filesystem trees.

use std::{
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
};

use metewand_core::records::ContentDigest;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::LOCAL_TREE_HASH_VERSION;

const TRANSCRIPT_DOMAIN: &[u8] = b"metewand-local-tree\0";

/// Hashes all entries below a local directory using the version-1 tree
/// transcript.
///
/// The root itself is not an entry. Descendant paths are normalized to
/// relative UTF-8 strings with `/` separators and sorted by their UTF-8 bytes.
/// Directory entries contribute their type, regular files also contribute
/// their bytes and whether any executable bit is set, and symbolic links also
/// contribute their exact UTF-8 target spelling. Other permissions, timestamps,
/// ownership, and the spelling of `root` do not contribute.
///
/// Symbolic links are hashed without being followed, so their targets need not
/// exist or remain within the tree. Safety policy for accepted link targets is
/// a separate validation step.
///
/// # Errors
///
/// Returns an error if the root cannot be resolved to a directory, a descendant
/// cannot be inspected or read, a path or link target is not UTF-8, an entry has
/// an unsupported type, or a file changes size while it is being read.
pub fn hash_local_tree(root: impl AsRef<Path>) -> Result<ContentDigest, LocalTreeHashError> {
    let supplied_root = root.as_ref();
    let root = fs::canonicalize(supplied_root).map_err(|source| LocalTreeHashError::Root {
        path: supplied_root.to_path_buf(),
        source,
    })?;
    if !root.is_dir() {
        return Err(LocalTreeHashError::RootNotDirectory { path: root });
    }

    let mut entries = Vec::new();
    collect_entries(&root, "", &mut entries)?;
    entries.sort_unstable_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));

    let mut hasher = Sha256::new();
    hasher.update(TRANSCRIPT_DOMAIN);
    hasher.update(LOCAL_TREE_HASH_VERSION.to_be_bytes());
    hasher.update((entries.len() as u64).to_be_bytes());
    for entry in entries {
        hash_entry(&root, &entry, &mut hasher)?;
    }

    Ok(ContentDigest::new(hasher.finalize().into()))
}

#[derive(Clone, Copy)]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Unsupported,
}

struct TreeEntry {
    path: String,
    kind: EntryKind,
}

fn collect_entries(
    absolute_directory: &Path,
    relative_directory: &str,
    entries: &mut Vec<TreeEntry>,
) -> Result<(), LocalTreeHashError> {
    let error_path = if relative_directory.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(relative_directory)
    };
    let mut directory_entries = fs::read_dir(absolute_directory)
        .map_err(|source| LocalTreeHashError::ReadDirectory {
            path: error_path.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| LocalTreeHashError::ReadDirectory {
            path: error_path,
            source,
        })?;
    directory_entries.sort_unstable_by_key(fs::DirEntry::file_name);

    for entry in directory_entries {
        let name =
            entry
                .file_name()
                .into_string()
                .map_err(|name| LocalTreeHashError::NonUtf8Path {
                    path: PathBuf::from(relative_directory).join(name),
                })?;
        if name.contains('\\') {
            return Err(LocalTreeHashError::NonNormalizedPath {
                path: PathBuf::from(relative_directory).join(name),
            });
        }
        let path = if relative_directory.is_empty() {
            name
        } else {
            format!("{relative_directory}/{name}")
        };
        let file_type = entry
            .file_type()
            .map_err(|source| LocalTreeHashError::EntryAccess {
                path: PathBuf::from(&path),
                source,
            })?;
        let kind = if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::File
        } else if file_type.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::Unsupported
        };
        entries.push(TreeEntry {
            path: path.clone(),
            kind,
        });
        if matches!(kind, EntryKind::Directory) {
            collect_entries(&entry.path(), &path, entries)?;
        }
    }
    Ok(())
}

fn hash_entry(
    root: &Path,
    entry: &TreeEntry,
    hasher: &mut Sha256,
) -> Result<(), LocalTreeHashError> {
    let path = Path::new(&entry.path);
    let absolute = root.join(path);
    let metadata =
        fs::symlink_metadata(&absolute).map_err(|source| LocalTreeHashError::EntryAccess {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_entry_kind(entry.kind, &metadata) {
        return Err(LocalTreeHashError::ChangedDuringHash {
            path: path.to_path_buf(),
        });
    }

    hasher.update((entry.path.len() as u64).to_be_bytes());
    hasher.update(entry.path.as_bytes());
    match entry.kind {
        EntryKind::Directory => hasher.update(b"d"),
        EntryKind::File => {
            hasher.update(b"f");
            hasher.update([u8::from(is_executable(&metadata))]);
            hash_file(&absolute, path, metadata.len(), hasher)?;
        }
        EntryKind::Symlink => {
            hasher.update(b"l");
            let target =
                fs::read_link(&absolute).map_err(|source| LocalTreeHashError::ReadSymlink {
                    path: path.to_path_buf(),
                    source,
                })?;
            let target_text =
                target
                    .to_str()
                    .ok_or_else(|| LocalTreeHashError::NonUtf8SymlinkTarget {
                        path: path.to_path_buf(),
                        target: target.clone(),
                    })?;
            hasher.update((target_text.len() as u64).to_be_bytes());
            hasher.update(target_text.as_bytes());
        }
        EntryKind::Unsupported => {
            return Err(LocalTreeHashError::UnsupportedEntryType {
                path: path.to_path_buf(),
            });
        }
    }
    Ok(())
}

fn same_entry_kind(expected: EntryKind, metadata: &Metadata) -> bool {
    let file_type = metadata.file_type();
    match expected {
        EntryKind::Directory => file_type.is_dir(),
        EntryKind::File => file_type.is_file(),
        EntryKind::Symlink => file_type.is_symlink(),
        EntryKind::Unsupported => {
            !file_type.is_dir() && !file_type.is_file() && !file_type.is_symlink()
        }
    }
}

fn hash_file(
    absolute: &Path,
    relative: &Path,
    expected_size: u64,
    hasher: &mut Sha256,
) -> Result<(), LocalTreeHashError> {
    hasher.update(expected_size.to_be_bytes());
    let mut file = File::open(absolute).map_err(|source| LocalTreeHashError::ReadFile {
        path: relative.to_path_buf(),
        source,
    })?;
    let mut remaining = expected_size;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let capacity = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("the read size is bounded by the in-memory buffer");
        let read =
            file.read(&mut buffer[..capacity])
                .map_err(|source| LocalTreeHashError::ReadFile {
                    path: relative.to_path_buf(),
                    source,
                })?;
        if read == 0 {
            return Err(LocalTreeHashError::ChangedDuringHash {
                path: relative.to_path_buf(),
            });
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }

    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|source| LocalTreeHashError::ReadFile {
            path: relative.to_path_buf(),
            source,
        })?
        != 0
    {
        return Err(LocalTreeHashError::ChangedDuringHash {
            path: relative.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &Metadata) -> bool {
    false
}

/// A failure to construct a deterministic local-tree digest.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalTreeHashError {
    /// The supplied root could not be resolved.
    #[error("failed to resolve local-tree root `{}`: {source}", path.display())]
    Root {
        /// Supplied root path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// The resolved root is not a directory.
    #[error("local-tree root `{}` is not a directory", path.display())]
    RootNotDirectory {
        /// Resolved root path.
        path: PathBuf,
    },

    /// A directory could not be enumerated.
    #[error("failed to read local-tree directory `{}`: {source}", path.display())]
    ReadDirectory {
        /// Normalized tree-relative directory path, or `.` for the root.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// An entry's type or metadata could not be inspected.
    #[error("failed to inspect local-tree entry `{}`: {source}", path.display())]
    EntryAccess {
        /// Normalized tree-relative entry path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A regular file could not be read.
    #[error("failed to read local-tree file `{}`: {source}", path.display())]
    ReadFile {
        /// Normalized tree-relative file path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A symbolic link target could not be read.
    #[error("failed to read local-tree symlink `{}`: {source}", path.display())]
    ReadSymlink {
        /// Normalized tree-relative link path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// An entry path is outside the portable tree contract.
    #[error("local-tree path `{}` is not valid UTF-8", path.display())]
    NonUtf8Path {
        /// Tree-relative path with the unrepresentable component.
        path: PathBuf,
    },

    /// An entry path cannot be represented unambiguously on every platform.
    #[error("local-tree path `{}` is not normalized", path.display())]
    NonNormalizedPath {
        /// Tree-relative path containing a backslash separator.
        path: PathBuf,
    },

    /// A symbolic link target is outside the portable tree contract.
    #[error(
        "target `{}` of local-tree symlink `{}` is not valid UTF-8",
        target.display(),
        path.display()
    )]
    NonUtf8SymlinkTarget {
        /// Normalized tree-relative link path.
        path: PathBuf,
        /// Unrepresentable target spelling.
        target: PathBuf,
    },

    /// The tree contains an entry that cannot be represented in an artifact.
    #[error("local-tree entry `{}` has an unsupported type", path.display())]
    UnsupportedEntryType {
        /// Normalized tree-relative entry path.
        path: PathBuf,
    },

    /// An entry changed after enumeration or while its bytes were read.
    #[error("local-tree entry `{}` changed while it was being hashed", path.display())]
    ChangedDuringHash {
        /// Normalized tree-relative entry path.
        path: PathBuf,
    },
}
