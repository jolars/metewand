# Fixed-dataset runtime path

`metewand-runtime::fixed_dataset_run::run_fixed_dataset` runs one trusted,
already resolved local dataset through a one-shot implementation and the
problem-owned evaluator. The caller supplies absolute dataset and private-root
paths, exact resolved launches and worker identities, the problem contract ID,
parameters and seed, selected result and metric schema paths, an offline schema
catalog, phase deadlines, and log limits. The checked-in integration test uses
the fixed dataset in `fixtures/worker-protocol/v1/fixed-dataset` with the raw
implementation and evaluator workers. A delayed evaluator fixture verifies that
evaluation is excluded from the returned timed duration. Delayed preparation
and result writing fixtures verify that both remain inside it.

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

`run_fixed_dataset_outcome` retains a private one-shot result as
`PendingScientificVerdict` or a failed outcome with an `AttemptDiagnostic` and
stable technical failure class. It distinguishes an implementation that exits
before replying, a phase timeout, an invalid canonical result, an evaluator
failure (including invalid metrics), and a malformed protocol exchange. These
diagnostics can later populate a failed `RunAttemptRecord`; the pending result
retains validated canonical result and metric values for publication.

The current version-1 evaluator responds to `evaluate` with only a common
acknowledgment (DESIGN.md, worker protocol). Its metric document contains data
but no contract-owned verdict. `ProblemSemantics.validity` and
`one_shot_completion` are family-owned rules that Metewand hashes without
interpreting. Consequently, schema-valid metrics—even a zero error value—do
not establish scientific validity or completion. A later versioned evaluator
interface must return those decisions before a result can become a valid
observation. `RunAttemptOutcome::Accepted` additionally requires durable
publication of the observation and enclosing attempt (DESIGN.md, validity and
completion). This path makes neither claim.

This path does not publish an observation, materialize a dataset, or provide a
private dataset view. The dataset must be a trusted local input that was
resolved before the call. Scientific verdicts, durable observation and attempt
publication, and the `metewand run` command follow in later Gate 1.6 slices.
