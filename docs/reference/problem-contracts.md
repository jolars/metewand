# Problem contracts

`metewand-core` parses each version-1 problem contract into a strict typed
envelope and optionally validates its family-owned semantics with repository schemas. The
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
document's directory. `semantics-schema` is optional. When supplied, it adds
offline family-specific validation and may require stronger scientific rules.

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

The common envelope requires accepted-dataset, result, and metric schemas.
Omitted parameters form a closed empty-object schema. Timing scopes default to
`[prepare_and_execute]`, supported budgets to `[none]`, and reference cases to
`[]`. Explicit timing scopes must be nonempty and duplicate-free. A flat problem
inherits its family from its mapping name. With a function evaluator, it also
defaults to the standard `data.json` inventory. Raw evaluators still require
accepted dataset schemas. Choose `dataset-schemas: []` explicitly for a
dataset-free problem.

Every referenced schema must be in the offline catalog, including generated
schemas. Dataset-backed reference cases require a dataset input, while
dataset-free cases must omit it.

Scientific rules are optional. Omitted `semantics` and missing groups normalize
to empty objects:

- `preprocessing` describes shared construction, conversion, and preparation
  allowed outside the timed scope.
- `validity` describes whether a schema-valid result is scientifically usable.
- `one_shot_completion` describes whether a valid final result meets the
  benchmark's completion target.

A declared group must be an object. A `semantics-schema` can impose stronger
family requirements. Without it, Metewand validates the common structure and
hashes all declared rules. Empty completion rules supply no convergence
criterion. Function evaluators returning plain metrics report `not_required`;
declared validity or completion rules require an explicit verdict. Metric
declarations never infer scientific formulas or thresholds.

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

An evaluator that reports the `problem_semantics` capability receives the entire
canonical `semantics` object with the contract identity and resolved parameters,
both during runs and reference checks. Python evaluators opt in with
`serve_evaluator(evaluate, semantics=True)`; R evaluators use
`semantics = TRUE`. The rules follow parameters in the callback signature.
Read declared tolerances from these rules rather than repeating them in code.
Legacy evaluators retain their original request and callback shapes.

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

## Reference cases

Reference cases are optional scientific checks run by `check --workers`.
The report states how many were checked; zero cases verifies no reference
solutions. Inputs can use existing repository paths or inline scientific
objects:

```yaml
reference-cases:
  - dataset: {values: [1, 2]}
    result: {total: 3}
    expected-metrics: {absolute_error: 0}
    expected-verdict: not_required
```

Inline datasets become `data.json`, and inline result and metric objects receive
standard envelopes. A string such as `result: reference/result.json` selects a
file instead. File inputs retain their existing formats: a dataset file or
artifact directory and result and metric envelopes. See the
[least-squares example](https://github.com/jolars/metewand/tree/main/examples/least-squares)
for analytic cases covering exact, incomplete, and invalid solutions.

`expected-verdict` accepts `invalid`, `reached`, `not_reached`, or `not_required`.
When omitted, it expects `reached` if completion rules exist, otherwise
`not_required`. Checks compare both the exact canonical metrics and the verdict.
Reference inputs and explicitly selected verdicts contribute to the contract's
identity. Adding checks therefore produces a new contract identity.

## Executable reference suites

The [SDK conformance suites](sdk-conformance.md#reference-problem-contracts)
include complete sum and argmin contracts with reference datasets, results,
and expected metrics. Both Python and R implementations run against both
evaluators through the Rust transport and scientific-schema validator. Correct
implementations reach completion; malformed, numerically wrong, and semantically
invalid variants fail their respective checks. The semantic cases include a
later tied minimum with zero numerical error, so matching the objective value
alone cannot pass conformance.
