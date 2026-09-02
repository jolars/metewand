# AGENTS.md

This file is the operational repository guide for AI agents. Read the relevant
parts of `DESIGN.md` before changing architecture or public behavior;
`DESIGN.md` is the source of truth for semantics and interfaces, while `TODO.md`
defines the implementation order and acceptance slices.

## Project priorities

Metewand is a reproducible, language-neutral computational benchmark system.
Preserve these properties throughout the implementation:

- Processes, not language-specific FFI, are the universal worker boundary.
- Machine-readable interfaces are versioned from their introduction.
- Identities, expansion, scheduling, and emitted records are deterministic.
- Human output is rendered from the same typed records as machine output.
- Commands are noninteractive and read-only unless mutation is explicitly
  authorized.

Implement roadmap slices from top to bottom. Do not check off a slice until its
tests and user-facing documentation are complete.

## Architecture and dependency direction

The workspace contains four crates:

- `metewand-core`: manifests, planning, scheduling, and run records.
- `metewand-protocol`: wire types and protocol versioning.
- `metewand-runtime`: artifacts, environments, executors, and provenance.
- `metewand-cli`: CLI parsing and user-facing diagnostics for the `metewand`
  executable.

Core and protocol are independent foundation crates. Runtime may depend on both;
the CLI may depend on all three. Runtime, core, and protocol must never depend
on the CLI. Keep domain types out of the CLI, and keep execution and filesystem
concerns out of core and protocol.

## Repository conventions

- Public JSON Schemas belong in `schemas/` and declare their dialect and
  compatibility version.
- Contract conformance fixtures live beside the contract under a `fixtures/`
  directory; shared protocol vectors may live in a repository-level fixture tree
  once they exist.
- Runnable end-to-end demonstrations belong in `examples/`.
- Language SDKs belong in `sdk/r`, `sdk/python`, and `sdk/julia`.
- Longer-form documentation belongs in `docs/`; `README.md` remains the concise
  entry point.

Do not create placeholder directories. Add each directory with its first real
schema, fixture, example, SDK, or document.

## Compatibility and releases

Package SemVer is shared across the workspace and managed by Versionary from
Conventional Commits. It is separate from the public compatibility constants for
the manifest, schemas, identities, execution semantics, and wire protocol. Any
change that alters one of those contracts must review the corresponding constant
explicitly and add or update its conformance vectors.

The crates are not published to a registry yet. Versionary creates release PRs,
tags, changelogs, and GitHub Releases; registry publication requires a separate,
deliberate workflow.

## Development and verification

Prefer test-driven development. Add a failing unit, integration, or golden test
that describes the behavior before implementing it. Keep fixtures deterministic
and assert exact machine-readable bytes where the design defines a canonical
representation.

Run focused tests while developing, then run the complete suite:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --all-targets --locked`
- `devenv test` for the local equivalent of GitHub CI

Use Rust 1.98.0 and edition 2024. Keep `Cargo.lock` current and use `--locked`
in verification. Comments should explain why a choice exists rather than
restating the code, and should use complete sentences with punctuation.
