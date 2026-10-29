# Metewand roadmap

The next milestone is a comparative local MVP that can run useful benchmarks in
`../basin` and `~/research/clustlasso-cd`. Keep one-shot execution, fresh
sequences, and buffered instrumented profiles. Defer streaming profiles, the
Julia package, and portable result archives.

[DESIGN.md](DESIGN.md) defines semantics. [AGENTS.md](AGENTS.md) defines
development and verification rules. Build the smallest usable step in the active
roadmap, test its behavior, and exercise it in an adopter workflow.
Compatibility work and documentation should match the changed boundary; they are
not separate gates before implementation.

## Completed foundation

### Gate 1: conformance kernel

- [x] Implement canonical JSON, offline schema validation, typed manifests and
  problem contracts, deterministic expansion, identities, and seeds.
- [x] Implement the bounded worker protocol, read-only checking and planning,
  independent evaluation, and a complete fixed-dataset execution path.

### Gate 2: reliable local foundation

- [x] Implement source resolution, generated datasets, verified artifact
  caching, private input views, durable publication, recovery, and explicit
  retries.
- [x] Implement local execution, timing scopes, worker reuse and warm-ups,
  deterministic scheduling, bounded concurrency, status, explanation, and
  exports.
- [x] Add Python and R packages, cross-language contract tests, authoring
  commands, cache maintenance, and runnable local examples.
- [x] Add one-shot scientific smoke cases for intercepts, SLOPE, and clustered
  lasso, and pin the currently implemented compatibility boundaries.
- [x] Support local Python, R, and Rust workers on Linux, macOS, and native
  Windows x64, including one-shot runs, fresh sequences, buffered profiles,
  caching, recovery, inspection, and exports. Verify native Windows on local
  NTFS in CI, with explicit process-death recovery guarantees.

