# Problem contracts

`metewand-core` parses each version-1 problem contract into a strict typed
envelope and validates its family-owned semantics with repository schemas. The
caller supplies the contract text, its logical repository path, and a complete
offline `SchemaCatalog`; parsing and validation perform no filesystem or network
access.

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
        Path::new("problems/example.toml"),
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
present in the supplied catalog. Dataset-backed reference cases require a
dataset path, while dataset-free cases must omit it.

The `semantics` table is otherwise family-owned, but it always separates two
operational concerns:

- `validity` defines whether a schema-valid result is semantically usable; and
- `one_shot_completion` defines whether a valid final result completes a
  one-shot attempt.

Both rule tables must be present and nonempty. Their exact fields, along with
all other family-specific semantics, are validated against `semantics_schema`.
This separation lets later profile observations remain valid without implying
that a one-shot completion threshold has been reached.

An evaluator is not embedded in the language-neutral contract. The owning
problem definition in `metewand.toml` must declare its evaluator worker,
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
