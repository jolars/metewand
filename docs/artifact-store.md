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
.lock                           serializes lease snapshots and mapping claims
```

Content addresses use the existing [local-tree transcript](local-tree-hashing.md).
Host paths, timestamps, ownership, and completion metadata do not enter the
address. Empty directories and executable flags are preserved, including the
empty tree used for dataset-free problems. The store accepts normalized UTF-8
paths, regular files, and directories; it rejects symlinks and special files.

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
`None`; incomplete or corrupted objects return errors. Extra files, missing
entries, changed directory structure, and invalid completion markers also fail.

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
These filesystem leases precede the later transactional run-state and garbage
collection interfaces. The store does not delete completed content objects.

`StoredArtifact::private_view` creates a unique owned copy for one attempt,
verifying canonical and copied bytes around the transfer. Each CLI attempt
passes that copy to its implementation and evaluator. Views share neither file
inodes nor writable dataset directories with other attempts or the canonical
object. Dropping the view removes its private directory and releases its lease.
The runner checks the copy after the attempt, so a changed view fails that
attempt and subsequent attempts receive fresh copies.

`import_source` similarly copies a verified `ResolvedSource`. It rechecks the
source around import and leaves resolver ownership and provider storage intact.
It never adopts, moves, chmods, deletes, or hard-links files from a provider's
cache. A later provider mutation cannot change the stored copy.

The local executor still permits undeclared filesystem access and reports its
existing best-effort controls. Per-operation mutation enforcement, transactional
quarantine, durable attempt recovery, and garbage collection remain later
roadmap slices.

## Compatibility and conformance

`ARTIFACT_STORE_VERSION = 1` versions the new object, mapping, and lease formats.
Their public schemas are `artifact-cache-object`, `dataset-cache-entry`, and
`artifact-cache-lease`. They join the initial version-1 catalog without changing
existing records. `LOCAL_TREE_HASH_VERSION`, identity, manifest, wire-protocol,
execution-semantics, and public-schema compatibility constants remain 1: content
transcripts, worker requests, and measurement timing are unchanged.

The schema [conformance cases](../schemas/v1/fixtures/conformance.json) cover the
new formats and reject unknown versions. Runtime tests cover exact canonical
marker bytes, empty trees, executable flags, concurrent publication, incomplete
objects, independent views and provider imports, construction conflicts, and
live and crashed lease owners. CLI tests cover persistent construction reuse,
private paths across attempts, and preflight without cache creation.
