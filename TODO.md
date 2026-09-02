# Metewand roadmap

This file turns [the design](DESIGN.md) into an implementation checklist. The
design is the source of truth for semantics and interfaces; this roadmap records
the order in which to build and verify them.

Work from top to bottom. Develop each slice test-first, keep machine-readable
interfaces versioned from their introduction, and check off a slice only when
its tests and user-facing documentation are complete.

## 0. Project foundation

See [Core architecture](DESIGN.md#3-core-architecture) and
[Automation and agentic workflows](DESIGN.md#automation-and-agentic-workflows).

- [ ] Enable the Rust toolchain in `devenv.nix`, including `rustfmt`, Clippy,
      and the tools needed to build and test the workspace reproducibly.
- [ ] Create the Cargo workspace with `metewand-cli`, `metewand-core`,
      `metewand-protocol`, and `metewand-runtime`; keep dependency directions
      explicit so protocol and core domain types do not depend on the CLI.
- [ ] Establish repository conventions for schemas, fixtures, examples, SDKs,
      and documentation, creating directories only when their first contents
      are added.
- [ ] Add baseline quality checks for formatting, warnings, unit tests, and
      integration tests, and make the same checks available through `devenv`.
- [ ] Define the initial version constants for the manifest, public schemas,
      wire protocol, identities, and execution semantics.

## 1. Gate 1: conformance kernel

This gate proves the language-neutral execution model without general caching,
environment provisioning, downloads, SDKs, or resume. See
[Gate 1](DESIGN.md#gate-1-conformance-kernel).

### 1.1 Canonical data and schemas

See [Schemas, canonical values, and identities](DESIGN.md#schemas-canonical-values-and-identities).

- [ ] Write failing golden tests for the restricted JSON value domain: safe
      integers, finite binary64 values, normalized negative zero, Unicode scalar
      strings, and rejection of duplicate keys and invalid numbers.
- [ ] Implement RFC 8785 canonicalization for that domain and use the exact
      canonical bytes for hashing and transport.
- [ ] Add JSON Schema 2020-12 validation with explicit dialect declarations,
      repository-local external `$ref` resolution, and no network retrieval.
- [ ] Implement recursive `parameter_defaults` merging before validation;
      preserve literal arrays, tables, and `null`, and never apply JSON Schema
      `default` annotations.
- [ ] Publish versioned schemas for machine output and the initial manifest,
      contract, artifact-manifest, result-manifest, and metrics envelopes.

### 1.2 Definitions and source bundles

See [User model](DESIGN.md#2-user-model) and
[Problem and fairness contracts](DESIGN.md#problem-and-fairness-contracts).

- [ ] Parse the version-1 manifest into typed dataset, problem,
      implementation, environment, experiment, and execution-policy definitions
      with source-spanned diagnostics and unknown-field rejection.
- [ ] Parse problem contracts, validate their common envelope and
      family-specific `semantics` object, and require schemas, evaluator
      definitions, acceptance rules, and reference cases.
- [ ] Validate parameter ownership and namespaces structurally while leaving
      their scientific classification visible to the benchmark author.
- [ ] Normalize repository-relative paths and implement declared source-bundle
      expansion with bytewise ordering, required matches, tree-boundary checks,
      and symlink-escape rejection.
- [ ] Validate dataset/problem schema compatibility, implementation/contract
      declarations, supported timing scopes, budgets, and the built-in unit
      dataset rules for dataset-free problems.

### 1.3 Identities and deterministic seeds

See [Schemas, canonical values, and identities](DESIGN.md#schemas-canonical-values-and-identities)
and [Main domain types](DESIGN.md#main-domain-types).

- [ ] Define typed domain records for definitions, configurations, instances,
      logical specifications, logical attempt slots, resolved specifications,
      resolved slots, and run attempts.
- [ ] Implement `mw1-<kind>-<sha256>` identities over typed, versioned canonical
      representations, with dependencies represented by their typed identities.
- [ ] Implement normalized local-tree hashing over paths, entry types, bytes,
      link targets, and relevant executable bits.
- [ ] Implement the whole-manifest hash after defaults, path normalization, and
      name resolution without making it a dependency of unrelated component
      identities.
- [ ] Implement dataset, implementation, and scheduling seed derivation exactly
      as specified, recording both the exposed 53-bit value and full digest.
- [ ] Add golden and metamorphic tests showing that ordering and irrelevant
      manifest additions preserve identities, while every transitive dependency
      change invalidates the identities that depend on it.

### 1.4 Planning and read-only CLI

See [Automation and agentic workflows](DESIGN.md#automation-and-agentic-workflows).

- [ ] Expand literal values and Cartesian grids across their explicit
      namespaces into deterministic dataset, problem, and implementation
      configurations.
- [ ] Expand configurations into logical run specifications and measurement
      attempt slots, including implementation repetitions, measurement
      repetitions, seeds, warm-up roles, and stable IDs.
- [ ] Reject invalid pairings, missing datasets, unexpected datasets for
      dataset-free problems, unsupported budgets, and incompatible timing and
      worker-reuse policies before execution.
- [ ] Model capability evidence as `declared`, `verified_now`, or
      `previously_verified`; ensure a side-effect-free plan reports only
      declared capabilities.
- [ ] Implement `metewand schema`, `metewand check`, and the non-mutating
      `metewand plan` without downloads, builds, worker launches, or filesystem
      writes.
- [ ] Add versioned JSON/JSONL output, stable exit and diagnostic codes, source
      spans, causal chains, affected identities, and strict stdout/stderr
      separation for machine mode.

### 1.5 Worker protocol

See [Worker protocol](DESIGN.md#4-worker-protocol).

- [ ] Write adversarial framing tests for fragmented reads, duplicate keys,
      invalid UTF-8, byte-order marks, oversized lines, malformed JSON, early
      EOF, extra responses, and mismatched request IDs.
- [ ] Implement the version-1 JSON-Lines messages, schemas, request IDs,
      negotiation, roles, SDK metadata, worker identity checks, and one-request
      concurrency limit.
- [ ] Implement dedicated POSIX protocol pipes and concurrent bounded draining
      of stdout and stderr, continuing to drain after log truncation.
- [ ] Implement the `materialize`, `prepare`, `execute`, `reset`, `evaluate`,
      and `shutdown` exchanges, including phase-specific timeouts and typed
      failures.
- [ ] Validate worker-returned manifests and paths; reject absolute or escaping
      paths, special files, undeclared entries, incomplete files, and invalid
      canonical results or metrics.
- [ ] Build raw fixture workers for dataset materializer, implementation, and
      evaluator roles, including fixtures for every protocol failure class.

### 1.6 First end-to-end execution

See [Problem and fairness contracts](DESIGN.md#problem-and-fairness-contracts)
and [Gate 1](DESIGN.md#gate-1-conformance-kernel).

- [ ] Add the minimal local launch path needed to run trusted raw fixture
      workers with a private working directory and allowlisted environment.
- [ ] Run one fixed dataset through one raw implementation and its independent
      problem evaluator using `prepare_and_execute` timing.
- [ ] Validate the canonical result and evaluator metrics before accepting the
      attempt; retain typed records for crashes, timeouts, invalid results, and
      evaluator failures.
- [ ] Finalize one self-describing attempt directory containing `attempt.json`,
      optional metrics and results, scoped logs, and a hash-complete
      `manifest.json` completion marker.
- [ ] Expose this vertical path through `metewand run` and emit versioned
      machine-readable events and final output.
- [ ] Pass Gate 1 acceptance tests, including dataset-free expansion, reference
      evaluation, timing-boundary fixtures, protocol abuse, identity vectors,
      seed vectors, and deterministic plan snapshots.

## 2. Gate 2: reliable local MVP

This is the first release called an MVP. It provides reproducible artifacts and
complete execution records while making only development-environment and
observed-control claims. See [Gate 2](DESIGN.md#gate-2-reliable-local-mvp).

### 2.1 Sources, datasets, and artifact cache

See [Environments, execution, and artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [ ] Add pinned downloads with digest verification before use and safe archive
      extraction with traversal, link, entry-type, entry-count, and expanded-size
      limits.
- [ ] Implement generated and transformed dataset materialization, including
      canonical dataset manifests, dataset seeds, schema checks, and optional
      contract-owned binary validation.
- [ ] Implement the content-addressed artifact store, derivation-to-output
      mappings, completion markers, leases, and private per-attempt dataset
      views.
- [ ] Prefer copy-on-write snapshots or copies for local views; otherwise verify
      dataset hashes around every worker operation and report enforcement as
      best-effort.
- [ ] Detect mutations, invalidate the attempt, and transactionally quarantine a
      changed canonical cache object before it can be reused.

### 2.2 Durable attempt state and recovery

See [Automation and agentic workflows](DESIGN.md#automation-and-agentic-workflows).

- [ ] Design the SQLite schema for resolved slots, attempts, leases,
      insert-only events, state transitions, finalization claims, artifact
      references, and the one-accepted-attempt-per-slot constraint.
- [ ] Initialize SQLite in WAL mode with full synchronous durability and refuse
      durable operation when required locking, synchronization, or atomic rename
      behavior is unavailable.
- [ ] Implement transactional attempt transitions from reservation through
      finalization and every terminal outcome.
- [ ] Publish artifacts from same-filesystem staging directories using complete
      validation, file and directory synchronization, atomic no-replace rename,
      parent synchronization, and a final database transaction.
- [ ] Recover expired reservations, interrupted staging directories, completed
      orphans, and finalization claims without manufacturing success or
      duplicating an accepted slot.
- [ ] Add fault injection at every database, synchronization, rename, and
      finalization boundary, plus concurrent-publication tests for identical
      content.

### 2.3 Local executor and timing policy

See [Problem and fairness contracts](DESIGN.md#problem-and-fairness-contracts)
and [Environments, execution, and artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [ ] Implement the reusable executor interface and a POSIX local-process
      executor with process-group containment, bounded termination, isolated
      directories, and explicit capability preflight.
- [ ] Build minimal, allowlisted worker environments with fixed locale,
      timezone, umask, working directory, private home/temp/cache locations, and
      redacted secret channels.
- [ ] Measure and record materialization, startup, prepare, execute request,
      timed wall, evaluation, CPU, and peak-memory fields with explicit missing
      or partial values.
- [ ] Implement `cold_end_to_end`, `prepare_and_execute`, and `execute_only`
      envelopes, scientific and operational timeouts, and all compatibility
      rules for reuse and warm-ups.
- [ ] Implement worker reuse keys, reset and replacement behavior, cache policy,
      and repeated warm-ups after worker replacement.
- [ ] Record every requested control as enforced, best-effort, or unsupported;
      fail known-insufficient required controls during planning and runtime
      enforcement loss as a typed attempt.

### 2.4 Scheduling, retries, and inspection

See [Schemas, canonical values, and identities](DESIGN.md#schemas-canonical-values-and-identities)
and [Environments, execution, and artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [ ] Implement sequential scheduling first, then deterministic block-randomized
      scheduling by dataset instance, problem configuration, implementation
      repetition, and measurement index.
- [ ] Preserve stable relative ordering when unrelated slots are added, and
      verify the exact `A x M` slot count and specified seed sharing.
- [ ] Add opt-in bounded concurrency as execution-policy identity and
      provenance; keep performance execution serial by default.
- [ ] Implement selected-ID and bounded execution, explicit retries, immutable
      retry attempts, superseded concurrent candidates, and `run --resume`
      without repeating accepted slots.
- [ ] Implement `metewand status` and `metewand explain <id>` over stable
      definition, run, slot, attempt, artifact, and diagnostic identities.

### 2.5 SDKs and conformance suites

See [Initial SDK APIs](DESIGN.md#7-initial-sdk-apis).

- [ ] Build the Python SDK helpers for implementations, dataset materializers,
      and evaluators without exposing orchestrator responsibilities.
- [ ] Build the equivalent R SDK helpers with the same protocol behavior and
      canonical-value restrictions.
- [ ] Measure only the user implementation callback as SDK
      `implementation_time`, while writing the result before replying to
      `execute`.
- [ ] Run canonicalization, identity, seed, error, and protocol golden vectors
      across Rust, Python, and R.
- [ ] Add reference problem-contract suites that accept correct implementations
      and reject malformed, numerically wrong, and semantically invalid results.

### 2.6 Authoring, exports, and local examples

See [Authoring and scaffolding](DESIGN.md#authoring-and-scaffolding) and
[Environments, execution, and artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [ ] Implement `metewand init` as a dry-runnable, no-overwrite operation that
      creates only a minimal, domain-neutral versioned manifest.
- [ ] Implement explicit dataset, problem, and implementation scaffolds; report
      every changed path, update only the requested manifest component, and do
      not invent scientific semantics or invoke package managers.
- [ ] Implement `metewand check --workers` with capability verification and
      problem reference-case execution.
- [ ] Implement a rebuildable JSONL index and tidy JSONL/CSV export that retain
      accepted, invalid, censored, superseded, and failed attempts without
      silently combining incompatible timing or guarantee classes.
- [ ] Add exact-target cache verification and garbage collection with snapshot
      roots, lease awareness, a dry-run mode, and no deletion authorized solely
      by a stale index.
- [ ] Add representative raw-command, Python-only, R-only, dataset-free, and
      parameter-matrix local examples without privileged built-in scientific
      templates.
- [ ] Pass Gate 2 acceptance tests for archive safety, dataset mutation, timing
      scopes, process escape, log flooding, control loss, retries, crash
      recovery, cache concurrency, garbage collection, exports, and the complete
      agent loop.

## 3. Gate 3: locked environments

Add locking without making any environment backend mandatory for benchmark
authors. See [Reproducibility model](DESIGN.md#5-reproducibility-model),
[Nix environments](DESIGN.md#nix-environments), and
[Gate 3](DESIGN.md#gate-3-locked-environments).

### 3.1 Resolution and lockfile model

- [ ] Separate unresolved definitions and logical IDs from resolved
      environments, launches, artifacts, runs, and attempt slots.
- [ ] Define and validate `metewand.lock`, binding it to the normalized manifest
      while preserving identities for unaffected components.
- [ ] Implement environment resolution separately from runner launch resolution;
      allow one environment fingerprint to provide several distinct launch
      programs.
- [ ] Implement `metewand lock` with explicit network/build/write reporting and
      atomic lockfile replacement only when the command was directly requested.
- [ ] Implement `metewand run --locked` so every definition, source, artifact,
      environment, executable, SDK, protocol, executor, and semantics identity
      must match before execution.

### 3.2 Nix package and app environments

- [ ] Resolve exact `packages.<system>` outputs without fallback lookup,
      recording filtered local inputs, `flake.lock`, installable, system,
      selected output, Nix version/configuration, derivation, and realized paths.
- [ ] Record output NAR hashes and a recursive closure manifest, create explicit
      GC roots, and verify the realized closure before locked execution.
- [ ] Resolve package runners to normalized executable paths inside a designated
      output and launch them directly, never through `nix run` or `nix develop`.
- [ ] Resolve app outputs to their exact store-resident program and keep worker
      arguments in the worker definition.
- [ ] Reject impure evaluation, unsupported Nix versions, lockfile rewrites,
      executable escapes, and changes to local inputs, derivations, outputs,
      programs, or recursive closures.
- [ ] Add a multi-runtime Nix fixture that launches Python and R from one
      environment fingerprint through distinct exact launch specifications.

### 3.3 Ecosystem-locked environments

- [ ] Implement `uv` environment resolution, recording the native lockfile,
      package-manager version, platform tags, source repositories, downloaded
      artifact hashes, realized environment manifest, and exact interpreter.
- [ ] Implement the corresponding `renv` resolution and verification model for
      R projects and their exact interpreter.
- [ ] Treat local and editable dependencies as declared source bundles, and run
      locked environments offline after verifying their realized manifests and
      launch programs.
- [ ] Add Nix-locked, `uv`-locked, `renv`-locked, and mixed R/Python/native
      examples.
- [ ] Pass Gate 3 mismatch tests for every transitive source, schema, contract,
      fixture, adapter, helper, native lockfile, environment artifact, closure,
      and resolved launch program.

## 4. Gate 4: isolated archival execution

This gate supports the strongest publication-archive claim while continuing to
treat benchmark code as trusted input. See
[Gate 4](DESIGN.md#gate-4-isolated-archival-execution).

### 4.1 OCI environments and executor

- [ ] Resolve OCI images exclusively by immutable digest and record the runtime,
      image configuration, userspace identity, and exact worker launches.
- [ ] Add Docker and Podman executor support on Linux without conflating the
      pinned image with the controls applied during execution.
- [ ] Mount verified datasets read-only, expose only declared source bundles and
      private writable paths, and keep Metewand artifacts outside image layers.
- [ ] Enforce disabled networking, CPU affinity, memory limits, process-tree
      containment, and complete CPU/memory accounting where required.
- [ ] Fail preflight or the attempt—according to when the deficiency becomes
      known—when a required control cannot be applied or observed continuously.

### 4.2 Archival provenance and qualification

- [ ] Record hardware, operating system, kernel, selected cores, memory,
      runtime, language, BLAS, thread settings, repository revision and dirty
      hash, plus optional governor, turbo, NUMA, accelerator, and background-load
      observations.
- [ ] Represent unavailable provenance explicitly, and enforce policy-selected
      required fields before execution.
- [ ] Keep attempt acceptance separate from environment reproducibility and
      observed control classes; expose all three in records and exports.
- [ ] Prevent compatibility readers from pooling different timing scopes,
      environment classes, control classes, or incomplete accounting.
- [ ] Add digest-mismatch, network-denial, resource-limit, read-only-mount,
      escaped-process, accounting, and provenance tests for Docker and Podman.
- [ ] Exercise the archival path with an OCI-pinned mixed-language benchmark and
      audit every user-facing acceptance scenario and release-blocking test in
      [MVP and acceptance criteria](DESIGN.md#8-mvp-and-acceptance-criteria).

## Deferred work

These items begin only after Gate 4. Their order remains deliberately unset
until the version-1 execution model is reliable.

- [ ] Problem-defined evaluation, iteration, sample, and time budgets.
- [ ] Streaming checkpoints, trajectories, and callback-based progress.
- [ ] Julia SDK and Julia `Pkg` environments.
- [ ] Conda environments shared across multiple language runners.
- [ ] BenchExec or another stronger isolation backend.
- [ ] SLURM and remote execution.
- [ ] Immutable, provenance-tracked analysis artifacts.
- [ ] Read-only interactive diagnostic viewer.
- [ ] Optional MCP adapter over the existing CLI semantics.

## Guardrails

The following remain out of scope for version 1; see
[Explicit non-goals](DESIGN.md#explicit-non-goals-for-v1).

- Do not create a universal package manager or replace native lockfiles.
- Do not embed language runtimes or transfer native language objects.
- Do not build a general workflow engine, generic sandbox, or distributed
  service.
- Do not embed a language model or provider-specific agent orchestration.
- Do not add publication plotting, figure composition, or statistical
  conclusions to the execution core.
- Do not claim bitwise-identical results or performance across machines.
