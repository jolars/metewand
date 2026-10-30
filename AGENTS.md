# AGENTS.md

This file is the operational repository guide for AI agents. Read the relevant
parts of `docs/design.md` and `CONTRIBUTING.md` before changing architecture or
public behavior. `docs/design.md` defines the general model; `CONTRIBUTING.md`
records implementation contracts and verification. Types, schemas, package help,
and fixtures specify individual interfaces. `TODO.md` lists remaining work by
area.

## Project priorities

Metewand is a reproducible, language-neutral computational benchmark system.
Preserve these properties throughout the implementation:

- Processes, not language-specific FFI, are the universal worker boundary.
- Public machine-readable boundaries are versioned from their introduction;
  internal records inherit their enclosing format's version.
- Identities, expansion, scheduling, and emitted records are deterministic.
- Human output is rendered from the same typed records as machine output.
- Commands are noninteractive and read-only unless mutation is explicitly
  authorized.

Prioritize useful local benchmarks in Basin and clustlasso-cd. Choose the next
small step from the remaining work to meet those integrations' needs. Complete
each behavior with focused tests and enough documentation to use it. A feature
does not need every package callback or a separate reference page before an
adopter can use it.

## Architecture and dependency direction

Metewand is one binary-only Cargo package named `metewand`. Its internal modules
preserve the existing separation of responsibilities:

- `src/core`: manifests, planning, scheduling, and run records.
- `src/protocol`: wire types and protocol versioning.
- `src/runtime`: artifacts, environments, executors, and provenance.
- `src/cli`: CLI parsing and user-facing diagnostics.

Core and protocol remain independent foundations. Runtime may use both; the CLI
composes all three. Runtime, core, and protocol must never use CLI modules. Keep
domain types out of the CLI, and keep execution and filesystem concerns out of
core and protocol.

There is no public Rust library API. Repository tests and native examples compile
internal modules directly through `src/modules.rs`. Keep the single integration
harness in `tests/integration.rs`, with tests grouped by layer under `tests/`.
Extract a reusable library only when a concrete consumer needs one.

## Repository conventions

- Public JSON Schemas belong in `schemas/` and declare their dialect and
  compatibility version.
- Publish schemas for externally consumed boundaries. Keep internal helper
  records private, and reuse the enclosing format's version for nested records.
  Prefer deriving new schemas from authoritative boundary types when practical;
  converting the existing schema catalog is not an MVP prerequisite.
- Contract conformance fixtures live beside the contract under a `fixtures/`
  directory; shared protocol vectors may live in a repository-level fixture tree
  once they exist.
- Runnable end-to-end demonstrations belong in `examples/`.
- Language packages belong in `packages/r`, `packages/python`, and
  `packages/julia`.
- User documentation belongs in `docs/`; `README.md` remains the concise entry
  point. The VitePress site lives in `docs/`, with the overview, getting
  started, and multipart authoring guide in `docs/guide/`. `docs/design.md`
  describes the general design; `CONTRIBUTING.md` consolidates contributor notes,
  implementation contracts, verification, and website development. Keep detailed
  interfaces in their authoritative types, schemas, package help, and fixtures
  rather than adding a reference page for each implementation step.

Do not create placeholder directories. Add each directory with its first real
schema, fixture, example, package, or document.

## Compatibility and releases

The Rust binary package, Python package, and R package have independent
package releases, managed by Versionary from Conventional Commits. Keep their
version-1 binding API stable across compatible CLI upgrades. Package SemVer is
separate from the public compatibility constants for the manifest, schemas,
identities, execution semantics, and wire protocol.
Consider only the versions affected by a change, and record a consequential
compatibility decision in the change description. Do not require a standalone
review, a decision table for unchanged versions, or fixtures for speculative
interfaces. See [Compatibility versions](CONTRIBUTING.md#compatibility-versions)
for the pre-MVP stability policy.

Preserve supported persisted records and canonical identity rules. Update
relevant conformance vectors when their actual contract changes; use byte-level
vectors where canonical bytes matter. New authoring syntax and profile APIs may
evolve together with their current consumers before the MVP. Do not silently
reinterpret existing results or promise readers for every development revision.

Versionary creates release PRs, tags, changelogs, and GitHub Releases. The
separate `crates-publish.yml` workflow publishes the binary package as `metewand`
on root release tags. Schemas and adapters ship directly from their canonical
source directories. Check the extracted package with
`cargo publish --locked --dry-run`; no asset preparation step is required.

## Development and verification

Prefer test-driven development. Add a failing unit, integration, or golden test
that describes the behavior before implementing it. Keep fixtures deterministic
and assert exact machine-readable bytes where the design defines a canonical
representation.

In `devenv shell`, use `test-fast` for core and protocol changes and `test-rust`
for the complete Rust suite. These commands use Nextest and accept its filters.
Keep combinatorial planning and scheduling assertions in core tests, with small
CLI cases covering worker execution, emitted records, and provenance.

Run focused tests while developing, then verify the changed scope once. Rust
changes require formatting, Clippy, and the complete Rust suite below. Website
or documentation changes require the documentation checks; documentation-only
changes do not require rebuilding or retesting Rust. Use `devenv test` for the
full local CI run when both scopes change. The equivalent commands are:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo nextest run --workspace --all-targets --locked --profile ci`
- `npm --prefix docs ci`
- `npm --prefix docs run deploy:check`

The exhaustive crash campaign is ignored by default. Run it explicitly when
changing durability, publication, recovery, or the run-store schema, or when
requested. Ordinary CLI, planning, and documentation changes do not require it.
See [Crash conformance](CONTRIBUTING.md#crash-conformance) for the command and
separate CI coverage. Keep the representative crash tests in the normal suite.

Use Rust 1.98.0 and edition 2024. Keep `Cargo.lock` current and use `--locked`
in verification. Comments should explain why a choice exists rather than
restating the code, and should use complete sentences with punctuation.
