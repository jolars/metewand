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

Implementations and generated datasets accept compact `parameters` mappings.
Problem contracts accept the same parameter declarations, plus compact `result`
and `metrics` mappings. These forms generate normalized JSON Schemas; explicit
schemas remain available for complex or reusable contracts.

An implementation may declare its parameter types, constraints, literal
defaults, and default grids together:

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
        grid: [100, 1000]
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
declarations cannot contain `default`, `grid`, or `optional`.

For example, an array can supply one literal default:

```yaml
parameters:
  weights:
    type: array
    items: {type: number, minimum: 0}
    minItems: 1
    default: [1, 2, 3]
```

Each parameter may declare either a literal `default` or a nonempty `grid`,
but not both. A grid selects values to benchmark without narrowing the field's
valid domain. For example, `max_iterations` above still accepts 500. Array grid
members are whole literal arrays, so `grid: [[1, 2], [3, 4]]` selects two arrays.
Defaults and every grid member are validated even when an experiment overrides
them. Supplied values must also satisfy the declared constraints. Objects,
schema references, and other advanced schema keywords use the existing
`parameter-schema` form.
`parameters` cannot be combined with `parameter-schema` or `parameter-defaults`
on the same implementation, including empty declarations. `parameters: {}`
accepts only an empty parameter object and adds no defaults.

Metewand lowers this form into an inline object schema with `properties`, a
sorted `required` array, and `additionalProperties: false`, plus the existing
literal `parameter-defaults` mapping and expansion axes. It does not insert JSON
Schema `default` annotations or add grid members to a schema's `enum`.
Equivalent explicit inline schemas and literal defaults produce identical
canonical manifest bytes and component identities. Declaration grids produce
the same run plans as equivalent experiment axes. Moving an
external schema inline can change a path-bearing identity as described in
[schema validation](schema-validation.md#inline-declarations).

`plan` shows resolved dataset, problem, and implementation parameters for each
candidate. Machine output places these mappings in the candidate's `parameters`
object under `dataset`, `problem`, and `implementation`.

### Dataset and problem declarations

Generated datasets use the same compact fields:

```yaml
datasets:
  sample:
    runner: python
    entrypoint: generate.py
    sources: [generate.py]
    environment: local
    output-schema: schemas/dataset.json
    parameters:
      rows: {type: integer, minimum: 1, grid: [10, 100]}
```

Inside a standalone problem contract or an inline `problems.<name>.contract`,
the corresponding fields can be written as follows. Scientific rules and
reference cases can be added independently:

```yaml
parameters:
  offset: {type: integer, default: 2}
result:
  answer: integer
metrics:
  absolute_error: {type: integer, minimum: 0}
```

Results and metrics share the supported types, constraints, and `optional`
marker. Their fields are required and their objects are closed by default.
They cannot declare `default` or `grid`; Metewand never fills in missing output.
Metric declarations describe data shape, while the evaluator and scientific
rules define the calculation, validity, and completion criteria.

Choose compact or explicit declarations independently for each boundary:
`parameters` or `parameter-schema`, `result` or `result-schema`, and `metrics`
or `metric-schema`. Declaring both forms for one boundary is an error. A
contract with compact `parameters` cannot also receive problem-level
`parameter-defaults`, including an empty mapping. Fixed datasets accept no
parameters.

### Selection precedence

Declaration grids supply initial expansion axes. Experiment parameter axes
replace matching declaration grids by parameter name; named cases then replace
matching experiment axes and inherit all other selections. Expanded values
merge recursively with literal defaults before validation. Missing required
parameters with no effective value remain errors. Cartesian expansion and
duplicate-candidate rejection follow the existing
[configuration expansion](configuration-expansion.md) rules.

Experiment and case overrides accept plain scalar literals: numbers, strings,
booleans, and `null`. A scalar is equivalent to an explicit `value` axis:

```yaml
problem-parameters:
  offset: 1
implementation-parameters:
  solver:
    tolerance: 1e-6
    trace: false
    weights: {value: [1, 2]}
    settings: {value: {left: 3}}
    max_iterations: {grid: [100, 1000]}
```

Arrays and objects require `value` for one literal value or `grid` for a sweep.
An unwrapped array never becomes an implicit sweep. Each expanded parameter
object merges with its definition's literal defaults. A case replaces an
experiment axis as a whole, including object-valued axes; it then merges with
the definition defaults, rather than with the replaced experiment value.

The [parameter-matrix example](https://github.com/jolars/metewand/tree/main/examples/parameter-matrix)
demonstrates compact dataset and problem fields, declaration grids, literal
defaults, and case overrides through worker execution.

Omitting both `parameters` and `parameter-schema` is equivalent to
`parameters: {}` for generated datasets, problem contracts, and implementations.
The resulting schema rejects every parameter key. Literal defaults do not
implicitly declare fields; nonempty defaults also need an explicit domain.

## Structural validation

`validate_parameter_namespaces` checks the parts of ownership that require no
filesystem access, contract loading, or matrix expansion:

- every name-qualified parameter mapping belongs to a definition selected by its
  enclosing experiment;
- the selected definition exists; and
- the definition accepts parameters. Generated datasets and implementations
  receive a closed empty-object schema when their declaration is omitted. Fixed
  datasets cannot receive dataset parameters.

The checks apply equally to experiment-level axes and named-case overlays. They
run in deterministic definition, experiment, case, and namespace order and
return the first `ParameterNamespaceError`.

Compact declarations lower into these fields during parsing,
before namespace validation. Invalid compact fields report the parameter's
authored path and YAML location. Supplied-value errors identify the parameter,
violated constraint, and exact YAML location of the effective scalar, `value`,
or grid member, including nested array and object values. An inherited invalid
default points to its declaration. A missing required compact parameter points
to its field declaration, including in a referenced problem contract. Human and
machine diagnostics use the same source spans; generated schema paths are not
shown for parameter validation failures.

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
