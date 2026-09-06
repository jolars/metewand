# Manifest and problem-contract compatibility

Parsing establishes the shape of each manifest and problem contract. The pure
`validate_manifest_compatibility` pass then resolves relationships among those
typed documents before planning, materialization, or worker launch.

The caller supplies parsed problem contracts keyed by their manifest-local
problem names. The validator requires a contract for every problem definition,
then checks that:

- every implementation's `problem_contracts` entry names a defined problem;
- every experiment selects defined datasets, implementations, execution
  policies, and observation policies;
- every selected implementation declares the experiment's problem contract;
- every selected dataset's `output_schema` is one of the problem contract's
  accepted `dataset_schemas`;
- the selected timing scope is allowed by the problem contract;
- the observation policy's scientific budget is supported by the problem; and
- each implementation declares the capability required by the observation
  kind.

Version 1 has one observation kind: `one_shot`. It requires the `one_shot`
implementation capability and the `none` scientific budget. These relationships
are validated explicitly even though the version-1 parsers also narrow each
individual document to those values.

## Dataset binding

A contract with an empty `dataset_schemas` list is dataset-free. Its experiments
must omit `datasets`; that empty selection instructs the later planner to bind
exactly one versioned, built-in unit dataset. Selecting a user dataset for such
a problem is an error.

A contract with one or more accepted dataset schemas requires every experiment
to select at least one dataset. Multiple selections remain separate bindings,
and each selected definition must emit an accepted schema. Metewand compares
the normalized repository schema references exactly—it never infers a
conversion from fields, dimensions, or similar structure.

## Timing-policy invariants

The compatibility pass applies the version-1 defaults relevant to timing:

- `worker_reuse = false`;
- `warmup_runs = 0`; and
- `timing_scope = "prepare_and_execute"`.

Process-local warm-ups require `worker_reuse = true`. The
`cold_end_to_end` scope instead requires `worker_reuse = false` and
`warmup_runs = 0`, because every measured slot must include a fresh worker
launch.

```rust
use std::collections::BTreeMap;

use metewand_core::{
    compatibility::validate_manifest_compatibility,
    manifest::Manifest,
    problem_contract::ProblemContract,
};

# fn check(
#     manifest: &Manifest,
#     contracts: &BTreeMap<metewand_core::manifest::Name, ProblemContract>,
# ) -> Result<(), Box<dyn std::error::Error>> {
validate_manifest_compatibility(manifest, contracts)?;
# Ok(())
# }
```

The pass returns the first error in deterministic problem, implementation,
execution-policy, and experiment order. It does not read contract files, load
schemas, expand parameter grids, construct unit-dataset records, or launch
workers. Those operations retain their own explicit boundaries.
