# Metewand TODO

Remaining work is grouped by area. Prioritize useful benchmarks in Basin and
clustlasso-cd.

The [design](docs/design.md) defines the general model.
[CONTRIBUTING.md](CONTRIBUTING.md) covers setup, testing, and submitting changes,
and [AGENTS.md](AGENTS.md) gives repository instructions.

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
- [x] Add explicit local runtime configuration for named adapter download caches.
  Keep host cache paths outside scientific dataset parameters, record effective
  settings in provenance, and retain scientific overrides in identities.
  Distinguish adapter caches from the artifact cache selected by `--cache-dir`.
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
  times. Increase pilot budgets until a solver converges, complete that round
  for all methods, then freeze a configurable multiple of the fastest
  verified pilot time for all methods and measured repetitions.
  "Convergence plus ten times its duration" corresponds to `11 * t_ref`.
  Supply useful defaults and allow unbounded pilot growth and derived limits.
  Keep an optional absolute ceiling and fallback when no solver converges.
  Diagnose completed rounds with no convergence and no time-limited pilot
  instead of increasing budgets for unrelated stopping conditions or failures.
  Keep calibration separate from measured runs so serial dispatch
  order does not determine the reference method. Record the reference
  attempts, timing basis, multiplier, and effective limit, and preserve
  budget decisions across resume and retries. Support cooperative stopping
  for buffered profiles, align execution watchdogs with the derived limit,
  and retain censored terminal states separately from execution failures.
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
- [ ] Bound memory during archive validation. `ResultSet::from_snapshot`
  clones the full attempt record for every buffered trace point. In the
  clustlasso-cd smoke test on GenomeDK, Metewand 0.8.0 exhausted 32 GiB during
  archive creation. The retry peaked at 39.35 GiB with 20,893 trace points,
  including 19,008 from one PG profile. Validate traces one row at a time.
  Add a long-trace regression for archive creation, verification, import, and
  restore that preserves the recorded evidence and rejects malformed rows.
  [Trace expansion](src/runtime/export.rs)
- [x] Publish versioned durable records incrementally to shared storage.
  Include ownership, retry authorization, acceptance, and budget decisions.
  Commit completed work before reporting success; preserve streaming
  publication boundaries and recover after losing all node-local state.
- [x] Support shared immutable dataset caches with portable retention roots
  and safe reclamation, without a shared SQLite WAL catalog.
- [ ] Qualify publication, cross-host visibility, and recovery on supported HPC
  filesystems. Distinguish compute-node failure from storage-server power loss,
  and batch metadata operations for large runs. Exercise clustlasso-cd on
  GenomeDK's BeeGFS project storage and XFS scratch.

## Distributed execution

- [ ] Partition plans into deterministic, disjoint shards that preserve
  comparison blocks, seeds, and scientific identities. Give each shard one
  coordinator and combine its evidence for status, explanation, and analysis.
- [ ] Fence ownership and retry generations across duplicate submissions,
  requeues, and late results. Preserve attempt history and cumulative budgets
  when continuing on another node.

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
