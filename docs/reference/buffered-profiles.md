# Buffered instrumented profiles

A buffered profile runs one solver trajectory and returns a bounded local trace
and selected immutable results. Metewand independently evaluates saved results
after execution. Trace points without a saved result remain diagnostic evidence.
The [runnable example](https://github.com/jolars/metewand/tree/main/examples/buffered-profile)
includes Python observers, direct recording, and a native Rust worker.

## Declaring a profile

Select `kind: profile` in an observation policy and declare
`capabilities: [buffered_profile]` on its implementations. Each selected
implementation needs a binding under the policy's `implementations` mapping:

```yaml
observation-policies:
  accuracy:
    kind: profile
    metric: gap
    direction: at_most
    threshold: 0.0001
    record-thresholds: [0.5, 0.05, 0.0001]
    max-observations: 3
    max-trace-points: 1000
    max-buffer-bytes: 65536
    terminal-reserve-bytes: 8192
    max-active-time: 1 s
    implementations:
      solver:
        delivery: buffered_profile
        measurement-basis: instrumented
        instrumented-scope: complete_solver_call
        instrumentation-schema:
          type: object
          properties: { observer: { const: gap } }
          required: [observer]
          additionalProperties: false
        instrumentation: { observer: gap }
        check-schedule: { every: 5, start: 0 }
```

Targets name a top-level numeric property in the problem's metric schema.
`at_most` and `at_least` define the comparison direction. Recording thresholds
must be finite, distinct, ordered from least to most demanding, and end at the
completion threshold. Omit `record-thresholds` to save only the target crossing
and terminal result. One capture serves all thresholds crossed at the same
check, and Metewand evaluates that capture once.

For a fixed-count profile, omit `metric`, `direction`, `threshold`, and
`record-thresholds`. Metewand requires `max-observations` valid snapshots, one
per checked iterate. A solver that terminates before that count is censored.

The initial schedule supports a positive integer interval and a nonnegative
start, which defaults to zero. Progress indices identify successive solver
boundaries. Regular checks must follow the declared schedule. Termination
between scheduled checks adds a terminal check at the current boundary.

Instrumentation schemas may be inline or referenced files. Instrumentation,
schedule, selected scope, thresholds, and bounds contribute to dependent run
identities. Editing another implementation's binding does not invalidate an
unchanged candidate. Scientific budgets beyond `none` and streaming remain
unsupported. The same policy can also bind [fresh-sequence delivery](fresh-sequence-profiles.md).

## Authorizing measurement scopes

Problem contracts must explicitly authorize each instrumented scope:

```yaml
instrumented-scopes:
  complete_solver_call:
    clock-origin: Before solver initialization.
    included-work: Initialization, updates, measurements, callbacks, and snapshots.
    excluded-preparation: Input loading and final buffer file serialization.
    metric-agreement:
      gap: { atol: 1e-12, rtol: 1e-8 }
```

The three descriptions are required. `metric-agreement` is optional. When
present, both local and independent metrics must exist and satisfy
`abs(local - evaluated) <= atol + rtol * abs(evaluated)`. A violation fails the
attempt while retaining any independently valid observations already published.

The local clock reports nonnegative, nondecreasing integer nanoseconds. A
timestamp follows metric computation and precedes that point's snapshot.
Measurement, dispatch, and earlier snapshot work remain in subsequent times.
The total instrumented duration includes terminal snapshot work. Final buffer
serialization remains inside the executor's selected timing envelope.

The executor retains its independently observed duration and enforces the
execution policy's scientific deadline, including preparation when selected and
buffer serialization. Local active-time limits are cooperative checks; a long
iteration or measurement still needs the executor deadline.

## Local observer and direct recording

Python adapters subclass `BufferedImplementation` and implement
`execute_buffered(self, budget, trace)`. The existing `prepare`, `reset`, and
`shutdown` lifecycle remains available. `budget` is currently `None`.

```python
decision = trace.observe(
    {"check_index": iteration, "work": {"updates": iteration}},
    lambda: {"gap": compute_gap(state)},
    lambda: {"coefficients": state.coefficients.tolist()},
)
if decision.stop:
    # Finalize this checked state before performing another update.
    ...
```

`Decision.reason` distinguishes target, fixed-count, trace, observation, byte,
and active-time stops. `trace.record(progress, metrics, snapshot)` accepts
already computed local metrics. The observer calls `measure` only at required
checks. Both interfaces call `snapshot` lazily and freeze its returned JSON
object immediately. Measurement and capture must leave algorithmic state,
random-number streams, and update schedules unchanged.

Call `trace.finish(progress, measure, snapshot)` before returning. It measures
and captures an unchecked terminal state, or reuses an already checked state
without assigning a new timestamp. Recording after a stop or advancing past it
violates the contract. The SDK writes the completed buffer after the callback
returns.

Rust implementations override `supports_buffered_profile` and
`execute_buffered(&mut self, &mut BufferedTrace)`. A buffered-only worker also
returns `false` from `supports_one_shot`; it needs no one-shot execute method.
Rust uses `Progress` and `Decision::Continue` or `Decision::Stop(reason)` with
the same collection rules.

Native solvers may write the equivalent buffer beneath `trace.directory` and
call `trace.adopt_buffer("buffer.json")` after execution. Native code must
enforce collection bounds while solving and capture selected results at their
checked iterates. This requires no Python callback or protocol exchange at
each observation. Replaying metric-only arrays cannot supply missing snapshots
or independently verified observations.

## Bounds, evaluation, and recovery

All count and byte bounds apply during collection. The collector leaves one
trace position, one snapshot position, and `terminal-reserve-bytes` available
for termination. If regular capacity runs out, it measures and saves the
current state and stops explicitly. If terminal output exceeds the total byte
limit, the worker reports `resource_limit`. Entries are never silently dropped.
Choose a reserve large enough for the result, terminal metrics, and inventory.

The completed version-1 buffer contains a JSONL trace, canonical snapshot
manifests, threshold references, clock and scope metadata, total instrumented
time, a stop reason, and a complete file inventory with sizes and hashes.
Paths must be relative and normalized; symbolic links are rejected. The worker
closes all files before returning its `execute_buffered` response.

Metewand validates and freezes the complete buffer before evaluating any
snapshot. Independent metrics determine target attainment. A locally claimed
target that is not confirmed becomes `censored` unless a contract violation
requires failure. Fixed counts require all declared observations. A valid
below-target result remains available.

Each evaluated valid snapshot is published independently. A later evaluation
or publication failure preserves that prefix. Selection commits with one
accepted terminal attempt, so a retry cannot splice curves. Failed and censored
attempts retain their own unselected results. A crash before a complete buffer
response produces no accepted trace or observations, even if files were written.

Runs containing buffered profiles use SQLite run-store version 2, or version 3
when mixed with fresh sequences. Existing
version-1 one-shot stores remain readable and resumable without migration.
Adding profiles requires a fresh output directory.

## Inspection and exports

`plan` exposes the selected profile binding and bounded capture slots. `run`
reports profile observation arrays and censored attempts. `status` and `explain`
retain failure evidence, producing-attempt identities, and artifact paths.

JSONL and CSV exports retain `instrumented_time_ns`, total instrumented time,
executor time, scope, independent metrics, and separately named provisional
metrics. Diagnostic `trace` rows have no independent metric coordinate.
Snapshot `observation` rows retain evaluator metrics and their local evidence.
Compatibility groups include the measurement basis and scope definition.

Threshold references describe first *observed* crossings. Independent evaluation
checks saved scientific results; it does not verify local timestamps or prove
that an earlier unsaved iterate failed the target.
