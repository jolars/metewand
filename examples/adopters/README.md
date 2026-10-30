# Basin and clustlasso-cd qualification

These adapters run the adopters' scientific code through the public CLI.
Basin compares its Nelder–Mead implementation with Gomez using the existing
`competitor-bench::nm` harness and identical starting simplexes. A separate
Basin experiment records an instrumented native trajectory. clustlasso-cd
compares native PG and hybrid solves on its
`analytic_diagonal_both_penalties` input, then records bounded trajectories
with selected immutable coefficient snapshots. Each comparison has two
measurement repetitions and one warm-up per worker configuration.

The adapters use the current local checkouts. They stage source copies and
compiled programs without changing either adopter repository. Native clustlasso
uses the original solver modules. Its existing recorder gains two bridge calls:
one sends checked iterates to the bounded collector, and one honors its stop
decision through the existing time-limit check. Numerical updates stay in the
adopter's code. Every local check remains in Rust, including snapshot capture.
The collector includes metric, bridge, clock, and snapshot costs in its elapsed
time. It does not convert a metric-only historical trace into validated results.

These repository adapters compile Metewand's internal modules directly and inherit
their dependencies from the root Cargo manifest. They require the Metewand source
checkout; the published binary package has no Rust library API.

## Build and stage

Use the Metewand development environment with Rust 1.98.0, Python, and CMake.
The Basin competitor harness also builds its existing external dependencies.
From the Metewand repository root, set paths to the adopter checkouts:

```sh
basin_dir=/path/to/basin
clustlasso_dir=/path/to/clustlasso-cd
staging_parent=$(mktemp -d)
stage="$staging_parent/benchmarks"
python3 examples/adopters/prepare.py --basin "$basin_dir" \
  --clustlasso "$clustlasso_dir" --output-dir "$stage"
cargo generate-lockfile --manifest-path "$stage/native/Cargo.toml"
python3 tools/build_evidence.py --executable target/release/metewand \
  --source metewand=. --compiler rustc --profile release --lockfile Cargo.lock -- \
  cargo build --release --locked -p metewand --bin metewand
for adopter in basin clustlasso; do
  python3 tools/build_evidence.py \
    --executable "$stage/native/target/release/$adopter-worker" \
    --source metewand=. --source "basin=$basin_dir" \
    --source "clustlasso=$clustlasso_dir" --source "adapters=$stage" \
    --compiler rustc --profile release \
    --lockfile "$stage/native/Cargo.lock" --lockfile "$basin_dir/Cargo.lock" \
    --lockfile "$clustlasso_dir/Cargo.lock" -- \
    cargo build --release --locked --manifest-path "$stage/native/Cargo.toml"
  cp "$stage/native/target/release/$adopter-worker" "$stage/$adopter/worker"
  cp "$stage/native/target/release/$adopter-worker.metewand-build.json" \
    "$stage/$adopter/worker.metewand-build.json"
done
metewand_bin="$PWD/target/release/metewand"
```

The first build compiles both adapters; the second captures the other
executable's receipt from a cached invocation. Keep the generated lockfile with
the staged benchmark. This combined adapter lockfile can differ from the
adopters' workspace lockfiles; receipts identify all three explicitly.

## Run through the CLI

For either staged native comparison:

```sh
benchmark="$stage/basin"
"$metewand_bin" check --manifest "$benchmark/metewand.yaml" --workers
"$metewand_bin" plan --manifest "$benchmark/metewand.yaml"
"$metewand_bin" run --manifest "$benchmark/metewand.yaml" --output-dir "$benchmark/run" --max-attempts 1
"$metewand_bin" status --output-dir "$benchmark/run"
"$metewand_bin" run --manifest "$benchmark/metewand.yaml" --output-dir "$benchmark/run" --resume
"$metewand_bin" export --output-dir "$benchmark/run" --destination "$benchmark/jsonl"
"$metewand_bin" export --output-dir "$benchmark/run" --format csv --destination "$benchmark/csv"
```

Use `--select ID` with an exact candidate, slot, or observation ID from `plan`
to execute a selection. Use `explain ID --output-dir PATH` for recorded evidence.
An interrupted unsatisfied slot requires `--resume --retry`; accepted slots
remain protected. These are ordinary benchmark manifests. Routine execution,
inspection, and export need no example-specific driver or internal store access.

