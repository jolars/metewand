//! Explicit acquisition and verification of privately staged source artifacts.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use metewand_core::{
    canonical::CanonicalValue,
    manifest::{Name, RemoteSource, RepositoryPath, Sha256Digest},
    records::{ContentDigest, RecordId, SourceBundleResource},
};
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};
use thiserror::Error;

use crate::{
    SOURCE_RESOLUTION_VERSION,
    local_tree::{LocalTreeHashError, hash_local_tree},
    source_bundle::{
        SourceBundleError, SourceBundleIdentityError, expand_source_bundle, identify_source_bundle,
        literal_prefix,
    },
};

/// An explicit source request supplied during runtime resolution.
///
/// Repository pins come from the checked repository. URL pins come from the
/// manifest. External selectors must fully specify the provider's immutable
/// selection; validating their meaning belongs to the selected acquirer.
#[derive(Debug)]
pub enum SourceRequest<'a> {
    /// Repository files selected by existing source-bundle rules.
    Repository {
        /// Repository root used only for filesystem access.
        root: &'a Path,
        /// Declared repository-relative paths and patterns.
        sources: &'a [RepositoryPath],
        /// Expected identity established by repository checking.
        expected: RecordId<SourceBundleResource>,
    },
    /// Exact downloaded bytes, preserved without extraction as `source`.
    PinnedUrl(&'a RemoteSource),
    /// An explicitly selected external provider and its immutable selector.
    External {
        /// Provider selected by the caller, without automatic discovery.
        provider: &'a Name,
        /// Provider-specific, canonical selection, including required pins.
        selector: &'a CanonicalValue,
    },
}

/// A regular file's normalized path, byte size, and raw SHA-256 digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFile {
    /// Path relative to the private source directory.
    pub path: RepositoryPath,
    /// Number of stored bytes, before decoding or extraction.
    pub size_bytes: u64,
    /// SHA-256 of those stored bytes.
    pub sha256: ContentDigest,
}

/// An acquirer's inventory and provenance, pending independent byte checks.
#[derive(Debug)]
pub struct AcquiredSource {
    /// Complete expected inventory obtained from the pinned source metadata.
    pub files: Vec<SourceFile>,
    /// Provider receipts and claims, separate from Metewand's byte verification.
    pub provenance: CanonicalValue,
}

/// Acquires raw source bytes into an assigned, initially empty directory.
///
/// Implementations may download files or invoke a provider through a process
/// boundary. They must honor the supplied request, verify provider selectors,
/// export independent files without accessing another client's cache internals,
/// and return the pinned inventory. The resolver checks that inventory against
/// the actual output. Acquirers never receive repository requests.
///
/// Network policy and operational deadlines belong to the caller and selected
/// backend. Implementations must report failures instead of falling back to an
/// unpinned selection. No acquirer is installed or invoked implicitly.
pub trait SourceAcquirer {
    /// Writes the selected source and returns its inventory and receipt.
    ///
    /// # Errors
    ///
    /// Returns an acquisition, selection, or filesystem failure.
    fn acquire(
        &self,
        request: &SourceRequest<'_>,
        output_dir: &Path,
    ) -> Result<AcquiredSource, SourceResolutionError>;
}

/// Source selection retained separately from the materialized content address.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceOrigin {
    /// The repository-relative bundle identity, without a host root path.
    Repository {
        /// Expected and verified bundle identity.
        source_bundle: RecordId<SourceBundleResource>,
    },
    /// Requested retrieval location and expected raw content digest.
    PinnedUrl {
        /// Retrieval location retained as provenance.
        url: String,
        /// Manifest's expected SHA-256 digest.
        sha256: Sha256Digest,
    },
    /// External provider and exact selector passed to acquisition.
    External {
        /// Explicit provider name.
        provider: Name,
        /// Canonical provider-specific immutable selector.
        selector: CanonicalValue,
    },
}

