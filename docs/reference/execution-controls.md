# Execution controls

Every local run records its effective control requests in
`provenance.data.context.controls`, including on failed attempts. Accepted
attempts and observations also include the report in
`provenance.data.execution.controls`. Each entry has a stable `name`, a
`requested` value, a `required` flag, a `status`, and a `scope`.

The three status values are `enforced`, `best_effort`, and `unsupported`.
`enforced` means the executor can guarantee the request over its entire scope.
`best_effort` means it applies or checks the control but cannot prevent every
violation. `unsupported` means it cannot apply the request. The report includes
policy defaults as well as explicit requests. Local isolation and resource
controls have `scope: "worker_lifetime"`, covering startup, preparation,
execution, evaluation, reset, and shutdown regardless of the timing envelope.

The trusted local executor reports these guarantees:

| Control | Request | Status |
| --- | --- | --- |
| Network | Disabled by default | Unsupported when disabled; enforced permission when allowed |
| Filesystem isolation | Prevent undeclared access | Unsupported |
| Dataset immutability | Prevent dataset changes | Best effort: private copies and integrity checks |
| Process tree | Contain and terminate descendants | Best effort: POSIX process groups cannot contain new sessions |
| Process-tree accounting | Account for every descendant | Unsupported; partial process measurements remain separate |
| Private working directory | Private worker storage | Best effort: owner-only directory with permission checks |
| CPUs | Policy `cpus`, when present | Unsupported |
| Threads | Policy `threads`, when present | Unsupported |
| Memory | Policy `memory`, when present | Unsupported |

Best-effort CPU, thread, and memory requests are retained and do not prevent
execution. They do not establish resource limits. The local backend does not
apply thread hints or claim that environment variables enforce a thread limit.
Allowing networking expresses permission, not a guarantee of connectivity.
The worker's fixed locale, timezone, umask, and environment settings remain in
its separate environment provenance.

With `enforcement = "required"`, logical planning checks the declared local
capabilities before launching workers or creating output or cache directories.
The local backend cannot enforce all lifetime isolation controls, so planning
rejects this policy. The CLI reports `logical_planning_failed` with a
`required_control_unsupported` cause and exits with code 3. Runtime policy
preflight repeats the check before each measured attempt; an insufficient
required control at that point produces a durable `control_preflight_failed`
attempt rather than a planning error.

Worker sessions check the private directory before and after each protocol
exchange and after shutdown exit. If its permissions change, it disappears, or
it becomes a symlink, the session discards the worker. The run publishes a typed
`control_enforcement_lost` failed attempt, records the affected control as
`unsupported` with a `control_failure` entry, and publishes no observation.
The same checks cover materializers, evaluators, reused workers, resets, and
warm-ups. Checks at exchange boundaries cannot detect a violation that a worker
restores between checks, which is why the directory guarantee remains best
effort. Worker replacement repeats configured warm-ups.

The manifest, identity, public-schema, and execution-semantics compatibility
constants remain 1. These reports complete the unreleased version-1 control
contract. Existing identity inputs are unchanged, and provenance already admits
canonical extension fields. The
[canonical conformance vector](https://github.com/jolars/metewand/blob/main/fixtures/execution-controls/v1.json) fixes the
default report bytes. Tests cover explicit resource entries, planning rejection,
best-effort resource requests, durable runtime failure evidence, failures during
hello, preparation, execution, evaluation, and shutdown, and worker replacement
with repeated warm-ups after reset loses a control.
