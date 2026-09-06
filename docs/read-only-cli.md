# Read-only command-line workflow

Metewand's first command-line workflow exposes public schemas, validates a
benchmark repository, and prints its unresolved logical plan. These commands
are noninteractive and perform no downloads, environment builds, worker
launches, or filesystem writes.

```sh
metewand schema
metewand schema manifest
metewand check
metewand plan
```

`check` and `plan` use `metewand.toml` in the current directory by default. An
explicit manifest establishes its parent directory as the repository root:

```sh
metewand check --manifest benchmarks/example/metewand.toml
metewand plan --manifest benchmarks/example/metewand.toml
```

All manifest paths remain repository-relative. The loader resolves declared
files beneath that root and rejects a path or symbolic link that escapes it.

## Public schemas

With no schema name, `metewand schema` lists every stable command-facing name
and its `metewand://schemas/v1/...` identifier. Supplying a name prints the
exact checked-in JSON Schema document embedded in the executable. For example,
`metewand schema manifest` writes the same bytes as
`schemas/v1/manifest.schema.json`.

The stable names are `machine-output`, `manifest`, `problem-contract`,
`one-shot-observation-policy`, `artifact-manifest`, `result-manifest`,
`observation`, `attempt`, and `metrics`.

## Repository checks

`metewand check` performs the complete side-effect-free validation pipeline:

1. parse the strict version-1 manifest and resolve every manifest-local name;
2. parse each declared problem contract;
3. load every directly or transitively referenced repository JSON Schema into
   an offline catalog;
4. validate schema dialects, schema references, and problem semantics;
5. validate problem, dataset, implementation, policy, capability, and
   parameter compatibility;
6. expand and validate every parameter configuration; and
7. expand, contain, and content-identify every declared source bundle.

A successful check reports the canonical whole-manifest hash and the numbers of
contracts, schemas, and source bundles examined. Worker capability declarations
are not verified by this command. `check --workers` belongs to the worker
protocol slice and currently fails explicitly without launching anything.

## Logical plans

`metewand plan` runs the same repository checks and then invokes the pure core
logical planner. It reports definition, configuration, candidate,
specification, observation-slot, and attempt-slot identities in deterministic
expansion order. The summary distinguishes candidate, applicable-run,
observation-slot, and execution-slot counts.

The plan also shows declared worker commands, environment provisioning needs,
execution controls, source artifacts, and capabilities. Capability evidence is
always `declared`: a read-only plan never promotes a manifest assertion to
`verified_now` or `previously_verified`. Version 1 has no conditional
applicability capability, so admitted candidates and their attempt slots are
reported as known applicable, with no unchecked decisions.

The dataset-free path uses Metewand's repository-independent, versioned unit
dataset definition. Repository schemas, validated contracts, environments, and
source contents contribute to the definition identities supplied to the
planner. Changing a worker source therefore changes its definition and every
transitive logical identity without affecting unrelated definitions.

Successful human output is written to standard output, and failures are
written to standard error. The versioned JSON/JSONL envelopes, stable
diagnostic codes, and structured diagnostic chains are a separate compatibility
slice.
