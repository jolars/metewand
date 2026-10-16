# Fixed-dataset runtime path

`metewand-runtime::fixed_dataset_run::run_fixed_dataset` runs one trusted,
already resolved local dataset through a one-shot implementation and the
problem-owned evaluator. The caller supplies absolute dataset and private-root
paths, exact resolved launches and worker identities, the problem contract ID,
parameters and seed, selected result and metric schema paths, an offline schema
catalog, phase deadlines, and log limits. The checked-in integration test uses
the fixed dataset in `fixtures/worker-protocol/v1/fixed-dataset` with the raw
implementation and evaluator workers. Separate delayed implementation startup, evaluator startup, and evaluation
fixtures verify that each remains outside the returned timed duration. Delayed preparation
and result writing fixtures verify that both remain inside it.

The function creates owner-only result and metrics directories beneath the
private root. It launches and negotiates both workers through the trusted local
process path before starting the timer. The orchestrator starts its wall clock
immediately before `prepare` and stops it when `execute` returns. The protocol
requires that response to follow complete result writing, so the measured
interval includes preparation, execution, and canonical result serialization.
Integrity checks between prepare and execute also contribute to this continuous
duration. Provenance records this as
`timing_dataset_integrity: "included_between_prepare_and_execute"`. Checks before
prepare and after execute remain outside the timer.
Result validation, independent evaluation, metrics validation, and worker
shutdown happen outside that interval.

`WorkerOutputValidator` checks the completed result and evaluator metrics
against Metewand's public envelopes and the selected problem schemas. The
runtime hashes the complete result tree after validation and checks the hash
again after evaluation and worker shutdown. A change fails the run as an
invalid result, so returned metrics cannot silently describe changed result
files. The function returns those canonical values, the orchestrator's timed
wall duration, and implementation diagnostics. Role-specific launch and session
errors and separate result and metric validation errors identify a failed
boundary. Failed runs remove temporary output immediately. A successful result
owns its private result and metric directories, including the validated result
manifest and declared files, until the caller drops it. The caller must keep
the private root available while it uses the result for later publication.

`run_fixed_dataset_outcome` retains a private one-shot result as `Validated`
or a failed outcome with an `AttemptDiagnostic` and stable technical failure
class. It distinguishes dataset mutation, implementation crashes, phase
timeouts, invalid results, evaluator failures (including invalid metrics), and
protocol violations.

The evaluator must return an explicit scientific verdict after writing its
metrics. `invalid` rejects even a schema-valid result. `reached`, `not_reached`,
and `not_required` retain a valid result with the corresponding typed
`CompletionStatus`. A valid result that misses a completion target remains
available for observation publication; validity and completion are separate.
Missing or unknown verdicts fail protocol validation. The raw fixture evaluator
requires an exact answer and reports `reached` only for zero absolute error.

The version-1 wire contract now requires this verdict on evaluation responses.
The compatibility constant remains 1 because this completes the unreleased
Gate-1 contract; acknowledgment-only evaluator fixtures must be updated.
Protocol conformance tests cover the required verdict and reject malformed
responses. No identity or execution-semantics constants change: problem-owned
validity and completion rules already belong to those contracts.

A validated result is not an accepted attempt. The result also retains drained
implementation and evaluator logs, independent evaluation duration, wall-clock
boundaries, validation snapshots, and launch provenance for
[durable publication](durable-publication.md). Publication finalizes its
observation and enclosing attempt before acceptance. This execution function
does not materialize a dataset or provide a private dataset view. The dataset
must be a trusted local input resolved before the call, containing only regular
files and directories. Links and special files fail verification because their
external contents cannot be protected by the dataset's tree hash. It hashes the dataset
before launching either worker and rechecks after each hello, and immediately
before and after prepare, execute, evaluate, and both shutdowns, including failed
exchanges and termination. A detected change produces `dataset_mutation` and takes
precedence over other worker failures. Provenance records a `caller_provided`
view with `best_effort` enforcement and `each_worker_operation` verification.
The caller retains ownership of that directory, including cleanup after failure.
The [`metewand run` command](run-cli.md) now resolves those inputs from a checked
manifest, supplies [private copies backed by the artifact store](artifact-store.md),
quarantines changed canonical objects, and publishes the resulting observations
and attempts. These checks complete the unreleased version-1 dataset immutability
contract without changing the timing boundary, wire messages, or compatibility
constants.
