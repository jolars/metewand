# Worker reuse lifecycle

A worker owner in the runtime's `worker_reuse` module manages one implementation
session at a time. Serial execution uses one owner; concurrent execution uses
one per batch position, or lane. Owners retain eligible workers across batches, and
each lane applies reset, cleanup, replacement, and warm-up rules independently.
Its eligibility key compares the protocol role, authored worker definition,
resolved source bundle, resolved environment, exact launch identity, and complete
effective execution policy. A changed key replaces the process. A policy with
`worker_reuse = false` drops the process after each slot.

Before another slot uses a retained process, the runtime clears the previous
scientific deadline, calls `reset` under its operational timeout, removes all
entries in the private working directory, and recreates the private directories
named by the worker environment. Cleanup restores owner-only working-directory
permissions and removes symbolic links themselves.
This trusted local executor does not isolate hostile workers from the host.

The current filesystem cache policy allows no writable cache to persist between
slots. This includes private home, temporary, configuration, data, and cache
locations. Interpreter state remains subject to the worker's `reset` and
`prepare` contracts. Every slot callback must prepare clean algorithmic state.

A newly launched process executes its requested warm-ups before a measured
slot. The caller supplies complete warm-up execution and validation, using the
planned warm-up index to select seeds and records. Each warm-up is followed by
reset and directory cleanup. A retained process does not repeat warm-ups within a specification. A new
specification or a replacement starts again at index zero.
Launch, reset, cleanup, warm-up, and measured-slot failures leave no retained
process. The lifecycle layer does not retry a failed measured slot.

`metewand run` retains workers while their reuse key matches, including across
implementation repetitions and compatible configurations. Each new logical
specification repeats its planned warm-ups on the eligible worker with its own
implementation seed. The same seed is used for that
specification's warm-ups and measurements, and every prepare establishes clean
algorithmic state. The runtime validates canonical results and independently
evaluates every warm-up. A warm-up failure fails the current measured attempt
before measurement; the next selected attempt starts a replacement process.
A failed reset likewise fails the pending attempt without retrying its slot.

Every attempt's provenance includes `context.worker_reuse`, with the cache
policy, startup measurement for a newly launched process, and
`warmups_on_this_attempt`. Each warm-up entry records its planned resolved slot,
index, implementation seed, validation status, measurements, and canonical
result and metrics, or its typed failure. Replacement workers and concurrent
lanes can refer to the same planned warm-up slot in different measured attempts.
These are process-local preconditions, not duplicate accepted observations.
Normal attempt publication
makes this evidence durable before emitting the terminal event.

Fresh workers return fully drained logs after shutdown. Reused workers return
bounded process-lifetime log snapshots, which include startup and warm-up output.
Their provenance explicitly records `complete = false` and
`scope = "worker_lifetime_to_snapshot"`; asynchronous streams may still contain
undrained bytes at that boundary. Snapshots never assert worker exit, and retain
the normal byte limits and secret filtering. The owner kills any retained
process when the run ends.

The fixed cache policy is part of the resolved executor configuration and
therefore its identity. Persistent filesystem caches are not selectable yet;
adding one requires a named, identity-bearing policy. Execution semantics
version 2 binds concurrent ownership and dispatch. The wire protocol and reset
and warm-up operations remain unchanged. See [Scheduling](scheduling.md) for
the concurrency setting and compatibility review.

The integration tests in `crates/metewand-runtime/tests/worker_reuse.rs` use real
POSIX workers to check process reuse, filesystem cleanup, process freshness,
reset failure, warm-up failure, replacement, and incompatible timing policies.

CLI integration tests also verify durable warm-up provenance, shared process
identity, cache cleanup, and repeated warm-ups after a crash, timeout, protocol
violation, or failed reset.
