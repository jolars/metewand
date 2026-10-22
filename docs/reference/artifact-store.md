# Artifact storage and private dataset views

`metewand-runtime::artifact_store` stores verified local trees independently of
their construction recipes. `ArtifactStore::open` takes an absolute directory
whose parent already exists. The caller assigns that directory to Metewand;
provider caches remain owned by their providers.

For persistent reuse from the CLI, select a separate cache directory:

```sh
cargo run --locked -p metewand-cli -- run \
  --manifest examples/generated-dataset/metewand.toml \
  --output-dir /tmp/metewand-first --cache-dir /tmp/metewand-artifacts
cargo run --locked -p metewand-cli -- run \
  --manifest examples/generated-dataset/metewand.toml \
  --output-dir /tmp/metewand-second --cache-dir /tmp/metewand-artifacts
```

Both output destinations must be new. The second invocation verifies and reuses
the completed dataset construction. Without `--cache-dir`, `run` uses a private
temporary store and removes it after execution. Cache and output paths resolve
from the invoking directory. The cache must be separate from run output and
outside declared source directories and glob prefixes. Complete selection
preflight precedes cache creation, acquisition, and materializer launches.
`check` and `plan` do not open or modify the cache.

## Content objects and completion

The version-1 layout is:

```text
objects/tree-v1-<tree-sha256>/
  data/                         original dataset or source tree
  manifest.json                 completion marker and exact tree inventory
derivations/<dataset-derivation-id>/
  dataset.json                  dataset manifest path and construction receipt
  manifest.json                 completion marker for the mapping
leases/lease-<unique-token>/
  lease.json                    versioned owner, purpose, and object digest
  object/ or mapping-*/         unfinished publication, when present
quarantine/lease-<token>/        abandoned staging retained for diagnosis
quarantine/tree-v1-<hash>-<token>/object/  corrupt canonical object and its marker
cache.sqlite3                  authoritative object, root, and collection catalog
garbage/tree-v1-<tree-sha256>/   exactly identified, detached collection work
.lock                           serializes cache reads, publication, and quarantine
```

Content addresses use the existing [local-tree transcript](local-tree-hashing.md).
Host paths, timestamps, ownership, and completion metadata do not enter the
address. Empty directories and executable flags are preserved, including the
empty tree used for dataset-free problems. The store accepts normalized UTF-8
paths, regular files, and directories; it rejects symlinks and special files.

See [Cache verification, roots, and garbage collection](cache-maintenance.md)
for exact-target verification, transactional collection, durable run roots,
kernel lease protection, and read-only dry runs.

`import_tree` takes a source directory and its expected tree digest. It copies
files into unique leased staging on the destination filesystem, checks the
original and copy against that digest and their complete inventories, and
synchronizes files and directories. It writes the canonical completion marker
last, verifies the completed object, publishes with an atomic no-replace rename,
and synchronizes the destination parent. Concurrent writers for the same
address verify the winner and compare its file bytes before discarding their
duplicate. Publication never replaces an existing object.

`get` returns only a completed object whose exact inventory, file hashes, sizes,
executable flags, and local-tree digest verify. A missing address returns
`None`; incomplete or corrupted objects return errors and are quarantined.
Extra files, missing entries, changed directory structure, and invalid
completion markers also fail.

## Construction mappings

`publish_dataset` first publishes the materialized output, then durably publishes
its mapping. The mapping contains the validated dataset manifest path and the
existing [construction receipt](dataset-materialization.md), retaining the
derivation transcript, source selection and decoding evidence, and separate
output hash. The first completed mapping wins. Republishing the same recipe
and output retains that mapping; a different output hash returns
`DerivationConflict` and leaves the existing mapping intact.

`identify_dataset_derivation` computes the identity before construction.
`lookup_dataset` verifies the mapping's completion inventory and full derivation
transcript, then verifies and leases the referenced output. CLI cache hits also
revalidate the dataset against the selected output schema before execution.
Different recipes may share one content object without sharing a mapping.
When quarantine removes an output, its mappings remain as constraints on the
expected output hash. A later lookup returns a cache miss, allowing construction
to run again. Publication must reproduce the original hash; a different hash
still fails with `DerivationConflict`.
Pinned external sources still require source resolution before recipe lookup;
the CLI does not provide an offline source-selection index or locked execution.
Runtime callers remain responsible for any contract-owned binary checker
required by their selected problem.

## Leases and private copies

Every publication, open object, and private view carries a versioned lease
receipt protected by an exclusive kernel file lock. The handle holds that lock
for its lifetime and removes its lease directory when dropped. `active_leases`
checks locks rather than trusting owner PIDs or timestamps. Leases do not expire
while an owner remains alive, even during a long operation or a clock change.

After a process exits or crashes, the kernel releases its locks.
`quarantine_abandoned_leases` acquires each abandoned lease before moving its
exact directory and unfinished staging into quarantine for diagnosis. It leaves
active leases, completed objects, and derivation mappings intact. A crash after object publication can leave a
complete object without a mapping; publishing it again verifies and reuses it.
The transactional cache catalog uses these same kernel leases to protect
completed objects during explicit collection. Durable named roots retain
datasets after their process-held leases end.