/// A versioned, in-memory source-resolution receipt.
///
/// Acquisition claims do not establish decoded-value verification. This type
/// has no serialized wire representation and does not change public schemas.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceProvenance {
    /// Source-resolution compatibility version.
    pub version: u32,
    /// Requested selection verified by this operation and its trusted backend.
    pub origin: SourceOrigin,
    /// Optional backend receipt, retained without upgrading its claims.
    pub acquisition: Option<CanonicalValue>,
}

/// Privately owned source artifacts that passed resolution-time verification.
///
/// The directory is temporary and is removed on drop. Durable caching,
/// publication, dataset materialization, and per-attempt views are separate
/// runtime operations. Callers must reverify contents before subsequent use.
#[derive(Debug)]
pub struct ResolvedSource {
    directory: TempDir,
    digest: ContentDigest,
    files: Vec<SourceFile>,
    provenance: SourceProvenance,
}

impl ResolvedSource {
    /// Returns the privately owned source directory.
    #[must_use]
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    /// Returns the existing versioned local-tree digest of the staged content.
    #[must_use]
    pub fn tree_digest(&self) -> ContentDigest {
        self.digest
    }

    /// Returns the complete regular-file inventory in UTF-8 path order.
    #[must_use]
    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    /// Returns the versioned selection and acquisition provenance.
    #[must_use]
    pub fn provenance(&self) -> &SourceProvenance {
        &self.provenance
    }

    /// Rechecks the staged content against its resolution-time digest.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory changed or cannot be inspected.
    pub fn verify(&self) -> Result<(), SourceResolutionError> {
        if !fs::symlink_metadata(self.directory())?.is_dir() {
            return Err(SourceResolutionError::UnsupportedEntry {
                path: self.directory().to_path_buf(),
            });
        }
        if hash_local_tree(self.directory())? != self.digest {
            return Err(SourceResolutionError::ChangedSource);
        }
        Ok(())
    }

    /// Transfers ownership of the verified staging directory to the caller.
    #[must_use]
    pub fn into_directory(self) -> TempDir {
        self.directory
    }
}

