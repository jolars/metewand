//! Validation of files and documents completed by trusted workers.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use metewand_core::{
    canonical::{CanonicalJsonError, CanonicalValue},
    public_schemas::{PublicSchema, PublicSchemaCatalogError, public_schema_catalog},
    records::ContentDigest,
    schema::{SchemaCatalog, SchemaValidationError},
};
use metewand_protocol::{ExecuteResponse, MaterializeResponse};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Validates completed worker output against Metewand and repository contracts.
#[derive(Debug)]
pub struct WorkerOutputValidator<'a> {
    public_schemas: SchemaCatalog,
    repository_schemas: &'a SchemaCatalog,
}

impl<'a> WorkerOutputValidator<'a> {
    /// Constructs a validator for the schemas admitted by a checked repository.
    ///
    /// # Errors
    ///
    /// Returns an error only if the embedded public schema set cannot be built.
    pub fn new(repository_schemas: &'a SchemaCatalog) -> Result<Self, PublicSchemaCatalogError> {
        Ok(Self {
            public_schemas: public_schema_catalog()?,
            repository_schemas,
        })
    }

    /// Validates a materializer response and its complete dataset output.
    ///
    /// The returned manifest is valid against both the public artifact-manifest
    /// envelope and the dataset definition's output schema. Every declared file
    /// is regular, complete, and hash-matched, and the output contains no other
    /// entries except directories needed to contain the manifest and files.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe manifest reference, invalid manifest,
    /// incomplete inventory, unsupported filesystem entry, or schema failure.
    pub fn validate_materialized_dataset(
        &self,
        output_root: &Path,
        response: &MaterializeResponse,
        dataset_schema: &Path,
    ) -> Result<ValidatedDataset, WorkerOutputError> {
        let manifest_path = validate_relative_path(response.manifest())?;
        let output = OutputTree::inspect(output_root)?;
        let manifest = output.read_document(&manifest_path)?;
        self.validate_public(PublicSchema::ArtifactManifest, &manifest, &manifest_path)?;
        let raw: RawArtifactManifest = decode_validated_document(&manifest, &manifest_path)?;
        let files = validate_file_declarations(raw.files)?;
        output.validate_inventory(&manifest_path, &files)?;
        output.verify_files(&files)?;
        self.repository_schemas
            .validate(dataset_schema, manifest.as_json())
            .map_err(|source| WorkerOutputError::ScientificSchema {
                document: manifest_path.clone(),
                schema: dataset_schema.to_path_buf(),
                source,
            })?;

        Ok(ValidatedDataset {
            manifest_path,
            manifest,
            files,
        })
    }

    /// Validates an execute response and its canonical result artifact.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe manifest reference, invalid envelope or
    /// scientific data, incomplete inventory, or unsupported filesystem entry.
    pub fn validate_execution_result(
        &self,
        output_root: &Path,
        response: &ExecuteResponse,
        result_schema: &Path,
    ) -> Result<ValidatedResult, WorkerOutputError> {
        let manifest_path = validate_relative_path(response.manifest())?;
        let output = OutputTree::inspect(output_root)?;
        let manifest = output.read_document(&manifest_path)?;
        self.validate_public(PublicSchema::ResultManifest, &manifest, &manifest_path)?;
        let raw: RawResultManifest = decode_validated_document(&manifest, &manifest_path)?;
        validate_schema_reference(&manifest_path, &raw.schema, result_schema)?;
        let files = validate_file_declarations(raw.files)?;
        output.validate_inventory(&manifest_path, &files)?;
        output.verify_files(&files)?;
        self.repository_schemas
            .validate(result_schema, &raw.data)
            .map_err(|source| WorkerOutputError::ScientificSchema {
                document: manifest_path.clone(),
                schema: result_schema.to_path_buf(),
                source,
            })?;
        let data = CanonicalValue::try_from(raw.data).map_err(|source| {
            WorkerOutputError::InvalidCanonicalJson {
                path: manifest_path.clone(),
                source,
            }
        })?;

        Ok(ValidatedResult {
            manifest_path,
            data,
            files,
        })
    }

