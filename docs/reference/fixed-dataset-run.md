# Fixed-dataset runtime path

`metewand-runtime::fixed_dataset_run::run_fixed_dataset` runs one trusted,
already resolved local dataset through a one-shot implementation and the
problem-owned evaluator. The caller supplies absolute dataset and private-root
paths, exact resolved launches and worker identities, the problem contract ID,
parameters and seed, selected result and metric schema paths, an offline schema
catalog, an `ExecutionTimingPolicy`, operational phase deadlines, and log limits.
The policy selects one of three orchestrator-observed envelopes:

| Scope | Measured interval |
| --- | --- |
| `cold_end_to_end` | Immediately before implementation launch through receipt of its complete canonical result. |
| `prepare_and_execute` | Immediately before prepare through the complete execute response. |
| `execute_only` | Immediately before execute through its complete response, including canonical result writing. |

The caller must select a scope authorized by the checked problem contract.
Execute-only comparisons require precise, hashed contract rules for untimed
preparation. They carry `operation_scope: true` in execution provenance.
`ExecutionTimingPolicy::default()` selects prepare-and-execute without a
scientific limit. `timeout: Some(duration)` limits the complete selected
interval with one absolute monotonic deadline. Preparation and execution never
restart that budget. Cold timing also charges private worker setup, interpreter
startup, and hello negotiation to it.

The function creates owner-only result and metrics directories beneath the
private root. Dataset integrity checks surround each worker operation. Cold
and prepare-and-execute timing include checks between their timed operations;
provenance records `timing_dataset_integrity` as
`included_between_prepare_and_execute`. Execute-only checks precede and follow
the measured exchange and use `outside_execute`. Resource accounting follows
the same boundaries, except that peak memory retains its explicit worker
lifetime scope. The independent evaluator launches after implementation
execution, so its startup cannot enter cold timing. Result validation,
evaluation, metrics validation, and shutdown also remain outside every envelope.

`PhaseTimeouts` separately bounds every worker exchange. Its default is ten
seconds for each phase; runtime callers can supply other fully resolved values.
The earlier of the scientific deadline and the current operational limit wins.
Untimed phases retain their operational limits even when the scientific budget
is shorter. Scientific expiry produces `scientific_timeout`; operational
expiry produces `phase_timeout`. Both interrupt the worker process group,
allow a 100 ms grace period, and force termination before discarding the worker. They retain a partial
elapsed measurement for a timing envelope that had begun. An untimed failure
before the selected envelope starts retains `not_started` for that envelope.

The raw fixtures in `fixtures/worker-protocol/v1` test startup, preparation,
execution, and evaluator delays independently. They verify the three envelope
boundaries, a shared prepare/execute budget, untimed phases with shorter
scientific budgets, and failure measurements.

`WorkerOutputValidator` checks the completed result and evaluator metrics
against Metewand's public envelopes and the selected problem schemas. The
runtime hashes the complete result tree after validation and checks the hash
again after evaluation and worker shutdown. A change fails the run as an
invalid result, so returned metrics cannot silently describe changed result
files. The function returns those canonical values, the orchestrator's timed
wall duration, and implementation diagnostics. It also returns typed phase and
resource measurements with explicit completeness and missing reasons; see
[Execution measurements](execution-measurements.md). Role-specific launch and
session errors and separate result and metric validation errors identify a failed
boundary. Failed runs remove temporary output immediately. A successful result
owns its private result and metric directories, including the validated result
manifest and declared files, until the caller drops it. The caller must keep
the private root available while it uses the result for later publication.

`run_fixed_dataset_outcome` retains a private one-shot result as `Validated`
or a failed outcome with an `AttemptDiagnostic` and stable technical failure
class. It distinguishes dataset mutation, implementation crashes, scientific
and operational timeouts, invalid results, evaluator failures (including invalid
metrics), and protocol violations.

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