/// Resolves an explicit request to verified files and provenance in private storage.
///
/// `private_parent` must already exist. Repository staging must be outside
/// selected source directories and glob prefixes to avoid copying staging
/// output into itself or changing the selection. Acquired URL bytes
/// retain their original encoding and compression as the single file `source`.
/// External outputs must exactly match the backend's pinned file inventory.
/// Only regular files and directories are admitted; links and special files
/// are rejected. Any error removes partial staging output.
///
/// # Errors
///
/// Returns an error for unavailable acquisition, invalid inventories, changed
/// repository inputs, mismatched pins, unsafe entries, or filesystem failures.
pub fn resolve_source(
    request: &SourceRequest<'_>,
    acquirer: Option<&dyn SourceAcquirer>,
    private_parent: &Path,
) -> Result<ResolvedSource, SourceResolutionError> {
    let (directory, origin, acquisition, expected_files) = match request {
        SourceRequest::Repository {
            root,
            sources,
            expected,
        } => {
            let root = fs::canonicalize(root)?;
            let parent = fs::canonicalize(private_parent)?;
            for source in *sources {
                let (prefix, has_meta) = literal_prefix(
                    source
                        .as_path()
                        .to_str()
                        .expect("validated UTF-8 source pattern"),
                );
                let prefix = root.join(prefix);
                if (has_meta || prefix.is_dir()) && parent.starts_with(prefix) {
                    return Err(SourceResolutionError::StagingOverlapsSource);
                }
            }
            if identify_source_bundle(&root, sources)? != *expected {
                return Err(SourceResolutionError::ChangedSource);
            }
            let expanded = expand_source_bundle(&root, sources)?;
            let directory = staging(private_parent)?;
            for path in expanded.paths() {
                copy_entry(&root.join(path), &directory.path().join(path))?;
            }
            if identify_source_bundle(&root, sources)? != *expected
                || identify_source_bundle(directory.path(), sources)? != *expected
            {
                return Err(SourceResolutionError::ChangedSource);
            }
            (
                directory,
                SourceOrigin::Repository {
                    source_bundle: *expected,
                },
                None,
                None,
            )
        }
        SourceRequest::PinnedUrl(_) | SourceRequest::External { .. } => {
            let acquirer = acquirer.ok_or(SourceResolutionError::AcquirerUnavailable)?;
            let directory = staging(private_parent)?;
            let acquired = acquirer.acquire(request, directory.path())?;
            let origin = match request {
                SourceRequest::PinnedUrl(source) => SourceOrigin::PinnedUrl {
                    url: source.url.clone(),
                    sha256: source.sha256.clone(),
                },
                SourceRequest::External { provider, selector } => SourceOrigin::External {
                    provider: (*provider).clone(),
                    selector: (*selector).clone(),
                },
                SourceRequest::Repository { .. } => {
                    unreachable!("repository requests are copied locally")
                }
            };
            (
                directory,
                origin,
                Some(acquired.provenance),
                Some(acquired.files),
            )
        }
    };
    let (files, directories) = inventory(directory.path())?;
    if let Some(expected_files) = expected_files {
        verify_inventory(&files, &expected_files)?;
        for directory in &directories {
            if !files
                .iter()
                .any(|file| file.path.as_path().starts_with(directory))
            {
                return Err(SourceResolutionError::InvalidInventory);
            }
        }
    }
    if let SourceRequest::PinnedUrl(source) = request {
        if files.len() != 1 || files[0].path.as_path() != Path::new("source") {
            return Err(SourceResolutionError::InvalidInventory);
        }
        if format!(
            "{:x}",
            sha2::digest::Output::<Sha256>::from(*files[0].sha256.bytes())
        ) != source.sha256.as_str()
        {
            return Err(SourceResolutionError::DigestMismatch {
                path: PathBuf::from("source"),
            });
        }
    }
    let digest = hash_local_tree(directory.path())?;
    let (checked_files, checked_directories) = inventory(directory.path())?;
    if checked_files != files || checked_directories != directories {
        return Err(SourceResolutionError::ChangedSource);
    }
    let resolved = ResolvedSource {
        directory,
        digest,
        files,
        provenance: SourceProvenance {
            version: SOURCE_RESOLUTION_VERSION,
            origin,
            acquisition,
        },
    };
    resolved.verify()?;
    Ok(resolved)
}

fn staging(parent: &Path) -> Result<TempDir, io::Error> {
    Builder::new().prefix("metewand-source-").tempdir_in(parent)
}

fn copy_entry(source: &Path, destination: &Path) -> Result<(), SourceResolutionError> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        fs::create_dir_all(destination.parent().expect("a copied file has a parent"))?;
        fs::copy(source, destination)?;
    } else {
        return Err(SourceResolutionError::UnsupportedEntry {
            path: source.to_path_buf(),
        });
    }
    Ok(())
}

