# Execution order and concurrency

`metewand run` executes one measured slot at a time by default. The execution
policy's `run_order` selects `sequential` (the version-1 default) or `randomized`:

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
their measured slots by the priorities assigned during logical planning. With
serial execution, all measurements in one block finish before the next starts.
Randomization therefore interleaves comparable implementations across measurement repetitions
without changing their scientific inputs or implementation seeds. Materialization
and launch resolution finish before scheduling, so the block uses the actual
dataset instance rather than an unresolved configuration or output hash alone.

`metewand-core::scheduling::schedule_slots` provides this pure ordering pass.
Its input contains only typed comparison fields, logical slot identities, and
priorities. It returns input indices in execution order and performs no I/O.
Sequential order preserves the supplied order, including for empty selections.

## Bounded concurrency

Set the execution policy's `max_concurrency` to a positive safe integer to
allow that many attempts to overlap:

```toml
[execution_policies.local]
version = 1
timing_scope = "prepare_and_execute"
run_order = "randomized"
max_concurrency = 2
```

Omitting the setting means `1`, which keeps performance execution serial.
`plan` shows the effective bound in both human and JSON output without launching
workers. The bound participates in the execution-policy identity and therefore
in candidate, specification, and slot identities. It also participates in the
whole-manifest hash. Dataset and implementation seed derivations remain the
same. Changing the bound changes slot identities and can change their scheduling
priorities. Concurrent measurements include contention from their peers.

Within each experiment, the runtime divides the selected schedule into
consecutive batches of at most `max_concurrency` attempts. Each batch finishes
and publishes its outcomes before the next begins. A final partial batch uses
only its remaining slots. Experiments retain their declared order, and batches
never combine experiments or execution policies. The runtime allocates workers
only for selected work, even when the configured bound exceeds the slot count.

Each position within a batch has its own worker owner. With reuse enabled, that
owner retains an eligible process for the same position in later batches and
applies the usual reset, cleanup, replacement, and warm-up rules. Workers never
serve two attempts simultaneously. Reducing the bound drops excess retained
workers before the next experiment starts. The bound covers complete attempts,
including preparation, warm-ups, evaluation, and publication. CPU, thread, and
memory requests continue to apply to each worker. Dataset construction remains
outside concurrent attempt dispatch.

The runtime emits a batch's start events in schedule order before dispatch and
buffers its terminal events until every dispatched attempt finishes. It emits
those terminal events and the final attempt list in schedule order, regardless
of completion order. The event sink stays on the calling thread. If it fails,
the runtime stops dispatching further batches; attempts already dispatched
finish and retain their durable outcomes. A typed worker failure does not skip
other slots in its batch or later batches.

Every accepted or failed attempt includes `context.execution_policy` and
`context.concurrency`. The latter records `max_concurrency`,
`dispatch = "fixed_batches"`, and zero-based `batch_index` and `lane_index`,
plus the selected `batch_size`. Batch indices restart for each experiment. These
records describe the configured bound and batch positions even for
a serial run or a failed attempt.

## Compatibility review

`EXECUTION_SEMANTICS_VERSION` is now `2` to identify concurrent dispatch, worker
ownership, and event delivery. The provenance schema accepts versions `1` and
`2`, so existing records remain readable. New execution-policy records always
include `max_concurrency`, including its default value `1`; this changes policy
identities, dependent run identities, and whole-manifest hashes from the earlier
representation. An omitted bound and an explicit `1` produce identical records
and hashes.

