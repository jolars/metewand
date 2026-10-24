# Parameter ownership and namespaces

Metewand keeps scientific parameters in three explicit namespaces. Their
location records the benchmark author's classification; Metewand does not infer
ownership from library option names or move a parameter between namespaces.

| Namespace | Owner | Effect |
| --- | --- | --- |
| `dataset-parameters.<dataset>` | One selected dataset definition | Acquires, generates, selects, or transforms a dataset artifact. |
| `problem-parameters` | The experiment's selected problem contract | Determines the requested computation and result semantics. |
| `implementation-parameters.<implementation>` | One selected implementation definition | Determines how that implementation performs the computation. |

Budget and execution settings remain in their own policy structures. They do
not become scientific parameters.

Dataset and implementation parameter mappings are name-qualified because an
experiment may select several of either. Problem parameters are unqualified
because an experiment selects exactly one problem. Named cases preserve the
same structure and overlay the experiment-level axes during later expansion.

## Compact implementation parameters

An implementation may declare its parameter types, constraints, and literal
defaults together:

```yaml
implementations:
  solver:
    runner: python
    entrypoint: solver.py
    sources: [solver.py]
    environment: local
    problem-contracts: [lasso]
    capabilities: [one_shot]
    parameters:
      tolerance:
        type: number
        exclusiveMinimum: 0
        default: 1e-6
      max_iterations:
        type: integer
        minimum: 1
        default: 1000
      mode:
        type: string
        enum: [cyclic, random]
        default: cyclic
      trace:
        type: boolean
        optional: true
```

This fragment assumes the named problem and environment are declared elsewhere
in the manifest. Parameter names remain inside `parameters` so they cannot
collide with worker settings.

A scalar type name is shorthand for a field mapping: `tolerance: number` means
`tolerance: {type: number}`. Block and flow mappings behave identically. Every
field requires an explicit type. Supported types are `number`, `integer`,
`string`, `boolean`, `"null"`, and `array`. Arrays require `items`, which accepts
a type name or another compact type mapping, including nested arrays.

Constraints use JSON Schema spelling:

| Type | Constraints |
| --- | --- |
| All types | `enum`, a nonempty array of values. |
| `number`, `integer` | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`. |
| `string` | `minLength`, `maxLength`, `pattern`. |
| `array` | `items`, `minItems`, `maxItems`, `uniqueItems`. |

Fields are required unless marked `optional: true`, and undeclared parameter
names are rejected. A default supplies a missing value before validation;
`optional` permits absence without supplying a value. An explicit
`default: null` is a literal null, subject to the field's type. Array item
declarations cannot contain `default` or `optional`.

For example, an array can supply one literal default:

```yaml
parameters:
  weights:
    type: array
    items: {type: number, minimum: 0}
    minItems: 1
    default: [1, 2, 3]
```

Defaults are validated even when an experiment overrides them. Supplied values
must also satisfy the declared constraints. Objects, schema references, and
other advanced schema keywords use the existing `parameter-schema` form.
`parameters` cannot be combined with `parameter-schema` or `parameter-defaults`
on the same implementation, including empty declarations. `parameters: {}`
accepts only an empty parameter object and adds no defaults.

Metewand lowers this form into an inline object schema with `properties`, a
sorted `required` array, and `additionalProperties: false`, plus the existing
literal `parameter-defaults` mapping. It does not insert JSON Schema `default`
annotations. Equivalent explicit inline schemas and defaults produce identical
canonical manifest bytes, component identities, and run plans. Moving an
external schema inline can change a path-bearing identity as described in
[schema validation](schema-validation.md#inline-declarations).

`plan` shows resolved dataset, problem, and implementation parameters for each
candidate. Machine output places these mappings in the candidate's `parameters`
object under `dataset`, `problem`, and `implementation`.

Compact declarations currently apply only to implementation parameters with
literal defaults. Keep sweeps in experiment or case `implementation-parameters`
using the existing `value` and `grid` entries. Declaration-level grids, plain
scalar experiment overrides, compact dataset and problem parameters, and compact
results and metrics remain planned work.

## Structural validation

`validate_parameter_namespaces` checks the parts of ownership that require no
filesystem access, contract loading, or matrix expansion:

- a dataset or implementation that declares `parameter-defaults` also declares
  `parameter-schema`;
- every name-qualified parameter mapping belongs to a definition selected by its
  enclosing experiment;
- the selected definition exists; and
- the definition declares a parameter schema. Fixed datasets never do and
  therefore cannot receive dataset parameters.

The checks apply equally to experiment-level axes and named-case overlays. They
run in deterministic definition, experiment, case, and namespace order and
return the first `ParameterNamespaceError`.

Compact implementation declarations lower into these fields during parsing,
before namespace validation. Invalid compact fields report the parameter's
authored path and YAML location; supplied-value errors identify the parameter
and violated constraint.

```rust
use std::path::Path;

use metewand_core::{
    manifest::parse_manifest,
    parameters::validate_parameter_namespaces,
};

# let source = "version: 1\nname: \"example\"\n";
let manifest = parse_manifest(Path::new("metewand.yaml"), source)?;
validate_parameter_namespaces(&manifest)?;

# Ok::<(), Box<dyn std::error::Error>>(())
```

Schema-content validation remains a separate step. In particular, this pass
does not decide whether a parameter is scientifically a dataset, problem, or
implementation concern. The same key may appear in all three namespaces; that
is valid and remains visible for review in the manifest and problem contract.
