# Configuration expansion

`metewand-core::planning::expand_manifest_configurations` turns validated
manifest parameter axes into complete dataset, problem, and implementation
configuration candidates. The expansion is pure: it reads no files, launches
no workers, and assigns no logical or resolved identities. Definition and
policy identities are attached by the subsequent logical-planning stage.

The function accepts a parsed manifest, the validated problem contracts keyed
by manifest-local problem name, and an offline schema catalog. It runs the
existing parameter-namespace and manifest-compatibility checks before
expansion. Every expanded parameter object is merged recursively with its
definition defaults and validated against the schema owned by that namespace.
Fixed and parameterless definitions bind `{}`. A dataset-free problem binds
one explicit built-in unit-dataset configuration with `{}` parameters.

## Matrix semantics

A `value` is one literal parameter value—even when it is an array or object. A
`grid` contributes one axis whose members retain their declared order. Axes
form a Cartesian product only for the selected dataset and implementation in a
candidate; a grid for one implementation does not multiply candidates for a
different implementation.

The output order is stable:

1. experiments in manifest order;
2. named cases in manifest order, or one experiment-level source when there
   are no cases;
3. selected datasets in manifest order;
4. dataset axes by parameter name, with grid members in manifest order;
5. problem axes by parameter name, with grid members in manifest order;
6. selected implementations in manifest order; and
7. applicable implementation axes by parameter name, with grid members in
   manifest order.

TOML table order therefore cannot perturb expansion. The order of selections,
cases, and grid values remains meaningful and is preserved.

## Named cases and duplicates

When `cases` are present, the experiment-level axes are defaults rather than a
separate matrix member. Each case replaces matching axes by parameter name,
inherits all other experiment-level axes, and expands independently. Case
results are unioned, not multiplied, so values such as dataset shape and
problem regularization can remain coupled. Grids inside one case still form a
Cartesian product.

`ExpandedCandidate::source` retains the case name for reporting, but the case
name is not part of logical equality. Duplicate detection compares the selected
dataset, problem, and implementation definitions and the canonical bytes of
their fully resolved parameters. It therefore catches overlapping cases,
repeated grid members, and distinct source spellings that resolve to the same
logical candidate. The error identifies both the first and repeated sources.

```rust
use std::collections::BTreeMap;

use metewand_core::{
    manifest::Manifest,
    planning::expand_manifest_configurations,
    problem_contract::ProblemContract,
    schema::SchemaCatalog,
};

# fn expand(
#     manifest: &Manifest,
#     contracts: &BTreeMap<metewand_core::manifest::Name, ProblemContract>,
#     schemas: &SchemaCatalog,
# ) -> Result<(), Box<dyn std::error::Error>> {
let experiments = expand_manifest_configurations(manifest, contracts, schemas)?;
for experiment in experiments {
    println!("{}: {} candidates", experiment.name, experiment.candidates.len());
}
# Ok(())
# }
```
