# Choose how the comparison runs

An execution policy controls what Metewand times, the order in which methods
run, and how long they may take. Start with the defaults, then change the
settings that your scientific comparison needs.

## Choose what to time

The default `prepare_and_execute` timer includes loading the shared JSON data,
solving, and serializing the result. It also includes coordination and integrity
checks between preparation and execution. Worker startup and imports, shared
dataset generation, independent evaluation, and saving the durable records
happen outside that interval.

The problem decides which timing scopes are allowed:

  | Scope                 | What it includes                                                      |
  | --------------------- | --------------------------------------------------------------------- |
  | `cold_end_to_end`     | Startup, preparation, solving, and result serialization.              |
  | `prepare_and_execute` | Preparation, solving, and result serialization.                       |
  | `execute_only`        | Solving and result serialization, with permitted untimed preparation. |

The least-squares problem in this guide allows only the default. To allow
another scope, set `allowed-timing-scopes` on the problem and describe what
preparation may do outside the timer. Use [separate lifecycle
callbacks](extending.md#separate-preparation-and-solving) when you need a
precise boundary between preparation and solving.

## Time the solver callback

A small solver may finish faster than Metewand can convert and write its answer.
To measure just the worker's `execute` callback, add:

```yaml
execution-policies:
  solver:
    timing-scope: execute_only
    primary-time: implementation_time
    timeout: 30 s
```

Then select `execution-policy: solver` in the experiment. The problem must allow
`execute_only` and define which preparation can happen outside the timer. Put
method-specific setup inside `execute` when the comparison should charge for it.

With lifecycle workers, `execute` can return a solver object and the optional
`result` callback can convert it to JSON-compatible values. Metewand's later
conversion, encoding, writing, and transport are outside callback time.
Conversion that you do inside `execute` still counts.

Python, R, and Rust adapters report this measurement as
`implementation_time_ns`. R uses elapsed `proc.time()`; nanosecond units do not
imply nanosecond resolution. Custom workers must report the measurement
themselves. If it is missing, the attempt fails with
`missing_implementation_time`.

This choice is available for one-shot and fresh-sequence comparisons. Buffered
and streaming profiles use their own declared clocks. Metewand also retains its
outer timer, `timed_wall_time_ns`. Scientific timeouts and cumulative budgets
for fresh sequences use that outer wall time, including result writing.

Reports and exports name the chosen clock in `primary_time`, its value in
`primary_time_ns`, and its availability in `primary_time_status`. JSONL tables
put the clock name in `compatibility.primary_time`.

## Name an execution policy

Add a policy at the top level of the manifest:

```yaml
execution-policies:
  local:
    timing-scope: prepare_and_execute
    primary-time: timed_wall_time
    run-order: randomized
    timeout: 5 s
```

Select it with `execution-policy: local` in the experiment. Metewand works
through experiments and dataset configurations in the order shown by `plan`.
With randomized order, it shuffles the method configurations within each problem
and repetition block, using the experiment seed. Each dataset configuration
finishes before the next starts.

Runs are serial unless you set `max-concurrency`. Concurrent batches stay within
one dataset configuration. Running methods together can change their
performance, so concurrency becomes part of the recorded execution policy.

## Set time limits

There are two kinds of limit. The policy's `timeout` limits the wall time
charged to the scientific comparison. Separately, each worker phase has an
operational deadline, which defaults to ten seconds. Raising the scientific
timeout does not raise those deadlines.

For example, a solver allowed thirty seconds by its policy can still hit the
default ten-second deadline for `execute`. Set `worker-timeouts` at the top
level to give individual phases longer:

```yaml
worker-timeouts:
  materialize: 10 min
  prepare: 2 min
  execute: 45 s
  evaluate: 1 min
```

The phases are `hello`, `materialize`, `prepare`, `execute`, `reset`,
`evaluate`, and `shutdown`. Use a positive integer followed by `ms`, `s`, `min`,
or `h`, within the safe nanosecond range. Omitted phases keep their ten-second
limits. Applicability checks use the preparation limit. `check --workers` uses
these same settings, including for reference evaluation and shutdown. The
materialization limit also bounds remote dataset requests.

With a thirty-second scientific timeout and `execute: 45 s`, an eleven-second
solver call can finish. Measured work still has a thirty-second limit.
Preparation counts toward it under `prepare_and_execute`; the problem must
permit `execute_only` to leave preparation untimed. Metewand records the
effective phase limits, and changing them changes resolved execution identities.

## Reuse workers and warm them up

Lifecycle workers can select `worker-reuse: true` and `warmup-runs: 1` in their
execution policy. Reuse requires a successful reset between measurements. If a
worker is replaced, the new process repeats the warm-ups. Cold timing cannot be
combined with reuse or warm-ups.

## Read CPU and memory measurements

On Linux, CPU and peak-memory measurements cover the implementation process, so
they provide only partial evidence. These measurements are unavailable on macOS
and Windows. Treat a missing value as unavailable; it is not a measured zero.
Wall time is available on all three platforms.

Return to [running the comparison](running.md), or [prepare worker
environments](environments.md) to control library settings and record the
software used by workers.
