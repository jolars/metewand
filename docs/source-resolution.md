# Source resolution

`metewand-runtime::source_resolution` turns an explicit source request into
verified local files and provenance. Benchmark authors provide dataset recipes;
Metewand runs those recipes with recorded inputs, code, environments, parameters,
and seeds. Optional data providers supply recorded data and any guarantees about
its interpretation.

Source resolution precedes dataset materialization and measured execution.
`metewand check` and `metewand plan` read declarations and hash repository files
without downloading sources or invoking acquirers or materializers.

## Requests and ownership

| Request | Selection | Resolution |
| --- | --- | --- |
| `SourceRequest::Repository` | Repository root, declared paths and patterns, and expected source-bundle ID | Copy selected files and directories and verify the original and copy against the checked identity. |
| `SourceRequest::PinnedUrl` | Existing manifest `RemoteSource`, including its SHA-256 | Explicit acquirer writes raw bytes to `source`; Metewand checks the manifest pin. |
| `SourceRequest::External` | Provider name and canonical immutable selector | Explicit acquirer validates the selector and exports files; Metewand checks its complete pinned inventory. |

The caller supplies an existing parent directory for private staging. Repository
staging must be outside selected source directories and glob prefixes. It may
use an unselected directory inside the repository. The returned `ResolvedSource`
owns a temporary directory, which is removed when dropped. `into_directory()`
transfers that ownership to a later runtime operation. For durable storage,
[`ArtifactStore::import_source`](artifact-store.md) makes another verified copy
without adopting resolver directories or changing provider-owned caches.

The directory preserves repository-relative paths for repository sources.
Acquired files have normalized relative UTF-8 paths. The boundary rejects
symlinks and special files. It does not extract archives, decode file formats,
materialize datasets, publish durable cache objects, or create worker views.

For example, a runtime caller can stage a checked local source bundle:

```rust
use std::path::Path;

use metewand_core::{
    manifest::RepositoryPath,
    records::{RecordId, SourceBundleResource},
};
use metewand_runtime::source_resolution::{
    ResolvedSource, SourceRequest, SourceResolutionError, resolve_source,
};

fn stage_checked_sources(
    repository: &Path,
    sources: &[RepositoryPath],
    expected: RecordId<SourceBundleResource>,
    private_parent: &Path,
) -> Result<ResolvedSource, SourceResolutionError> {
    resolve_source(
        &SourceRequest::Repository {
            root: repository,
            sources,
            expected,
        },
        None,
        private_parent,
    )
}
```

The expected ID comes from repository checking. See [Source
bundles](source-bundles.md) for path selection and [Local-tree
hashing](local-tree-hashing.md) for content addressing. The current `run` path
uses this operation for fixed datasets and worker sources and keeps its existing
checks of original inputs before and after execution.

## Acquisition contract

URL and external requests require `Some(&dyn SourceAcquirer)`. Without one,
resolution returns `AcquirerUnavailable` before creating staging output.
Selecting a backend is explicit; the boundary does not search installed
programs, inherit provider selectors, or fall back to another dataset version.

An acquirer receives the request and an initially empty private directory. It
must honor the exact request, enforce provider-specific selector pins, and
export independent regular files through supported provider interfaces. Its
`AcquiredSource` contains the complete expected inventory and a canonical
provenance receipt. Acquirers own transport policy and operational deadlines;
process-based providers must use a bounded, noninteractive interface.

Metewand compares every acquired file's size and SHA-256 with the supplied
inventory, rejecting duplicate paths, missing or undeclared files, and
mismatches. Acquired directories must contain declared files, so unrelated empty
directories are rejected. URL requests additionally require exactly `source` and
compare its raw hash with the manifest's pin, independently of the backend
inventory.
Compression remains intact. Resolution errors remove partial staging output.

For an external provider, validating that the inventory belongs to the selected
registry or dataset is the backend's responsibility. Generic byte checks cannot
establish that relationship. Providers retain control over their own caches;
Metewand receives exported files and never treats a provider cache path as an
owned artifact.

## Verification and provenance

`ResolvedSource::files()` returns the verified inventory in UTF-8 path order.
`tree_digest()` uses the existing version-1 local-tree transcript, including
paths and executable bits. Host paths and acquisition provenance do not affect
the digest. `verify()` rechecks the private directory before later use; a
resolution-time check does not make a writable directory immutable.

The in-memory `SourceProvenance` receipt carries
`version = SOURCE_RESOLUTION_VERSION = 1`, its `SourceOrigin`, and an optional
backend receipt. Repository origins retain the checked bundle ID. URL origins
retain the requested location and digest. External origins retain the provider
name and exact canonical selector.

Metewand verifies artifact bytes. Provider receipts may separately report
decoded-value verification, but preserving a claim does not upgrade byte
verification into verification of logical values. Datamonger's current Rust
CLI retrieves verified raw artifacts; a Datamonger integration must explicitly
provide decoding and record whether decoded values were verified.

## Current support and compatibility

The built-in implementations resolve repository files and pinned HTTP(S) URLs.
External acquisition remains an extension point; a Datamonger adapter remains a
separate roadmap item.
`metewand run` continues to support the [trusted local
selection](run-cli.md#supported-selection).

This boundary adds no manifest syntax or serialized public interface. The
manifest, public schema, identity, local-tree hash, protocol, and execution
semantics versions retain their existing values. External-provider manifest
syntax or serialized receipts must introduce the corresponding versioned
contracts and conformance fixtures before use.

## Pinned HTTP downloads and archive extraction

Runtime callers explicitly construct `source_acquisition::HttpAcquirer` with a
request timeout and maximum raw byte count, then pass it to `resolve_source`.
The backend accepts HTTP and HTTPS URLs without embedded credentials. It rejects
redirects and non-success responses and preserves the response body without
transparent content decompression. It streams into an unnamed temporary file,
checks the SHA-256 pin, and only then writes `source`. The resolver independently
checks the resulting inventory and manifest pin. Failed resolution removes all
staging files. TLS uses the transport's bundled public roots.

`extract_archive(&resolved, format, limits, private_parent)` rechecks a resolved
pinned URL source before extraction and again before returning a separate owned
`TempDir`. Choose `ArchiveFormat::Tar` or `ArchiveFormat::TarGzip` explicitly and
supply both `ArchiveLimits::max_entries` and `max_expanded_bytes`. Formats are
never inferred from URL suffixes. Other formats are unsupported.

The extractor accepts regular files and directories with relative UTF-8 paths.
It rejects absolute paths, parent traversal, backslashes, colons, duplicate
paths, file/directory conflicts, all symbolic and hard links, devices, FIFOs,
and other special entries. It also rejects tar metadata extensions, including
PAX and GNU long-name and sparse entries, rather than allowing hidden entries
to bypass the budgets. Directory entries count toward the entry limit. Declared
entry data sizes count toward the total expanded byte limit before any payload
is written; truncated payloads fail. Archive permissions, executable bits,
ownership, and timestamps are not restored. Errors remove partial extraction.
The caller must provide a private staging parent that untrusted processes cannot
modify. This operation does not publish an artifact or create a cache lease.

These runtime APIs add no manifest syntax or serialized compatibility contract.
`SOURCE_RESOLUTION_VERSION`, local-tree identities, schemas, wire messages, and
execution semantics retain their current versions. The CLI selects the HTTP
backend during explicitly requested generated-dataset runs. [Dataset materialization](dataset-materialization.md) records source and
decoding evidence alongside the construction identity. `check` and `plan` never
invoke acquisition or materialization.