`MANIFEST_VERSION` and `PUBLIC_SCHEMA_VERSION` remain `1`: the setting and the
additional provenance version extend those formats. `IDENTITY_VERSION` stays
`1` because the typed SHA-256 construction and canonical-value rules are
unchanged. `WIRE_PROTOCOL_VERSION`, `SEED_DERIVATION_VERSION`, and
`SCHEDULING_POLICY_VERSION` also remain `1`; workers, seed transcripts, and
priority transcripts retain their existing contracts. Updated identity and
planning vectors pin the expanded policy representation. The policy vectors in
[fixtures/identities/concurrency-v1.json](https://github.com/jolars/metewand/blob/main/fixtures/identities/concurrency-v1.json)
pin the canonical bytes and identities for bounds of one, two, and three.

## Slot counts and seed sharing

For one dataset and problem configuration, `A` implementation configurations
and `M` measurement repetitions create exactly `A × M` measured attempt slots
and observation slots when `implementation_repetitions = 1`. `A` counts expanded
configurations: two parameter choices for one implementation and a parameterless
second implementation contribute three configurations. An implementation's grid
does not multiply the configurations of another implementation.

With `I` implementation repetitions, the count becomes `A × I × M`. Independent
dataset and problem configurations multiply that count. For `W` warm-ups, the
logical plan also reserves `A × I × W` warm-up slots per dataset and problem
configuration. Those slots produce no measured observations. Worker replacement
can repeat warm-ups during execution without adding measured slots.

The seed dependencies distinguish scientific replication from measurement:

| Seed | Shared across | Changes with |
| --- | --- | --- |
| Dataset | Problem and implementation configurations, implementation repetitions, and measurements | Experiment seed, dataset definition, or dataset parameters |
| Implementation | Implementation definitions and parameter configurations, measurements, and warm-ups in the same replication block | Experiment seed, dataset configuration, problem configuration, or implementation-repetition index |
| Scheduling | Every slot in one experiment | Experiment seed, benchmark name, experiment name, or scheduling-policy version |

Measurement repetitions keep the same logical specification and implementation
seed while receiving distinct observation and attempt-slot identities. Different
seed inputs produce separate derivations; the worker-visible 53-bit values can
coincide by chance. See [Seed derivation](seeds.md) for the complete transcripts
and recorded digests.

## Version-1 priorities

`SCHEDULING_POLICY_VERSION` and `SEED_DERIVATION_VERSION` are both `1`.
[Seed derivation](seeds.md) and
[logical slot priorities](logical-planning.md#stable-identities-and-priorities)
use their original derivation transcripts.

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
The conformance vectors in [fixtures/scheduling/v1.json](https://github.com/jolars/metewand/blob/main/fixtures/scheduling/v1.json)
pin the block transcript. Existing attempt-slot vectors continue to pin
within-block priorities.

The relative-order guarantee also covers additional implementation parameter
choices, dataset and problem configurations, implementation and measurement
repetitions, and unrelated experiments. New slots may appear between existing
slots. Removing the new slots from the expanded randomized schedule recovers
the original order with the same identities, priorities, and seeds. Sequential
order continues to follow the authored selections and grid order.

The CLI conformance test exercises each addition independently through `plan`
and `run`, checks the exact measured slot and observation sets, and compares the
seed received by each worker with the derivation for its planned configuration.
The core matrix test also checks dataset parameters, implementation parameter
grids, implementation replication, and warm-ups against the counts above.
These checks cover the expanded execution policy and the version-1 seed and
priority transcripts.

## Warm-ups, events, and provenance

Warm-ups run immediately before their associated measurement when the worker
lifecycle requires them. They do not enter the comparison sort or increase the
measured slot count. Switching implementation definitions replaces the retained
worker and repeats its warm-ups. An eligible retained worker runs the warm-ups
again when its logical specification changes. See [Worker reuse](worker-reuse.md).

Started and finished events and the final attempt list follow deterministic
dispatch order.
Accepted and failed attempt provenance records the selected order, scheduling
policy version, scheduling seed and full digest, comparison-block fields, and
block and slot priorities under `context.scheduling`. These records allow callers
to inspect the ordering independently of worker completion times and wall clocks.
The read-only `plan` command continues to show deterministic logical expansion
order because dataset materialization has not happened yet.
