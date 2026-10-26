# Trusted local execution

`metewand run` connects the checked manifest and logical plan to the Gate-1
[fixed-dataset runtime](fixed-dataset-run.md) and
[transactional run store](run-store-schema.md):

```sh
cargo run --locked -p metewand-cli -- run \
  --manifest examples/fixed-dataset/metewand.yaml \
  --output-dir /tmp/metewand-example \
  --output jsonl
```

The destination must be new, and its parent must exist. With `--resume`, it must
instead be an existing durable run directory. Without that flag, the command
refuses an existing file, directory, or symlink without changing it. `run` authorizes
worker launches, private temporary input copies, and publication beneath this
explicit destination. Optional `--cache-dir PATH` assigns a separate directory to
Metewand for persistent [artifact storage](artifact-store.md) and construction
reuse; its parent must exist. Without it, the run uses a temporary store. It
never rewrites the manifest or original source files.
Persistent runs create [cache roots](cache-maintenance.md) for resolved datasets
before registering their run-store slots. Those roots remain until explicitly
released, including after failed runs or removal of the output directory.
Manifest paths resolve from the invoking directory; declared inputs resolve
from the manifest's parent. The output directory resolves from the invoking
directory and must lie outside declared source directories and glob prefixes.

The [runnable example](https://github.com/jolars/metewand/tree/main/examples/fixed-dataset) uses Python's standard library
to implement the raw protocol without an SDK. It sums three fixed integers,
adds the problem-owned offset, and independently evaluates the answer. The
expected result is `{"answer":8}` and the metrics are `{"absolute_error":0}`.

## Supported selection

The command executes every selected candidate, implementation repetition, and
measurement repetition. It supports fixed
local datasets, local generators and transformations with pinned HTTP(S) sources,
the built-in empty unit dataset for dataset-free problems,
one-shot and [buffered profile](buffered-profiles.md) workers, local environments, dedicated POSIX protocol pipes, fresh
or reused implementation processes, process-local warm-ups, all three
contract-authorized wall-timing scopes, sequential or block-randomized order, and
`best_effort` enforcement. Complete preflight precedes every worker launch and
creation of the output directory. Overlapping resolved attempt slots across
experiments are rejected; distinct experiment seeds select independent runs.

The default `run-order: "sequential"` follows logical-plan order. Select
`run-order: "randomized"` to interleave implementation configurations within
comparison blocks defined by dataset instance, problem configuration,
implementation repetition, and measurement index. Both modes execute serially
by default. Set `max-concurrency: 2` in the execution policy to opt into
bounded concurrent attempts. The bound is part of the policy and run identities
and is recorded in every attempt's provenance. Dispatch batches preserve
deterministic event order and never cross experiment boundaries.
Events and attempts follow execution order, and attempt provenance retains the
block fields, seed, and deterministic priorities. See [Scheduling](scheduling.md)
for the versioned ordering contract and warm-up behavior.

Set `worker-reuse: true` to retain an eligible implementation process across
measurement and implementation repetitions. Set `warmup-runs` to a nonnegative
count to run validated, untimed warm-ups on that process first. Every boundary
calls `reset` and clears private writable directories, including filesystem
caches. A replacement repeats its warm-ups. Warm-up results and failures are
recorded in the enclosing attempt's provenance and do not increase the measured
observation count. See [worker reuse](worker-reuse.md) for lifecycle details.
Compatibility checking rejects warm-ups without reuse and cold timing with reuse
or warm-ups.

CPU, thread, and memory requests execute in best-effort mode and are recorded
as unsupported by the local backend. Required enforcement fails logical
planning because the local backend cannot guarantee lifetime isolation.
See [Execution controls](execution-controls.md) for the complete report and
runtime failure behavior.
Provisioning backends, standard-I/O protocol transport,
symbolic links in copied inputs and locked execution are also rejected.
These features belong to later roadmap slices. A rejected selection emits a
structured diagnostic without launching any worker or creating the destination.

The execution policy chooses `cold_end_to_end`, `prepare_and_execute`, or
`execute_only`; the selected problem contract must allow that scope. Cold
timing includes implementation launch and startup. Prepare-and-execute starts
after startup. Execute-only excludes preparation, requires contract-defined
untimed preparation rules, and labels its provenance as an operation-scope
comparison. All three include execution and canonical result serialization.
Shared dataset materialization and independent evaluation remain outside them.
See [Timing envelopes](fixed-dataset-run.md).

The optional execution-policy `timeout` is one scientific wall-time budget over
the complete selected scope. It accepts a positive integer followed by `ms`,
`s`, `min`, or `h`, with optional intervening whitespace. Omitting it adds no
scientific limit. Separate operational defaults bound each worker phase to ten
seconds, including materialization, hello, prepare, execute, evaluation, reset,
and shutdown. A scientific budget does not lengthen or shorten untimed phase
limits. Runtime API callers can supply custom `PhaseTimeouts`. Scientific
expiry becomes a typed `scientific_timeout` attempt; operational expiry uses
`phase_timeout`. A failed measurement never publishes a scientific observation.
The published context records the selected scope and both kinds of limits,
including on failed attempts.

Published attempts and observations retain [phase and resource
measurements](execution-measurements.md), including explicit missing values and
partial elapsed time on failure. Linux CPU and peak-memory measurements cover
the implementation process and are marked partial because they exclude its
descendants.

Generated datasets execute the materializer protocol before any measurements.
Each dataset configuration is constructed once per run or recovered from a
verified persistent cache mapping, validated against its output schema, and
recorded with its derivation identity and separate output hash. Every attempt
receives its own verified dataset copy, including fixed and unit datasets. See
[Dataset materialization](dataset-materialization.md) and its
[runnable example](https://github.com/jolars/metewand/tree/main/examples/generated-dataset). External providers and optional
contract-owned binary checkers are explicit runtime APIs; the current manifest
does not select those integrations.
The optional [Datamonger example](https://github.com/jolars/metewand/tree/main/examples/datamonger) demonstrates external
resolution and shared materialization through those runtime APIs.

## Inputs and launches

Fixed datasets are copied with their repository-relative paths intact. For
example, `sources: ["data.json"]` exposes `data.json` directly under
`dataset_dir`; `sources: ["data/train.json"]` exposes `data/train.json`.
Metewand adds a canonical `dataset-manifest.json` containing the complete file
inventory and validates it against the dataset's output schema. That filename
is reserved for the generated inventory. Dataset-free workers receive an empty
directory for the versioned unit artifact.

Workers likewise receive private copies of their declared source bundles.
Interpreted runners resolve their entrypoints within those copies. A command
program containing a relative path, such as `bin/worker` or `./worker`, resolves
within its declared sources. A bare program name resolves through the invoking
process's `PATH`; absolute external programs are also supported. Repository
scripts may use a single-interpreter shebang, including `/usr/bin/env python3`.
More complex shebangs require an explicit interpreter launch.

Command arguments remain literal unless a separate argument starts with
`{sources}/`, which explicitly refers to a normalized declared source path:

```yaml
runner: "command"
program: "python3"
args: ["{sources}/worker.py", "implementation"]
sources: ["worker.py"]
environment: "local"
```

The same source-reference syntax is available in interpreted workers' additional
arguments. Metewand does not infer file references from arbitrary argument text.
References must exist in the selected bundle and cannot escape it. Embedded
workspace and temporary paths are rejected rather than included in identities.

Workers receive the resolved allowlist, including fixed locale and timezone,
Python user-site and bytecode settings, a minimal `PATH`, and the transport's
descriptor variables. Their [worker environments](worker-environments.md) use
umask `0077` and private working, home, temp, configuration, and cache directories.
Resolved launch identities bind stable path templates; attempt provenance records
the allocated paths. This remains a trusted local
development path with observed provenance. Network isolation, undeclared-file
access prevention, process-tree containment, and comprehensive accounting are
unsupported and are reported as such.

Input artifact IDs use the version-1 local-tree transcript and canonical
`{"tree_hash_version":1,"tree_sha256":"<digest>"}` representation. Resolved
launch identities retain `{sources}/PATH` references, content-addressed external
program selectors (`sha256:<digest>`), and environment path templates. The local
environment fingerprint binds the program's SHA-256, platform, and explicit
absence of a complete dependency closure. The resolver substitutes actual paths
only for execution and provenance. Moving a repository, choosing another output
directory, or allocating another temporary directory therefore preserves logical
and resolved identities. Execution input changes invalidate their dependencies.

The runtime checks original declared bundles, worker source copies, and executable
contents before and after each attempt. Detected changes produce `input_changed`.
It verifies private dataset copies and their canonical cache objects immediately
before and after each worker operation, including hello, failed exchanges, and
shutdown. Dataset changes produce `dataset_mutation` and prevent observation
publication, even if a later operation would restore the original bytes. A
changed canonical object is atomically quarantined under the cache lock before
another attempt can reuse it. Later slots continue with fresh private copies if
the canonical dataset still verifies; otherwise, they fail before worker launch.

Successful and failed attempt provenance records `dataset_immutability` with
`view: "copy"`, `enforcement: "best_effort"`, and
`verification: "each_worker_operation"`. The local executor cannot prevent its
OS user from accessing undeclared paths or restoring bytes within one operation.
The `prepare_and_execute` timer remains continuous, including result writing and
integrity checks between the two exchanges. Execution provenance records that
overhead as `timing_dataset_integrity: "included_between_prepare_and_execute"`.
Durable phase transitions also occur between preparation and execution. Their
coordination overhead is included in cold and prepare-and-execute wall timing,
excluded from execute-only timing, and labeled in `timing_coordination`.

## Events and final output

The published `run-output` schema refines the common `machine-output` envelope.
`jsonl` flushes each event before execution advances. `json` emits one terminal
envelope containing the same ordered events. Both final reports contain the
benchmark name, manifest hash, output directory, attempt summaries, and counts.
Human progress goes to standard error, and its final report uses those same
typed records. Worker logs never enter command output.

Every event has `command: "run"`, a zero-based `sequence`, and one transition:

| Event | Additional data |
| --- | --- |
| `run_started` | Total measured `attempt_slots`. |
| `attempt_started` | `logical_slot_id` and `resolved_slot_id`. |
| `attempt_finished` | The finalized `attempt` summary. |
| `run_finished` | `accepted_attempts`, `failed_attempts`, and `superseded_attempts`. |

An attempt summary contains its logical and resolved slot IDs, attempt ID,
`accepted`, `failed`, or `superseded` status, relative attempt directory, optional
observation ID and directory,
validated result and metrics, and an optional diagnostic. Inapplicable values
are explicit nulls. The final report repeats the emitted events in `events` so
JSON and JSON Lines consumers can inspect the same lifecycle.

`attempt_finished` follows durable publication. A technical failure publishes
an attempt with no observation. A valid result that misses its scientific target
still publishes its observation and records `completion_target_not_reached` on
the failed attempt. Failed attempts do not cancel the remaining selection.
Their diagnostics distinguish implementation failure or crash, phase timeout,
invalid result, evaluator failure, protocol error, setup failure, input changes,
and dataset mutation. The final report uses exit code `5` if any attempt failed;
the same diagnostics and affected IDs appear as human text on standard error.

Successful or scientifically incomplete runs retain the existing publisher's
scoped implementation and evaluator logs. Technical failures currently retain
the typed diagnostic and resolution provenance, with `logs_available: false`.
The lower-level runtime discards those failed workers' private log buffers.

Cache failures use `artifact_cache_failed` with exit code `74`. A corrupted or
incomplete content object fails verification and is moved into quarantine without
being reused. The invocation that discovers corruption during source resolution
stops before creating run output. A later invocation can reconstruct the missing
object, but its derivation mapping still requires the original output hash.
Corrupted mapping metadata also fails verification.

Preflight or publication errors produce a terminal diagnostic envelope, following
any events already streamed in JSON Lines mode. Publication failure never emits
an accepted terminal event. A previously finalized observation may remain as an
orphan, as described by the publisher. Closing the event pipe stops further
execution and returns successful cancellation; already published artifacts remain.

## Selection, bounds, retries, and resume

Use `--select ID` to execute the measured slots associated with a stable logical
candidate, logical or resolved run, logical or resolved observation slot, or
logical or resolved attempt slot. Repeat the flag to select their union.
Recorded attempt and observation IDs can also select their producing slot when
using `--resume`. Unknown IDs fail with `invalid_run_selection`; filters never
silently expand to the complete benchmark. A selection containing only logical
IDs narrows the plan before acquisition. Resolving other IDs may require dataset
construction and launch resolution before filtering. Checked applicability IDs
select their candidate's slots. Selecting a candidate that resolves to
`not_applicable` retains the exclusion without executing a measured attempt.
Unconditional runs also recognize the store's existing
`run-store-v1:unconditional:<logical-candidate-id>` decision key.

`--max-attempts N` limits the new measured attempts allocated by this invocation.
It accepts a positive integer up to `9007199254740991`. Filtering preserves the
scheduler's relative order. Eligible slots are then bounded and divided into
concurrency batches within each experiment. Warm-ups remain part of their
measured attempt and do not consume additional entries in this limit. Invocation
bounds and filters do not change the scientific policy, seeds, or slot identities.

```sh
metewand run --output-dir results --select mw1-logical-candidate-<digest> --max-attempts 2
metewand run --output-dir results --resume --max-attempts 2
metewand run --output-dir results --resume --retry --select mw1-attempt-<digest>
```

`--resume` opens `run.sqlite3`, recovers expired reservations and finalization
claims, and fills unsatisfied slots from the current resolved plan. It always
skips accepted slots. Changing a declared input can produce new resolved slots;
the prior attempts stay intact. Resume does not restore implementation process
state. Replacement workers repeat their contracted warm-ups.

Previous failures require `--retry`. This flag authorizes one additional attempt
per selected unsatisfied slot in this invocation, including an explicitly
requested concurrent candidate for a live slot. It does not create an automatic
retry loop. Every reservation allocates the next retry index transactionally,
so every retry has a distinct attempt ID and immutable directory. Accepted slots
remain protected even when `--retry` is present.

Processes renew their 30-second ownership leases every five seconds. A killed
owner can be recovered after its lease expires. A claim alone never satisfies a
slot: the terminal database transaction follows durable artifact publication.
Concurrent successful candidates compete for that claim before writing their
immutable attempt documents. One candidate is accepted; a loser publishes a
`superseded` attempt and retains its valid, unselected observation. Existing valid
observation selections are never replaced.

The run directory contains `run.sqlite3`, coordination lock files, and immutable
`artifacts/observation-<content-hash>` and `artifacts/attempt-<content-hash>`
directories. Executed attempts include both the public `attempt.json` and the
canonical coordination `outcome.json`. Recovery-generated interruption failures
retain their self-describing coordination outcome. Reports give their relative paths; clients
should use those paths instead of constructing directory names from record IDs.
SQLite records and finalized artifacts retain all retries across invocations.
The JSON and JSON Lines event sequence and final counts describe the current
invocation. A resume with no eligible slots succeeds with an empty `attempts`
array and only `run_started` and `run_finished` events.

## Compatibility

The unreleased public-schema version 1 now includes superseded one-shot outcomes,
explicit superseded counts, and empty resume reports. Its conformance fixtures
cover these cases. The manifest, identity, seed, scheduling, run-store, artifact,
and wire contracts remain at version 1. Execution semantics remain at version 2:
selection and resume use its existing policy-bounded batch order, and retries use
the existing slot-plus-retry identity transcript. They do not redefine timing
scopes or scientific policies. These compatibility constants were reviewed for
this slice. End-to-end tests cover selection, bounds, immutable retries,
concurrent finalization, interruption recovery, and accepted-slot preservation.

A materializer failure occurs before resolved attempt identities exist. It emits
`dataset_materialization_failed` with exit code `5`, removes private construction
output, and leaves the requested run destination uncreated.
