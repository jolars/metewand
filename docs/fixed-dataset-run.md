# Fixed-dataset runtime path

`metewand-runtime::fixed_dataset_run::run_fixed_dataset` runs one trusted,
already resolved local dataset through a one-shot implementation and the
problem-owned evaluator. The caller supplies absolute dataset and private-root
paths, exact resolved launches and worker identities, the problem contract ID,
parameters and seed, selected result and metric schema paths, an offline schema
catalog, phase deadlines, and log limits. The checked-in integration test uses
the fixed dataset in `fixtures/worker-protocol/v1/fixed-dataset` with the raw
implementation and evaluator workers. A delayed evaluator fixture verifies that
evaluation is excluded from the returned timed duration.

The function creates owner-only result and metrics directories beneath the
private root. It launches and negotiates both workers through the trusted local
process path before starting the timer. The orchestrator starts its wall clock
immediately before `prepare` and stops it when `execute` returns. The protocol
requires that response to follow complete result writing, so the measured
interval includes preparation, execution, and canonical result serialization.
Result validation, independent evaluation, metrics validation, and worker
shutdown happen outside that interval.

`WorkerOutputValidator` checks the completed result and evaluator metrics
against Metewand's public envelopes and the selected problem schemas. The
function returns only those canonical values, the orchestrator's timed wall
duration, and implementation diagnostics. Role-specific launch and session
errors and separate result and metric validation errors identify a failed
boundary. Temporary output is removed on both success and failure.

This path does not accept an attempt, publish an observation, materialize a
dataset, or provide a private dataset view. The dataset must be a trusted local
input that was resolved before the call. Durable observation and attempt
publication, typed attempt outcomes, and the `metewand run` command follow in
later Gate 1.6 slices.
