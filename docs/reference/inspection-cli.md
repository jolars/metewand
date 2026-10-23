# Status and identity explanations

`metewand status` reports measured execution slots and the attempts that have
occupied them. `metewand explain <id>` shows the available record for an exact
stable identity, its paths, and related identities. Both commands support
`--output human`, `--output json`, and `--output jsonl`.

```sh
metewand status
metewand status --manifest examples/fixed-dataset/metewand.yaml
metewand status --output-dir results --output json
metewand status --output-dir results --manifest metewand.yaml
metewand explain 'mw1-attempt-<64-lowercase-hex-digits>' --output-dir results
metewand explain 'mw1-implementation-definition-<64-lowercase-hex-digits>'
metewand schema inspection-output
```

The bracketed digests above stand for complete IDs copied from actual output.
Lookup accepts complete identities, not prefixes, manifest-local names, paths,
or diagnostic codes.

## Selecting evidence

Without `--output-dir`, the commands check `./metewand.yaml`, or the explicit
`--manifest PATH`, and expand its logical plan. They inspect definitions,
schemas, problem contracts, source bundles, policies, configurations,
candidates, logical runs, and slots without resolving environments, acquiring
datasets, or launching workers. Before execution, every measured slot is
pending. Warm-up identities are explainable, but status counts measured slots:
the current runtime performs warm-ups as part of worker preparation rather than
registering separate durable attempts for them.

With `--output-dir PATH`, the commands inspect that existing directory's
`run.sqlite3` and its registered artifacts. They do not need a manifest or the
original worker sources. This view includes all registered slots and all
recorded attempts, across bounded invocations and retries. It cannot report
slots that no invocation has registered. Benchmark name and current manifest
hash are null unless a manifest is also selected.

Supply both flags to compare recorded history with the current plan. Each slot
has `in_current_plan: true` or `false`; this field is null without a current
plan. New logical slots appear as pending, while historical slots remain
visible. Multiple resolved dependency sets for one logical slot remain separate.
A logical match does not verify that the current host would resolve the same
runtime dependencies.

## Status records

The report contains `slots`, `attempts`, `observations`, `diagnostics`, and a
`summary` derived from those records. Slot states describe the recorded
scheduling situation:

| State | Meaning |
| --- | --- |
| `pending` | No attempt has been allocated. |
| `active` | A nonterminal attempt has an unexpired reservation lease. |
| `satisfied` | A durably accepted attempt occupies the resolved slot. |
| `retry_required` | Previous attempts ended without acceptance; another attempt requires explicit authorization. |
| `recovery_required` | A nonterminal attempt has an expired or absent reservation lease. |

Acceptance takes precedence over other attempts in the same slot. An unexpired
reservation takes precedence over expired reservations. A lease describes
recorded ownership, not proof that an operating-system process is alive.
Inspection does not change an expired attempt's state. Use the explicit
`run --resume --retry --output-dir PATH` workflow to recover interrupted work
and authorize new attempts; accepted slots remain satisfied.

Attempts retain their complete state, retry index, revision, terminal artifact
ID, diagnostic ID, and reservation expiry. Status counts accepted, failed,
superseded, censored, and nonterminal attempts separately. Valid observations
remain visible even when their producing attempt failed or was superseded.
Their `selected` flag is independent of validity and completion.

Epoch nanoseconds are decimal strings, preserving values beyond JSON's safe
integer range. Unavailable timestamps and artifact references are null. Slots
sort by logical and resolved ID, attempts by resolved slot and retry index,
observations by ID, and diagnostics by ID. Human output renders these same
typed records. A successful inspection exits with code `0` even when it reports
failed benchmark attempts.

## Explanation records

An explanation contains one `subject` with `id`, `kind`, `label`, `record`,
`related_ids`, and `paths`. Planned records expose their canonical identity
representations. Stored records expose committed coordination, available
immutable documents, and attempt events. Artifact explanations include the
recorded digest and path; stored directory artifacts include their completion
inventory, and result and metrics artifacts include their public documents.

Related IDs collect literal identity references and coordination links in
sorted, duplicate-free order. They are navigation aids, not an assertion that
every string reference is a Merkle dependency. An identity known only through a
reference has `record.availability: "reference_only"`. This is common when
historical provenance references a definition whose original repository is
unavailable. Inspection does not fabricate its definition or resolve it again.
Paths are repository-relative for current definitions and relative to the run
directory for stored artifacts.

The commands accept public `mw1-<kind>-<digest>` IDs and the existing internal
store keys `run-store-v1:<artifact-kind>-<digest>` and
`run-store-v1:unconditional:<logical-candidate-id>`. They retain these spellings
without inventing new scientific run identities.

Persisted attempt failures receive a content ID of kind `diagnostic`. Its
canonical resource value is `{"code":...,"message":...,"details":...}`, with
missing details normalized to null. It uses the existing version-1 resource
wrapper and hash transcript. The same diagnostic content has the same ID across
attempts; `related_ids` connects all producing attempts and slots. Copy this ID
from status to explain the failure. Ephemeral command-line validation errors
retain their existing diagnostic codes and are not persisted in a run store.

## Read-only behavior and compatibility

These inspection commands currently require a POSIX host. Store inspection
reads the main SQLite file and WAL into a private temporary snapshot, checking
file identity, size, and modification metadata across the copy. SQLite opens
only the private copy,
so even a closed WAL store or a store without a shared-memory sidecar is
inspected without creating files in the run directory. A changing database is
retried for up to five seconds. The private temporary directory is removed when
the read completes.

The reader requires the exact initialized version-1 store schema. It reads only
committed records and verifies registered artifact inventories and hashes.
Unregistered orphan directories and stages cannot establish acceptance.
Inspection performs no initialization, capability probes, checkpoints on the
source database, recovery, quarantine, retries, or repository writes.
Corruption fails inspection and leaves the evidence in place.

Machine results conform to the new version-1 `inspection-output` schema inside
the existing `machine-output` envelope. JSON Lines emits one terminal record.
Unknown IDs and unavailable inputs exit with code `4`; malformed requests use
`2`, incompatible or corrupt evidence uses `3`, and snapshot contention uses
`74`. Failures use the existing structured diagnostic envelope and standard
stream rules in [Machine-readable command output](machine-output.md).

This slice adds a schema entry point and a diagnostic identity kind. It leaves
`PUBLIC_SCHEMA_VERSION = 1`, `IDENTITY_VERSION = 1`,
`RUN_STORE_SCHEMA_VERSION = 1`, `WIRE_PROTOCOL_VERSION = 1`, and
`EXECUTION_SEMANTICS_VERSION = 2`: existing schemas, identity transcripts,
execution behavior, and persisted store layouts are unchanged. Public-schema
conformance cases and [diagnostic identity vectors](https://github.com/jolars/metewand/blob/main/fixtures/identities/diagnostics-v1.json)
cover the additions.
