# Metewand

Metewand is a reproducible, language-neutral system for computational
benchmarks. It describes benchmark components in a versioned manifest, runs
workers as isolated processes, and records content-addressed identities and
provenance for every result.

The project is in its initial implementation phase. [DESIGN.md](DESIGN.md) is
the source of truth for semantics and interfaces, and [TODO.md](TODO.md) records
the implementation order.

## Workspace

The Rust workspace separates the user interface from its domain and execution
layers:

- `metewand-cli` provides the `metewand` executable.
- `metewand-core` owns manifests, planning, scheduling, and run records.
- `metewand-protocol` owns worker wire types and protocol versioning.
- `metewand-runtime` owns artifacts, environments, executors, and provenance.

Core and protocol are independent foundation crates. Runtime depends on both,
and the CLI composes all three.

Repository-level schemas live in `schemas/`; language SDKs will live in `sdk/`;
runnable examples in `examples/`; and longer-form documentation in `docs/`.
These directories are added only when they have real contents. Conformance
fixtures are colocated with the contracts or components they exercise.

## Compatibility contracts

Metewand's [version-1 canonical JSON contract](docs/canonical-json.md) defines
the language-neutral value domain and the exact bytes used for hashing and
transport. All implementations share the corresponding
[conformance vectors](fixtures/canonical-json/v1.json).

Version 1 also defines [offline JSON Schema validation and parameter-default
resolution](docs/schema-validation.md). Repository-relative references resolve
only through an explicitly supplied schema catalog; validation never retrieves
schemas from the filesystem or network.

The [version-1 public schemas](docs/public-schemas.md) define the initial
manifest, problem contract, one-shot policy, artifact and execution records,
the worker handshake, and strict envelopes for machine output, canonical
results, and evaluator metrics. `metewand-core` embeds the checked-in documents
for offline consumers.
The [manifest parser](docs/manifest-parsing.md) turns `metewand.toml` into strict
domain types and reports path-aware source spans for invalid input. Its
[parameter-namespace validation](docs/parameter-namespaces.md) binds dataset and
implementation parameter tables to selected, schema-bearing definitions while
preserving the benchmark author's explicit scientific classification.
Together, the manifest and [problem-contract parser](docs/problem-contracts.md)
require each schema, reference case, evaluator definition, and distinct validity
and one-shot completion rule, then validate family semantics offline. The
[compatibility pass](docs/manifest-compatibility.md) checks dataset schemas,
implementation declarations, timing scopes, budgets, and dataset-free unit
bindings across those parsed documents.
`metewand-runtime` expands each declared [source bundle](docs/source-bundles.md)
into unique bytewise-ordered repository paths, requiring every pattern to match
and rejecting selected trees that escape through symlinks. Its versioned
[local-tree hash](docs/local-tree-hashing.md) covers normalized paths, entry
types, file bytes, link targets, and the portable executable-bit semantic while
excluding incidental filesystem metadata.

The Gate 1 [domain-record graph](docs/domain-records.md) separates definitions,
configurations, logical plans, resolved execution dependencies, observations,
and attempts. Its [version-1 typed identities](docs/identities.md) hash explicit
canonical representations and encode dependencies as typed IDs.
The [whole-manifest hash](docs/manifest-hashing.md) instead binds the complete,
defaulted, path-normalized, and name-resolved typed manifest without becoming a
dependency of unrelated component identities.
The [seed-derivation contract](docs/seeds.md) produces independently domain-
separated dataset, implementation, and scheduling seeds while retaining each
complete derivation digest.
The deterministic [configuration expander](docs/configuration-expansion.md)
resolves literal values, Cartesian grids, and coupled named cases within their
explicit parameter namespaces and rejects duplicate logical candidates.
The [one-shot logical planner](docs/logical-planning.md) then assigns stable
typed identities, derives dataset, implementation, and scheduling values, and
expands implementation repetitions, measurement repetitions, warm-ups,
observation slots, and measurement attempt slots without runtime side effects.
Its capability reports distinguish declarations from current and previously
recorded runtime verification; this side-effect-free stage reports declarations
only.
The [read-only command-line workflow](docs/read-only-cli.md) composes these
layers into `metewand schema`, `metewand check`, and `metewand plan`. Repository
checks load contracts and transitive schema references offline, source contents
feed definition identities, and plans expose every unresolved logical identity
without downloads, builds, worker launches, or filesystem writes. Every command
also has [versioned JSON and JSON Lines output](docs/machine-output.md), stable
exit and diagnostic codes, structured source spans and causal chains, and
strict machine-output stream separation.
The initial [worker protocol framing layer](docs/worker-protocol-framing.md)
reads bounded JSON Lines independently of transport fragmentation and reports
typed failures for invalid bytes or malformed and ambiguous frames. Its typed
[version-1 handshake](docs/worker-protocol.md) negotiates the protocol, fixes a
worker role and resolved identity, records SDK and capability metadata, and
enforces one pending request per session.

## Development

The development environment pins Rust 1.98.0 and includes rustfmt, Clippy, and
pre-commit hooks:

```sh
devenv shell
```

Run the complete local CI suite with:

```sh
devenv test
```

The equivalent commands are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

## License

Metewand is licensed under either the Apache License, Version 2.0, or the MIT
license, at your option.
