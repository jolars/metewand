# Metewand

Metewand is a reproducible, language-neutral system for computational
benchmarks. It runs workers as separate processes, evaluates results
independently, and records deterministic identities, timing, and provenance.

The [website](https://metewand.org) provides a
[getting started guide](docs/guide/getting-started.md) and
[reference documentation](docs/reference/index.md).

The local one-shot runner, durable recovery, JSONL and CSV exports, and Python
and R SDKs are implemented. The next milestone makes local comparisons practical
for Basin and clustlasso-cd, with simpler authoring and all three profile modes:
fresh sequences, streaming checkpoints, and buffered instrumented traces.
Those profile modes and the compact authoring form are planned work.
[TODO.md](TODO.md) records the active roadmap; [DESIGN.md](DESIGN.md) defines
semantics and intended interfaces.

## Try a benchmark

Build from source with Rust 1.98.0. The fixed-dataset example also needs Python
3.11 or later:

```sh
cargo build --release --locked -p metewand-cli
target/release/metewand check --manifest examples/fixed-dataset/metewand.yaml
target/release/metewand plan --manifest examples/fixed-dataset/metewand.yaml
target/release/metewand run \
  --manifest examples/fixed-dataset/metewand.yaml \
  --output-dir /tmp/metewand-first-run \
  --output jsonl
```

Choose a new output directory. Pass the same path with `--resume` to continue
an interrupted run without repeating accepted slots. See
[running benchmarks](docs/reference/run-cli.md) for execution policies and
[exporting results](docs/reference/exports.md) for analysis.

Start your own benchmark with `metewand init [PATH]`, then add components with
`metewand scaffold dataset|problem|implementation NAME --environment NAME`.
Both commands support `--dry-run` and refuse to overwrite existing files.
The [authoring reference](docs/reference/authoring-cli.md) describes the current
requirements. The [examples](examples) include raw-command, Python, R,
dataset-free, generated-data, and parameter-matrix benchmarks.

## Architecture and interfaces

The workspace separates domain and execution layers:

- `metewand-core`: manifests, planning, scheduling, and run records.
- `metewand-protocol`: worker wire types and protocol versioning.
- `metewand-runtime`: artifacts, environments, executors, and provenance.
- `metewand-cli`: the `metewand` executable and diagnostics.

Core and protocol are independent foundation crates. Runtime depends on both,
and the CLI composes all three. Workers can use the
[Python SDK](docs/reference/python-sdk.md),
[R SDK](docs/reference/r-sdk.md), or implement the
[worker protocol](docs/reference/worker-protocol.md) directly.

Machine-readable boundaries use [versioned public schemas](docs/reference/public-schemas.md).
[Canonical JSON](docs/reference/canonical-json.md),
[typed identities](docs/reference/identities.md), and
[seed derivation](docs/reference/seeds.md) define deterministic representations.
The [compatibility policy](docs/reference/compatibility-versions.md) distinguishes
supported records from interfaces still under development. Current local runs
record development-environment and observed-control guarantees; locked and
isolated execution follow the local MVP.

## Development

The development environment pins Rust 1.98.0 and includes rustfmt, Clippy,
Nextest, Python, R, and Node.js:

```sh
devenv shell
test-fast
test-rust
```

`test-fast` runs core and protocol tests; `test-rust` runs the complete Rust
suite. Both accept Nextest filters. Outside devenv, use:

```sh
cargo nextest run -p metewand-core -p metewand-protocol --locked
cargo nextest run --workspace --all-targets --locked
```

Run focused tests while developing and verify the changed scope once.
Documentation-only changes need the documentation checks; Rust changes need
formatting, Clippy, and the complete Rust suite. `devenv test` runs all local
CI checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --workspace --all-targets --locked --profile ci
npm --prefix docs ci
npm --prefix docs run deploy:check
```

The VitePress site lives in `docs/`. See
[website development](docs/README.md) for preview and deployment commands.

Normal Rust tests include SDK, protocol, execution, and representative crash and
recovery coverage. CLI and runtime tests use bounded concurrency to avoid
oversubscribing workers and filesystem resources. The complete agent-loop
acceptance test is opt-in:

```sh
cargo test -p metewand-cli --test agent_loop --locked -- --ignored
```

Run the exhaustive crash campaign when changing durability, publication,
recovery, or the run-store schema. Its command and separate CI coverage are in
[Crash conformance](docs/reference/run-store-schema.md#crash-conformance).

## License

Metewand is licensed under either the Apache License, Version 2.0, or the MIT
license, at your option.
