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
use their original transcripts and identities.

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
These checks preserve the version-1 identity, seed, and scheduling contracts.

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
