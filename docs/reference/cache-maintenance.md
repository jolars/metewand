# Cache verification, roots, and garbage collection

Cache maintenance operates on the Metewand-owned directory selected by
`run --cache-dir PATH`. It never opens a provider's cache or a run output
directory as an artifact cache. Commands require an existing initialized cache,
launch no workers, and perform no source acquisition or run-store recovery.

```sh
metewand cache roots --cache-dir /tmp/metewand-artifacts
metewand cache gc --cache-dir /tmp/metewand-artifacts --dry-run
```

Both commands print complete object identities such as `tree-v1-` followed by
64 lowercase hexadecimal digits. Copy the entire identity into `OBJECT_ID` to
verify or collect that object:

```sh
metewand cache verify --cache-dir /tmp/metewand-artifacts --id "$OBJECT_ID"
metewand cache gc --cache-dir /tmp/metewand-artifacts --id "$OBJECT_ID" --dry-run
metewand cache gc --cache-dir /tmp/metewand-artifacts --id "$OBJECT_ID"
```

`verify` requires at least one `--id`. It checks the completion marker, exact
inventory, file sizes, hashes, executable flags, tree digest, and cataloged
manifest hash. Paths, prefixes, globs, shortened hashes, uppercase hashes, and
unknown address versions are rejected. Repeated identities are deduplicated,
and reports sort identities, root names, and lease purposes deterministically.
Missing or corrupt objects produce a receipt and exit status 5. Verification
reports corruption without changing the cache; normal artifact consumption
continues to quarantine corrupted canonical objects before reuse.

`gc` without `--id` considers every registered content identity in a fresh
transactional snapshot of `cache.sqlite3`. With `--id`, it considers only those
exact registered identities. An unknown identity fails before any deletion.
Rooted or actively leased objects remain intact. A corrupt eligible object or
unsafe path fails the complete selection before collection begins. Unregistered
directories, derivation mappings, quarantine evidence, and abandoned staging
are preserved. A stale, missing, or malicious `index.jsonl` has no effect on
verification or deletion decisions.

Verification, root listing, and all `--dry-run` operations leave cache files
unchanged, including SQLite files and sidecars. The reader copies the database
and WAL into private temporary storage and reads a committed transaction there.
It never initializes a missing cache. Collection and root changes without
`--dry-run` explicitly authorize their respective mutations. A dry-run report
is advisory; a later collection always takes a new snapshot.

## Durable roots and process leases

Each persistent run pins its resolved datasets under the root name
`run:<absolute-output-directory>` before registering their run-store slots.
Registration may conservatively retain a root if execution is interrupted or
bounded before a slot runs. Removing or moving the run directory does not
release its root. Collection never guesses that a run is disposable from a
missing path or an index.

An explicit root protects one tree independently of any run:

```sh
metewand cache root add --cache-dir /tmp/metewand-artifacts \
  --root manual:keep --id "$OBJECT_ID"
metewand cache root remove --cache-dir /tmp/metewand-artifacts \
  --root manual:keep --id "$OBJECT_ID" --dry-run
metewand cache root remove --cache-dir /tmp/metewand-artifacts \
  --root manual:keep --id "$OBJECT_ID"
```

To release a run root, use its exact name from `cache roots` in the same command.
Each removal requires exactly one root and one object identity. Removing a root
does not delete data or override another root or a live lease. Adding an existing
edge is idempotent; removing a nonexistent edge fails. An explicit root removal
allows a later collection to reclaim that dataset even if its historical run
directory still exists.

Publication, mapping, reader, and private-view handles hold kernel locks on their
lease receipts. Collection probes these locks while holding the cache lock.
PID reuse, elapsed wall time, and stale lease files cannot turn an active owner
into garbage. Dead owners' unlocked receipts do not protect completed content;
the separate `quarantine_abandoned_leases` runtime API retains their staging for
diagnosis.

## Catalog and crash behavior

The catalog uses SQLite WAL mode, full synchronous durability, foreign keys,
strict tables, and an independently versioned layout, `CACHE_CATALOG_VERSION = 1`.
It records verified object and manifest hashes, named root edges, and pending
collection intents. Every cache writer, root change, lease acquisition, and
collection serializes through `.lock`; callers must keep the cache directory
and its ancestors under trusted control. Collection retains that lock from its
database snapshot through protection checks, detachment, and cleanup. A new
reader, publisher, or root therefore cannot race the deletion of its object.

Before deleting, collection commits an intent for one exact digest, moves that
object without replacement to `garbage/tree-v1-<digest>`, synchronizes both
parents, and commits its detached state. Only then does it recursively remove
that exact detached directory and retire the catalog entry. An interruption can
leave a pending intent, a complete detached object, or partial detached garbage.
The next explicit collection rechecks roots and live leases and completes only
that recorded identity. It never expands an interrupted cleanup into deletion
of a parent, sibling, or path supplied by an index. Reimporting the same digest
between collections preserves the new live object until a fresh protection
check permits collection.

Object publication synchronizes bytes before registering them in the catalog.
A crash before registration leaves a complete unregistered orphan, which
collection preserves. Verified imports, reads, or explicit root additions can
register it. Opening an older filesystem cache through `ArtifactStore::open`
adds the catalog without registering arbitrary directories. Existing objects
remain uncollectible until a verified operation registers their identities.
Historical runs created before root registration need explicit pins for any
datasets that should remain retained after those objects are registered.

Collection preserves construction mappings so a later generated-dataset run
can reconstruct a removed output and must reproduce its original output hash.
This adds the initial version-1 `cache-output` public schema and catalog layout.
The existing artifact-store, tree-hash, identity, manifest, public-schema,
execution-semantics, and wire-protocol constants retain their existing values.
Object and derivation transcripts do not change.

## Machine output and runtime API

`--output json` and `--output jsonl` return the same typed `CacheReport` rendered
by human output, inside the version-1 machine envelope. The `cache-output` schema
defines its operation, dry-run flag, exact targets, states, durable roots, active
lease purposes, and optional corruption detail. Use
`metewand schema cache-output` to inspect it offline.

Runtime consumers use `ArtifactStore::open_existing`, `verify_targets`, `roots`,
`update_root`, and `collect`. `StoredArtifact::add_root` pins a verified live
handle. `ARTIFACT_STORE_VERSION` still versions object, mapping, and lease
receipts; `CACHE_MAINTENANCE_VERSION = 1` versions maintenance reports.

The tests cover committed WAL snapshots, exact byte preservation during reads
and dry runs, roots, live and abandoned leases, concurrent publication and
collection, corrupted and unregistered paths, stale indexes, and process death
at each collection database, synchronization, rename, and cleanup boundary.

```sh
cargo test -p metewand-runtime --test cache_maintenance --locked
cargo test -p metewand-runtime --lib --locked artifact_store::maintenance::tests
cargo test -p metewand-cli --test cache --locked
```

The exhaustive cache crash campaign is ignored in the normal suite. Run it
explicitly after changing catalog durability, publication, or collection:

```sh
cargo test -p metewand-runtime --lib --locked \
  artifact_store::maintenance::tests::collection_recovers_after_each_database_sync_rename_and_cleanup_boundary \
  -- --exact --ignored --nocapture
```
