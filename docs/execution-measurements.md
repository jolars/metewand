# Execution measurements

The local runtime records phase durations and resource use in
`provenance.data.measurements` on both published observations and attempts.
`metewand-core::measurements::ExecutionMeasurements` defines this version-2
payload. The fixed-dataset API returns the same typed payload in
`FixedDatasetResult::measurements`. Failed `run_fixed_dataset_outcome` calls
retain it in `diagnostic.details.measurements`; failed CLI attempts retain it
in their published provenance even when no observation exists.

Every measurement has one of three forms:

```json
{"status":"complete","value":42000}
{"status":"partial","value":12000,"reason":"interrupted"}
{"status":"unavailable","reason":"not_started"}
```

Values use integer nanoseconds or bytes, as specified by the field name. Zero
is a measured value. Missing measurements have no numeric value. `interrupted`
marks elapsed time observed before an exchange failed, timed out, or lost its
worker. Such durations can include termination and cleanup performed by the
failed exchange. `process_only` marks resource accounting that excludes
descendants. A partial value cannot satisfy complete process-tree accounting
or replace the authoritative wall-time measure.

| Field | Measured interval or quantity |
| --- | --- |
| `materialization_time_ns` | Shared generator or transformation request through its complete response, including dataset writing. |
| `worker_startup_time_ns` | Implementation launch, private worker setup, and hello negotiation, including interpreter and package loading. |
| `evaluator_startup_time_ns` | Evaluator launch, private worker setup, and hello negotiation. |
| `prepare_time_ns` | Complete implementation prepare request and response. |
| `execute_request_time_ns` | Complete execute request and response, including canonical result writing and serialization. |
| `timed_wall_time_ns` | Complete selected orchestrator timing envelope through the complete execute response. |
| `evaluation_time_ns` | Complete independent evaluate request and response, including metrics writing. |
| `cpu_time_ns` | Implementation process CPU use across the selected timing envelope. |
| `peak_rss_bytes` | Implementation process memory high-water mark from launch through the execute response. |

Phase timers use the orchestrator's monotonic clock. They exclude surrounding
dataset integrity checks, result and metric validation, and shutdown. The
continuous `timed_wall_time_ns` also includes integrity checks and resource
accounting between timed operations; execution provenance records their
inclusion. Implementation startup belongs to cold timing. Independent evaluator
startup and evaluation remain outside every envelope. The scalar
`timing.timed_wall_time_ns` and `timing.evaluation_time_ns` on successful
observations equal their complete diagnostic measurements exactly. Worker
`implementation_time_ns` remains a separate, optional diagnostic.

Materialization excludes acquisition, materializer startup, subsequent schema
or binary validation, and cache publication. A run constructs each dataset
configuration once and shares its construction duration across dependent
attempts, alongside the derivation identity in
`provenance.data.context.dataset_materialization`. It is not charged to each
attempt's primary time. Materialization receipts retain the original duration
in `materialization_time_ns`. Reusing a persistent construction records
`unavailable` with `cache_hit` for the current run rather than reporting the
historical construction duration as newly measured work. Fixed, unit, and
caller-supplied datasets use `provided_dataset` because no generator or
transformation runs.

On Linux, the local executor reads the implementation process's user and
system CPU ticks from `/proc/<pid>/stat` and converts them using the host's
clock-tick frequency. CPU values therefore retain that source's resolution;
a small amount of work can produce a measured zero. Peak memory comes from
`VmHWM` in `/proc/<pid>/status`, converted from KiB to bytes. CPU snapshots
bracket the selected envelope. Peak memory is a lifetime high-water mark and
includes startup. The payload records these boundaries explicitly.
`cpu_time_scope` is `worker_lifetime_through_execute` for cold timing,
`prepare_and_execute` for that envelope, or `execute_only` for operation-scope
timing. `peak_rss_scope` remains `worker_lifetime_through_execute` because
`VmHWM` includes memory used before the selected envelope.

Both resource values are explicitly `partial` with `process_only`, including
when a worker happens not to spawn descendants. The local executor cannot
prove complete process-tree accounting. The existing observation fields
`timing.cpu_time_ns` and `timing.peak_rss_bytes`, which promise complete
accounting, remain null for these partial measurements. If accounting files
are unavailable or the worker has exited, the measurement uses
`accounting_unavailable`. Other POSIX platforms use `unsupported`. Phases and
accounting that have not begun use `not_started`. A quantity outside the
canonical JSON safe-integer range uses `out_of_range`.

The common public schema validates the payload when present, and the dataset
materialization schema accepts the optional construction measurement.
Version-1 records with or without diagnostic measurements remain readable.
`MEASUREMENTS_VERSION` is now 2 to identify the expanded CPU accounting scope
vocabulary. The common schema still accepts version-1 payloads with their
original prepare-and-execute CPU scope and accepts version-2 payloads for all
three scopes. The public schema, identity, manifest, and wire-protocol versions
remain 1. Execution semantics version 2 adds [bounded concurrency](scheduling.md)
while preserving these timing envelopes and accounting scopes. The scope
conformance vectors verify both payload versions and reject cross-version and
unknown scopes.
Conformance cases cover complete, partial, and unavailable states and reject missing fields, negative durations,
and contradictory state/value combinations. Canonical byte tests cover the
three measurement forms. Phase-delay fixtures, CPU and memory workloads,
timeouts, crashes, shared materialization, and persistent-cache tests verify
runtime recording and publication.