The optional qualification harness automates the public commands, observes a
live worker, kills that invocation, waits for its ownership lease to expire,
and resumes explicitly. Choose new output directories:

```sh
python3 examples/adopters/qualify.py --metewand "$metewand_bin" \
  --manifest "$stage/basin/metewand.yaml" --output-dir "$staging_parent/basin-evidence"
python3 examples/adopters/qualify.py --metewand "$metewand_bin" \
  --manifest "$stage/clustlasso/metewand.yaml" --output-dir "$staging_parent/clustlasso-evidence"
```

The harness retains command receipts, exact JSONL and CSV table hashes, history,
and a compact qualification report. It verifies that both formats preserve
independent metrics, profile evidence, identities, timing scopes, and guarantees.
The normal suite separately retains fresh-sequence and buffered end-to-end cases,
metric disagreement, timing bounds, stopping, failed attempts, retry isolation,
interruption, and recovery. External adopter dependencies are required only for
these real qualification runs; their independent analytic checks run in normal CI.

## Python and R package comparison

Keep the package track separate from native trajectories. Reuse a prepared
clustlasso-cd package smoke bundle and its installed Python wheel and R
`fusedlasso` runtime. The adopter's `experiments/package_smoke.py stage --help`
documents how to prepare one from its fixed inputs and built wheel. Refresh an
older bundle's mechanical preprocessing declaration in a copy:

```sh
package_bundle=/path/to/prepared/package-smoke/bundle
python3 examples/adopters/stage_packages.py --bundle "$package_bundle" \
  --output-dir "$staging_parent/packages"
python3 examples/adopters/qualify.py --metewand "$metewand_bin" \
  --manifest "$staging_parent/packages/smoke.yaml" \
  --output-dir "$staging_parent/package-evidence"
```

This preserves the package adapters, input bytes, independent evaluator,
references, and scientific tolerances. `adapter-refresh.json` records hashes of
the two changed declarations. The installed runtimes remain external local
dependencies; staging does not claim a new package build. Available interpreter
hashes are recorded, and missing build receipts remain explicitly unavailable.

## Scientific checks and measured scopes

The Basin evaluator independently computes Rosenbrock's objective and parameter
error from the returned coordinates. Completion uses objective at most `1e-10`.
The native clustlasso evaluator recomputes the unnormalized half-squared loss,
L1 penalty, and every unordered fusion pair. It independently enumerates the
four ordered partitions for the three-variable proximal map to compute the
normalized PG residual `epsilon_g`. Native endpoint completion requires both
`epsilon_g` and relative objective error at most `1e-8`; instrumented stopping
uses `epsilon_g`. The package track retains its own evaluator, PG and objective
criteria, reference verification, and package-success requirement. No new
scientific budget is needed; all three tracks declare `none`.

Primary wall time covers preparation, solving, and canonical result writing.
It excludes worker startup, warm-ups, independent evaluation, and durable
publication. Native profile coordinates use a separately named instrumented
scope beginning before native initialization and excluding final buffer
serialization. Snapshot metrics belong to the independent evaluator; unsaved
checks remain provisional diagnostics. Keep one-shot timings and instrumented
coordinates in separate analyses. These tiny workloads qualify behavior;
[release overhead measurements](../overhead) show why their primary timings
should not be interpreted as pure solver performance.

## Retained local qualification

[qualification.json](qualification.json) records the local run on October 26,
2026, including source revisions and dirty states in build receipts. The runs
used development environments and observed local controls, with unsupported
isolation and complete process-tree accounting. Their classes remain explicit
in every export.

| Comparison | Accepted attempts | Evaluated selected snapshots | Interrupted failures retained |
| --- | ---: | ---: | ---: |
| Basin and Gomez, plus Basin trajectories | 6 | 12 | 1 |
| Native clustlasso PG and hybrid, plus trajectories | 8 | 14 | 1 |
| Python packaged hybrid and R fusedlasso | 4 | 4 | 1 |

Several clustlasso thresholds share one snapshot when a checked iterate crosses
them together. All three comparisons preserved their completed prefix after a
live-worker interruption, completed after recovery and explicit retry, and
allocated no attempts on the final resume. JSONL and CSV retained identical
nested evidence. The retained report is a compact record of this qualification;
full local run artifacts and exports live at the paths recorded by its commands.
