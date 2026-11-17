# Repository Guidelines

## Project Structure & Module Organization

`src/core/` owns domain logic; `src/protocol/` defines worker messages;
`src/runtime/` handles execution and storage; `src/cli/` provides commands.
Tests use `tests/integration.rs` and internals through `src/modules.rs`; no
public library exists. `packages/` holds Python and R workers, `schemas/` and
`fixtures/` public formats, `examples/` benchmarks, `docs/` VitePress guides,
and `assets/` branding.

## Architecture & Project Priorities

Read [docs/design.md](docs/design.md) before architecture or public behavior
changes. Prioritize Basin and clustlasso-cd benchmarks in [TODO.md](TODO.md),
with focused tests and documentation.

Keep core and protocol independent. Runtime may use both; CLI composes all
three. Lower layers must not depend on CLI. Keep domain types outside CLI and
filesystem and execution concerns outside core and protocol.

Workers communicate across process boundaries. Keep identities, expansion,
scheduling, and emitted records deterministic. Render human output from typed
machine records. CLI commands must be noninteractive; mutations require explicit
intent.

Version public machine-readable boundaries from introduction; nested internal
records inherit enclosing format versions. Preserve saved-record meanings and
canonical identities; update conformance fixtures when contracts change.

## Build, Test, and Development Commands

Run from the root in `devenv shell`. Keep `Cargo.lock` current.

- `cargo build --locked`: build CLI.
- `cargo run --locked -- --help`: run locally.
- `test-fast`: core and protocol tests; `test-rust`: complete Nextest suite.
- `npm --prefix docs ci`: install dependencies.
- `npm --prefix docs run dev`: preview docs.
- `npm --prefix docs run deploy:check`: validate website build, tests, and
  packaging.
- `cargo publish --locked --dry-run`: check packaging.

## Coding Style & Naming Conventions

Use Rust 2024 with four-space indentation, `snake_case` functions and modules,
and `PascalCase` types. Use four-space Python and two-space R indentation. Run
both formatting checks and Clippy:

```sh
cargo fmt --all -- --check
rustfmt --edition 2024 --check src/modules.rs
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

## Testing Guidelines

Add deterministic regressions by layer; assert exact canonical bytes. Use Python
`unittest` and R `testthat`. Run focused tests during development; use the full
suite for broad or uncertain changes. Prose changes need diff and link review.
Storage changes need representative crash tests. Keep exhaustive crash campaigns
in scheduled or manual CI; run locally only on explicit request.

## Commit & Pull Request Guidelines

Use Conventional Commits (`fix(runtime): preserve records`). Versionary uses
`feat`, `fix`, and `perf` for releases. Use ASCII messages and backticks for
code. PRs should explain the problem, behavior, compatibility, checks, and
linked issues. Discuss substantial changes in issues. See
[CONTRIBUTING.md](CONTRIBUTING.md) for workflows.
