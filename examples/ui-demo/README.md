# Terminal UI demo

This benchmark takes about 30 seconds to exercise preparation spinners, the
overall progress bar, nine dataset and implementation bars, repetition counters,
and the final outcome summary. It compares three integer-sum implementations
on three seeded datasets, with two implementation repetitions and two
measurement repetitions per pair: 36 measurements total.

The implementations use Python's built-in `sum`, a manual loop, and
`functools.reduce`. The datasets contain 100, 1,000, and 10,000 integers. An
independent evaluator checks every sum, and the metric schema requires zero
error.

From the repository root, build the current CLI:

```sh
compute run -- cargo build --locked
```

Run in an interactive terminal with both stdout and stderr attached. The
following command uses uv to provide the in-tree Python package without a
separate installation step. Choose a fresh output directory on each invocation:

```sh
compute run -- uv run --no-project --python python3 --with ./packages/python -- \
  target/debug/metewand run --manifest examples/ui-demo/metewand.yaml \
  --output-dir "$(mktemp -d)/run"
```

If the Metewand Python package is already installed in your Python environment,
you can omit the `uv run` prefix. The Python workers otherwise use only the
standard library. The run ends with one completion line containing outcome counts
and elapsed time, followed by the benchmark name and results path. Failed,
censored, and superseded counts appear when nonzero. Inspect individual
measurements afterward with
`target/debug/metewand status --output-dir /path/to/run`.

Do not pipe or redirect the command when testing the live bars. JSON output and
`TERM=dumb` also disable them. The run uses serial execution, deterministic
randomized ordering, and worker reuse. Each dataset takes an extra half-second
to generate so preparation is visible.

The implementations deliberately sleep for 0.4, 0.55, and 0.7 seconds per
measurement, totaling 19.8 seconds of sleep plus preparation and runner overhead.
These timings demonstrate the UI; they do not measure algorithm performance.
Change `delay_seconds` in the manifest to adjust the duration. Increasing
`max-concurrency` exercises simultaneous attempts and shortens the run.

To inspect the plan or focus on one dataset and implementation:

```sh
target/debug/metewand plan --manifest examples/ui-demo/metewand.yaml
compute run -- uv run --no-project --python python3 --with ./packages/python -- \
  target/debug/metewand run --manifest examples/ui-demo/metewand.yaml \
  --dataset medium --implementation manual_loop \
  --output-dir "$(mktemp -d)/run"
```

The filtered run has four measurements and takes a few seconds. Reusing an
output directory skips accepted measurements, so use a new directory to watch
the bars again.
