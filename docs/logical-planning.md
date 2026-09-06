# One-shot logical planning

`metewand-core::planning::expand_manifest_logical_plan` turns a validated
manifest into a complete unresolved one-shot plan. It first performs
[configuration expansion](configuration-expansion.md), then assigns typed
identities to every configuration, logical candidate, run specification,
observation slot, and attempt slot. The pass is pure: it does not read the
filesystem, materialize a dataset, resolve an environment, or launch a worker.

Definition identities depend on schemas, contracts, source bundles, and other
resources resolved before logical planning. Callers supply those typed IDs in a
`LogicalPlanningCatalog`. Named definitions use their manifest-local names;
the built-in dataset for a dataset-free problem uses
`DatasetConfigurationDefinition::Unit`. Missing entries produce typed,
deterministically ordered planning errors rather than placeholder identities.

## Expansion and cardinality

For an experiment with `C` expanded candidates, `I` implementation
repetitions, `M` measurement repetitions, and `W` process-local warm-ups, the
logical plan contains:

- `C` logical candidate records;
- `C × I` one-shot logical specifications;
- `C × I × M` observation slots; and
- `C × I × (W + M)` attempt slots.

Each specification receives its zero-based implementation-repetition index and
one implementation seed. Implementations in the same dataset configuration,
problem configuration, and repetition block receive the same seed because the
implementation definition is deliberately absent from the seed transcript.
Changing the repetition index changes the seed.

Each specification owns `W` warm-up slots followed by `M` measured slots. A
warm-up carries an `AttemptSlotRole::Warmup` and produces no observation. A
measured role names exactly one identified observation slot. Measurement
repetition changes only the observation and measured-slot identities; it does
not alter the candidate, specification, or implementation seed.

The returned vectors retain deterministic expansion order. This stage attaches
scheduling priorities but does not apply sequential or block-randomized runtime
scheduling.

## Stable identities and priorities

The planner constructs each record only after all of its dependencies have
identities. The resulting Merkle chain is:

```text
identified configurations -> logical candidate -> logical specification
                                               -> observation slot
                                               -> attempt slot
```

Version 1 derives an attempt slot's 32-byte scheduling priority from the full
scheduling-seed digest and an already identified logical anchor:

```text
SHA-256(
  "metewand-scheduling-priority-v1" || NUL ||
  scheduling_seed.derivation_digest || NUL ||
  role || NUL || anchor
)
```

The scheduling-seed digest is its raw 32 bytes. For `measured`, `anchor` is the
UTF-8 typed logical-observation-slot identity. For `warmup`, it is the UTF-8
typed logical-specification identity followed by `NUL` and the warm-up index
encoded as an eight-byte big-endian integer. The role strings are exactly
`measured` and `warmup`. Using the full scheduling-seed digest avoids depending
only on its 53-bit worker-facing projection.

The scheduling seed excludes the expanded member set, and each priority uses a
typed identity rather than an expansion position. Reordering implementations,
adding an unrelated candidate, or appending measurement repetitions therefore
preserves every existing identity and priority.

## Rust API

```rust
use std::collections::BTreeMap;

use metewand_core::{
    manifest::{Manifest, Name},
    planning::{
        LogicalPlanningCatalog, LogicalPlanningError,
        expand_manifest_logical_plan,
    },
    problem_contract::ProblemContract,
    schema::SchemaCatalog,
};

# fn plan(
#     manifest: &Manifest,
#     contracts: &BTreeMap<Name, ProblemContract>,
#     schemas: &SchemaCatalog,
#     definitions: &LogicalPlanningCatalog,
# ) -> Result<(), LogicalPlanningError> {
let plan = expand_manifest_logical_plan(
    manifest,
    contracts,
    schemas,
    definitions,
)?;

for experiment in plan.experiments {
    println!("{}: {} candidates", experiment.name, experiment.candidates.len());
}
# Ok(())
# }
```
