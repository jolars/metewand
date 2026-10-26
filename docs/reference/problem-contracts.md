# Problem contracts

`metewand-core` parses each version-1 problem contract into a strict typed
envelope and validates its family-owned semantics with repository schemas. The
caller supplies the contract text, its logical repository path, and a complete
offline `SchemaCatalog`; parsing and validation perform no filesystem or network
access.

A manifest can declare a problem's contract fields and evaluator directly under
`problems.<name>`:

```yaml
problems:
  sum:
    family: arithmetic
    parameters:
      offset: { type: integer, default: 0 }
    dataset-schemas: []
    result:
      answer: integer
    metrics:
      absolute_error: { type: integer, minimum: 0 }
    semantics-schema: schemas/sum-semantics.json
    allowed-timing-scopes: [prepare_and_execute]
    supported-budgets: [none]
    semantics:
      preprocessing: { allowed_outside_timed_scope: [] }
      validity: { finite_integer: true }
      one_shot_completion: { absolute_error: 0 }
    reference-cases:
      - result: reference/result.json
        expected-metrics: reference/metrics.json
    evaluator:
      runner: python
      entrypoint: evaluate.py
      sources: [evaluate.py]
      environment: local
```

This flat form inherits contract `version: 1` and `name: sum` from the manifest
mapping. Both fields may also be explicit. Metewand normalizes it into the same
internal contract as a complete versioned mapping under `contract`, preserving
canonical manifest bytes and component identities for equivalent declarations.
The evaluator and explicit `parameter-defaults` remain problem-definition
fields rather than part of the scientific contract.

Reusable contracts remain supported with `contract: problems/sum.yaml`. An inline
`contract: { ... }` mapping and a standalone contract document still require
`version` and `name`. A problem cannot combine `contract` with flat contract
fields, even if their values agree. Its evaluator remains alongside `contract`
in both forms. See the runnable [Python SDK example](python-sdk.md) for a flat
problem with scientific rules and reference cases.

Every schema field can mix [inline schemas and reusable JSON
files](schema-validation.md#inline-declarations), including `semantics-schema`.
Either inline form or a standalone contract may independently replace
`parameter-schema` with compact `parameters`, `result-schema` with `result`, and
`metric-schema` with `metrics`.
These [compact declarations](parameter-namespaces.md#dataset-and-problem-declarations)
generate closed object schemas with required fields by default. Parameter
fields accept validated literal defaults or default grids; result and metric
fields accept neither. Competing forms for the same boundary are errors.
Contract version, scientific semantics, reference cases, and validation rules
are identical in both forms. File paths in the envelope remain relative to the
repository root. Relative `$ref` values inside an inline schema use the owning
document's directory. Separate semantics schemas remain required.

```rust
use std::path::{Path, PathBuf};

use metewand_core::{
    problem_contract::parse_problem_contract,
    schema::SchemaCatalog,
};
use serde_json::Value;

fn load_contract(
    source: &str,
    schema_documents: Vec<(PathBuf, Value)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let schemas = SchemaCatalog::try_new(schema_documents)?;
    let contract = parse_problem_contract(
        Path::new("problems/example.yaml"),
        source,
        &schemas,
    )?;
    assert_eq!(contract.version, 1);
    Ok(())
}
# let _ = load_contract;
```

## Validation boundary

The common envelope requires parameter, accepted-dataset, result, metric, and
semantics schemas; a nonempty, duplicate-free set of timing scopes; the Gate 1
`none` budget; and at least one reference case. Every referenced schema must be
present in the supplied catalog, including schemas generated from compact
declarations. Dataset-backed reference cases require a
dataset path, while dataset-free cases must omit it.

The `semantics` mapping is otherwise family-owned, but it always separates two
operational concerns:

- `validity` defines whether a schema-valid result is semantically usable; and
- `one_shot_completion` defines whether a valid final result completes a
  one-shot attempt.

Both rule mappings must be present and nonempty. Their exact fields, along with
all other family-specific semantics, are validated against `semantics-schema`.
This separation lets later profile observations remain valid without implying
that a one-shot completion threshold has been reached.

The family semantics also define preprocessing and any preparation allowed
outside the timed scope. These rules distinguish shared scientific dataset
construction from implementation-specific conversion and form part of the
contract identity. The execution policy must select an allowed timing scope;
contract permission alone does not exclude work from that policy's timer. See
[materialization and implementation preparation](dataset-materialization.md#materialization-and-implementation-preparation)
for the boundary, a concrete contract, and executable conformance cases.

An evaluator is not embedded in the language-neutral contract. The owning
problem definition in `metewand.yaml` must declare its evaluator worker,
including its environment and, for interpreted workers, its complete source
bundle. The manifest parser rejects problem definitions without an evaluator.

Typed-envelope failures retain the decoder's source span. Missing schemas name
their role and repository path, while family-schema failures retain every JSON
Schema violation in deterministic order.

Repository loaders may use the same validation in two explicit phases.
`parse_problem_contract_document` parses the strict typed envelope so the
loader can discover its schema paths; `validate_problem_contract` checks that
parsed document after the complete offline catalog has been compiled.
`parse_problem_contract` remains the convenient combined operation. A document
returned by the parsing-only phase is not valid for planning until the second
phase succeeds.

## Executable reference suites

The [SDK conformance suites](sdk-conformance.md#reference-problem-contracts)
include complete sum and argmin contracts with reference datasets, results,
and expected metrics. Both Python and R implementations run against both
evaluators through the Rust transport and scientific-schema validator. Correct
implementations reach completion; malformed, numerically wrong, and semantically
invalid variants fail their respective checks. The semantic cases include a
later tied minimum with zero numerical error, so matching the objective value
alone cannot pass conformance.
