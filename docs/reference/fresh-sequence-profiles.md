# Fresh-sequence profiles

A fresh sequence calls an implementation at finite operating points, starting
from clean algorithmic state each time. Metewand independently evaluates every
returned result. This supports black-box solvers that expose an iteration limit
or tolerance but have no observation callback.

The [runnable Python and R example](https://github.com/jolars/metewand/tree/main/examples/fresh-sequence)
compares bisection with `stats::uniroot` using a shared residual evaluator.
Their controls do not represent equivalent scientific work.

## Declaring controls and completion

Declare `capabilities: [fresh_sequence]` on the implementation and select its
binding under a profile policy:

```yaml
observation-policies:
  accuracy:
    kind: profile
    metric: residual
    direction: at_most
    threshold: 0.000001
    max-observations: 4
    max-active-time: 10 s
    implementations:
      solver:
        delivery: fresh_sequence
        control-schema:
          type: object
          properties: { iterations: { type: integer, minimum: 1 } }
          required: [iterations]
          additionalProperties: false
        schedule:
          kind: explicit
          controls: [{ iterations: 1 }, { iterations: 4 }, { iterations: 16 }, { iterations: 32 }]
```

A control schema can be inline or a referenced JSON Schema. Explicit schedules
contain nonempty arrays of control objects. Generated schedules set one named
field in each object:

```yaml
schedule: { kind: linear, field: iterations, start: 1, step: 5, count: 4 }
schedule: { kind: geometric, field: tolerance, start: 0.1, ratio: 0.01, count: 4 }
```

Linear expansion repeatedly adds `step`; geometric expansion repeatedly
multiplies by `ratio`. Metewand canonicalizes and validates every expanded
control, rejecting unsafe or nonfinite arithmetic. Repeated controls are
separate schedule positions. No monotonic improvement in scientific quality is
assumed.

For fixed counts, omit `metric`, `direction`, and `threshold`. Every binding's
schedule must have exactly `max-observations` entries. A target profile permits
shorter schedules up to that bound. Exhausting a schedule without reaching the
target censors the repetition. Targets name a top-level numeric evaluator
metric; comparisons include equality in either direction.

One policy can mix fresh and buffered bindings. Existing trace and byte limits
are required when a buffered binding is present. They do not constrain fresh
result collection. `record-thresholds` controls buffered capture selection;
fresh delivery retains every independently valid returned point.

## Clean execution and timing

Each point receives `prepare` with the specification's implementation seed.
Eligible reused workers run `reset` and clear writable caches before another
point. A new or replacement worker performs the execution policy's warm-ups,
using the control of the point that triggers them. Warm-ups do not satisfy
measured slots or contribute to the profile time budget. A failed warm-up or
reset discards the worker and fails the pending point.

The curve coordinate is that point's own executor-observed `timed_wall_time`,
under the selected timing scope. Result serialization remains included.
Independent evaluation, reset, and excluded preparation retain their existing
separate accounting. Diagnostic in-worker time does not replace the curve
coordinate.

`max-active-time` limits cumulative measured work across one measurement
repetition, including failed points and explicit retries. It persists across
resume. Each point receives the smaller of its execution-policy timeout and
remaining profile allowance. Evaluation and warm-ups have separate operational
limits and are excluded from this budget.

Before timed execution, Metewand durably reserves that allowance. Terminal
commit refunds measured unused time. If interruption loses timing evidence,
recovery retains the reserved charge. `conservative_time_ns` identifies charges
without corresponding measured work; they never become plotted observations.
An active reservation can also contribute to that field before settlement.
Budget evidence names the bounded charge `charged_ns`; the full observed
interval, including a partial interval after timeout, remains in attempt timing.

Randomized comparisons interleave implementations by schedule round within
comparison blocks. At most one point from a repetition runs at once, even
across concurrent invocations. Batch boundaries permit independent evaluation
and a stopping decision before another point from that repetition starts.

## Results, stopping, and resume

Validity and completion are separate. An invalid evaluator verdict fails the
point. Other verdicts permit profile evaluation from the independent metric,
without applying the contract's one-shot completion threshold. A valid
below-target result satisfies its own slot and remains selected.

Technical failures retain their attempts while later controls continue within
the remaining budget. A repetition is accepted when its target is reached or
all fixed-count slots are satisfied. Schedule or time exhaustion without
completion is censored. Selected execution that leaves other work unattempted
is partial. A censored repetition makes the run unsuccessful even when all its
individual returned points were valid.

Target acceptance atomically marks the repetition's remaining unsatisfied
slots `skipped_target_reached`. These slots retain any earlier failed history;
Metewand creates no new attempt or observation for the skip. Target completion
also prevents `--retry` from reopening earlier failures in that repetition.

Selecting a single observation or attempt slot runs only that point. Its full
repetition membership remains inspectable. `--max-attempts` counts allocated
attempts, excluding skipped slots. Resume preserves accepted points and stopping
decisions; failed slots require `--retry`, and retries cannot replenish time.
A schedule-exhausted repetition with failed slots can become accepted after
eligible retries satisfy them within the remaining budget.

## Protocol, records, and compatibility

Fresh workers negotiate `fresh_sequence` under wire protocol 1. The existing
`execute` request adds an `observation_control` object, and its successful response
must echo the selected canonical value. Missing, unsolicited, or changed echoes
fail the exchange. One-shot requests and responses retain their original shape.
Raw workers can implement this directly without an SDK.

Python opts in with `serve_implementation(worker, fresh_sequence=True)`; its
existing `execute(budget, observation_control)` receives the control. R uses
`serve_implementation(..., fresh_sequence=TRUE)` and forwards it to
`execute(state, budget, observation_control)`. SDKs preserve the echo before
callbacks can mutate their argument. The budget argument remains `None` or
`NULL`; problem-defined scientific budgets are a separate future interface.
The Rust worker SDK does not yet expose a fresh execution callback.

Plans expose both repetition and schedule indices, expanded controls, and their
stable slot identities. Schedules, control schemas, and selected bindings enter
dependent identities. Editing another implementation's binding leaves an
unchanged candidate's identity intact.

`run` and `status` expose typed `fresh_sequences` progress. `explain` retains
controls, stopping evidence, budget reservations, and producing-attempt lineage.
JSONL and CSV exports retain those fields in `profile` and `evidence`, alongside
independent metrics and timing. Skipped `slot` rows have null metric and attempt
coordinates. Compatibility groups identify executor-observed fresh delivery
separately from buffered instrumentation. Group curves by resolved specification
and measurement index, retaining each point's producing attempt.

Runs containing fresh delivery initialize SQLite layout 3, including mixed
fresh and buffered runs. Existing layouts 1 and 2 remain readable and resumable
without migration. Adding fresh delivery requires a new output directory.
Manifest, public-schema, identity, and wire versions remain 1, and execution
semantics remain 2. Existing one-shot and buffered identity transcripts retain
their exact representation.
