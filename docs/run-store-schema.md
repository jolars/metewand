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
support to the current one-shot domain records or CLI. The writer uses these
existing version-1 edges and constraints without changing any compatibility
constant. Identity conformance tests also check the reservation helper against
the existing terminal-attempt vectors.

`RunStore::open` reads `user_version` before changing the persistent journal
mode or applying DDL. It creates version 1 only in an empty database, rejects
unknown versions and unversioned databases with user objects, and checks an
existing version-1 database against the embedded tables, indexes, and triggers.
Migration must be deliberate and transactional. The SQL intentionally omits
`IF NOT EXISTS` so an existing, incompatible table cannot silently pass
initialization. The initializer applies the whole script in one immediate
transaction and rolls it back on any error.

Every connection must enable and verify `PRAGMA foreign_keys = ON` and
`PRAGMA recursive_triggers = ON` before starting a transaction. The latter makes
insert-only guards apply to the deletion performed by `INSERT OR REPLACE`.
Foreign keys must remain enabled during writes and recovery. Tables use
`STRICT`, and JSON columns require valid JSON objects. The SQLite build must
support strict tables and JSON functions. These requirements follow SQLite's
[foreign-key rules](https://www.sqlite.org/foreignkeys.html),
[strict table rules](https://www.sqlite.org/stricttables.html), and
[partial unique index rules](https://www.sqlite.org/partialindex.html).

## Durable initialization

On Unix, `metewand_runtime::run_store::RunStore::open` initializes or reopens a
durable connection. The caller authorizes mutation by invoking it and supplies
an existing absolute directory on a filesystem shared by processes on one host.
The initializer does not create that root; the caller must durably create it
and its parents first. Paths and ancestors must remain under trusted control
while the store is open.

```rust
use metewand_runtime::run_store::RunStore;

let store = RunStore::open(output_root)?;
let database_path = store.database_path();
```

The root contains `run.sqlite3`, SQLite's live `run.sqlite3-wal` and
`run.sqlite3-shm` sidecars, and a persistent `.run-store-init.lock` file. The
initializer rejects a symbolic link at the root, database, lock file, or an
existing SQLite sidecar, and requires regular files for database objects. It
opens SQLite without URI interpretation or VFS overrides. All initializers
serialize on the lock file with a five-second timeout; the file stays in place
so another process cannot accidentally lock a replacement inode.

Every connection sets and reads back `journal_mode = WAL`, `synchronous = FULL`,
`locking_mode = NORMAL`, `foreign_keys = ON`, and `recursive_triggers = ON`.
It also enables and verifies `fullfsync` and `checkpoint_fullfsync` for platforms
that implement stronger synchronization through those settings. SQLite busy
handling waits at most five seconds per lock acquisition. The runtime uses
bundled SQLite, including strict tables and JSON functions. The connection
stays private so callers cannot weaken these settings. WAL persists in the
database, while connection-local settings are reapplied on every open.

Every open runs capability probes in a private disposable directory within the
supplied root before opening the real database:

- Two independently opened descriptors must enforce exclusive file locks and
  permit reacquisition after release. These locks serialize initialization.
- Short-lived child processes must fail to acquire a conflicting POSIX record
  lock and succeed after its release. SQLite uses this locking family. The
  child performs only `fcntl` and immediate process exit after `fork`, and the
  parent reaps it. Process creation or probe failure refuses initialization.
- A written file and its containing directories must synchronize successfully.
- Atomic no-replace directory rename must preserve the payload and refuse to
  replace both populated and empty destinations without changing either tree.
  The probe synchronizes the parent after successful publication.
- Two WAL connections must exclude competing immediate write transactions,
  hide uncommitted data, expose commits, discard rolled-back writes, and
  complete a WAL checkpoint. The probe also exercises strict tables and JSON.

The initializer removes its disposable directory and synchronizes the root.
After the schema transaction commits, it synchronizes the database file and
root before returning the connection. A failed probe or synchronization returns
an error; there is no fallback to rollback journals, weaker synchronization,
ordinary replacing rename, or an in-memory database. Platforms without the
required no-replace rename refuse initialization. Linux also rejects recognized
network filesystems, including NFS, SMB/CIFS, AFS, Ceph, Coda, NCP, and 9P,
because a local probe cannot establish shared-memory safety across hosts.
SQLite documents the [WAL filesystem restrictions](https://sqlite.org/wal.html)
and [synchronization settings](https://sqlite.org/pragma.html#pragma_synchronous).

These checks establish that the required operations report success and enforce
exclusion on the current filesystem. They cannot detect storage hardware that
acknowledges synchronization without honoring it. Durable use requires storage
that honors these operations; network filesystems are unsupported even when a
mount cannot be recognized automatically.

Opening records no attempts or success events and imports no artifacts. An
error after the schema commits can leave an initialized database, which a retry
checks and reopens. Interrupted probe directories do not establish any attempt
state. The writer API below adds transactional attempt transitions and final
artifact registration. The publisher API below connects durable filesystem publication to these
transactions. Explicit recovery is described below; CLI integration remains a
later slice. Executing the schema SQL alone
does not perform any of these durability checks.

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

On Unix, `RunStore` provides the following transactional writer API. These are
coordination operations for the trusted orchestrator. They require resolved
slots and applicability records already registered by the planner. This slice
does not connect the existing CLI planner to SQLite. The transactional
publisher below uses the same fencing and finalization operations. The connection
remains private.

| Operation | Committed effect |
| --- | --- |
| `reserve(slot, authorization, lease, now_ns)` | Allocates the next retry, derives its existing version-1 attempt ID, and inserts the attempt, reservation lease, event, and revision-zero transition together. |
| `advance(&attempt, phase, now_ns)` | Checks ownership and revision, appends one legal transition and event, and advances the projection. |
| `renew(&attempt, expires_at_ns, now_ns)` | Renews the reservation, any owned finalization lease, and live staging leases together, without shortening expiration. |
| `begin_finalization(&attempt, lease, now_ns)` | Enters `finalizing` and records either acquisition of a success claim or a `superseded` decision in the same transaction. |
| `commit_finalization(&attempt, outcome, artifact, references, now_ns)` | Registers the published attempt artifact and dependency references, appends the terminal event and transition, updates the projection, deletes its resolved claim, and releases its leases together. |

`RetryAuthorization::FirstAttempt` permits only retry zero.
`RetryAuthorization::Retry { next_index, max_index }` authorizes exactly the
next retry and imposes an inclusive limit. Both values are checked while holding
the write lock, so competing requests cannot silently allocate extra retries.
A satisfied slot refuses every new reservation. An explicitly authorized retry
may overlap an active attempt; finalization claims serialize acceptance.
`run_attempt_id(slot, retry_index)` computes the same identity as the eventual
`RunAttemptRecord`, without inventing a terminal record at reservation time.
Retry indices and event revisions must also fit the canonical JSON integer
range, which is narrower than SQLite's integer range.

A `LeaseRequest` supplies a unique lease ID, a private owner token, and an
expiration. Every successful transition returns a new opaque `OwnedAttempt`
guard. Reusing an earlier revision fails, including from another connection.
Each operation takes `BEGIN IMMEDIATE` before checking the reservation owner,
lease ID, release status, expiration, and current revision. If the attempt owns
a finalization claim, its lease must also belong to that owner and remain live.
Expiration is exclusive: a lease cannot write or renew at its expiration time.
Renewal checks all applicable leases before committing, so a failed renewal
cannot partly extend ownership. It also extends live staging leases belonging to
the attempt. Expired staging leases remain for recovery and do not prevent the
owner from renewing its reservation. Callers supply current epoch nanoseconds for
deterministic testing and must use an actual current clock in production.
Lease checks use at least the preceding transition time when the clock moves
backward. Newly acquired finalization leases must extend beyond that time.

The writer emits canonical object payloads and these internal event kinds:

- `attempt_reserved`: `from: null`, `to: "reserved"`, `revision: 0`, the resolved
  `slot`, and `retry_index`.
- `attempt_transition`: `from`, `to`, and `revision` for an execution phase.
- `attempt_finalizing`: the same transition fields and, for a success contender,
  `claim: "claimed"` or `claim: "superseded"`.
- `attempt_finished`: the terminal transition fields; failures also include
  `failure_code` and an object `diagnostic`, and censoring includes a nonempty
  `reason`.

These events belong to the internal version-1 coordination layout, not the
CLI's public event stream. Operations return guards or terminal receipts only
after commit. `TerminalAttempt.event_sequence` identifies the committed terminal
event even when unrelated events occur between transitions. Errors roll back
all partial writes, including failure after the terminal projection changes.
Recovery uses the same terminal projection and event writer; a
delivery/subscription API remains a later slice.

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

Before requesting acceptance, the orchestrator must validate the policy-specific
completion rule and every required selected observation and artifact dependency.
For one-shot and fresh-observation execution, check that the attempt produced
its required valid result and reached its completion rule. The writer checks
that each required slot has a valid selection and a complete valid observation
produced by this attempt. These can be different observations: a retry may
produce a new result while preserving an earlier immutable selection. A stream
may reuse previously selected checkpoint slots but must reach its contracted target and
retain the producing-attempt lineage of every observation. A warm-up is never
selected as a measured observation. SQL enforces relational integrity, not
problem-owned scientific rules or the truth of filesystem verification.
The writer checks that every stream checkpoint slot has a valid selected
observation. Intermediate checkpoints may be `not_reached`; the trusted
publisher must separately validate the stream's contracted target before
requesting acceptance. Warm-up acceptance requires no measured slot membership.

## Finalization claims and recovery handoff

Move to `finalizing` before acquiring a claim. In a short transaction, verify
lease ownership and expiry, recheck slot satisfaction, create a finalization
lease, and insert its claim. The claim's primary key serializes competing
successful finalizers. Its composite foreign keys require the same attempt,
execution slot, and finalization lease. A claim cannot use a released lease,
change owners, or attach to an already accepted slot. A concurrent loser records
`superseded` in its immutable outcome instead of publishing a second success.

`begin_finalization` returns a committed `ClaimDisposition`. A loser retains
its own retry history and must publish a `TerminalOutcome::Superseded` document.
It does not create an unused finalization lease. A competitor that finishes
acceptance before this operation also causes a superseded decision. A failed
or censored claim owner can release its claim only by committing its terminal
outcome; another retry can then acquire the slot. An expired claim is retained
and cannot be renewed, finalized, or stolen through this API.

Outside the transaction, finalize the outcome document and complete manifest,
synchronize files and directories, rename from same-filesystem staging with
no replacement, and synchronize the destination parent. The final transaction
must check owner tokens, current revision, and the still-unexpired claim again,
register the verified artifact and all references, append the terminal event and
transition, update the attempt, delete the resolved claim, and release its
leases. Acceptance requires that exact attempt to own the claim when its
projection changes. Failure or censoring can finalize without a success claim.

`TerminalOutcome` covers `Accepted`, `Censored { reason }`, `Superseded`, and
`Failed { code, details }`. Failures require a nonempty stable code and object
diagnostic. `ArtifactRegistration` carries the already verified attempt
directory's identity, normalized store-relative path, content and manifest
digests, and verification time. Each additional `(role, artifact_id)` reference
must name an existing artifact. Every terminal outcome retains these roots.
The writer validates the metadata and database relationships; it does not hash,
synchronize, rename, or otherwise publish directories. Its trusted caller must
perform complete artifact and scientific validation before invoking it. The
transactional publisher below connects these responsibilities. The current public
one-shot documents still support only accepted and failed outcomes; the new
coordination outcomes do not extend those schemas or add streaming execution.

Lease expiry is a recovery signal, never an automatic deletion condition. Claims
remain unique even after expiry. Recovery must acquire the required filesystem
locks and recheck owner fencing before inspecting staging and completed paths.
It must either verify and import the claimed attempt's complete published
outcome or durably finalize it as a typed failure. Only then may it remove the
claim and permit another retry to claim that slot. The delete guard prevents
claim release while its attempt is nonterminal. Owner tokens cannot change;
renewals cannot shorten expiry, and released leases cannot become active again.
An expired owner cannot renew or publish through the writer API.

Recovering an orphan must validate its full directory, record identities,
resolved memberships, references, and completion semantics. Importing an
already accepted attempt or selected observation must respect the same unique
constraints. Recovery must never infer success from the presence of a staging
directory, a completion marker alone, an artifact row, or a claim. Replay uses
the same reservation and transition constraints, preserving every recovered
retry identity. The explicit recovery API below implements this handoff, and
the [crash conformance tests](#crash-conformance) exercise its durability
boundaries.

## Transactional publication

On Unix, `RunStore::publish_artifact` and `RunStore::publish_finalization` own the
filesystem-to-database boundary. These methods mutate the explicitly opened
store. They do not change the current CLI execution path. Recovery is an
explicit operation described below.

`publish_artifact(&attempt, kind, populate, clock)` publishes an independent
artifact. `ArtifactKind` covers observation, result, metrics, dataset, source,
environment, and other directories. The callback writes the complete payload
beneath its supplied private path and relinquishes all writers before returning.
It must validate problem-owned content before returning; inventory verification
does not establish scientific validity. The runtime owns `manifest.json` and
rejects a caller-supplied completion marker. An observation artifact registration
alone creates neither an observation row nor a selection.

```rust
use metewand_runtime::run_store::ArtifactKind;

let dataset = store.publish_artifact(
    &attempt,
    ArtifactKind::Dataset,
    |directory| std::fs::write(directory.join("data.bin"), &dataset_bytes),
    &mut clock,
)?;
```

`clock` must return actual current epoch nanoseconds as `u64` in production.
The publisher samples it before staging and after filesystem publication. It
checks the reservation, current revision, any owned claim, and staging lease
again in the final write transaction. It samples the final clock after acquiring
the SQLite write lock, so lock contention cannot preserve stale authorization. Time spent in the callback, hashing,
synchronization, and rename cannot authorize an expired owner. Timestamp samples
are clamped to the staging acquisition and preceding transition times when the
wall clock moves backward.

Staging lives in uniquely named, owner-only `artifacts/.staging-*/` directories
under the same parent as completed objects. Each stage has a committed SQLite
lease and an exclusively locked `lock` file outside its `object/` inventory.
The stage lease inherits the reservation owner and expiration; `renew` extends
live stages together with the reservation. The lock remains held through
publication and commit. No SQLite write transaction remains open during the
callback, hashing, synchronization, or rename.

The publisher validates normalized paths and the complete file inventory,
rejecting symbolic links, special files, and extra empty directories. It
computes file hashes and sizes, synchronizes every file and nested directory,
writes and synchronizes the version-1 public completion manifest last, and
synchronizes the object root. It then computes the version-1 local-tree hash of
the **entire completed directory**, including `manifest.json`, and the SHA-256
of the exact manifest bytes. The addressed path is
`artifacts/<kind>-<tree-sha256>`, with internal artifact ID
`run-store-v1:<kind>-<tree-sha256>`. These are storage identities, independent of
public typed result, dataset, observation, and attempt identities.

Publication uses atomic no-replace rename. If another writer won the address,
the publisher verifies the existing complete inventory and compares every byte
before discarding the duplicate. It independently synchronizes the reused files
and directories, verifies the complete tree hash, and synchronizes both rename
parents. An empty, incomplete, changed, or conflicting destination fails without
replacement. Identical concurrent publications share one immutable artifact row;
registration checks every indexed digest, kind, and path before reusing that
row. The final transaction registers the artifact, roots it under the live
reservation lease, and releases the staging lease together. A receipt is
returned only after commit.

`publish_finalization(&attempt, outcome, references, populate, clock)` adds the
terminal transaction. The attempt must already be finalizing. For success, call
`begin_finalization` first and honor its committed claim decision. Use
`TerminalOutcome::Superseded` for a losing contender. The callback may add logs
or validated public documents, but the runtime owns `outcome.json` as well as
`manifest.json`.

```rust
let published = store.publish_finalization(
    &finalizing_attempt,
    outcome,
    &[("dataset".into(), dataset.artifact.id.clone())],
    |directory| std::fs::write(directory.join("stderr.log"), &stderr_bytes),
    &mut clock,
)?;
let terminal_event = published.terminal.event_sequence;
```

Before writing the immutable outcome, a short transaction checks finalizing
ownership, any success claim, observation acceptance constraints, and dependency
membership. Dependency roles must be nonempty, cannot be `final`, and cannot
repeat the same artifact within a role. The canonical internal `outcome.json`
uses `run_store_version: 1` and binds:

- the attempt and resolved slot IDs, retry index, and finalizing revision;
- the terminal state and complete failure or censoring details;
- reservation start and immutable finish times as decimal nanosecond strings,
  preserving SQLite's full integer range within canonical JSON;
- sorted dependency roles, IDs, paths, storage kinds, and content and manifest
  digests; and
- snapshots of the applicable decision, resolved run, execution slot, observation
  slots, produced observations, and selected observations used by that slot.

The snapshot retains canonical domain records alongside their indexed columns.
It contains no owner tokens, lease IDs, or staging paths. The internal
[conformance vector](../crates/metewand-runtime/src/run_store/fixtures/outcome-v1.json)
pins its exact canonical bytes. This internal coordination record supplements
public one-shot documents; it does not extend their schemas with profile
outcomes. The SQLite layout and all existing public compatibility constants
remain at version 1.

The publisher takes an additional clock sample before writing the outcome to
fix its finish timestamp. After durable publication, the final transaction uses
the latest clock sample to recheck reservation, staging, and claim expiry and
revision. It also checks that the coordination snapshot still describes the
committed records. It then registers the exact attempt artifact, retains dependency
roots, appends the terminal transition and event, updates the projection with
the immutable finish time, removes the resolved claim, and releases the
attempt's leases. The artifact verification and lease-release timestamps can
therefore follow the immutable finish timestamp. All terminal outcomes use the
same publication and commit boundary.

Any failure before the final commit returns an error and no terminal receipt.
Registered stages survive errors with their lock released, and a failure after
rename may leave a fully verified orphan. Neither state implies acceptance.
After commit, staging cleanup is best effort; a leftover released stage cannot
revoke completion. Reopening the store imports neither stages nor orphans.
The explicit recovery API below handles these leftovers. Automatic observation
selection and CLI integration remain subsequent slices.

Publication tests cover exact outcome bytes, current epoch nanoseconds, backward
clock adjustments, live-stage renewal, same-filesystem placement, kernel locks,
transaction-free population, all terminal outcomes, retained dependency roots,
unsafe entries, stale revisions, missing claims and dependencies, corruption,
concurrent identical content, expiry after rename, and rollback after a terminal
projection update. Late failures leave verifiable orphans while preserving the
claim and nonterminal database state.

## Explicit recovery

On Unix, `RunStore::recover(clock)` mutates an explicitly opened store. Opening
alone never imports an orphan, marks an expired attempt successful, or allocates
a retry. The clock follows the publisher's epoch-nanosecond convention and must
read the actual current time in production.

```rust
let recovered = store.recover(&mut clock)?;
for terminal in recovered.terminals {
    // Each receipt identifies a committed terminal event.
    println!("{} {}", terminal.id, terminal.event_sequence);
}
```

Recovery takes an exclusive kernel lock on `.run-store-recovery.lock` and
rechecks expiration and revisions under SQLite write locks. Publishers take
this lock in shared mode while creating and registering a stage, closing the
interval before a stage has its own lock file or database lease. Lock contention
returns `RunStoreError::Busy`; callers may retry explicitly. Recovery also tries
each stage's kernel lock without waiting. A held lock defers that stage and its
attempt even after lease expiry. An unlocked stage with a live lease stays
available to its owner. An expired staging lease can be quarantined independently
when its reservation was renewed after that staging lease expired.

An attempt becomes recoverable when its reservation or owned finalization claim
expires or is released. Recovery never renews expired owners or changes their
tokens. For each recoverable attempt it validates completed, unregistered
`artifacts/attempt-<tree-sha256>` directories, including:

- the complete manifest, exact file inventory, content address, and canonical
  version-1 coordination outcome;
- the attempt identity derived from the resolved slot and retry index, current
  finalizing revision, reservation start, and immutable finish timestamp;
- exact coordination records, resolved memberships, observation selections, and
  dependency metadata against the authoritative database snapshot; and
- the content, completion manifests, storage kinds, and safe paths of referenced
  artifacts, produced and selected observations, and their artifact roots.

Acceptance additionally requires the original claim, owned by that attempt's
reservation owner, to have authorized the immutable finish time. Recovery uses
the writer's policy-specific observation acceptance checks. As with normal
publication, the trusted orchestrator must have validated problem-owned
scientific rules before publishing the outcome; recovery does not rerun workers
or infer scientific success from a completion marker. Missing dependencies,
changed records, invalid identities, missing claims, and ambiguous completed
outcomes cannot establish acceptance.

When exactly one completed outcome passes these checks, recovery synchronizes
its files and directories and their publication parent. Its final transaction
rechecks the coordination evidence and expired owner, registers the exact
artifact and roots, appends the terminal transition and event, releases the
resolved claim, and releases the attempt's leases. The immutable finish time and
original accepted, failed, censored, or superseded outcome survive import.
The existing uniqueness constraints still permit at most one accepted attempt
per execution slot and one selected observation per observation slot.

Without an unambiguous verified outcome, recovery moves the expired attempt to
`finalizing` if needed, then publishes a canonical `Failed` outcome with code
`interrupted_attempt`. It retains the attempt's leased artifact roots and commits
failure before releasing its claim. Its private recovery stages use the global
kernel lock for their lifetime. A crash after publishing this failure but before
commit leaves another recoverable orphan. The
[recovered-failure conformance vector](../crates/metewand-runtime/src/run_store/fixtures/recovered-failure-v1.json)
pins the canonical bytes for an expired reservation. The schema and public
compatibility constants remain at version 1; recovery reuses their existing
representations and constraints.

Recovery atomically moves abandoned stages intact to
`artifacts/.quarantine/<original-stage-name>` and synchronizes both parents before
releasing their staging leases. It also handles stages left before lease
registration, stages whose cleanup was interrupted after a successful commit,
and missing stages whose database leases remain. A complete but unpublished
`object/` inside staging never establishes success. Repeating recovery after a
quarantine rename or terminal commit is safe.

Independent completed orphans can be registered as verified artifact bytes.
This creates no observation, selection, or accepted attempt. Recovery preserves
existing selections and valid observations when their producing attempt fails.
It does not reconstruct a missing database from outcome snapshots, restore
implementation state, start workers, or allocate retries. Resuming failed slots
still requires an explicit retry authorization and a new attempt identity.

`RecoveryReport` returns committed `terminals`, independently
`imported_artifacts`, store-relative `quarantined_stages` and `busy_stages`, and
`rejected_artifacts` with paths and diagnostic reasons. Rejected completed
objects stay on disk for inspection. Publication, synchronization, unsafe stage
paths, and database failures return an error; transactions already committed earlier
in a pass remain committed. Repeating the operation resolves surviving work
without duplicating terminal events or acceptance.

Recovery tests cover expiration at the exact boundary, live-owner preservation,
expired claims, active process locks, process death before and after publication,
unregistered and complete staging directories, independent orphans, all terminal
outcomes, corrupted dependencies, changed metadata, missing claims, ambiguous
outcomes, preserved observations, concurrent recoverers, and transaction rollback
followed by import of the same immutable orphan.

## Crash conformance

The runtime's private test build traces each SQLite statement before execution
and after completion, including initialization, transaction begin, commit,
rollback, and the individual writes inside a transaction. Shared helpers expose
crash points before and after every explicit file and directory synchronization
and no-replace rename in run-store initialization, publication, cleanup, and
recovery. Additional points surround finalization, outcome construction, and
terminal projection. These hooks have no production configuration or public
API. The SQLite trace dependency is enabled only for test builds.

Each subprocess campaign prepares one closed store fixture, records a successful
execution, then repeats it in a fresh copy for every numbered boundary. Copies
preserve staging lease names and their quarantine order. Each child writes its
boundary trace and exits immediately at that point without running Rust
destructors or rolling back SQLite explicitly. The parent checks the exact trace
prefix, so a changed execution path cannot silently count as a tested crash point.
Campaigns cover initialization, all terminal outcomes, execution transitions,
lease renewal, identical-content reuse, observation registration and selection,
expired reservations and claims, independent completed orphans, accepted
outcome import, recovery failure publication, and staging quarantine. Recovery
itself is interrupted and repeated through the same campaign.

After every crash, the tests reopen and recover the store. They require each
verified, claimed published success to survive, reject acceptance without that
evidence, check terminal documents against database identities and digests,
preserve existing selections, resolve claims and leases, and verify SQLite
integrity and foreign keys. A second recovery must add no terminal records or
artifacts. An explicit authorized retry fills an unsatisfied slot while a
satisfied slot refuses another reservation. At most one attempt becomes
accepted for the slot. Separate synchronized subprocesses publish identical
content and must converge on one complete object with both lease roots. The
same race also kills the winning publisher immediately after its successful
rename and before parent synchronization; the survivor must finish verification
and publication, and recovery must retain its bytes.

Run these tests directly with:

```sh
cargo test -p metewand-runtime --lib --locked run_store::fault_tests -- --nocapture
```

They also run in the workspace suite and `devenv test`. The campaign models
abrupt process death around application and SQLite statement boundaries; it
does not simulate a storage device losing power. The schema, identities,
canonical bytes, and public compatibility constants remain at version 1.

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

Initialization tests cover fresh and repeated opens, all required connection
settings, history retention, concurrent threads and processes, unsupported WAL,
unknown or altered schemas, unsafe paths, and refusal on injected locking,
file-sync, directory-sync, and rename failures. Failures at later capability
boundaries never return a store handle, and reopening checks the surviving
layout without manufacturing events.

Transactional writer tests exercise every execution phase and terminal outcome,
exact canonical event bytes, backward clock adjustments, stale guards, expired
and released owners, retry authorization, duplicate lease rollback, missing
artifact roots, and rollback after a terminal projection update. Independent
connections race to allocate a retry and acquire a success claim. Acceptance
tests cover missing, invalid, unselected, and incomplete observations, retained
stream checkpoints, and successful retries that preserve earlier selections.
Reopening verifies committed terminal state, history, artifacts, and claim
release without replaying events or inferring acceptance from directories.