These capabilities and their existing regression tests remain the starting
point. The complete agent-loop test and exhaustive crash campaign remain opt-in;
see [Verification](CONTRIBUTING.md#verification).

## 3. Gate 3: comparative local MVP

### Adopter workflows

  | Project       | Reuse first                                                                                                                                              | First useful result                                                                                                                                                |
  | ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
  | Basin         | `../basin/crates/competitor-bench`, including the existing comparison harnesses and `src/bin/trace.rs`.                                                  | Repeated comparisons of Basin and an existing competitor on shared cases, independent solution checks, instrumented curves, and a usable export.                   |
  | clustlasso-cd | `~/research/clustlasso-cd/experiments/package_track`, its independent package evaluator, and the native trajectories used by `experiments/real_data.py`. | Current Python and R package adapters running on the local runner, followed by internal solver curves with selected independently evaluated coefficient snapshots. |

Keep scientific definitions, adapters, and retained benchmark outputs in the
adopter repositories. Keep small regression cases in Metewand that can run
without the adopter dependencies. Reuse existing inputs and evaluators rather
than creating another scientific protocol or repeating full experiment
campaigns. The older intercept and SLOPE smoke cases remain useful regression
coverage; full ports of those studies do not block this MVP.

### 3.1 Useful one-shot runs and compact authoring

Start the adopter integrations on the existing runner while simplifying the
declarations they actually need. Neither a generic schema-generation rewrite nor
a new compatibility review is a prerequisite.

- [x] Run one Basin method and an existing competitor through the process
  boundary on a small shared case. Retain final parameters, independently
  checked objective or feasibility metrics, settings, repeats, and failures.
  Basin's `../basin/metewand.yaml` compares Basin and argmin Nelder–Mead on
  Rosenbrock, retaining three repeats at each of two iteration caps. Its
  `crates/competitor-bench/metewand/` directory contains the independent
  evaluator, reference cases, reproduction commands, and a verified run with
  six accepted results and six scientific completion failures.
- [x] Refresh the existing clustlasso-cd package integration for the current
  YAML CLI. Reuse its fixed inputs, Python and R adapters, and independent
  evaluator, and retain the original package track's scientific criteria.
  Its `experiments/package_smoke.py` now stages YAML manifests and the YAML
  problem contract. The retained `outputs/package-smoke-yaml-v4` run and
  source-verified audit cover 24 oracle fits, four dense fits, 116 Python/R
  input round trips, five timing probes, and nine expected failures. See the
  adopter's `docs/package-integration.md` for reproduction commands.
- [x] Allow schemas and problem contracts inline as well as through reusable
  file references. Resolve references offline, validate the same effective
  declarations, and hash their normalized contents.
- [x] Support compact implementation `parameters` with types, bounds, enums,
  arrays, optional fields, and validated literal defaults. Lower them into
  the existing schema and default records, expose effective parameters in
  plans, and exercise compact/explicit identity equivalence and defaulted
  execution in the parameter-matrix example.
- [x] Extend compact declarations to dataset and problem `parameters`, `result`,
  and `metrics`, and add declaration-level default grids. These declarations
  generate normalized JSON Schemas. Declare each field once, with its type,
  bounds, and any parameter default or default grid together. Keep
  `implementations.<name>.parameters` separate from worker settings such as
  `runner` and `entrypoint`. Types and bounds define the full valid domain;
  grids select runs without narrowing that domain. Treat block and flow YAML
  mappings identically, require fields and close objects by default, and
  retain explicit schemas for complex or reusable contracts.
- [x] Let authors define a problem's parameters, `result`, `metrics`, scientific
  rules, reference cases, and evaluator directly under `problems.<name>`
  without an inline `contract` wrapper. Normalize these fields into the same
  internal contract and identities. Preserve referenced contracts and
  supported explicit inline contracts; reject competing declarations of the
  same boundary.
- [x] Finish compact parameter overrides and diagnostics. Declaration defaults
  and grids now use the existing resolution and expansion model, with
  experiment axes overriding declaration grids and named cases overriding
  experiment axes. Preserve recursive merging of literal defaults. Accept
  plain scalar literals in experiment and case parameters, retaining
  explicit `value` and `grid` forms for arrays and objects. Validate all
  effective values, record resolved settings, and keep equivalent compact
  and explicit declarations on the same identity path. Report errors at the
  author's YAML field or supplied value rather than a generated schema
  location.
- [x] Default an omitted parameter schema to a closed empty object. Allow
  unambiguous local-environment and one-shot-policy defaults, recording all
  resolved values and exposing effective settings through planning. Keep
  profile selection explicit. Scientific rules and timing controls can
  refine the basic defaults described below.
- [x] Make a separately authored family `semantics-schema` optional. Keep
  declared preprocessing, validity, and completion rules in the problem
  declaration, with an independent evaluator and optional reference cases.
  Pass the resolved scientific rules to the evaluator so authors do not
  repeat declared tolerances or other constants in evaluator code.
- [x] Provide small Rust worker helpers for Basin and improve the Python and R
  helpers where adopters still write protocol boilerplate. Let simple
  one-shot adapters return result data, and evaluators return metric data
  and an explicit verdict; helpers handle handshakes, transport, errors,
  standard envelopes, and artifact writing. Keep preparation, solving,
  scientific conversion, and metric formulas in adapters. Retain lifecycle
  hooks and custom artifact writers for more involved cases, preserve timing
  boundaries, and support native process workers without requiring a Python
  host.
- [x] Update scaffolding and one small example so a benchmark needs one
  manifest, small adapters, and scientific reference fixtures. Separate
  contract and schema files remain available for reuse. Let authors select a
  worker language and generate small worker adapters with scientific
  decisions clearly left to the author. Test invalid declarations, override
  precedence, normalized defaults, and equivalent declaration identities
  without duplicating the example into a fixture matrix.
- [x] Verify the authoring workflow with two existing methods, an independent
  evaluator, and reference data. Configure three measurement repetitions per
  method and include a case that misses its completion target. Run and
  inspect the comparison using one manifest and small adapters, without
  hand-written protocol handling or separate JSON Schema files. Check that
  metrics, effective settings, timings, and failure reasons are
  understandable from the public CLI. Use this comparison to assess a
  Benchopt-style migration: reuse dataset, solver, and evaluation logic,
  adding small adapters and declarations without repeating scientific
  settings or implementing artifact envelopes.
- [x] Preserve Python virtual-environment context when resolving interpreter
  symlinks. The [Benchopt SLOPE comparison](examples/benchopt-slope)
  verified the original failure; the local runner now retains the selected
  interpreter path, with a regression test for imports from a symlink-based
  venv.
- [x] Make callback exception details inspectable after failed attempts.
  Packages include bounded exception text in persisted failure messages,
  available through `status` and `explain` without retained worker logs.

### 3.1.1 Start small and add rigor

- [x] Make `init` produce a runnable two-file Python benchmark, with an R option
  and an `--empty` escape hatch. Preflight all targets, preserve overwrite
  protection, and report package setup and next commands.
- [x] Add ordinary function workers with matching contexts in Python and R.
  Infer the standard JSON inventory, sole problem binding, one-shot
  capability, local execution, and final-observation defaults before
  planning. Keep static checks free of imports and preserve lifecycle and
  custom artifact APIs.
- [x] Make scientific rules and reference cases optional. Distinguish absent
  completion criteria from convergence. Require explicit verdicts for
  declared rules and support inline references with expected verdicts.
- [x] Render shared typed comparison rows for runs and stored status, including
  names, settings, repetitions, metrics, timing scopes, retries, and
  failures. Retain labels in provenance and report unavailable historical
  evidence honestly.
- [x] Verify both starters through execution, resume, inspection without the
  manifest, and exports. Add a small simulated least-squares GD/CD example
  with basic and validated manifests using the same scientific functions.

The authoring design is in [Authoring](DESIGN.md#authoring). Judge ergonomics by
the complete comparison, including native workers. Existing external
declarations, explicit policies, and advanced worker APIs remain supported.

### 3.2 Configuration-dependent applicability

- [x] Add the optional `check_applicability` exchange before measured
  preparation. Record `unchecked`, `applicable`, and `not_applicable`
  outcomes with stable reason codes and the exact checked inputs.
- [x] Bind applicability evidence to dependent execution identities, protect the
  shared dataset during the check, and create no measured slots for an
  excluded candidate.
- [ ] Keep excluded candidates visible in plans, status, explanation, and
  exports. Exercise a real representation or dimension restriction from an
  adopter and distinguish it from a failed attempt.

### 3.3 Buffered instrumented profiles and local observers

Prioritize this delivery mode because both adopters already collect local
traces. Native loops must work without a host-language callback or protocol
round trip at every observation.

- [x] Introduce the shared profile policy and observation fields with this first
  producer and consumer, and reuse them across the remaining modes.
- [x] Add bounded `execute_buffered` delivery with direct recording and
  `observe(progress, measure, snapshot) -> continue | stop(reason)`. Rust
  and Python helpers support native buffer submission without per-check host
  callbacks. Adoption in Basin and clustlasso-cd remains below.
- [x] Validate clock and timing scope, check schedules, count and byte bounds,
  monotonic times, selected immutable snapshots, and a required terminal
  check. Include measurement, callback, and earlier snapshot work in local
  time; retain the executor-observed duration separately.
- [x] Independently evaluate saved results and retain provisional local metrics
  separately. Unsaved trace rows remain diagnostics. Do not turn existing
  metric-only trajectories into independently verified observations.
- [x] Support several thresholds from one unchanged trajectory, deduplicate
  snapshots, and reserve terminal capacity. Preserve numerical behavior
  apart from the declared stopping rule.
- [ ] Exercise a Basin trace and a clustlasso-cd native trajectory with selected
  result snapshots. Test metric disagreement, snapshot mutation, bounds,
  partial publication, and failure without splicing retries or accepting an
  unreturned buffer.

The [buffered profile example](examples/buffered-profile) covers both packages,
fixed counts, targets, inspection, resume, and exports. Focused tests cover
metric disagreement, bounds, snapshot mutation, partial publication, unreturned
buffers, and retry isolation. Adopter repository changes are deferred.

See [Instrumented profiles and local
observers](DESIGN.md#instrumented-profiles-and-local-observers) for the timing
and scientific semantics.

### 3.4 Fresh-sequence profiles

- [x] Add fixed-count and common-metric-target profiles with finite explicit,
  linear, or geometric implementation-control schedules. Validate controls
  without claiming equal iterations or tolerances mean equal scientific
  work.
- [x] Run each observation from clean algorithmic state, preserving the
  execution policy's worker-reuse and warm-up rules. Independently evaluate
  every returned result.
- [x] Keep deterministic observation ordering and identities. Stop on the
  evaluator-owned target, label later slots `skipped_target_reached`, and
  preserve valid below-target observations and censored or failed attempts.
- [x] Exercise a black-box adapter through the public protocol and the needed
  Python and R helpers. Test clean resets, target stopping, timeout, resume,
  inspection, and export.

The [fresh-sequence example](examples/fresh-sequence) exercises Python bisection
and R's `uniroot` through the public protocol. CLI tests cover selected points,
mixed delivery, failure continuation, target skips, timeout, explicit retries,
resume, inspection, and both exports. Layout-3 crash conformance preserves time
reservations, conservative interrupted charges, and terminal target decisions.

### 3.5 Qualify the local MVP

Each delivery mode must be usable through the CLI and its needed adapters.
Extend package callbacks where an adopter needs them; completing every callback
in every language is not a release prerequisite.

- [x] Run `check`, `plan`, selected execution, interruption, resume, inspection,
  and JSONL and CSV export on small Basin and clustlasso-cd comparisons.
- [x] Keep both MVP profile modes covered by small end-to-end cases. Retain
  focused tests for scientific validity, timing, stopping, failure evidence,
  and recovery, plus the existing foundation regressions.
- [x] Measure orchestration and result-serialization overhead in release builds
  using a small controlled worker. Report primary wall timing and worker
  diagnostics with their scopes so users can judge whether overhead obscures
  their workload.
- [x] Preserve logical versus resolved identities, independent evaluator
  metrics, and timing and guarantee classes through durable records and
  exports. Add only problem-defined scientific budgets needed by these
  pilots.
- [x] Bind available build evidence to worker and runner executable hashes.
  Capture source state, compiler, profile, features, flags, and lockfile
  hashes during builds, and distinguish that evidence from post-run host
  observations.

## After the local MVP

Choose these by demonstrated adopter need rather than requiring another complete
gate before improving a useful workflow.

- [ ] Add portable result archive creation, offline verification, and import.
  This is separate from the result snapshots required by profile modes.
- [ ] Complete full intercept-strategy and SLOPE study ports if still useful.
- [ ] Extend export formats and provenance packaging when an analysis needs more
  than the existing JSONL and CSV tables and lineage receipt.

### Streaming profiles

- [ ] Add one active `execute_stream` request with bounded checkpoint and
  decision exchanges. Validate sequence, count, paths, and stopping
  behavior.
- [ ] Pause at each checkpoint, independently evaluate and durably publish the
  result, then reply `continue` or `stop` from evaluator metrics and policy
  bounds.
- [ ] Include checkpoint serialization in active wall and process-tree CPU time;
  exclude evaluation pauses and retain elapsed and evaluation time
  separately.
- [ ] Preserve published observations after later timeout or failure. Retry from
  clean state without claiming implementation-state restoration.
- [ ] Exercise a cooperative adapter and Python helper. Test interruption at
  publication and decision boundaries, continuation after stop,
  partial-curve exports, and resume selecting at most one observation per
  slot.

### Gate 4: locked environments

- [ ] Add `metewand.lock` and `run --locked`, preserving logical identities
  while binding resolved environments and exact worker launches.
- [ ] Implement Nix package and app resolution first, with verified sources,
  lockfiles, realized outputs, closures, and executable paths.
- [ ] Add `uv` and `renv` resolution as adopters need them, including local
  dependencies and offline verification. Keep Nix optional.
- [ ] Test transitive mismatches and distinguish locked guarantees from
  development environments.

See [Reproducibility model](DESIGN.md#reproducibility-model) and [Nix
environments](DESIGN.md#nix-environments).

### Gate 5: isolated archival execution

- [ ] Add digest-pinned OCI environments and Docker and Podman execution on
  Linux.
- [ ] Enforce requested resource, network, read-only input, and process-tree
  controls, with explicit failures when required enforcement is unavailable.
- [ ] Complete archival machine provenance and accounting, and qualify a
  mixed-language benchmark under those stronger guarantees.

See [Reproducibility model](DESIGN.md#reproducibility-model).
Publication-archive claims require these controls; they do not block the local
MVP.

## Later extensions

- Adaptive scientific-budget schedules and checkpoint-state restoration.
- Julia `Pkg`, Conda, and stronger isolation backends.
- SLURM and remote execution.
- Provenance-tracked analysis artifacts and a read-only diagnostic viewer.
- An optional MCP adapter over the CLI.

## Guardrails

Keep processes as the language-neutral boundary, version public formats, and
preserve deterministic identities and records. Do not build a universal package
manager, embed runtimes, add a distributed workflow service, or put statistical
conclusions and publication plotting in the execution core. Never claim
bitwise-identical results or performance across machines.
