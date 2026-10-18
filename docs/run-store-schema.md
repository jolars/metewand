# Durable run-store schema

The runtime embeds an executable SQLite design in
[`src/run_store/v1.sql`](../crates/metewand-runtime/src/run_store/v1.sql).
`metewand_runtime::run_store::RUN_STORE_SCHEMA_SQL` exposes the same bytes, and
`RUN_STORE_SCHEMA_VERSION = 1` matches SQLite's `user_version`. This is the
coordination schema for the durable-attempt roadmap. The current `run` command
still uses the [filesystem publisher](durable-publication.md); it does not open
this database or provide transactional resume yet.

## Compatibility and connection requirements

The database layout has its own compatibility version. It does not change the
manifest, public JSON Schemas, identities, execution semantics, wire protocol,
or artifact-store compatibility constants, which remain at version 1. Profile
states and slot kinds in this layout reserve the semantics described in
[DESIGN.md](../DESIGN.md#automation-and-agentic-workflows); they do not add profile
support to the current one-shot domain records or CLI.

A future initializer must read `user_version` before applying any DDL, create
version 1 only in an empty database, and reject unknown versions. Migration must
be deliberate and transactional. The SQL intentionally omits `IF NOT EXISTS` so
an existing, incompatible table cannot silently pass initialization. Apply the
whole script in one transaction and roll back on any error.

Every connection must enable and verify `PRAGMA foreign_keys = ON` and
`PRAGMA recursive_triggers = ON` before starting a transaction. The latter makes
insert-only guards apply to the deletion performed by `INSERT OR REPLACE`.
Foreign keys must remain enabled during writes and recovery. Tables use
`STRICT`, and JSON columns require valid JSON objects. The SQLite build must
support strict tables and JSON functions. These requirements follow SQLite's
[foreign-key rules](https://www.sqlite.org/foreignkeys.html),
[strict table rules](https://www.sqlite.org/stricttables.html), and
[partial unique index rules](https://www.sqlite.org/partialindex.html).

WAL mode, full synchronous durability, busy handling, filesystem capability
checks, and refusal of unsupported durable operation belong to the next slice.
Executing this schema alone proves none of those properties. Tests use bundled
SQLite through a development dependency; the runtime has no SQLite connection
or writer dependency yet.

## Rows and relationships

| Table | Meaning and constraints |
| --- | --- |
| `applicability_decisions` | Immutable identified decisions, including exclusions and their full evidence. |
| `resolved_runs` | Resolved specification and logical specification IDs, bound by a composite foreign key to an applicable decision for the same candidate. |
| `observation_slots` | Resolved observation IDs and full slot records; a logical slot occurs once per resolved run. |
| `attempt_slots` | Resolved execution IDs, full slot records, execution kind, and measurement or warm-up role; a logical slot occurs once per resolved run. |
| `attempt_observation_slots` | Explicit membership within one resolved run. One execution slot can own several checkpoint slots; each observation slot belongs to one execution slot. |
| `attempts` | Distinct IDs and nonnegative retry indices, current state and revision, timestamps, typed failure code, and final directory reference. `(slot_id, retry_index)` is unique. |
| `events` | Insert-only, globally sequenced structured events, optionally scoped to an attempt. |
| `attempt_transitions` | Insert-only contiguous state history, with one event per transition and an exact attempt and timestamp match. |
| `leases` | Unique fencing tokens, owner tokens, optional producing attempt, purpose, acquisition, expiry, release, and staging path. |
| `finalization_claims` | At most one claim per execution slot, with one attempt and its own finalization lease. |
| `observations` | Independently published results and checkpoint records, their validity and completion status, producing attempt, permitted slot membership, and finalized directory. |
| `selected_observations` | Insert-only selection, keyed by resolved observation slot and constrained to a valid observation in that exact slot. |
| `artifacts` | Verified immutable artifact identities, kinds, content digests, normalized store-relative paths, and directory completion-manifest digests. |
| `attempt_artifacts` | Named dependency and output references from attempts, including dataset, source, and environment artifacts. |
| `observation_artifacts` | Named result, metric, and other references from durable observations. |
| `leased_artifacts` | Artifact roots protected by a lease, including resources not yet referenced by a finalized record. |

IDs are the existing typed identity strings, compared with SQLite's default
binary collation. SQL does not derive IDs. Writers must validate typed IDs and
recompute them from canonical domain records. `record_json` stores the complete
canonical record used to interpret each identity, including observation
control, timing, provenance, schedule priority, and applicability evidence as
appropriate. SQL validates the outer JSON object only; domain validation,
canonical encoding, and agreement between JSON and indexed columns remain
writer responsibilities.

All timestamps are nonnegative integer nanoseconds since the Unix epoch and
must fit SQLite's signed 64-bit integer. Retry indices must also fit that range.
Out-of-range values must fail explicitly at the writer boundary. Wall-clock timestamps remain
provenance, not identity inputs. Event sequence numbers are store-local delivery
positions allocated by SQLite, not schedule priorities or scientific identities.
Emit events to consumers after committing their transaction; a rolled-back
transition must never appear as a committed event.

## Reservation and transitions

Reserve an attempt in a short `BEGIN IMMEDIATE` transaction. Check the resolved
slot, satisfaction, explicit retry authorization, and retry bounds; allocate a
new retry index and its deterministic attempt ID. Insert the `reserved` attempt,
a reservation lease, its event, and revision-zero transition, then commit.
The deferred foreign key from the attempt's current revision to its transition
ensures that a reservation without its history cannot commit.

For each subsequent state change, verify the caller's owner and lease tokens,
its unexpired lease, and the expected current revision in the same transaction.
Append an event and transition, update the current attempt projection by exactly
one revision, renew or release the appropriate lease, and commit. Check the
number of updated rows; stale owners must fail. No transaction should remain
open across worker execution, hashing, synchronization, or rename.

The schema permits these edges:

```text
reserved -> materializing -> starting -> preparing -> executing
executing -> checkpointing -> validating -> evaluating
executing -> validating -> evaluating
evaluating -> executing                         another checkpoint
any nonterminal execution phase -> finalizing
finalizing -> accepted | censored | superseded | failed
```

Every transition references its predecessor's exact state and revision. The
current attempt references its exact transition and timestamp. Updates preserve
the attempt ID, resolved slot, retry index, and start time; revision increases
by one, and wall-clock transition timestamps cannot decrease. Writers must clamp
backward wall-clock adjustments to the preceding timestamp while retaining
executor timing separately. Terminal attempts cannot change or be deleted.
A failed attempt requires a nonempty diagnostic code; full diagnostic details
belong in its event payload and immutable attempt document.

This slice defines and tests the relational constraints. The transactional
writer API, lease fencing checks, complete event vocabulary, and recovery
operations remain subsequent slices. In particular, writers must update the
projection with every appended transition; they must not append future history
without advancing the current state.

## Observation selection and attempt acceptance

Publish each observation independently. Validate the result, evaluator metrics,
scientific validity, and completion status, synchronize and publish its immutable
directory, and then insert its artifact metadata, observation, result and metric
references, event, and optional selection in one database transaction. The
observation's producing attempt and execution slot must match its observation
slot's declared membership. A directory on disk or an artifact row alone does
not select an observation.

`selected_observations.slot_id` is the primary key. Its composite foreign key
requires the exact selected observation to belong to that slot and have
`valid = 1`. Selection cannot be updated, deleted, or replaced. A valid
`not_reached` observation may be selected: scientific validity and reaching a
completion target are separate decisions.

Retries may retain several published observations for the same slot. A retry
that recomputes a selected checkpoint keeps its own observation record, but
cannot replace the selection. Observations and selections remain after the
producing stream is censored, superseded, or fails. A one-shot or fresh-observation
execution slot is satisfied only by an accepted attempt, so an earlier valid
observation from a failed attempt does not independently satisfy that slot.

The partial unique index `one_accepted_attempt_per_slot` permits at most one
`accepted` row per resolved execution slot while retaining every other retry.
`accepted`, `censored`, `superseded`, and `failed` all require a final artifact of
kind `attempt` and a finish timestamp. Merely entering `finalizing` or owning a
claim never satisfies the slot.

Before recording acceptance, the writer must validate the policy-specific
completion rule and every required selected observation and artifact dependency.
For one-shot and fresh-observation execution, check that the attempt produced
its required valid result and reached its completion rule. A stream may reuse
previously selected checkpoint slots but must reach its contracted target and
retain the producing-attempt lineage of every observation. A warm-up is never
selected as a measured observation. SQL enforces relational integrity, not
problem-owned scientific rules or the truth of filesystem verification.

## Finalization claims and recovery handoff

Move to `finalizing` before acquiring a claim. In a short transaction, verify
lease ownership and expiry, recheck slot satisfaction, create a finalization
lease, and insert its claim. The claim's primary key serializes competing
successful finalizers. Its composite foreign keys require the same attempt,
execution slot, and finalization lease. A claim cannot use a released lease,
change owners, or attach to an already accepted slot. A concurrent loser records
`superseded` in its immutable outcome instead of publishing a second success.

Outside the transaction, finalize the outcome document and complete manifest,
synchronize files and directories, rename from same-filesystem staging with
no replacement, and synchronize the destination parent. The final transaction
must check owner tokens, current revision, and the still-unexpired claim again,
register the verified artifact and all references, append the terminal event and
transition, update the attempt, delete the resolved claim, and release its
leases. Acceptance requires that exact attempt to own the claim when its
projection changes. Failure or censoring can finalize without a success claim.

Lease expiry is a recovery signal, never an automatic deletion condition. Claims
remain unique even after expiry. Recovery must acquire the required filesystem
locks and recheck owner fencing before inspecting staging and completed paths.
It must either verify and import the claimed attempt's complete published
outcome or durably finalize it as a typed failure. Only then may it remove the
claim and permit another retry to claim that slot. The delete guard prevents
claim release while its attempt is nonterminal. Owner tokens cannot change;
renewals cannot shorten expiry, and released leases cannot become active again.
An expired owner cannot renew or publish through the future writer API.

Recovering an orphan must validate its full directory, record identities,
resolved memberships, references, and completion semantics. Importing an
already accepted attempt or selected observation must respect the same unique
constraints. Recovery must never infer success from the presence of a staging
directory, a completion marker alone, an artifact row, or a claim. Replay uses
the same reservation and transition constraints, preserving every recovered
retry identity. Recovery algorithms and fault injection are later slices.

## Artifact references and retention

Artifact rows describe completed, verified content, never staging. Paths are
normalized relative to the store root, reject traversal and absolute paths,
and identify exact objects. Content digests and directory manifest digests are
lowercase SHA-256 strings. A directory requires its completion-manifest digest.
The filesystem publisher must still reject escaping symlinks and special files,
verify complete inventories and content identities, and perform durable
publication before registering the artifact. SQL cannot establish these facts.

Final attempt and observation directory references are typed foreign keys;
named dependency references and lease roots also require existing artifact
rows. Include all transitive dependencies needed to interpret the finalized
records. Failed and superseded attempts retain their references. Garbage
collection must use all committed record references and active lease roots from
a consistent database snapshot, not only selected observations or accepted
attempts. Expired leases require recovery before their roots can be discarded.
This layout retains coordination history and metadata; later garbage collection
must explicitly record content removal without rewriting that history.

The in-memory SQLite conformance tests exercise strict tables, schema version,
applicability, resolved membership, distinct retries, transaction rollback,
transition history and event linkage, all terminal outcomes, claim ownership,
recovery ordering, immutable selections, retained checkpoints, artifact kinds,
and normalized paths. They establish schema invariants without claiming crash
durability or implementing the later store operations.
