# Metewand roadmap

This file turns [the design](DESIGN.md) into an implementation checklist. The
design is the source of truth for semantics and interfaces; this roadmap records
the order in which to build and verify them.

Work from top to bottom. Develop each slice test-first, keep machine-readable
interfaces versioned from their introduction, and check off a slice only when
its tests and user-facing documentation are complete.

## 0. Project foundation

See [Core architecture](DESIGN.md#3-core-architecture) and [Automation and
agentic workflows](DESIGN.md#automation-and-agentic-workflows).

- [x] Enable the Rust toolchain in `devenv.nix`, including `rustfmt`, Clippy,
  and the tools needed to build and test the workspace reproducibly.
- [x] Create the Cargo workspace with `metewand-cli`, `metewand-core`,
  `metewand-protocol`, and `metewand-runtime`; keep dependency directions
  explicit so protocol and core domain types do not depend on the CLI.
- [x] Establish repository conventions for schemas, fixtures, examples, SDKs,
  and documentation, creating directories only when their first contents are
  added.
- [x] Add baseline quality checks for formatting, warnings, unit tests, and
  integration tests, and make the same checks available through `devenv`.
- [x] Define the initial version constants for the manifest, public schemas,
  wire protocol, identities, and execution semantics.

## 1. Gate 1: conformance kernel

This gate proves the language-neutral execution model without general caching,
environment provisioning, downloads, SDKs, or resume. See [Gate
1](DESIGN.md#gate-1-conformance-kernel).

### 1.1 Canonical data and schemas

See [Schemas, canonical values, and
identities](DESIGN.md#schemas-canonical-values-and-identities).

- [x] Write failing golden tests for the restricted JSON value domain: safe
  integers, finite binary64 values, normalized negative zero, Unicode scalar
  strings, and rejection of duplicate keys and invalid numbers.
- [x] Implement RFC 8785 canonicalization for that domain and use the exact
  canonical bytes for hashing and transport.
- [x] Add JSON Schema 2020-12 validation with explicit dialect declarations,
  repository-local external `$ref` resolution, and no network retrieval.
- [x] Implement recursive `parameter-defaults` merging before validation;
  preserve literal arrays, mappings, and `null`, and never apply JSON Schema
  `default` annotations.
- [x] Publish versioned schemas for machine output and the initial manifest,
  contract, one-shot observation policy, artifact-manifest, result-manifest,
  observation, attempt, and metrics envelopes.

### 1.2 Definitions and source bundles

See [User model](DESIGN.md#2-user-model) and [Problem and fairness
contracts](DESIGN.md#problem-and-fairness-contracts).

- [x] Parse the version-1 YAML manifest into typed dataset, problem, implementation,
  environment, experiment, execution-policy, and observation-policy
  definitions with source-spanned diagnostics and unknown-field rejection.
- [x] Parse YAML problem contracts, validate their common envelope and
  family-specific `semantics` object, and require schemas, evaluator
  definitions, result-validity and one-shot completion rules, and reference
  cases.
- [x] Validate parameter ownership and namespaces structurally while leaving
  their scientific classification visible to the benchmark author.
- [x] Normalize repository-relative paths and implement declared source-bundle
  expansion with bytewise ordering, required matches, tree-boundary checks,
  and symlink-escape rejection.
- [x] Validate dataset/problem schema compatibility, implementation/contract
  declarations, supported timing scopes, budgets, and the built-in unit
  dataset rules for dataset-free problems.

### 1.3 Identities and deterministic seeds

See [Schemas, canonical values, and
identities](DESIGN.md#schemas-canonical-values-and-identities) and [Main domain
types](DESIGN.md#main-domain-types).

- [x] Define typed domain records for definitions, configurations, instances,
  logical candidates, one-shot logical specifications, observation and
  attempt slots, resolved specifications and slots, run observations, and
  run attempts.
- [x] Implement `mw1-<kind>-<sha256>` identities over typed, versioned canonical
  representations, with dependencies represented by their typed identities.
- [x] Implement normalized local-tree hashing over paths, entry types, bytes,
  link targets, and relevant executable bits.
- [x] Implement the whole-manifest hash after defaults, path normalization, and
  name resolution without making it a dependency of unrelated component
  identities.
- [x] Implement dataset, implementation, and scheduling seed derivation exactly
  as specified, recording both the exposed 53-bit value and full digest.
- [x] Add golden and metamorphic tests showing that ordering and irrelevant
  manifest additions preserve identities, while every transitive dependency
  change invalidates the identities that depend on it.

### 1.4 Planning and read-only CLI

See [Automation and agentic
workflows](DESIGN.md#automation-and-agentic-workflows).

- [x] Expand literal values, Cartesian grids, and named coupled cases across
  their explicit namespaces into deterministic dataset, problem, and
  implementation configurations; reject duplicate logical candidates.
- [x] Expand configurations into logical candidates, one-shot run
  specifications, observation slots, and measurement attempt slots,
  including implementation repetitions, measurement repetitions, seeds,
  warm-up roles, and stable IDs.
- [x] Reject invalid pairings, missing datasets, unexpected datasets for
  dataset-free problems, unsupported budgets, and incompatible timing and
  worker-reuse policies before execution.
- [x] Model capability evidence as `declared`, `verified_now`, or
  `previously_verified`; ensure a side-effect-free plan reports only
  declared capabilities.
- [x] Implement `metewand schema`, `metewand check`, and the non-mutating
  `metewand plan` without downloads, builds, worker launches, or filesystem
  writes.
- [x] Add versioned JSON/JSONL output, stable exit and diagnostic codes, source
  spans, causal chains, affected identities, and strict stdout/stderr
  separation for machine mode.

### 1.5 Worker protocol

See [Worker protocol](DESIGN.md#4-worker-protocol).

- [x] Write adversarial framing tests for fragmented reads, duplicate keys,
  invalid UTF-8, byte-order marks, oversized lines, malformed JSON, early
  EOF, extra responses, and mismatched request IDs.
- [x] Implement the version-1 JSON-Lines messages, schemas, request IDs,
  negotiation, roles, SDK metadata, worker identity checks, and one-request
  concurrency limit.
- [x] Implement dedicated POSIX protocol pipes and concurrent bounded draining
  of stdout and stderr, continuing to drain after log truncation.
- [x] Implement the one-shot `materialize`, `prepare`, `execute`, `reset`,
  `evaluate`, and `shutdown` exchanges, including phase-specific timeouts
  and typed failures; reserve negotiated capabilities for the later
  applicability and profile subprotocols.
- [x] Validate worker-returned manifests and paths; reject absolute or escaping
  paths, special files, undeclared entries, incomplete files, and invalid
  canonical results or metrics.
- [x] Build raw fixture workers for dataset materializer, implementation, and
  evaluator roles, including fixtures for every protocol failure class.

### 1.6 First end-to-end execution

See [Problem and fairness contracts](DESIGN.md#problem-and-fairness-contracts)
and [Gate 1](DESIGN.md#gate-1-conformance-kernel).

- [x] Add the minimal local launch path needed to run trusted raw fixture
  workers with a private working directory and allowlisted environment.
- [x] Run one fixed dataset through one raw implementation and its independent
  problem evaluator using `prepare_and_execute` timing.
- [x] Validate the canonical result and evaluator metrics before accepting the
  attempt; retain typed records for crashes, timeouts, invalid results, and
  evaluator failures.
- [x] Finalize one self-describing observation directory containing its result,
  metrics, provenance, and completion marker, then one attempt directory
  that references it and contains scoped logs and its own hash-complete
  completion marker.
- [x] Expose this vertical path through `metewand run` and emit versioned
  machine-readable events and final output.
- [x] Pass Gate 1 acceptance tests, including dataset-free expansion, reference
  evaluation, timing-boundary fixtures, protocol abuse, identity vectors,
  seed vectors, and deterministic plan snapshots.

## 2. Gate 2: reliable local foundation

This gate provides reliable one-shot artifacts and execution records while
making only development-environment and observed-control claims. It is the
substrate for the comparative local MVP, not the general MVP itself. See [Gate
2](DESIGN.md#gate-2-reliable-local-foundation).

### 2.1 Source acquisition, materialization, and artifact cache

See [Environments, execution, and
artifacts](DESIGN.md#6-environments-execution-and-artifacts) and [Problem and
fairness contracts](DESIGN.md#problem-and-fairness-contracts).

Metewand owns reproducible construction, validation, storage, and protection of
benchmark inputs. Benchmark authors supply the scientific recipes for data
generation, fold selection, preprocessing, and format conversion. Datamonger
provides an optional source of recorded datasets and verified decoding.

- [x] Define and document a source-resolution boundary for repository files,
  pinned URLs, and optional external providers that returns verified local
  artifacts and provenance. Keep `check` and `plan` free of downloads and
  materialization, and document the contract in `DESIGN.md` and user-facing
  documentation.
- [x] Add pinned downloads with digest verification before use and safe archive
  extraction with traversal, link, entry-type, entry-count, and
  expanded-size limits.
- [x] Execute benchmark-owned generators and transformations through the
  materializer protocol, including canonical dataset manifests, dataset seeds,
  schema checks, and optional contract-owned binary validation.
- [x] Record external source selectors, resolved dataset versions, artifact
  digests, decoding recipes, and verification status. Bind verified sources,
  materializer code and environment, canonical parameters, and dataset seeds
  into derivation identities, with separately recorded output hashes.
- [x] Implement the content-addressed artifact store, derivation-to-output
  mappings, completion markers, leases, and private per-attempt dataset
  views. Import provider exports through verified copies or snapshots while
  preserving ownership of provider caches.
- [x] Prefer copy-on-write snapshots or copies for local views; otherwise verify
  dataset hashes around every worker operation and report enforcement as
  best-effort.
- [x] Detect mutations, invalidate the attempt, and transactionally quarantine a
  changed canonical cache object before it can be reused.
- [x] Add conformance tests and documentation for the boundary between shared
  dataset materialization and implementation preparation, including
  implementation-specific conversions and the problem contract's rules for
  work outside the timed scope.
- [x] Add an optional Datamonger adapter and runnable example using a pinned
  registry selector and dataset version through a process boundary. Distinguish
  artifact-only verification from decoded-value verification, produce a
  standard Metewand dataset artifact, and demonstrate that all implementations
  consume private views of the same instance. Test pin and verification
  failures and offline execution after source resolution.

### 2.2 Durable attempt state and recovery

See [Automation and agentic
workflows](DESIGN.md#automation-and-agentic-workflows).

- [x] Design the SQLite schema for resolved observation and attempt slots, run
  observations, attempts, leases, insert-only events, state transitions,
  finalization claims, artifact references, and the uniqueness constraints
  for selected observations and accepted attempts.
- [x] Initialize SQLite in WAL mode with full synchronous durability and refuse
  durable operation when required locking, synchronization, or atomic rename
  behavior is unavailable.
- [x] Implement transactional attempt transitions from reservation through
  finalization and every terminal outcome.
- [x] Publish artifacts from same-filesystem staging directories using complete
  validation, file and directory synchronization, atomic no-replace rename,
  parent synchronization, and a final database transaction.
- [x] Recover expired reservations, interrupted staging directories, completed
  orphans, and finalization claims without manufacturing success or
  duplicating an accepted slot.
- [x] Add fault injection at every database, synchronization, rename, and
  finalization boundary, plus concurrent-publication tests for identical
  content.

### 2.3 Local executor and timing policy

See [Problem and fairness contracts](DESIGN.md#problem-and-fairness-contracts)
and [Environments, execution, and
artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [x] Implement the reusable executor interface and a POSIX local-process
  executor with process-group containment, bounded termination, isolated
  directories, and explicit capability preflight.
- [x] Build minimal, allowlisted worker environments with fixed locale,
  timezone, umask, working directory, private home/temp/cache locations, and
  redacted secret channels.
- [x] Measure and record materialization, startup, prepare, execute request,
  timed wall, evaluation, CPU, and peak-memory fields with explicit missing
  or partial values.
- [x] Implement `cold_end_to_end`, `prepare_and_execute`, and `execute_only`
  envelopes, scientific and operational timeouts, and all compatibility
  rules for reuse and warm-ups.
- [x] Implement worker reuse keys, reset and replacement behavior, cache policy,
  and repeated warm-ups after worker replacement.
- [x] Record every requested control as enforced, best-effort, or unsupported;
  fail known-insufficient required controls during planning and runtime
  enforcement loss as a typed attempt.

### 2.4 Scheduling, retries, and inspection

See [Schemas, canonical values, and
identities](DESIGN.md#schemas-canonical-values-and-identities) and
[Environments, execution, and
artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [x] Implement sequential scheduling first, then deterministic block-randomized
  scheduling by dataset instance, problem configuration, implementation
  repetition, and measurement index.
- [x] Preserve stable relative ordering when unrelated slots are added, and
  verify the exact `A x M` slot count and specified seed sharing.
- [x] Add opt-in bounded concurrency as execution-policy identity and
  provenance; keep performance execution serial by default.
- [x] Implement selected-ID and bounded execution, explicit retries, immutable
  retry attempts, superseded concurrent candidates, and `run --resume`
  without repeating accepted slots.
- [x] Implement `metewand status` and `metewand explain <id>` over stable
  definition, run, slot, attempt, artifact, and diagnostic identities.

### 2.5 SDKs and conformance suites

See [Initial SDK APIs](DESIGN.md#7-initial-sdk-apis).

- [x] Build the Python SDK helpers for implementations, dataset materializers,
  and evaluators without exposing orchestrator responsibilities.
- [x] Build the equivalent R SDK helpers with the same protocol behavior and
  canonical-value restrictions.
- [x] Measure only the user implementation callback as SDK
  `implementation_time`, while writing the result before replying to
  `execute`.
- [x] Run canonicalization, identity, seed, error, and protocol golden vectors
  across Rust, Python, and R.
- [x] Add reference problem-contract suites that accept correct implementations
  and reject malformed, numerically wrong, and semantically invalid results.

### 2.6 Authoring, exports, and local examples

See [Authoring and scaffolding](DESIGN.md#authoring-and-scaffolding) and
[Environments, execution, and
artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [x] Implement `metewand init` as a dry-runnable, no-overwrite operation that
  creates only a minimal, domain-neutral versioned manifest.
- [x] Implement explicit dataset, problem, and implementation scaffolds; report
  every changed path, update only the requested manifest component, and do
  not invent scientific semantics or invoke package managers.
- [x] Implement `metewand check --workers` with capability verification and
  problem reference-case execution.
- [x] Implement a rebuildable JSONL index and tidy JSONL/CSV export that retain
  valid observations and accepted, invalid, censored, superseded, and failed
  attempts without silently combining incompatible timing or guarantee
  classes.
- [x] Add exact-target cache verification and garbage collection from a
  transactional database snapshot, with root and lease awareness, a dry-run
  mode, and no deletion authorized solely by a stale index.
- [x] Add representative raw-command, Python-only, R-only, dataset-free, and
  parameter-matrix local examples without privileged built-in scientific
  templates.
- [x] Pass Gate 2 acceptance tests for archive safety, dataset mutation, timing
  scopes, process escape, log flooding, control loss, retries, crash
  recovery, cache concurrency, garbage collection, exports, and the complete
  agent loop.

The complete agent-loop acceptance test is opt-in; see the command in
[Development](README.md#development). The normal suite retains focused coverage
of the individual commands and failure modes.

## 3. Gate 3: comparative local MVP

This is the first general MVP. It adds the applicability and convergence-profile
semantics required by the motivating research workflows, the Julia SDK, and a
portable publication handoff while retaining development-environment and
observed-control guarantees. See [Gate
3](DESIGN.md#gate-3-comparative-local-mvp).

Implement this gate as vertical slices. Start the motivating workflow ports
before extending the profile machinery, and grow their smoke tests with each
slice. Publish each schema with its first producer and consumer rather than
freezing all Gate 3 interfaces up front. Each execution slice includes its
Rust fixtures, R and Python helpers, durable records, inspection, export, and
user-facing documentation before the next delivery mode begins.

### 3.1 Workflow fixtures and shared prerequisites

See [Problem and fairness contracts](DESIGN.md#problem-and-fairness-contracts)
and [Main domain types](DESIGN.md#main-domain-types).

- [ ] Extract smoke-sized inputs, independent evaluators, expected results, and
  acceptance cases from the intercept-strategy, SLOPE, and internal
  clustered-lasso workflows. Run their one-shot portions on the Gate 2
  foundation, and document the missing applicability and profile behavior.
- [ ] Review the manifest, public-schema, wire-protocol, identity, and
  execution-semantics compatibility constants before introducing new fields or
  capabilities. Add conformance vectors with each slice; the design-only
  extension does not change existing version-1 bytes.
- [ ] Preserve the distinction between logical definitions and slots and their
  resolved execution identities as applicability and profiles extend them.
  Keep environment fingerprints separate from exact worker launch identities,
  including development environments, so later locking adds verification
  without redefining the identity model.
- [ ] Validate canonical problem-defined scientific budget values against each
  contract's budget schema and every selected implementation's declared
  capability without conflating them with observation controls.

### 3.2 Configuration-dependent applicability

See [Schemas, canonical values, and
identities](DESIGN.md#schemas-canonical-values-and-identities) and [Worker
protocol](DESIGN.md#4-worker-protocol).

- [ ] Publish the versioned applicability-decision schema and extend identities
  so checked inputs and decisions invalidate exactly their dependent records.
- [ ] Represent applicability as `unchecked`, `applicable`, or `not_applicable`
  with typed evidence; make a side-effect-free plan report candidate,
  applicable, observation-slot, and execution-slot counts without claiming
  that a worker was run.
- [ ] Implement the optional `check_applicability` exchange before measured
  preparation, passing the immutable dataset view and manifest, exact
  contract and problem parameters, and implementation parameters.
- [ ] Validate stable reason codes and structured details, hash and durably
  record each decision, and verify dataset immutability around the check.
- [ ] Produce no measured slots for a not-applicable candidate, but retain it in
  plans, status, explanation, indexes, and exports without classifying it as
  an attempt failure.
- [ ] Add fixtures for unsupported intercepts, sparse layouts, and
  dimension/implementation-parameter combinations, plus fixtures proving
  that the exact checked inputs and reasons remain visible for scientific
  review.
- [ ] Add R and Python applicability helpers and grow the motivating smoke
  fixtures to cover structured exclusions through the public SDKs.

### 3.3 Fresh-sequence profiles

See [Problem and fairness contracts](DESIGN.md#problem-and-fairness-contracts).

- [ ] Publish versioned profile-policy, observation-control, schedule, and
  extended run-observation schemas. Add conformance vectors proving that
  policies, schedules, controls, and observation slots invalidate exactly their
  dependent records.
- [ ] Validate `one_shot` and fresh-sequence `profile` policies, metric names and
  directions against problem metric schemas, finite explicit/linear/geometric
  schedules, delivery capabilities, maximum observation counts, and active
  time bounds.
- [ ] Expand each implementation's explicit, linear, or geometric control
  schedule into stable observation slots without treating controls from
  different implementations as comparable budgets.
- [ ] Execute every fresh observation from clean algorithmic state while
  preserving the execution policy's process-reuse and warm-up semantics.
- [ ] Evaluate every result independently, apply fixed-count or evaluator-owned
  metric-target completion, and mark later unneeded observation and
  execution slots as `skipped_target_reached` without creating observations.
- [ ] Extend block-randomized scheduling with deterministic observation rounds
  that preserve each implementation's schedule order and stable relative
  ordering when unrelated candidates are added.
- [ ] Add R and Python fresh-sequence helpers and exercise the SLOPE workflow's
  iteration and tolerance schedules, target stopping, partial results, resume,
  inspection, and exports before adding streaming.

### 3.4 Streaming profiles and durable observations

See [Worker protocol](DESIGN.md#4-worker-protocol) and [Environments, execution,
and artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [ ] Publish versioned checkpoint event and decision schemas, validate streaming
  delivery policies, and add identity and protocol conformance vectors.
- [ ] Implement the bounded checkpoint event/decision subexchange inside one
  active `execute_stream` request, including strict sequence, count, path,
  and continuation-after-stop validation.
- [ ] Pause at each complete checkpoint, validate and independently evaluate it,
  durably publish its observation, and return only a `continue` or `stop`
  decision based on evaluator-owned metrics and policy bounds.
- [ ] Record cumulative executor-observed active wall and process-tree CPU time
  at each checkpoint, include result serialization, exclude evaluator
  pauses, and retain total elapsed wall and evaluation time separately.
- [ ] Retain finalized valid observations when a later checkpoint times out,
  crashes, or fails, while leaving the enclosing attempt censored or failed.
  On retry, restart clean state unless a future restoration capability is
  explicitly selected; never claim implicit state recovery.
- [ ] Add fault injection around every checkpoint publication and decision
  boundary, proving that resume selects at most one valid observation per
  slot and never manufactures an accepted attempt.
- [ ] Add R and Python streaming callbacks and exercise the SLOPE smoke workflow
  through the public SDKs, including partial-curve inspection and exports.

### 3.5 Buffered instrumented profiles and local observers

See [Instrumented profiles and local
observers](DESIGN.md#instrumented-profiles-and-local-observers) and [Initial
SDK APIs](DESIGN.md#7-initial-sdk-apis).

- [ ] Publish versioned buffered-trace and snapshot-manifest schemas and their
  identity and protocol conformance vectors. Validate opt-in instrumented
  scopes, measurement bases, thresholds, instrumentation schemas, check
  schedules, trace and byte bounds, and reserved terminal-observation capacity.
  Keep provisional metrics distinct from independent evaluator metrics.
- [ ] Implement the negotiated `buffered_profile` capability and bounded
  `execute_buffered` response, with one state-preserving solve, a trace, selected
  immutable canonical snapshots, clock metadata, and a provisional stop reason.
- [ ] Provide equivalent direct recording and local
  `observe(progress, measure, snapshot) -> continue | stop(reason)` helpers in
  Rust, R, and Python. Support native loops without a host-language callback or
  protocol round trip at each check; finish with an explicit terminal check
  when needed.
- [ ] Apply declared schedules, first-observed threshold recording, local
  stopping, and collection bounds without silently dropping observations.
  Snapshot the checked iterate before subsequent mutation, deduplicate snapshots
  serving several thresholds, and reserve terminal capacity.
- [ ] Include metrics, callback dispatch, and prior snapshot work in cumulative
  local time; record each timestamp before its own snapshot copy. Retain total
  instrumented and executor-observed durations and independently enforce the
  scientific timeout over the selected scope, including serialization.
- [ ] Validate the returned buffer, independently evaluate every saved result,
  and publish observations in capture order. Preserve provisional metrics,
  evaluator-confirmed target status, mismatch outcomes, measurement evidence,
  and diagnostic-only unsaved trace rows in exports and snapshots.
- [ ] Test that one trajectory serves several thresholds only under unchanged
  solver settings, callback and direct recording agree, and instrumentation
  preserves the trajectory apart from its declared stopping rule. Cover
  decreasing times, malformed indices, mutated snapshots, metric disagreement,
  ignored stop decisions, oversized buffers, terminal checks, and bound exits.
- [ ] Add crash and recovery fixtures proving that incomplete buffers publish
  no accepted points, independently finalized returned snapshots survive later
  failures, and retries never splice trajectories. Refuse silent pooling with
  executor-observed curves or different instrumented scopes.
- [ ] Exercise the internal clustered-lasso smoke workflow through direct traces
  and local observers before extending the same APIs to Julia.

### 3.6 Julia SDK and cross-language conformance

See [Initial SDK APIs](DESIGN.md#7-initial-sdk-apis).

- [ ] Build Julia helpers for implementation, dataset-materializer, evaluator,
  applicability, fresh-sequence, and streaming-profile callbacks without
  exposing orchestrator responsibilities; add buffered recording and local
  observers with the same timing and validation semantics.
- [ ] Include Julia in canonicalization, identity, seed, error, framing, and
  protocol golden vectors alongside Rust, R, and Python.
- [ ] Add a local multi-runtime fixture in which Julia materializes one dataset
  and Julia, R, and Python implementations return the same canonical result
  for one independent evaluator.

### 3.7 Export lineage and portable snapshots

See [Environments, execution, and
artifacts](DESIGN.md#6-environments-execution-and-artifacts).

- [ ] Publish versioned export-lineage and portable result-snapshot schemas.
- [ ] Emit an export manifest containing the canonical selector, ordered source
  identities, schema and compatibility identities, tool version, and hashes
  of every JSONL, CSV, or Parquet output.
- [ ] Implement `metewand snapshot create` for an exact selected result set,
  including applicability, attempt, observation, result, metric, schema,
  contract, resolved-dependency, lock-state, and provenance records without
  cache or workspace paths.
- [ ] Implement offline `snapshot verify` over the complete Merkle graph and
  atomic, no-replace `snapshot import` after verification.
- [ ] Add corruption, omission, path-traversal, incompatible-schema, duplicate
  object, and round-trip tests; prove that snapshotting preserves rather
  than upgrades recorded guarantee classes.

### 3.8 Complete motivating workflow ports and qualify the MVP

- [ ] Complete the smoke-sized intercept-strategy grid with a seeded Julia
  materializer, Julia/R/Python implementations, paired data across
  strategies, per-cell resume, pinned pre/post implementation variants, and
  an export used to compute a shared downstream summary.
- [ ] Complete the smoke-sized SLOPE comparison with dense and sparse datasets,
  coupled dataset cases, iteration and tolerance fresh schedules, one
  callback stream, conditional implementation applicability, a common
  evaluator-owned relative-duality-gap target, and timeout censoring.
- [ ] Complete the smoke-sized internal clustered-lasso comparison with shared
  inputs, native local observers and direct traces, several gap thresholds from one
  trajectory, independent snapshot evaluation, and separate instrumented and
  executor-observed timing evidence. Keep internal and package-track conclusions
  separate, and preserve historical measurements under their original scopes.
- [ ] Document the resulting adapter, schema, and manifest surface and simplify
  it until the ports are materially easier to inspect and resume than their
  source workflows; keep their smoke tests release-blocking.
- [ ] Pass Gate 3 acceptance tests for applicability, observation identities,
  fresh resets, checkpoint protocol and timing, target stopping, durable
  partial curves, buffered timing and local stopping, Julia conformance, export
  lineage, snapshot round trips, and all three motivating ports.

## 4. Gate 4: locked environments

Add locking without making any environment backend mandatory for benchmark
authors. See [Reproducibility model](DESIGN.md#5-reproducibility-model), [Nix
environments](DESIGN.md#nix-environments), and [Gate
4](DESIGN.md#gate-4-locked-environments).

### 4.1 Resolution and lockfile model

- [ ] Extend the existing logical and resolved identity model with locked
  environment evidence, preserving the separation of environment fingerprints
  and exact worker launch identities established before this gate.
- [ ] Define and validate `metewand.lock`, binding it to the normalized manifest
  while preserving identities for unaffected components.
- [ ] Add backend environment resolution separately from runner launch
  resolution; preserve one environment fingerprint across several distinct
  launch programs.
- [ ] Implement `metewand lock` with explicit network/build/write reporting and
  atomic lockfile replacement only when the command was directly requested.
- [ ] Implement `metewand run --locked` so every definition, source, artifact,
  environment, executable, SDK, protocol, executor, observation policy,
  applicability decision, and semantics identity must match before
  execution.

### 4.2 Nix package and app environments

- [ ] Resolve exact `packages.<system>` outputs without fallback lookup,
  recording filtered local inputs, `flake.lock`, installable, system,
  selected output, Nix version/configuration, derivation, and realized
  paths.
- [ ] Record output NAR hashes and a recursive closure manifest, create explicit
  GC roots, and verify the realized closure before locked execution.
- [ ] Resolve package runners to normalized executable paths inside a designated
  output and launch them directly, never through `nix run` or `nix develop`.
- [ ] Resolve app outputs to their exact store-resident program and keep worker
  arguments in the worker definition.
- [ ] Reject impure evaluation, unsupported Nix versions, lockfile rewrites,
  executable escapes, and changes to local inputs, derivations, outputs,
  programs, or recursive closures.
- [ ] Add a multi-runtime Nix fixture that launches Python, R, and Julia from
  one environment fingerprint through distinct exact launch specifications;
  exercise a Devenv repository through standard flake outputs without a
  separate environment backend.

### 4.3 Ecosystem-locked environments

- [ ] Implement `uv` environment resolution, recording the native lockfile,
  package-manager version, platform tags, source repositories, downloaded
  artifact hashes, realized environment manifest, and exact interpreter.
- [ ] Implement the corresponding `renv` resolution and verification model for R
  projects and their exact interpreter.
- [ ] Treat local and editable dependencies as declared source bundles, and run
  locked environments offline after verifying their realized manifests and
  launch programs.
- [ ] Add Nix-locked, `uv`-locked, `renv`-locked, and mixed
  R/Python/Julia/native examples.
- [ ] Pass Gate 4 mismatch tests for every transitive source, schema, contract,
  fixture, adapter, helper, native lockfile, environment artifact, closure,
  resolved launch program, applicability decision, observation policy, and
  control schedule.

## 5. Gate 5: isolated archival execution

This gate supports the strongest publication-archive claim while continuing to
treat benchmark code as trusted input. See [Gate
5](DESIGN.md#gate-5-isolated-archival-execution).

### 5.1 OCI environments and executor

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

### 5.2 Archival provenance and qualification

The earlier execution slices already record provenance and expose environment,
control, and timing compatibility classes. This slice completes archival
coverage and enforces the stronger policy requirements.

- [ ] Record hardware, operating system, kernel, selected cores, memory,
  runtime, language, BLAS, thread settings, repository revision and dirty
  hash, plus optional governor, turbo, NUMA, accelerator, and
  background-load observations.
- [ ] Represent unavailable provenance explicitly, and enforce policy-selected
  required fields before execution.
- [ ] Verify that archival execution preserves the existing separation of
  attempt acceptance, environment reproducibility, and observed control classes
  in records and exports.
- [ ] Extend compatibility readers to reject pooling across archival control
  classes or incomplete accounting, preserving the existing timing-scope and
  environment-class checks.
- [ ] Add digest-mismatch, network-denial, resource-limit, read-only-mount,
  escaped-process, accounting, and provenance tests for Docker and Podman.
- [ ] Exercise the archival path with an OCI-pinned mixed-language benchmark and
  audit every user-facing acceptance scenario and release-blocking test in
  [MVP and acceptance criteria](DESIGN.md#8-mvp-and-acceptance-criteria).

## Deferred work

These items begin only after Gate 5. Their order remains deliberately unset
until the version-1 execution model is reliable.

- [ ] Adaptive or richer schedules for problem-defined scientific budgets.
- [ ] Contracted checkpoint-state serialization and restoration.
- [ ] Julia `Pkg` environments.
- [ ] Conda environments shared across multiple language runners.
- [ ] BenchExec or another stronger isolation backend.
- [ ] SLURM and remote execution.
- [ ] Immutable, provenance-tracked analysis artifacts.
- [ ] Read-only interactive diagnostic viewer.
- [ ] Optional MCP adapter over the existing CLI semantics.

## Guardrails

The following remain out of scope for version 1; see [Explicit
non-goals](DESIGN.md#explicit-non-goals-for-v1).

- Do not create a universal package manager or replace native lockfiles.
- Do not embed language runtimes or transfer native language objects.
- Do not build a general workflow engine, generic sandbox, or distributed
  service.
- Do not embed a language model or provider-specific agent orchestration.
- Do not add publication plotting, figure composition, or statistical
  conclusions to the execution core.
- Do not claim bitwise-identical results or performance across machines.
