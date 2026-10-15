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
transfers that ownership to a later runtime operation.

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

The built-in implementation resolves repository files. URL and external
acquisition are runtime extension points tested with offline backends; download
transports and the Datamonger adapter remain separate roadmap items.
`metewand run` continues to support the [trusted local
selection](run-cli.md#supported-selection).

This boundary adds no manifest syntax or serialized public interface. The
manifest, public schema, identity, local-tree hash, protocol, and execution
semantics versions retain their existing values. External-provider manifest
syntax or serialized receipts must introduce the corresponding versioned
contracts and conformance fixtures before use.
