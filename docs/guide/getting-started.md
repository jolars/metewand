# Getting started

Metewand runs computational benchmarks as separate processes, evaluates results
independently, and records their inputs, timings, and execution history. Start
with ordinary functions and a short YAML manifest, then add scientific checks
and stronger execution controls as the benchmark grows.

The project is under active development. This guide builds the current CLI and
SDK from source on a POSIX system.

## Install the CLI and SDK

With Git, Rust 1.98.0, and Python 3.11 or later:

```sh
git clone https://github.com/jolars/metewand.git
cd metewand
cargo install --locked --path crates/metewand-cli
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install ./sdk/python
```

Keep this environment active when running the benchmark. Metewand uses the
`python3` on `PATH`, with its installed SDK and dependencies. The SDK is not yet
published to a package registry. If you use Nix and devenv, `devenv shell` loads
the repository's development tools.

## Create and run a benchmark

```sh
metewand init /tmp/my-benchmark
cd /tmp/my-benchmark
metewand check
metewand plan
metewand check --workers
metewand run --output-dir run
```

Initialization creates two files: `metewand.yaml` and `benchmark.py`. The example
generates seeded integers, compares a built-in sum with a loop, and independently
computes each answer's absolute error. Replace the functions with your own
generator, methods, and metrics.

`check` and `plan` validate declarations and inspect the effective settings
without importing benchmark code or creating run files. `check --workers`
explicitly launches workers to verify imports and function names and run any
declared reference cases. The starter has zero reference cases; add them when
you want scientific conformance checks. Add `--output json` to any command for
versioned machine output.

The default execution uses a local environment, one final result per method,
and one measurement repetition. Its primary wall time includes preparation,
solving, and result serialization. The comparison table shows method names,
effective parameters, timings, metrics, and outcomes. With no declared accuracy
target, completion is `not_required`; successful execution does not establish
convergence.

The output directory must be new, and its parent must exist. Resume the same
run with `metewand run --output-dir run --resume` without repeating accepted
slots. Inspect or export its recorded evidence with:

```sh
metewand status --output-dir run
metewand export --output-dir run --format csv --destination tables
```

## Use R

Install the matching R package and its `digest` dependency in the selected R
environment's site library, then select the R starter. From the Metewand checkout:

```sh
R CMD INSTALL sdk/r
metewand init /tmp/my-r-benchmark --language r
cd /tmp/my-r-benchmark
metewand check --workers
metewand run --output-dir run
```

The two files are `metewand.yaml` and `benchmark.R`. The function signatures and
defaults match the Python starter. See the [R SDK](../reference/r-sdk.md) for
installation requirements and library paths.

## Grow the benchmark

Keep parameter declarations, data shapes, experiments, and scientific rules in
YAML. Keep data generation, methods, and independent metric formulas in code.
Add repetitions, named cases, execution policies, validity rules, reference
cases, and documentation when they help your comparison. Profiles require an
explicit observation policy and the corresponding worker capabilities.

The [least-squares example](https://github.com/jolars/metewand/tree/main/examples/least-squares)
compares in-house gradient descent and coordinate descent on simulated data.
It includes both a basic manifest and a version that adds an accuracy target
and inline reference cases to the same functions.

Use `metewand init --empty` for a neutral version-and-name manifest, then
`metewand scaffold dataset|problem|implementation NAME` to add components.
The [authoring reference](../reference/authoring-cli.md) covers function
declarations and scaffolding. The [Python SDK](../reference/python-sdk.md),
[R SDK](../reference/r-sdk.md), [Rust helpers](../reference/rust-sdk.md), and
[worker protocol](../reference/worker-protocol.md) support more involved
lifecycle and artifact requirements.

See [running benchmarks](../reference/run-cli.md) for execution controls,
[status and explanations](../reference/inspection-cli.md) for stored evidence,
and [exporting results](../reference/exports.md) for analysis.
