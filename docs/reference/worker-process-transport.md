# POSIX worker process transport

The `metewand-runtime` crate launches version-1 protocol workers through
`worker_process::PosixWorkerProcess`. The transport keeps protocol messages
separate from worker logs: requests and responses travel through two dedicated
unidirectional pipes, while standard output and standard error remain log
streams.

## Dedicated protocol descriptors

Before spawning a worker, the runtime creates one request pipe and one response
pipe with close-on-exec enabled. It then clears close-on-exec only for the two
child endpoints during the child setup step. The worker receives their numeric
descriptor values in:

- `METEWAND_PROTOCOL_READ_FD`, for requests from the orchestrator; and
- `METEWAND_PROTOCOL_WRITE_FD`, for responses to the orchestrator.

The parent closes its copies of the child endpoints immediately after a
successful spawn. The child does not inherit the parent endpoints. Consuming
the `Command` for a single spawn also prevents descriptor setup from being
reused after those descriptors have closed.

`protocol_reader()` exposes the response endpoint through the version-1
bounded `FrameReader`. `protocol_writer()` exposes the request endpoint as a
byte writer. The higher-level `WorkerSession` takes ownership of both endpoints
when it begins typed exchanges; raw accessors are for transport-level use before
that transfer.

## Concurrent bounded logs

`WorkerLogLimits` requires independent retained-byte limits for standard output
and standard error. Each stream begins draining on its own thread immediately
after spawn. `CapturedWorkerLog` retains only the prefix within its configured
limit, records the total number of bytes drained, and marks the capture as
truncated when it discards any bytes.

`log_snapshot()` reads bounded process-lifetime prefixes while a worker remains
alive. A `WorkerLogSnapshot` marks these asynchronous captures incomplete and
withholds a suffix long enough to avoid exposing a secret split across reads.
Converting a final `WorkerProcessOutput` to a snapshot marks it complete.
Reused execution publishes these snapshots with explicit lifetime scope, as
explained in [worker reuse](worker-reuse.md).

Reaching a limit changes only retention. The drain continues to EOF, so a
worker that writes more than the retained limit cannot block on a full standard
stream pipe. Logs remain raw bytes because arbitrary worker output need not be
UTF-8.

`wait()` closes both protocol endpoints before waiting for the process. After
process-group termination, it allows both log drains one shared second to reach
EOF. If either pipe remains open, it cancels both drains, closes their read
endpoints, and returns a typed `LogDrainDeadline` error instead of reporting
complete capture metadata. Successful waits return the exit status and complete
capture metadata.
Callers that need a final protocol exchange must complete it before calling
`wait()`. `WorkerSession::shutdown` performs that exchange, closes the protocol
endpoints, requires clean process exit within the shutdown deadline, and then
returns the same output record. The runtime also exposes nonblocking status
observation, process-group interruption, and forced process-group termination.
Each worker starts a new process group with disconnected standard input. After
the leader exits, `wait()` kills remaining group members before joining log
drains. Dropping the handle also kills the group.

The integration tests run an actual inherited-descriptor exchange and flood
both standard streams beyond the operating-system pipe capacity. They verify
that protocol output never depends on standard output, both streams reach EOF,
only the configured prefixes are retained, and truncation preserves the total
drained-byte counts.

## Minimal trusted-local launch

`local_worker::launch_trusted_local_worker` consumes an already resolved
`ResolvedLaunchRecord`. It constructs the exact program and ordered arguments,
clears the parent environment, supplies the fixed
[worker environment](worker-environments.md), and adds the record's allowlisted
variables after validating them.
The process transport subsequently adds the two version-1 protocol descriptor
variables; they are transport state rather than inherited user configuration.

The launcher creates a unique working directory beneath a caller-owned private
root with owner-only `0700` permissions. The process handle retains that
directory until it is consumed, so it remains available throughout the worker
session and is removed afterward. The private root must exist and remain
available for the session.

No variable is implicitly copied from the orchestrator. In particular, an
entrypoint using `/usr/bin/env` needs an explicitly allowlisted `PATH`. This
launch path fixes locale and timezone, sets umask `0077` in the child, and points
home, temporary, configuration, and ecosystem cache locations to private storage.
Declared secret channels receive values separately from identity-bearing launch
records, with filtered logs and typed-session diagnostics. Resource controls
remain the responsibility of the executor backend.

The local-launch integration tests inspect the worker's actual current
directory and environment after `exec`, and establish a typed session with the
checked-in raw dataset-materializer fixture. This launch path protects
reproducibility from ambient configuration; as specified by the design, it is
not a sandbox for hostile workers.

This transport implements the POSIX mechanism already specified by wire
protocol version 1. It does not change the wire-protocol compatibility version.


## Reusable local executor

`executor::Executor` separates capability preflight, launch, and termination
from environment resolution and protocol requests. Its associated process type
allows another backend to provide its own handle. `LocalProcessExecutor`
returns the existing `PosixWorkerProcess`, so callers can pass it directly to
`WorkerSession::connect`.

Call `preflight` during planning and pass the same requirements to `spawn`,
which repeats the check before allocating a directory or starting a worker.
The capability report explicitly identifies private working directories and
process groups as supported, and complete process-tree containment as
unsupported. A requirement for complete containment fails with
`UnsupportedCompleteProcessTree`. POSIX descendants can escape their group by
creating a new session, so this backend cannot satisfy that requirement.
Private directories isolate working files but do not restrict filesystem access.

`terminate` sends SIGINT to the group, waits the configured grace period, and
sends SIGKILL to the group even if the leader already exited. It polls the
leader for up to one additional second and returns `TerminationDeadline` if
the kernel has not reported exit. Callers retain the handle after an error and
can retry cleanup. Call `wait` after successful termination to collect logs and
release the private directory. Escaped descendants can keep inherited pipes
open. Protocol reads and nonblocking writes check cancellation while waiting for
pipe readiness, so phase timeouts and session disposal can stop the protocol
thread even when such descendants retain its endpoints. Log drains use the same
cancellation mechanism and the shared post-exit deadline described above. A
shutdown log deadline becomes a typed `control_enforcement_lost` attempt failure
for either worker role and prevents acceptance. Dropping a process also cancels
its drains, so abandoned handles do not leave blocked drain threads behind.

This bounds cleanup of inherited pipes; it does not identify or terminate a
process that escaped the group, or detect an escaped process that closed its
pipes. Complete process-tree containment remains unsupported. Regression tests
launch descendants that call `setsid`, retain log and protocol endpoints, and
stop reading a request pipe. They verify bounded cleanup, timeout reporting, and
rejection of otherwise valid runs whose shutdown logs cannot reach EOF.

This implements the local lifecycle and cleanup requirements of execution
semantics version 1. It does not change wire types, identities, or canonical
record encodings. The
local executor tests cover preflight rejection and forced cleanup of ordinary
descendants that ignore interruption.
