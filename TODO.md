# Metewand TODO

Remaining work is grouped by area. Prioritize useful benchmarks in Basin and
clustlasso-cd.

The [design](docs/design.md) defines the general model.
[CONTRIBUTING.md](CONTRIBUTING.md) covers setup, testing, and submitting
changes, and [AGENTS.md](AGENTS.md) gives repository instructions.

## Adopter integrations

Keep scientific definitions, adapters, and retained benchmark outputs in the
adopter repositories. Keep small regression cases in Metewand that can run
without the adopter dependencies.

- [ ] Exercise a Basin trace through buffered profiles with selected result
  snapshots. Reuse `../basin/crates/competitor-bench`, including
  `src/bin/trace.rs`.
- [ ] Exercise a clustlasso-cd native trajectory through buffered profiles with
  selected independently evaluated coefficient snapshots. Reuse
  `~/research/clustlasso-cd/experiments/real_data.py` and the existing
  independent evaluator.
- [ ] Complete full intercept-strategy and SLOPE study ports if still useful.

Use the [buffered profile example](examples/buffered-profile) as a starting
point. Exercise metric disagreement, snapshot mutation, bounds, partial
publication, and failures in the adopter workflows. Preserve retry isolation and
reject unreturned buffers.

## Performance and storage

Prioritize memory that grows with completed results and validation inside the
measured interval. Compare release-build peak RSS, wall time, scratch usage, and
retained bytes and file counts as workloads grow. Start with the long buffered
trace test and the orchestration overhead benchmark.
[Long buffered trace test](tests/cli/archives.rs) ·
[Orchestration overhead benchmark](examples/overhead)

### Export

- [x] Build JSON analysis directly from observation and coordination records.
  Resolve exact `--id` selectors before expanding traces and preserve lineage.
  The 25,001-point fixture passes under the same 256 MiB Linux address-space
  limit as archive validation.
- [x] Stream CSV, JSONL, and index output in deterministic order while sharing
  attempt evidence across trace points and metrics. Release builds produced
  identical v1 bytes on the smaller fixtures. For 1,001 points, JSONL retained
  107 MiB, CSV retained 115 MiB, and the index retained 106 MiB.

### Execution and inspection

- [ ] Release inline result data from progress and summary state after durable
  publication. Render complete machine output incrementally from saved
  results. Exercise human, JSON, and JSON Lines output with large results
  and concurrent attempts.
- [ ] Stream live buffered trace validation and retain the points referenced by
  snapshots. Track manifest byte counts incrementally in the Rust and Python
  collectors and in the validator. Check terminal reserves, first threshold
  crossings, and malformed rows with both long traces and many snapshots.
- [ ] Stream source and executable hashing. Reuse dataset and worker source
  snapshots across candidates, and register cache roots once per object and
  run. Consolidate repeated dataset tree scans while preserving mutation
  detection and timing scopes. Reuse committed inputs on resume when their
  identities can be verified from saved evidence. Measure a large fixed
  dataset, cached and uncached generated datasets, and resume with every
  selected slot already accepted.
- [ ] Reuse validated parameter expansion, apply name filters before building
  excluded slots, and index observation slots within each specification.
  Resolve and register large schedules in batches to bound memory while
  keeping pending work visible. Exercise a small selection beside a large
  grid and a run with many repetitions under `--max-attempts 1`.
- [ ] Index attempts and observations by slot for inspection. Use targeted
  queries for status summaries, exact-ID explanations, and the final run
  report. Index archive inventory paths and observation ownership to replace
  per-artifact scans. Preserve read-only and integrity guarantees. Exercise
  many small attempts, including retries and failed attempts.

### Disk and shared storage

- [ ] Let buffered SDK collectors capture file-backed snapshots using the
  existing result artifact inventory and buffer byte limits. Exercise binary
  coefficient arrays in the adopter benchmarks.
- [ ] Share one immutable copy of result files between buffered snapshots and
  their evaluated observations. Measure per-attempt metadata and log costs,
  and reuse repeated evidence by ID.