    /// Validates the completed evaluator metrics document in its private output.
    ///
    /// The assigned metrics document must be the output's only regular file;
    /// ancestor directories are permitted. Its schema reference must equal the
    /// selected problem contract's metric schema before its data are validated.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe assigned path, invalid envelope or
    /// scientific data, undeclared output, or unsupported filesystem entry.
    pub fn validate_metrics(
        &self,
        output_root: &Path,
        metrics_path: &Path,
        metric_schema: &Path,
    ) -> Result<ValidatedMetrics, WorkerOutputError> {
        let metrics_path =
            metrics_path
                .to_str()
                .ok_or_else(|| WorkerOutputError::InvalidRelativePath {
                    path: metrics_path.to_path_buf(),
                })?;
        let metrics_path = validate_relative_path(metrics_path)?;
        let output = OutputTree::inspect(output_root)?;
        output.validate_document_only_inventory(&metrics_path)?;
        let metrics = output.read_document(&metrics_path)?;
        self.validate_public(PublicSchema::Metrics, &metrics, &metrics_path)?;
        let raw: RawMetrics = decode_validated_document(&metrics, &metrics_path)?;
        validate_schema_reference(&metrics_path, &raw.schema, metric_schema)?;
        self.repository_schemas
            .validate(metric_schema, &raw.data)
            .map_err(|source| WorkerOutputError::ScientificSchema {
                document: metrics_path.clone(),
                schema: metric_schema.to_path_buf(),
                source,
            })?;
        let data = CanonicalValue::try_from(raw.data).map_err(|source| {
            WorkerOutputError::InvalidCanonicalJson {
                path: metrics_path.clone(),
                source,
            }
        })?;

        Ok(ValidatedMetrics { metrics_path, data })
    }

    fn validate_public(
        &self,
        schema: PublicSchema,
        document: &CanonicalValue,
        path: &Path,
    ) -> Result<(), WorkerOutputError> {
        self.public_schemas
            .validate(schema.repository_path(), document.as_json())
            .map_err(|source| WorkerOutputError::PublicSchema {
                document: path.to_path_buf(),
                schema,
                source,
            })
    }
}

/// One dataset whose manifest and complete file inventory have been validated.
#[derive(Debug)]
pub struct ValidatedDataset {
    manifest_path: PathBuf,
    manifest: CanonicalValue,
    files: Vec<ValidatedFile>,
}

impl ValidatedDataset {
    /// Returns the normalized manifest path relative to the assigned output.
    #[must_use]
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Returns the validated canonical manifest document.
    #[must_use]
    pub const fn manifest(&self) -> &CanonicalValue {
        &self.manifest
    }

    /// Returns the hash-verified payload files in canonical bytewise order.
    #[must_use]
    pub fn files(&self) -> &[ValidatedFile] {
        &self.files
    }
}

/// One canonical result and its complete file inventory after validation.
#[derive(Debug)]
pub struct ValidatedResult {
    manifest_path: PathBuf,
    data: CanonicalValue,
    files: Vec<ValidatedFile>,
}

impl ValidatedResult {
    /// Returns the normalized result-manifest path relative to assigned output.
    #[must_use]
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Returns the canonical result data validated by the problem contract.
    #[must_use]
    pub const fn data(&self) -> &CanonicalValue {
        &self.data
    }

    /// Returns the hash-verified payload files in canonical bytewise order.
    #[must_use]
    pub fn files(&self) -> &[ValidatedFile] {
        &self.files
    }
}

/// One evaluator metrics document after envelope and problem-schema validation.
#[derive(Debug)]
pub struct ValidatedMetrics {
    metrics_path: PathBuf,
    data: CanonicalValue,
}

