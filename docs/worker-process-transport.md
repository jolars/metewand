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
byte writer. Typed message encoding and execution exchanges remain separate
protocol layers.

## Concurrent bounded logs

`WorkerLogLimits` requires independent retained-byte limits for standard output
and standard error. Each stream begins draining on its own thread immediately
after spawn. `CapturedWorkerLog` retains only the prefix within its configured
limit, records the total number of bytes drained, and marks the capture as
truncated when it discards any bytes.

Reaching a limit changes only retention. The drain continues to EOF, so a
worker that writes more than the retained limit cannot block on a full standard
stream pipe. Logs remain raw bytes because arbitrary worker output need not be
UTF-8.

`wait()` closes both protocol endpoints before waiting for the process and then
joins both drains. It returns the exit status and the complete capture metadata.
Callers that need a final protocol exchange must complete it before calling
`wait()`. The runtime also exposes nonblocking status observation and immediate
single-process termination; executor-defined process-tree containment and the
interrupt, grace, and forced-termination sequence are later execution layers.

The integration tests run an actual inherited-descriptor exchange and flood
both standard streams beyond the operating-system pipe capacity. They verify
that protocol output never depends on standard output, both streams reach EOF,
only the configured prefixes are retained, and truncation preserves the total
drained-byte counts.

This transport implements the POSIX mechanism already specified by wire
protocol version 1. It does not change the wire-protocol compatibility version.
