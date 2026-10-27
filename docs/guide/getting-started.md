# Getting started

This guide installs Metewand from source and runs a complete benchmark. You
need a POSIX system, Git, Rust 1.98.0, and Python 3.11 or later.

## Install

```sh
git clone https://github.com/jolars/metewand.git
cd metewand
cargo install --locked --path crates/metewand-cli
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install ./packages/python
```

Keep this environment active. Workers use the `python3` on `PATH` and its
installed dependencies. The CLI and package are not yet published to package
registries. If you use Nix, `devenv shell` provides the repository's development
tools.

## Create a benchmark

Choose a new directory:

```sh
metewand init /tmp/my-benchmark
cd /tmp/my-benchmark
```

This creates `metewand.yaml` and `benchmark.py`. The starter generates seeded
integers, compares a built-in sum with a loop, and independently checks each
answer's absolute error. Initialization writes the files without installing
dependencies or running benchmark code.

## Check and run

```sh
metewand check
metewand plan
metewand check --workers
metewand run --output-dir run
```

`check` validates the declarations. `plan` shows the expanded comparison and
effective settings. Neither command imports benchmark code or creates run
files. `check --workers` launches the workers to verify imports and function
names and evaluate any declared reference cases. The starter has no reference
cases yet.

The run prints a comparison with method names, settings, timings, metrics, and
outcomes. It uses one measurement per method. The default wall time includes
preparation, solving, and result serialization, with startup and independent
evaluation outside that interval.

The starter declares no accuracy target, so completion is `not_required`.
Successful execution means the result passed the configured checks; it does
not establish convergence.

## Inspect and export

```sh
metewand status --output-dir run
metewand export --output-dir run --format csv --destination tables
```

`tables/` contains `results.csv` and `export.json`, which records the export's
lineage. Keep the run directory as the original evidence.

Run and export destinations must be new directories, and their parents must
exist. To continue an interrupted run, reuse its directory explicitly:

```sh
metewand run --output-dir run --resume
```

Resume preserves accepted measurements. Failed attempts require `--retry`
before another attempt is authorized. Add `--output json` to commands when
you need machine-readable output.

## Start with R

Use R 4.2 or later, the `digest` package, and a C++17 compiler with floating-point
`std::to_chars` support. Install `digest` and the package into the selected R
environment's site library; workers use a private user library.

From the Metewand checkout:

```sh
R CMD INSTALL packages/r
metewand init /tmp/my-r-benchmark --language r
cd /tmp/my-r-benchmark
metewand check --workers
metewand run --output-dir run
```

The R starter creates `metewand.yaml` and `benchmark.R`, with the same roles and
defaults as Python. See the [R package](https://github.com/jolars/metewand/tree/main/packages/r)
for installation details.

## Write your own comparison

Continue with [authoring a benchmark](authoring/index.md) to build a
least-squares comparison step by step. It starts with two methods and adds
parameter sweeps, reference checks, repeated measurements, and analysis.

Use `metewand init --empty` if you prefer a blank manifest. Component
[scaffolding](authoring/extending.md#add-components) helps extend it.