- [ ] Stream archive creation from verified payloads to reduce scratch usage.
  Preserve atomic publication and full verification on import and restore.
- [ ] Store fresh sequence membership once per repetition and reference it from
  attempt outcomes. Verify recovery and archive restore for both existing
  and new records.
- [ ] Use indexed key lookups during shared journal replay and verify row images
  after locating each row. Measure replay time and journal file counts
  before batching durable writes. Preserve recovery after loss of local
  state.
- [ ] Replace full shared cache snapshots with versioned deltas and checkpoints.
  Measure catalog growth as objects and roots accumulate. Keep existing
  records readable and cache collection safe across crashes.
- [ ] If trace encoding and decoding remain costly after streaming and
  deduplication, compare compressed JSON Lines with a chunked binary trace
  format on Basin and clustlasso-cd traces. Measure recording and reading
  time, peak RSS, and retained bytes.

## Benchmark authoring

The clustlasso-cd manifest motivates these improvements. Prioritize compact
default overrides and shared profile bindings. Lower new authoring conveniences
into the existing effective model, preserving validation and identities for
equivalent configurations.

- [x] Allow compact `parameters` alongside `parameter-defaults`. Explicit
  defaults override declared defaults before expanded-value validation.
  Declaration grids still select values; experiment axes and cases replace
  matching selections. Continue rejecting competing `parameter-schema`
  declarations.
- [x] Support a default profile binding with per-implementation overrides.
  Explicit bindings replace the default completely. Validate each resolved
  binding's schema, problem contract, and implementation capabilities.
- [x] Infer `max-observations` when omitted and all resolved bindings use finite
  `fresh_sequence` schedules with the same expanded length, including
  prefixes.
- [x] Add explicit local runtime configuration for named adapter download
  caches. Keep host cache paths outside scientific dataset parameters,
  record effective settings in provenance, and retain scientific overrides
  in identities. Distinguish adapter caches from the artifact cache selected
  by `--cache-dir`.
- [x] Show the scientific timeout, worker execution watchdog, and cumulative
  profile time limit together in `plan --verbose`. Diagnose when the
  watchdog cuts the requested scientific budget short while retaining the
  separate purposes of these limits.

Existing grids and named cases can simplify the adopter's simulation matrix.
Moving algorithm controls into implementation parameters belongs in the adopter
configuration.

## Applicability

- [ ] Keep excluded candidates visible in plans, status, explanation, and
  exports. Exercise a real representation or dimension restriction from an
  adopter and distinguish it from a failed attempt.

## Profiles

- [x] Add adaptive scientific-budget schedules for buffered profiles. For
  clustlasso-cd, calibrate a common per-solver limit for each input, penalty
  setting, and accuracy target from independently verified pilot convergence
  times. Increase pilot budgets until a solver converges, complete that
  round for all methods, then freeze a configurable multiple of the fastest
  verified pilot time for all methods and measured repetitions. "Convergence
  plus ten times its duration" corresponds to `11 * t_ref`. Supply useful
  defaults and allow unbounded pilot growth and derived limits. Keep an
  optional absolute ceiling and fallback when no solver converges. Diagnose
  completed rounds with no convergence and no time-limited pilot instead of
  increasing budgets for unrelated stopping conditions or failures. Keep
  calibration separate from measured runs so serial dispatch order does not
  determine the reference method. Record the reference attempts, timing
  basis, multiplier, and effective limit, and preserve budget decisions
  across resume and retries. Support cooperative stopping for buffered
  profiles, align execution watchdogs with the derived limit, and retain
  censored terminal states separately from execution failures.
- [ ] Extend adaptive calibration to fresh-sequence or streaming delivery when
  an adopter needs it.
- [ ] Add checkpoint-state restoration.

## Language packages and environments

- [ ] Add the Julia worker package.
- [ ] Add `renv` resolution as adopters need it, including local dependencies
  and offline verification.
