# Parameter ownership and namespaces

Metewand keeps scientific parameters in three explicit namespaces. Their
location records the benchmark author's classification; Metewand does not infer
ownership from library option names or move a parameter between namespaces.

| Namespace | Owner | Effect |
| --- | --- | --- |
| `dataset_parameters.<dataset>` | One selected dataset definition | Acquires, generates, selects, or transforms a dataset artifact. |
| `problem_parameters` | The experiment's selected problem contract | Determines the requested computation and result semantics. |
| `implementation_parameters.<implementation>` | One selected implementation definition | Determines how that implementation performs the computation. |

Budget and execution settings remain in their own policy structures. They do
not become scientific parameters.

Dataset and implementation parameter tables are name-qualified because an
experiment may select several of either. Problem parameters are unqualified
because an experiment selects exactly one problem. Named cases preserve the
same structure and overlay the experiment-level axes during later expansion.

## Structural validation

`validate_parameter_namespaces` checks the parts of ownership that require no
filesystem access, contract loading, or matrix expansion:

- a dataset or implementation that declares `parameter_defaults` also declares
  `parameter_schema`;
- every name-qualified parameter table belongs to a definition selected by its
  enclosing experiment;
- the selected definition exists; and
- the definition declares a parameter schema. Fixed datasets never do and
  therefore cannot receive dataset parameters.

The checks apply equally to experiment-level axes and named-case overlays. They
run in deterministic definition, experiment, case, and namespace order and
return the first `ParameterNamespaceError`.

```rust
use std::path::Path;

use metewand_core::{
    manifest::parse_manifest,
    parameters::validate_parameter_namespaces,
};

# let source = "version = 1\nname = \"example\"\n";
let manifest = parse_manifest(Path::new("metewand.toml"), source)?;
validate_parameter_namespaces(&manifest)?;

# Ok::<(), Box<dyn std::error::Error>>(())
```

Schema-content validation remains a separate step. In particular, this pass
does not decide whether a parameter is scientifically a dataset, problem, or
implementation concern. The same key may appear in all three namespaces; that
is valid and remains visible for review in the manifest and problem contract.
