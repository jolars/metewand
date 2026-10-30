//! Bounded pinned HTTP downloads and private extraction of verified archives.

use crate::runtime::source_resolution::{
    AcquiredSource, ResolvedSource, SourceAcquirer, SourceFile, SourceRequest,
    SourceResolutionError,
};
use crate::core::{canonical::CanonicalValue, materialization::hex, records::ContentDigest};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path},
    time::Duration,
};
use tempfile::{Builder, TempDir};

/// Explicit HTTP transport policy. Redirects and transparent decompression are disabled.
pub struct HttpAcquirer {
    client: reqwest::blocking::Client,
    max_bytes: u64,
}

fn failure(message: impl Into<String>) -> SourceResolutionError {
    SourceResolutionError::Acquisition(Box::new(io::Error::other(message.into())))
}

impl HttpAcquirer {
    /// Builds a transport with a total request deadline and raw download size limit.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be initialized.
    pub fn new(timeout: Duration, max_bytes: u64) -> Result<Self, SourceResolutionError> {
        let client = reqwest::blocking::Client::builder()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| failure(e.to_string()))?;
        Ok(Self { client, max_bytes })
    }
}

impl SourceAcquirer for HttpAcquirer {
    fn acquire(
        &self,
        request: &SourceRequest<'_>,
        output_dir: &Path,
    ) -> Result<AcquiredSource, SourceResolutionError> {
        let SourceRequest::PinnedUrl(source) = request else {
            return Err(failure("HTTP acquisition requires a pinned URL"));
        };
        let url = reqwest::Url::parse(&source.url).map_err(|e| failure(e.to_string()))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(failure(
                "only HTTP(S) URLs without credentials are supported",
            ));
        }
        let mut response = self
            .client
            .get(url)
            .send()
            .map_err(|e| failure(e.to_string()))?;
        if !response.status().is_success() {
            return Err(failure(format!(
                "download returned HTTP {}",
                response.status()
            )));
        }
        let mut file = tempfile::tempfile_in(output_dir)?;
        let mut hash = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = [0; 65536];
        loop {
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or_else(|| failure("download size overflow"))?;
            if size > self.max_bytes {
                return Err(failure("download exceeds byte limit"));
            }
            hash.update(&buffer[..count]);
            io::Write::write_all(&mut file, &buffer[..count])?;
        }
        let digest = ContentDigest::new(hash.finalize().into());
        if hex(digest) != source.sha256.as_str() {
            return Err(SourceResolutionError::DigestMismatch {
                path: "source".into(),
            });
        }
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        let mut output = File::create_new(output_dir.join("source"))?;
        io::copy(&mut file, &mut output)?;
        Ok(AcquiredSource {
            files: vec![SourceFile {
                path: serde_json::from_value(serde_json::json!("source")).expect("constant path"),
                size_bytes: size,
                sha256: digest,
            }],
            provenance: CanonicalValue::from_slice(br#"{"transport":"http","redirects":false}"#)
                .expect("canonical receipt"),
        })
    }
}

/// Supported archive encodings; format selection is explicit, never inferred from a URL.
#[derive(Clone, Copy, Debug)]
pub enum ArchiveFormat {
    /// Uncompressed tar.
    Tar,
    /// Gzip-compressed tar.
    TarGzip,
}

/// Mandatory extraction budgets, including directories in the entry count.
#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    /// Maximum number of explicit entries, including directories.
    pub max_entries: u64,
    /// Maximum sum of declared entry payload sizes.
    pub max_expanded_bytes: u64,
}

/// Extracts a pinned URL archive only after rechecking its verified source bytes.
///
/// Links, special entries, duplicate paths, absolute paths, and traversal are
/// rejected. An error drops the entire private output. Permissions and ownership
/// from the archive are never applied. The staging parent must be private.
///
/// # Errors
///
/// Returns an error for changed source bytes, malformed or unsupported entries,
/// exceeded budgets, or filesystem failures.
pub fn extract_archive(
    source: &ResolvedSource,
    format: ArchiveFormat,
    limits: ArchiveLimits,
    parent: &Path,
) -> Result<TempDir, SourceResolutionError> {
    source.verify()?;
    if !matches!(
        source.provenance().origin,
        crate::runtime::source_resolution::SourceOrigin::PinnedUrl { .. }
    ) {
        return Err(failure("archive extraction requires a verified pinned URL"));
    }
    let file = File::open(source.directory().join("source"))?;
    let reader: Box<dyn Read> = match format {
        ArchiveFormat::Tar => Box::new(file),
        ArchiveFormat::TarGzip => Box::new(flate2::read::MultiGzDecoder::new(file)),
    };
    let output = Builder::new()
        .prefix("metewand-extract-")
        .tempdir_in(parent)?;
    let mut archive = tar::Archive::new(reader);
    let mut paths = std::collections::BTreeSet::new();
    let mut total = 0_u64;
    for (index, entry) in archive.entries()?.raw(true).enumerate() {
        if index as u64 >= limits.max_entries {
            return Err(failure("archive exceeds entry limit"));
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let text = path
            .to_str()
            .ok_or_else(|| failure("archive path is not UTF-8"))?;
        if text.is_empty()
            || text.contains('\\')
            || text.contains(':')
            || text.split('/').any(|p| p == "..")
            || path
                .components()
                .any(|p| !matches!(p, Component::Normal(_)))
            || !paths.insert(path.clone())
        {
            return Err(failure("unsafe or duplicate archive path"));
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(failure("archive links and special entries are unsupported"));
        }
        let size = entry.size();
        total = total
            .checked_add(size)
            .ok_or_else(|| failure("archive size overflow"))?;
        if total > limits.max_expanded_bytes {
            return Err(failure("archive exceeds expanded byte limit"));
        }
        let destination = output.path().join(&path);
        if kind.is_dir() {
            if size != 0 {
                return Err(failure("archive directory contains data"));
            }
            fs::create_dir_all(destination)?;
        } else {
            fs::create_dir_all(destination.parent().expect("entry has parent"))?;
            let mut file = File::create_new(destination)?;
            if io::copy(&mut entry, &mut file)? != size {
                return Err(failure("truncated archive entry"));
            }
        }
    }
    source.verify()?;
    Ok(output)
}