fn inventory(root: &Path) -> Result<(Vec<SourceFile>, Vec<PathBuf>), SourceResolutionError> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut Vec<SourceFile>,
        directories: &mut Vec<PathBuf>,
    ) -> Result<(), SourceResolutionError> {
        if !fs::symlink_metadata(directory)?.is_dir() {
            return Err(SourceResolutionError::UnsupportedEntry {
                path: directory.to_path_buf(),
            });
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                directories.push(
                    path.strip_prefix(root)
                        .expect("inventory remains beneath root")
                        .to_path_buf(),
                );
                visit(root, &path, files, directories)?;
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("inventory remains beneath root");
                let text =
                    relative
                        .to_str()
                        .ok_or_else(|| SourceResolutionError::UnsupportedEntry {
                            path: relative.to_path_buf(),
                        })?;
                let logical_path = serde_json::from_value(serde_json::Value::String(
                    text.replace(std::path::MAIN_SEPARATOR, "/"),
                ))
                .map_err(|_| SourceResolutionError::UnsupportedEntry {
                    path: relative.to_path_buf(),
                })?;
                let mut file = File::open(&path)?;
                let mut hash = Sha256::new();
                let mut size_bytes = 0;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    hash.update(&buffer[..read]);
                    size_bytes += read as u64;
                }
                files.push(SourceFile {
                    path: logical_path,
                    size_bytes,
                    sha256: ContentDigest::new(hash.finalize().into()),
                });
            } else {
                return Err(SourceResolutionError::UnsupportedEntry { path });
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    let mut directories = Vec::new();
    visit(root, root, &mut files, &mut directories)?;
    files.sort_by(|a, b| {
        a.path
            .as_path()
            .to_str()
            .expect("validated UTF-8 path")
            .as_bytes()
            .cmp(
                b.path
                    .as_path()
                    .to_str()
                    .expect("validated UTF-8 path")
                    .as_bytes(),
            )
    });
    directories.sort();
    Ok((files, directories))
}

fn verify_inventory(
    actual: &[SourceFile],
    expected: &[SourceFile],
) -> Result<(), SourceResolutionError> {
    let mut pins = BTreeMap::new();
    for file in expected {
        if pins.insert(file.path.as_path(), file).is_some() {
            return Err(SourceResolutionError::InvalidInventory);
        }
    }
    if pins.len() != actual.len() {
        return Err(SourceResolutionError::InvalidInventory);
    }
    for file in actual {
        let pin = pins
            .get(file.path.as_path())
            .ok_or(SourceResolutionError::InvalidInventory)?;
        if pin.size_bytes != file.size_bytes {
            return Err(SourceResolutionError::SizeMismatch {
                path: file.path.as_path().to_path_buf(),
            });
        }
        if pin.sha256 != file.sha256 {
            return Err(SourceResolutionError::DigestMismatch {
                path: file.path.as_path().to_path_buf(),
            });
        }
    }
    Ok(())
}

/// A typed failure to acquire or verify a source artifact.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SourceResolutionError {
    /// No explicit backend was supplied for an acquisition request.
    #[error("source acquisition requires an explicitly supplied acquirer")]
    AcquirerUnavailable,
    /// The selected backend could not acquire the pinned source.
    #[error("source acquisition failed: {0}")]
    Acquisition(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The inventory has duplicate, missing, extra, or misnamed files.
    #[error("source files do not match the complete expected inventory")]
    InvalidInventory,
    /// A file's stored byte count does not match its pin.
    #[error("source file `{}` does not match its expected size", path.display())]
    SizeMismatch {
        /// Source-relative file path.
        path: PathBuf,
    },
    /// A file's raw SHA-256 does not match its pin.
    #[error("source file `{}` does not match its expected SHA-256", path.display())]
    DigestMismatch {
        /// Source-relative file path.
        path: PathBuf,
    },
    /// A source entry cannot be safely copied or represented.
    #[error("source entry `{}` must be a regular file or directory with a normalized UTF-8 path", path.display())]
    UnsupportedEntry {
        /// Entry that failed validation.
        path: PathBuf,
    },
    /// A repository or staged source changed during or after resolution.
    #[error("source contents changed after validation or during resolution")]
    ChangedSource,
    /// Staging could become an input to its own copy operation.
    #[error("private source staging must be outside selected source directories and glob prefixes")]
    StagingOverlapsSource,
    /// Source staging or acquisition encountered a filesystem failure.
    #[error("source resolution failed: {0}")]
    Io(#[from] io::Error),
    /// Repository declarations could not be expanded safely.
    #[error(transparent)]
    Bundle(#[from] SourceBundleError),
    /// Repository contents could not be identified.
    #[error(transparent)]
    Identity(#[from] SourceBundleIdentityError),
    /// The private source tree could not be hashed.
    #[error(transparent)]
    Tree(#[from] LocalTreeHashError),
}
