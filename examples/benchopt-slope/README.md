# Reusing a Benchopt SLOPE benchmark

This comparison reuses `benchmark_slope`'s simulated dataset, PGD and ADMM
implementations, and objective evaluator unchanged. One manifest supplies the
scientific settings, solver controls, and three measurement repetitions per
configuration. Small package adapters translate arrays and callback iteration
counts; they contain no protocol handling, result envelopes, or metric formulas.
No separate contract or JSON Schema files are needed.

The two methods each run a 1,000-iteration case and a deliberately limited
one-iteration case. Expect 12 attempts: six accepted and six failed with
`completion_target_not_reached`. All 12 produce valid observations and retain
their metrics. Failure to converge is distinct from an invalid result or a
worker crash.

## Run the comparison

Use a Python environment containing Benchopt, NumPy, SciPy, scikit-learn, Numba,
and PyYAML. The validated environment used Python 3.13.11, Benchopt 1.9.1, NumPy
2.3.5, SciPy 1.16.3, scikit-learn 1.7.2, and Numba 0.63.1. Metewand's ordinary
development Python has no scientific dependencies. Put the scientific
environment's `bin` directory first on `PATH`: the local Python runner resolves
`python3` from that path. An ambient `PYTHONPATH` is not inherited by workers.

For a virtual environment, create it with `python3 -m venv .venv`, install
its dependencies, and prepend `.venv/bin` to `PATH`. The local runner preserves
the selected interpreter path, so ordinary symlink-based virtual environments
retain their package path.

From the Metewand repository root, with a local `benchmark_slope` checkout:

```sh
cargo build --locked -p metewand-cli
comparison_dir=$(mktemp -d)
python3 examples/benchopt-slope/prepare.py \
  --benchmark "$HOME/projects/benchmark_slope" \
  --destination "$comparison_dir/benchmark"
python3 "$comparison_dir/benchmark/validate.py" \
  --metewand "$PWD/target/debug/metewand" \
  --output-dir "$comparison_dir/run"
```

`prepare.py` creates a new directory, copies the four scientific source files
and the in-tree package, and records the upstream Git revision, source modification
status, and SHA-256 hashes in `origins.json`. Every worker declares its needed
sources, including that origin record. The source checkout remains untouched.
The example was checked against upstream revision
`4a12de89672c26f55e0292b5c425417c52903682`. Other revisions must pass the same
reference and comparison checks.

`validate.py` runs scientific unit tests and the public `check --workers`,
`plan`, `run`, `status`, `explain`, and `export` commands. It verifies effective
settings, all three repetitions, independent completion verdicts, retained
observations, diagnostics, and positive timing evidence. It prints one summary
per method and case, with gaps and median elapsed time. Its command receipts
and tidy export remain beside the run for inspection.

Run those commands individually to assess the authoring experience:

```sh
cd "$comparison_dir/benchmark"
metewand check --workers
metewand plan
metewand run --output-dir "$comparison_dir/another-run"
metewand status --output-dir "$comparison_dir/another-run"
metewand export --output-dir "$comparison_dir/another-run" \
  --format csv --destination "$comparison_dir/tables"
```

Use an installed `metewand`, or the absolute path to the built binary. The
`run` command exits unsuccessfully because the limited cases miss their target.
Human output includes metrics and the failure reason. Plan output shows method
and case names, effective parameters, and logical slot IDs. Copy an attempt,
observation, or diagnostic ID from run/status output into
`metewand explain ID --output-dir PATH` to inspect its committed evidence.
Exports provide numeric timing columns and recorded settings for tabulation.

The optional Rust integration test runs the same complete validation:

```sh
METEWAND_SLOPE_ROOT="$HOME/projects/benchmark_slope" \
METEWAND_SLOPE_PYTHON="$(command -v python3)" \
  cargo nextest run --locked -p metewand-cli --test benchopt_slope \
  --run-ignored all
```

The ordinary suite excludes it because the upstream repository and scientific
environment are external dependencies.

## Scientific and measurement choices

The generated design has 100 rows, 20 columns, five signals, correlation 0.8,
and explicit upstream random state 27. Both methods receive exactly that data,
with no centering or scaling. `random_state` is a dataset parameter, preserving
the original generator's seed policy. The deterministic solvers do not consume
the supplied implementation seeds. Repetitions measure the same scientific
problem rather than regenerating it.

The problem retains the upstream normalized squared-loss SLOPE objective,
Benjamini–Hochberg weight construction, `reg=0.1`, `q=0.1`, and no intercept.
The evaluator consumes the completion target `rel_duality_gap <= 1e-7` from the
manifest. It imports the original objective and no solver, checks coefficient
dimensions and the absent intercept, and rejects a relative gap below the
declared roundoff floor. The metric formulas and scientific settings each have
one authoritative source.

An identity-design reference has an analytic solution with two pooled
coefficients. `test_science.py` independently reconstructs its weights using
the standard-library normal quantile and verifies the pooled KKT conditions.
It also tests invalid dimensions and intercepts, target changes, and verdicts
near the rounding boundary. Metrics are reported to the declared 12 decimal
places so `check --workers` can compare exact reference data across small
floating-point differences. Completion uses the unrounded certificate.
Invalid results have no fabricated metric values; the metric fields permit
their absence.

The validated run retained three identical gap values per method and case:

| Method | Iterations | Relative duality gap | Accepted / measured | Median primary / solve time (ms) |
| --- | --- | --- | --- | --- |
| PGD | 1,000 | 0, rounded | 3 / 3 | 99.065 / 86.694 |
| ADMM | 1,000 | 0, rounded | 3 / 3 | 124.961 / 112.884 |
| PGD | 1 | 0.601550585653 | 0 / 3 | 67.754 / 1.130 |
| ADMM | 1 | 0.338901297123 | 0 / 3 | 87.903 / 0.919 |

These timings describe one development run. The primary interval includes
preparation and transport as well as solving, which matters especially for the
one-iteration cases. Use the preserved export for exact per-repetition values.

`prepare_and_execute` includes array conversion, objective setup, solver
initialization, the complete solver call, and result serialization. PGD's
spectral norm and ADMM's factorization remain inside the solver call. Worker
startup, imports, dataset generation, and independent evaluation are outside
the primary interval; evaluator and package implementation times are recorded
separately. The PGD adapter selects the isotonic prox, avoiding a first-call
Numba compilation in this particular comparison. No warm-up, process reuse,
thread limit, or scientific stopping callback is implied.

This validates a small Benchopt migration: dataset generation, both solvers,
weight construction, and evaluation are reused; adapters supply process
callbacks, finite iteration counts, and array conversion. It compares fixed
endpoints. Benchopt convergence curves, callback scheduling, target-based
stopping, sparse applicability, and environment provisioning need their own
later integrations. These local development timings are useful for checking
measurement boundaries and reporting, but do not establish a performance
ranking or a locked publication environment.

Two authoring issues remain visible from this exercise. Human run output uses
IDs to connect outcomes to method and case names in the plan; the validation
script joins those public records to produce its summary. Callback exception
diagnostics direct authors to worker logs, but failed-attempt inspection does
not currently retain those tracebacks. During adapter development, a direct
call to `prepare` and `execute` exposed the Benchopt parameter-binding mistake;
the added analytic adapter test now catches it before a full comparison.

The comparison and human rendering reuse the existing versioned interfaces;
no compatibility constants or persisted formats change.
