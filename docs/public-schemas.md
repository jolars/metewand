# Public schemas

Metewand's version-1 public schemas define the machine-readable contracts used
by the conformance kernel. The checked-in documents under [`schemas/v1`](../schemas/v1/)
are the source of truth. Each document declares JSON Schema 2020-12, a stable
`metewand://schemas/v1/...` identifier, and
`x-metewand-compatibility-version: 1`.

The public schema compatibility version is independent of the package version,
manifest version, wire-protocol version, identity version, and execution-semantics
version. A reader must select the schema version declared by the record rather
than infer it from the Metewand release that produced the record.

## Entry points

| Schema | Contract |
| --- | --- |
| `machine-output` | Outer envelope for command events, results, and structured diagnostics. |
| `manifest` | Initial benchmark manifest, including definitions, environments, experiments, and policies. |
| `problem-contract` | Problem-owned schemas, timing scopes, semantics, and reference cases. |
| `one-shot-observation-policy` | One result without a scientific budget. |
| `artifact-manifest` | Completion marker and hash-complete file inventory for an immutable artifact. |
| `result-manifest` | Metewand metadata around problem-defined canonical result data and referenced files. |
| `observation` | Independently finalized result, metrics, timing, validity, and provenance. |
| `attempt` | Terminal accepted or failed execution with its observations and provenance. |
| `metrics` | Metewand metadata around evaluator-owned metric data. |

All Metewand-owned objects reject unknown properties. Open values appear only
where another contract owns their meaning. In particular, command-specific
machine data, family-specific problem semantics, parameter values, provenance
details, canonical results, and evaluator metrics may contain fields unknown to
the common envelope.

The `data` value in a result manifest is validated separately against the
problem contract's `result_schema`. The `data` value in a metrics envelope is
similarly validated against `metric_schema`, and a problem contract's
`semantics` object is validated against `semantics_schema`. These referenced
schemas are repository documents admitted to the same offline catalog; validation
never retrieves them from the network.

## Gate 1 boundary

The initial manifest represents the runner and environment kinds declared by the
design so definitions can be inspected before their runtime backends exist. Its
observation policies and implementation capabilities are intentionally narrower:
version 1 of this public schema set admits only `one_shot` and the `none`
scientific budget. Applicability, profiles, observation-control schedules,
checkpoint messages, and portable snapshots receive their own versioned schemas
in later gates. A later change to an existing strict entry point requires a new
public schema compatibility version.

JSON Schema enforces the portable record shape. The typed parser and runtime
remain responsible for constraints that depend on repository or execution
state, including name resolution, source-bundle expansion, bytewise path order,
unique file paths, path and symlink containment, schema-content identity,
dataset/problem compatibility, policy combinations, and timestamp ordering.

## Rust access

`metewand-core` embeds the exact checked-in documents. Consumers can enumerate
the stable entry points or construct the complete offline catalog:

```rust
use metewand_core::public_schemas::{PUBLIC_SCHEMAS, public_schema_catalog};

for schema in PUBLIC_SCHEMAS {
    println!("{} {}", schema.slug(), schema.id());
}

let catalog = public_schema_catalog()?;

# Ok::<(), metewand_core::public_schemas::PublicSchemaCatalogError>(())
```

The catalog also contains the shared `common.schema.json` resource needed by the
entry points. Conformance examples and negative cases live beside the schemas in
[`schemas/v1/fixtures`](../schemas/v1/fixtures/).