impl ValidatedMetrics {
    /// Returns the normalized metrics path relative to its assigned output.
    #[must_use]
    pub fn metrics_path(&self) -> &Path {
        &self.metrics_path
    }

    /// Returns the evaluator-owned data validated by the problem contract.
    #[must_use]
    pub const fn data(&self) -> &CanonicalValue {
        &self.data
    }
}

/// One regular output file whose size and SHA-256 digest match its declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedFile {
    path: PathBuf,
    sha256: ContentDigest,
    size_bytes: u64,
    media_type: String,
    representation: Option<Value>,
}

impl ValidatedFile {
    /// Returns the normalized path relative to the assigned output.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the verified SHA-256 content digest.
    #[must_use]
    pub const fn sha256(&self) -> ContentDigest {
        self.sha256
    }

    /// Returns the verified byte length.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Returns the declared media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Returns optional logical representation metadata.
    #[must_use]
    pub const fn representation(&self) -> Option<&Value> {
        self.representation.as_ref()
    }
}

#[derive(Debug, Deserialize)]
struct RawArtifactManifest {
    files: Vec<RawFile>,
}

#[derive(Debug, Deserialize)]
struct RawResultManifest {
    schema: String,
    data: Value,
    files: Vec<RawFile>,
}

#[derive(Debug, Deserialize)]
struct RawMetrics {
    schema: String,
    data: Value,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    path: String,
    sha256: String,
    size_bytes: u64,
    media_type: String,
    representation: Option<Value>,
}