- [ ] Add Julia `Pkg` and Conda environment resolution.

See [Reproducibility model](docs/design.md#reproducibility-model) for
environment guarantees and the existing Nix and uv backends.

## Execution and isolation

- [x] Add digest-pinned OCI environments and Docker execution on Linux.
- [x] Add Apptainer execution of local SIF images on Linux.
- [ ] Add Singularity and Podman execution on Linux.
- [x] Add whole-benchmark bundling. Start with a generated Dockerfile and build
  context containing a pinned Metewand runner, declared benchmark inputs,
  and explicit dependency recipes or a supplied base image. Run workers
  within one image and write results to a mounted output directory. Preserve
  separate OCI worker environments in a later bundling mode rather than
  merging them implicitly.
- [ ] Enforce requested resource, network, read-only input, and process-tree
  controls, with explicit failures when required enforcement is unavailable.
- [ ] Complete archival machine provenance and accounting, and qualify a
  mixed-language benchmark under those stronger guarantees.
- [ ] Add stronger isolation backends as needed.

Publication-archive claims require the enforced controls described in the
[reproducibility model](docs/design.md#reproducibility-model).

## Shared storage and recovery

Make a shared project directory the durable home of an HPC run. Keep worker
scratch and rebuildable indexes local to each host. Preserve existing SQLite
runs and scientific identities as the storage model evolves.

- [x] Restore resumable local run state from verified portable evidence,
  preserving pending work, failures, retries, accepted observations, and
  profile accounting without the original coordination database.
- [x] Validate buffered traces one row at a time during archive creation,
  verification, import, and restore. Test evidence preservation and
  malformed rows with a 25,001-point trace. Archive creation peaked at 209
  MiB on a local copy of the GenomeDK clustlasso-cd smoke run with 20,893
  trace points. The original retry with Metewand 0.8.0 peaked at 39.35 GiB.
- [x] Publish versioned durable records incrementally to shared storage. Include
  ownership, retry authorization, acceptance, and budget decisions. Commit
  completed work before reporting success; preserve streaming publication
  boundaries and recover after losing all node-local state.
- [x] Support shared immutable dataset caches with portable retention roots and
  safe reclamation, without a shared SQLite WAL catalog.
- [ ] Qualify publication, cross-host visibility, and recovery on supported HPC
  filesystems. Distinguish compute-node failure from storage-server power
  loss, and batch metadata operations for large runs. Exercise clustlasso-cd
  on GenomeDK's BeeGFS project storage and XFS scratch.

## Distributed execution

- [ ] Partition plans into deterministic, disjoint shards that preserve
  comparison blocks, seeds, and scientific identities. Give each shard one
  coordinator and combine its evidence for status, explanation, and
  analysis.
- [ ] Fence ownership and retry generations across duplicate submissions,
  requeues, and late results. Preserve attempt history and cumulative
  budgets when continuing on another node.

## Slurm and remote execution

- [ ] Generate reviewable Slurm job scripts and array assignments from a plan,
  with explicit account, partition, CPU, memory, and wall-time settings.
- [ ] Add explicit submission and reconciliation with scheduler states,
  cancellation, and accounting. Record allocations and observed hardware
  separately from scientific timing and parameters.
- [ ] Run and audit clustlasso-cd sweeps on GenomeDK using prepared Apptainer
  environments, shared durable results, and disposable node-local scratch.
- [ ] Add other remote execution backends when an adopter needs one.

## Results and analysis

- [ ] Extend export formats and provenance packaging when an analysis needs more
  than the existing JSONL and CSV tables and lineage receipt.
- [ ] Add provenance-tracked analysis artifacts.
- [ ] Add a read-only diagnostic viewer.

## Tooling

- [ ] Extend CLI name filters with parameter values (such as the response model)
  so a comparison can select part of a dataset, problem, or implementation
  grid alongside `--select ID`.
- [ ] Add an optional MCP adapter over the CLI.
