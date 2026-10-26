# Fresh sequences with Python and R

This example solves `x * x - 2 = 0` on `[1, 2]` using a Python bisection function
and R's `stats::uniroot`. Each receives its own finite control schedule. A
separate Python worker evaluates the returned root without importing a solver.
The evaluator reports absolute residuals rounded to 12 decimal places so its
reference check is stable across small floating-point differences.

The target comparison has two measurement repetitions per implementation.
The fixed-count comparison evaluates all four bisection operating points.
Every point starts clean, including when its process is reused. Iteration
counts and tolerances are implementation controls, not equivalent work units.

## Stage and run

Use `devenv shell`, which provides Python, R, and the R SDK's `digest` dependency.
From the repository root:

```sh
cargo build --locked -p metewand-cli
example_dir=$(mktemp -d)
cp -R examples/fresh-sequence/. "$example_dir/"
cp -R sdk/python/src/metewand "$example_dir/metewand"
mkdir "$example_dir/library"
R CMD INSTALL --preclean --clean --library="$example_dir/library" sdk/r
metewand_bin="$PWD/target/debug/metewand"
cd "$example_dir"
"$metewand_bin" check --workers
"$metewand_bin" plan
"$metewand_bin" run --output-dir run
"$metewand_bin" status --output-dir run
"$metewand_bin" run --output-dir run --resume
"$metewand_bin" export --output-dir run --format jsonl --destination jsonl
"$metewand_bin" export --output-dir run --format csv --destination csv
```

Use `--output json` on commands to inspect their versioned records. Copy a slot,
attempt, or observation ID into `explain ID --output-dir run` for its evidence.
Resume after completion allocates no new attempts. To exercise interruption
between durable points, create another run with `--max-attempts 2`, then resume
that output directory.

For a fixed count, omit target fields and make the schedule length equal
`max-observations`. For a target, independent evaluator metrics control stopping.
Lower the example's target threshold to exercise censoring, or raise it to
observe earlier `skipped_target_reached` slots. Valid below-target results remain
selected and exported.

Python opts in with `fresh_sequence=True`, and R uses `fresh_sequence=TRUE`.
Both SDKs forward the selected control through their existing execute callback.
The scientific implementation has no observer or per-iteration protocol calls.

Primary timing covers preparation, solving, and result serialization. It
excludes worker startup, warm-ups, and independent evaluation. Each curve point
uses its own clean execution time; the cumulative profile time limit is recorded
separately and persists across resume and retries.

See [Fresh-sequence profiles](../../docs/reference/fresh-sequence-profiles.md)
for schedules, failure behavior, conservative interruption accounting, and
compatibility.