`StoredArtifact::private_view` creates a unique owned copy for one attempt,
verifying canonical and copied bytes around the transfer. Each CLI attempt
passes that copy to its implementation and evaluator. Views share neither file
inodes nor writable dataset directories with other attempts or the canonical
object. Dropping the view removes its private directory and releases its lease.
Copies use separate file inodes, never hard links. This portable strategy works
without filesystem-specific snapshot support. The runner verifies the private
view and canonical object immediately before worker launch and after hello, and
before and after prepare, execute, evaluate, and each worker's shutdown. It also
checks after terminating workers on a failed exchange. These checks cover the
complete tree, including empty directories, executable bits, and the dataset
manifest. The `prepare_and_execute` timer runs continuously from immediately
before prepare through receipt of the execute response. Integrity checks between
those exchanges contribute to its duration. Execution provenance records
`timing_dataset_integrity: "included_between_prepare_and_execute"`; checks before
prepare and after execute, evaluation, and shutdown remain outside the timer.

A changed view fails the attempt with `dataset_mutation`, prevents observation
publication, and is discarded. Mutation takes precedence over a simultaneous
worker error, crash, timeout, or invalid output. Later attempts receive fresh
copies if the canonical object still verifies. Attempts record
`dataset_immutability` in their provenance:

```json
{"enforcement":"best_effort","verification":"each_worker_operation","view":"copy"}
```

The lower-level `run_fixed_dataset` API accepts a caller-owned directory and
records `view: "caller_provided"`. It establishes the expected hash before
launch and applies the same operation checks, rejecting links and special files.
It does not own that directory
or quarantine it.

`import_source` similarly copies a verified `ResolvedSource`. It rechecks the
source around import and leaves resolver ownership and provider storage intact.
It never adopts, moves, chmods, deletes, or hard-links files from a provider's
cache. A later provider mutation cannot change the stored copy.

The local executor permits undeclared filesystem access. Copies isolate the
normal worker paths, but do not enforce read-only access against the worker's
OS user. Hash checks detect changes at operation boundaries; a change restored
within one operation can escape detection. Enforcement therefore remains
`best_effort`. Read-only mounts and stronger isolation belong to other executors.

## Canonical-object quarantine

All object readers, verification, view transfers, deduplicating publishers, and
mapping claims use the same exclusive store lock. When verification finds an
unverifiable canonical object, the store atomically moves that exact object,
including its completion marker, into a unique quarantine directory. It
synchronizes both affected parents and the quarantine directory before releasing
the lock. The detecting call returns `ArtifactStoreError::Quarantined`; no new
reader or view can reuse the changed address. Live leases do not prevent
quarantine of corrupt content. Existing handles must reverify and fail while
their canonical address is absent, even when their private copies are intact.

The store preserves quarantined evidence and other objects, mappings, and
provider-owned exports. It never repairs an object in place. A verified import
may republish the original bytes at the vacant content address while retaining
the quarantined object for diagnosis. Concurrent detectors quarantine an address
only once. Reopening the cache preserves the move; a missing address cannot be
returned as a completed object.

This transaction uses a kernel lock, atomic rename, and directory synchronization.
It does not depend on the later SQLite run-state store. Durable attempt recovery
and garbage collection remain later roadmap slices.

## Compatibility and conformance

`ARTIFACT_STORE_VERSION = 1` versions the new object, mapping, and lease formats.
Their public schemas are `artifact-cache-object`, `dataset-cache-entry`, and
`artifact-cache-lease`. They join the initial version-1 catalog without changing
existing records. `LOCAL_TREE_HASH_VERSION`, identity, manifest, wire-protocol,
and public-schema compatibility constants remain 1. Execution semantics version 2
adds [bounded concurrency](scheduling.md) without changing these storage formats.
This completes the unreleased version-1 immutability contract: content transcripts,
worker requests, and the timing boundary remain unchanged. The continuous timer
includes integrity verification between prepare and execute, and the new
diagnostic and provenance use the existing extensible fields. Quarantine adds a private storage location
without changing the object, mapping, or lease formats.

The schema [conformance cases](https://github.com/jolars/metewand/blob/main/schemas/v1/fixtures/conformance.json) cover the
new formats and reject unknown versions. Runtime tests cover exact canonical
marker bytes, empty trees, executable flags, concurrent publication, incomplete
objects, independent views and provider imports, construction conflicts,
live and crashed lease owners, corruption at each cache boundary, concurrent
quarantine, survival after a detector exits abruptly, and reconstruction
constrained by the original mapping. Worker tests
cover mutations during hello, prepare, execute, evaluation, and both shutdowns,
including failed exchanges. CLI tests cover persistent construction reuse,
private paths across attempts, mutation restored by a later operation, canonical
quarantine before the next attempt, enforcement provenance, continuous timing
across integrity checks, and preflight without cache creation.
