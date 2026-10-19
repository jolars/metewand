# Serial execution order

`metewand run` executes one measured slot at a time. The execution policy's
`run_order` selects `sequential` (the version-1 default) or `randomized`:

```toml
[execution_policies.local]
version = 1
timing_scope = "prepare_and_execute"
run_order = "randomized"
```

Sequential order follows the logical plan: experiments, expanded candidates,
implementation repetitions, and then measurement repetitions. Randomized order
keeps experiments in manifest order and forms comparison blocks within each
experiment from:

- the materialized dataset-instance identity;
- the problem-configuration identity;
- the zero-based implementation-repetition index; and
- the zero-based measurement index.

Each block contains the implementation configurations selected for that
comparison. Metewand sorts blocks by deterministic block priority, then sorts
their measured slots by the priorities assigned during logical planning. All
measurements in one block finish before the next block starts. Randomization
therefore interleaves comparable implementations across measurement repetitions
without changing their scientific inputs or implementation seeds. Materialization
and launch resolution finish before scheduling, so the block uses the actual
dataset instance rather than an unresolved configuration or output hash alone.

`metewand-core::scheduling::schedule_slots` provides this pure ordering pass.
Its input contains only typed comparison fields, logical slot identities, and
priorities. It returns input indices in execution order and performs no I/O.
Sequential order preserves the supplied order, including for empty selections.

## Version-1 priorities

`SCHEDULING_POLICY_VERSION` remains `1`: this is the first runtime implementation
of the existing version-1 scheduling contract. [Seed derivation](seeds.md) and
[logical slot priorities](logical-planning.md#stable-identities-and-priorities)
retain their original transcripts and identities.

The block priority is SHA-256 over this concatenation:

```text
"metewand-scheduling-priority-v1" || NUL ||
scheduling-seed digest || NUL || "block" || NUL ||
dataset-instance identity || NUL || problem-configuration identity || NUL ||
implementation-repetition index || NUL || measurement index
```

The scheduling-seed digest is its complete raw 32 bytes. Identities use their
complete typed UTF-8 spelling. Both indices use eight-byte big-endian encoding.
The scheduling seed excludes the expanded member set. Priorities sort in
ascending byte order. Equal block priorities use the typed block fields in the
order listed above as a tie-breaker; equal within-block priorities use the
logical attempt-slot identity. Adding an unrelated slot or reordering the input
therefore preserves the relative randomized order of existing slots.
The conformance vectors in [fixtures/scheduling/v1.json](../fixtures/scheduling/v1.json)
pin the block transcript. Existing attempt-slot vectors continue to pin
within-block priorities.

## Warm-ups, events, and provenance

Warm-ups run immediately before their associated measurement when the worker
lifecycle requires them. They do not enter the comparison sort or increase the
measured slot count. Switching implementation definitions replaces the retained
worker and repeats its warm-ups. An eligible retained worker runs the warm-ups
again when its logical specification changes. See [Worker reuse](worker-reuse.md).

Started and finished events and the final attempt list follow execution order.
Accepted and failed attempt provenance records the selected order, scheduling
policy version, scheduling seed and full digest, comparison-block fields, and
block and slot priorities under `context.scheduling`. These records allow callers
to inspect the ordering independently of worker completion times and wall clocks.
The read-only `plan` command continues to show deterministic logical expansion
order because dataset materialization has not happened yet.
