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

Repository-level schemas will live in `schemas/`; language SDKs in `sdk/`;
runnable examples in `examples/`; and longer-form documentation in `docs/`.
These directories are added only when they have real contents. Conformance
fixtures are colocated with the contracts or components they exercise.

## Compatibility contracts

Metewand's [version-1 canonical JSON contract](docs/canonical-json.md) defines
the language-neutral value domain and the exact bytes used for hashing and
transport. All implementations share the corresponding
[conformance vectors](fixtures/canonical-json/v1.json).

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