fn decode_validated_document<T>(
    document: &CanonicalValue,
    path: &Path,
) -> Result<T, WorkerOutputError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(document.as_json().clone()).map_err(|source| {
        WorkerOutputError::TypedDocument {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn validate_schema_reference(
    document: &Path,
    actual: &str,
    expected: &Path,
) -> Result<(), WorkerOutputError> {
    if Path::new(actual) == expected {
        Ok(())
    } else {
        Err(WorkerOutputError::SchemaReferenceMismatch {
            document: document.to_path_buf(),
            expected: expected.to_path_buf(),
            actual: PathBuf::from(actual),
        })
    }
}

fn validate_file_declarations(
    raw_files: Vec<RawFile>,
) -> Result<Vec<ValidatedFile>, WorkerOutputError> {
    let mut files = Vec::with_capacity(raw_files.len());
    let mut paths = BTreeSet::new();

    for raw in raw_files {
        let path = validate_relative_path(&raw.path)?;
        if !paths.insert(path.clone()) {
            return Err(WorkerOutputError::DuplicateDeclaredPath { path });
        }
        files.push(ValidatedFile {
            path,
            sha256: decode_digest(&raw.sha256),
            size_bytes: raw.size_bytes,
            media_type: raw.media_type,
            representation: raw.representation,
        });
    }
    files.sort_unstable_by(|left, right| utf8_bytes(&left.path).cmp(utf8_bytes(&right.path)));

    Ok(files)
}

fn validate_relative_path(value: &str) -> Result<PathBuf, WorkerOutputError> {
    let invalid = value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value.contains('\\')
        || is_windows_drive_path(value)
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
    if invalid {
        Err(WorkerOutputError::InvalidRelativePath {
            path: PathBuf::from(value),
        })
    } else {
        Ok(PathBuf::from(value))
    }
}

fn is_windows_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn decode_digest(value: &str) -> ContentDigest {
    let mut bytes = [0_u8; 32];
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    for (output, pair) in bytes.iter_mut().zip(pairs) {
        *output = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    ContentDigest::new(bytes)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("the public schema accepts only lowercase SHA-256 hex"),
    }
}

fn encode_digest(digest: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn utf8_bytes(path: &Path) -> &[u8] {
    path.to_str()
        .expect("validated output paths are UTF-8")
        .as_bytes()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Directory,
    File,
}

#[derive(Debug)]
struct OutputEntry {
    path: PathBuf,
    kind: EntryKind,
}

#[derive(Debug)]
struct OutputTree {
    root: PathBuf,
    entries: Vec<OutputEntry>,
}

impl OutputTree {
    fn inspect(output_root: &Path) -> Result<Self, WorkerOutputError> {
        let root = fs::canonicalize(output_root).map_err(|source| {
            WorkerOutputError::OutputRootAccess {
                path: output_root.to_path_buf(),
                source,
            }
        })?;
        if !root.is_dir() {
            return Err(WorkerOutputError::OutputRootNotDirectory { path: root });
        }
        let mut entries = Vec::new();
        collect_entries(&root, Path::new(""), &mut entries)?;
        entries.sort_unstable_by(|left, right| utf8_bytes(&left.path).cmp(utf8_bytes(&right.path)));
        Ok(Self { root, entries })
    }

    fn read_document(&self, path: &Path) -> Result<CanonicalValue, WorkerOutputError> {
        match self.entry_kind(path) {
            Some(EntryKind::File) => {}
            Some(EntryKind::Directory) => {
                return Err(WorkerOutputError::DocumentNotRegularFile {
                    path: path.to_path_buf(),
                });
            }
            None => {
                return Err(WorkerOutputError::MissingDocument {
                    path: path.to_path_buf(),
                });
            }
        }

        let bytes = read_stable_file(&self.root, path)?;
        CanonicalValue::from_slice(&bytes).map_err(|source| {
            WorkerOutputError::InvalidCanonicalJson {
                path: path.to_path_buf(),
                source,
            }
        })
    }

    fn entry_kind(&self, path: &Path) -> Option<EntryKind> {
        self.entries
            .binary_search_by(|entry| utf8_bytes(&entry.path).cmp(utf8_bytes(path)))
            .ok()
            .map(|index| self.entries[index].kind)
    }

    fn validate_inventory(
        &self,
        manifest_path: &Path,
        files: &[ValidatedFile],
    ) -> Result<(), WorkerOutputError> {
        if files.iter().any(|file| file.path == manifest_path) {
            return Err(WorkerOutputError::ManifestDeclaredAsPayload {
                path: manifest_path.to_path_buf(),
            });
        }
        let expected = std::iter::once(manifest_path)
            .chain(files.iter().map(|file| file.path.as_path()))
            .collect::<BTreeSet<_>>();
        self.validate_expected_entries(&expected)?;
        for file in files {
            match self.entry_kind(&file.path) {
                Some(EntryKind::File) => {}
                Some(EntryKind::Directory) => {
                    return Err(WorkerOutputError::DeclaredFileNotRegular {
                        path: file.path.clone(),
                    });
                }
                None => {
                    return Err(WorkerOutputError::MissingDeclaredFile {
                        path: file.path.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_document_only_inventory(
        &self,
        document_path: &Path,
    ) -> Result<(), WorkerOutputError> {
        let expected = BTreeSet::from([document_path]);
        self.validate_expected_entries(&expected)?;
        match self.entry_kind(document_path) {
            Some(EntryKind::File) => Ok(()),
            Some(EntryKind::Directory) => Err(WorkerOutputError::DocumentNotRegularFile {
                path: document_path.to_path_buf(),
            }),
            None => Err(WorkerOutputError::MissingDocument {
                path: document_path.to_path_buf(),
            }),
        }
    }

    fn validate_expected_entries(
        &self,
        expected_files: &BTreeSet<&Path>,
    ) -> Result<(), WorkerOutputError> {
        for entry in &self.entries {
            let declared = match entry.kind {
                EntryKind::File => expected_files.contains(entry.path.as_path()),
                EntryKind::Directory => expected_files
                    .iter()
                    .any(|expected| *expected != entry.path && expected.starts_with(&entry.path)),
            };
            if !declared {
                return Err(WorkerOutputError::UndeclaredEntry {
                    path: entry.path.clone(),
                });
            }
        }
        Ok(())
    }

    fn verify_files(&self, files: &[ValidatedFile]) -> Result<(), WorkerOutputError> {
        for file in files {
            let absolute = self.root.join(&file.path);
            let metadata = fs::symlink_metadata(&absolute).map_err(|source| {
                WorkerOutputError::EntryAccess {
                    path: file.path.clone(),
                    source,
                }
            })?;
            if !metadata.is_file() {
                return Err(WorkerOutputError::DeclaredFileNotRegular {
                    path: file.path.clone(),
                });
            }
            if metadata.len() != file.size_bytes {
                return Err(WorkerOutputError::FileSizeMismatch {
                    path: file.path.clone(),
                    expected: file.size_bytes,
                    actual: metadata.len(),
                });
            }
            let actual = hash_stable_file(&self.root, &file.path, file.size_bytes)?;
            if &actual != file.sha256.bytes() {
                return Err(WorkerOutputError::FileHashMismatch {
                    path: file.path.clone(),
                    expected: encode_digest(file.sha256.bytes()),
                    actual: encode_digest(&actual),
                });
            }
        }
        Ok(())
    }
}

fn hash_stable_file(
    root: &Path,
    relative: &Path,
    expected_size: u64,
) -> Result<[u8; 32], WorkerOutputError> {
    let absolute = root.join(relative);
    let before =
        fs::symlink_metadata(&absolute).map_err(|source| WorkerOutputError::EntryAccess {
            path: relative.to_path_buf(),
            source,
        })?;
    if !before.is_file() {
        return Err(WorkerOutputError::DeclaredFileNotRegular {
            path: relative.to_path_buf(),
        });
    }
    if before.len() != expected_size {
        return Err(WorkerOutputError::FileSizeMismatch {
            path: relative.to_path_buf(),
            expected: expected_size,
            actual: before.len(),
        });
    }

    let mut file = File::open(&absolute).map_err(|source| WorkerOutputError::EntryAccess {
        path: relative.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut remaining = expected_size;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let capacity = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("the read size is bounded by the in-memory buffer");
        let read = file.read(&mut buffer[..capacity]).map_err(|source| {
            WorkerOutputError::EntryAccess {
                path: relative.to_path_buf(),
                source,
            }
        })?;
        if read == 0 {
            return Err(WorkerOutputError::FileChangedDuringRead {
                path: relative.to_path_buf(),
            });
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|source| WorkerOutputError::EntryAccess {
            path: relative.to_path_buf(),
            source,
        })?
        != 0
    {
        return Err(WorkerOutputError::FileChangedDuringRead {
            path: relative.to_path_buf(),
        });
    }
    let after = file
        .metadata()
        .map_err(|source| WorkerOutputError::EntryAccess {
            path: relative.to_path_buf(),
            source,
        })?;
    if !after.is_file() || after.len() != expected_size {
        return Err(WorkerOutputError::FileChangedDuringRead {
            path: relative.to_path_buf(),
        });
    }

    Ok(hasher.finalize().into())
}

fn collect_entries(
    absolute_directory: &Path,
    relative_directory: &Path,
    entries: &mut Vec<OutputEntry>,
) -> Result<(), WorkerOutputError> {
    let display_path = if relative_directory.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative_directory.to_path_buf()
    };
    let mut children = fs::read_dir(absolute_directory)
        .map_err(|source| WorkerOutputError::EntryAccess {
            path: display_path.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkerOutputError::EntryAccess {
            path: display_path,
            source,
        })?;
    children.sort_unstable_by_key(fs::DirEntry::file_name);

    for child in children {
        let name = child.file_name().into_string().map_err(|name| {
            WorkerOutputError::NonUtf8OutputPath {
                path: relative_directory.join(name),
            }
        })?;
        if name.contains('\\') {
            return Err(WorkerOutputError::InvalidRelativePath {
                path: relative_directory.join(name),
            });
        }
        let path = relative_directory.join(name);
        let file_type = child
            .file_type()
            .map_err(|source| WorkerOutputError::EntryAccess {
                path: path.clone(),
                source,
            })?;
        let kind = if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            return Err(WorkerOutputError::UnsupportedEntryType { path });
        };
        entries.push(OutputEntry {
            path: path.clone(),
            kind,
        });
        if kind == EntryKind::Directory {
            collect_entries(&child.path(), &path, entries)?;
        }
    }
    Ok(())
}

fn read_stable_file(root: &Path, relative: &Path) -> Result<Vec<u8>, WorkerOutputError> {
    let absolute = root.join(relative);
    let before =
        fs::symlink_metadata(&absolute).map_err(|source| WorkerOutputError::EntryAccess {
            path: relative.to_path_buf(),
            source,
        })?;
    if !before.is_file() {
        return Err(WorkerOutputError::DeclaredFileNotRegular {
            path: relative.to_path_buf(),
        });
    }
    let expected_size =
        usize::try_from(before.len()).map_err(|_| WorkerOutputError::FileTooLarge {
            path: relative.to_path_buf(),
            size: before.len(),
        })?;
    let mut file = File::open(&absolute).map_err(|source| WorkerOutputError::EntryAccess {
        path: relative.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(expected_size);
    file.read_to_end(&mut bytes)
        .map_err(|source| WorkerOutputError::EntryAccess {
            path: relative.to_path_buf(),
            source,
        })?;
    let after = file
        .metadata()
        .map_err(|source| WorkerOutputError::EntryAccess {
            path: relative.to_path_buf(),
            source,
        })?;
    if !after.is_file() || before.len() != after.len() || bytes.len() as u64 != before.len() {
        return Err(WorkerOutputError::FileChangedDuringRead {
            path: relative.to_path_buf(),
        });
    }
    Ok(bytes)
}

/// A worker output that cannot be accepted as a complete canonical artifact.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WorkerOutputError {
    /// A worker or caller supplied a non-normalized or non-relative output path.
    #[error("output path `{path}` must be normalized, relative UTF-8 without backslashes")]
    InvalidRelativePath {
        /// Rejected path.
        path: PathBuf,
    },

    /// The assigned output root could not be resolved.
    #[error("failed to access assigned output root `{}`: {source}", path.display())]
    OutputRootAccess {
        /// Assigned root.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// The assigned output root did not resolve to a directory.
    #[error("assigned output root `{}` is not a directory", path.display())]
    OutputRootNotDirectory {
        /// Rejected root.
        path: PathBuf,
    },

    /// An output path could not be listed, inspected, opened, or read.
    #[error("failed to access worker output `{}`: {source}", path.display())]
    EntryAccess {
        /// Output-relative path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A filesystem path in the output is not valid UTF-8.
    #[error("worker output path `{}` is not UTF-8", path.display())]
    NonUtf8OutputPath {
        /// Rejected path.
        path: PathBuf,
    },

    /// The output contains a symlink, socket, FIFO, device, or other special entry.
    #[error("worker output `{}` is not a regular file or directory", path.display())]
    UnsupportedEntryType {
        /// Rejected output-relative path.
        path: PathBuf,
    },

    /// The named manifest or metrics document does not exist.
    #[error("worker output document `{}` does not exist", path.display())]
    MissingDocument {
        /// Missing output-relative path.
        path: PathBuf,
    },

    /// The named manifest or metrics document is a directory.
    #[error("worker output document `{}` is not a regular file", path.display())]
    DocumentNotRegularFile {
        /// Rejected output-relative path.
        path: PathBuf,
    },

    /// A document is not in Metewand's restricted canonical JSON domain.
    #[error("worker output document `{}` is not valid canonical-domain JSON: {source}", path.display())]
    InvalidCanonicalJson {
        /// Invalid output-relative document path.
        path: PathBuf,
        /// Canonical JSON failure.
        #[source]
        source: CanonicalJsonError,
    },

    /// A document does not satisfy its Metewand-owned public envelope.
    #[error("worker output document `{}` does not satisfy public schema `{}`: {source}", document.display(), schema.slug())]
    PublicSchema {
        /// Invalid output-relative document path.
        document: PathBuf,
        /// Public schema that rejected the document.
        schema: PublicSchema,
        /// Validation failures.
        #[source]
        source: SchemaValidationError,
    },

    /// A schema-valid public document unexpectedly failed typed decoding.
    #[error("worker output document `{}` could not be decoded: {source}", path.display())]
    TypedDocument {
        /// Output-relative document path.
        path: PathBuf,
        /// Typed decoding failure.
        #[source]
        source: serde_json::Error,
    },

    /// A result or metrics envelope names a schema other than the selected one.
    #[error("worker output document `{}` names schema `{}` instead of `{}`", document.display(), actual.display(), expected.display())]
    SchemaReferenceMismatch {
        /// Output-relative document path.
        document: PathBuf,
        /// Schema selected by the checked problem contract.
        expected: PathBuf,
        /// Schema named by the worker document.
        actual: PathBuf,
    },

    /// Dataset, result, or metric data violate their repository-owned schema.
    #[error("worker output document `{}` does not satisfy scientific schema `{}`: {source}", document.display(), schema.display())]
    ScientificSchema {
        /// Output-relative document path.
        document: PathBuf,
        /// Repository schema that rejected the value.
        schema: PathBuf,
        /// Validation failures.
        #[source]
        source: SchemaValidationError,
    },

    /// Two file declarations name the same relative path.
    #[error("worker manifest declares `{}` more than once", path.display())]
    DuplicateDeclaredPath {
        /// Duplicated path.
        path: PathBuf,
    },

    /// A manifest attempts to include itself in its payload inventory.
    #[error("worker manifest `{}` cannot declare itself as a payload file", path.display())]
    ManifestDeclaredAsPayload {
        /// Manifest path.
        path: PathBuf,
    },

    /// The output contains a file or directory absent from its inventory.
    #[error("worker output contains undeclared entry `{}`", path.display())]
    UndeclaredEntry {
        /// Undeclared output-relative path.
        path: PathBuf,
    },

    /// A declared payload does not exist.
    #[error("worker manifest declares missing file `{}`", path.display())]
    MissingDeclaredFile {
        /// Missing output-relative path.
        path: PathBuf,
    },

    /// A declared payload resolves to a directory rather than a regular file.
    #[error("worker manifest path `{}` is not a regular file", path.display())]
    DeclaredFileNotRegular {
        /// Rejected output-relative path.
        path: PathBuf,
    },

    /// A file cannot fit in the current process's address space for validation.
    #[error("worker output file `{}` is too large to validate ({size} bytes)", path.display())]
    FileTooLarge {
        /// Output-relative path.
        path: PathBuf,
        /// Observed size.
        size: u64,
    },

    /// A file changed length or type while it was being read.
    #[error("worker output file `{}` changed while being validated", path.display())]
    FileChangedDuringRead {
        /// Changed output-relative path.
        path: PathBuf,
    },

    /// A file's observed byte length differs from its manifest declaration.
    #[error("worker output file `{}` has size {actual}, expected {expected}", path.display())]
    FileSizeMismatch {
        /// Output-relative path.
        path: PathBuf,
        /// Manifest size.
        expected: u64,
        /// Observed size.
        actual: u64,
    },

    /// A file's observed SHA-256 digest differs from its declaration.
    #[error("worker output file `{}` has SHA-256 `{actual}`, expected `{expected}`", path.display())]
    FileHashMismatch {
        /// Output-relative path.
        path: PathBuf,
        /// Manifest digest.
        expected: String,
        /// Observed digest.
        actual: String,
    },
}
