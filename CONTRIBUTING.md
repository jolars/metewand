# Contributing

Bug reports, documentation improvements, examples, and code contributions are
welcome. The [TODO list](TODO.md) suggests work by area. For a substantial
change, open an issue to discuss the approach before investing in it.

If you are new to Metewand, try the [authoring tutorial](docs/guide/authoring/index.md)
first. The [design](docs/design.md) explains the ideas behind the system.

## Set up your environment

Run commands in this guide from the repository root. With Nix and
[devenv](https://devenv.sh/) installed, enter the development environment:

```sh
devenv shell
test-fast
```

This provides the pinned Rust toolchain, Clippy, rustfmt, Nextest, Python, R,
and Node.js. `test-fast` runs the core and protocol tests so you can check your
setup before working on a change.

Without devenv, install the Rust toolchain in [rust-toolchain.toml](rust-toolchain.toml)
and `cargo-nextest`. Worker tests also need Python 3.11 or later and R 4.2 or
later with `jsonlite`. For R package development, add `roxygen2`, `testthat`, and
`withr`. Website development needs Node.js 22.18 or later. Linux filesystem fault
injection tests use `strace`.

Metewand supports Linux, macOS, and native Windows x64. On Windows, use Rust's
MSVC toolchain and install the Visual Studio C++ build tools and Windows SDK.
The [source installation guide](docs/guide/getting-started.md#from-source) has
PowerShell commands.

## Find your way around

| Location | What you will find |
| --- | --- |
| [src/core](src/core) | Manifests, planning, scheduling, and run records. |
| [src/protocol](src/protocol) | Worker messages and protocol negotiation. |
| [src/runtime](src/runtime) | Worker execution, environments, artifacts, and storage. |
| [src/cli](src/cli) | Commands, diagnostics, and output. |
| [tests](tests) | Tests grouped by layer, with one integration harness. |
| [schemas](schemas) and [fixtures](fixtures) | Public formats and conformance examples. |
| [packages](packages) | Python and R packages. |
| [examples](examples) | Runnable benchmarks. |
| [docs](docs) | User guides and the VitePress website. |

The Rust package builds one binary and has no public library API. Core and
protocol are independent foundations; runtime uses them, and the CLI composes
all three. Repository tests and native examples load internal modules through
[src/modules.rs](src/modules.rs).

For a particular interface, start with its types, schema, package help, and
tests. Keep user instructions in `docs/guide/` and general design in
`docs/design.md`.

## Make a focused change

Start with a small reproducer or failing test when fixing a bug. Keep planning
and scheduling tests in core, and use CLI tests for behavior that needs actual
workers. Use deterministic fixtures and assert exact bytes when changing a
canonical format.

Update the relevant user guide or runnable example when behavior changes.

## Check your changes

Run focused checks while developing, then verify the affected behavior before
submitting. CI tests Rust and language packages on Linux, macOS, and Windows.
Keep platform-specific tests limited to operations that require that platform.

### Rust

In devenv, `test-fast` runs core and protocol tests, and `test-rust` runs the
complete Rust suite. Both accept Nextest filters, for example:

```sh
test-rust -E 'test(/^cli_tests::worker_checks::/)'
```

For Rust changes, run both formatting checks and Clippy:

```sh
cargo fmt --all -- --check
rustfmt --edition 2024 --check src/modules.rs
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

The second formatting check covers shared modules that Cargo's formatter does
not discover. To apply formatting, omit `--check` from both commands.

Run the complete suite for broad changes or when the impact is uncertain:

```sh
cargo nextest run --workspace --all-targets --locked --profile ci
```

Keep `Cargo.lock` current and use `--locked` for verification. Some integration
tests require extra tools or external services and are ignored by default.
Their [examples](examples) and the [CI workflow](.github/workflows/ci.yml) show
how to run them.

### Python and R packages

For Python, install the package in a virtual environment and run its tests:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -e packages/python
python -m unittest discover -s packages/python/tests -v
```

Run the R package tests with:

```sh
Rscript --vanilla -e 'testthat::test_local("packages/r")'
```

After changing R documentation comments or public function signatures,
regenerate the help pages and exports:

```sh
Rscript --vanilla -e 'roxygen2::roxygenise("packages/r")'
```

Commit the generated `packages/r/man/*.Rd` and `packages/r/NAMESPACE` files
alongside the source changes. Do not edit them directly. The package workflows
also check built [Python distributions](.github/workflows/python-package.yml)
and run [R CMD check](.github/workflows/r-package.yml).

### Documentation and website

Edit user guides in `docs/guide/`. Use relative links with `.md` extensions for
site pages, and GitHub links for source files, schemas, fixtures, and examples.
Preview the site with `npm --prefix docs run dev` after installing dependencies.
Before submitting website changes, run:

```sh
npm --prefix docs ci
npm --prefix docs run deploy:check
```

The deployment check builds the site, tests it, and validates Cloudflare
packaging without uploading anything. Changes only to repository Markdown,
such as this guide, need a diff and link review.

Use `devenv test` for a full local CI check when a change spans Rust and the
website.

### Crash conformance

For changes to storage, recovery, persisted schemas, or crash tests, add a
focused regression test and run the relevant representative crash tests in the
normal suite.

Exhaustive crash campaigns run weekly and on manual dispatch through the
[Durability workflow](.github/workflows/durability.yml), which contains the
commands. They are not required for routine local verification. These tests
exercise process death on the test filesystem; they do not establish guarantees
about storage hardware losing power or visibility across hosts.

### Scheduled tests

The [Durability workflow](.github/workflows/durability.yml) also runs the
large concurrent-results stress test weekly and on manual dispatch on Linux
and macOS. The default suite checks the same output formats with smaller
payloads.

## Compatibility versions

Package releases and public format versions are separate. The CLI, Python
package, and R package have independent releases; their version numbers do not
need to match. Compatibility constants live with the types and schemas that
own each boundary.

Preserve supported persisted records and canonical identity meanings. Before
the MVP, authoring syntax and profile APIs may evolve with their current
consumers. A change must not silently reinterpret saved results. When a change
affects compatibility, explain the decision in its description and update the
relevant conformance fixtures.

## Submit your change

Explain the problem, what changes for the user, and which checks you ran. Link
the relevant issue and call out any compatibility implications. Keep unrelated
cleanup separate so reviewers can assess the change.

Use Conventional Commits. Versionary uses `feat`, `fix`, and `perf` commits to
create version bumps and changelog entries. Release automation is configured in
[versionary.jsonc](versionary.jsonc) and the [release workflows](.github/workflows).

For packaging changes, check the extracted Cargo package with
`cargo publish --locked --dry-run`. If you change [distribution settings](dist-workspace.toml),
use the pinned cargo-dist version to run `dist generate --mode=ci`,
`dist generate --check`, and `dist plan`. Commit the generated workflow with the
configuration change.
