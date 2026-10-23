# Getting started

Metewand describes computational benchmarks in a versioned manifest. It runs
implementations and evaluators as separate processes, validates their outputs,
and records the inputs and execution history behind each result.

The project is under active development. This guide builds the current CLI from
source and runs a small example on a POSIX system.

## Build the CLI

Install Git, Rust 1.98.0, and Python 3.11 or later, then clone the repository:

```sh
git clone https://github.com/jolars/metewand.git
cd metewand
cargo build --release --locked -p metewand-cli
```

The executable is `target/release/metewand`. If you use Nix and devenv, run
`devenv shell` in the repository to load its development environment.

## Check and plan

The fixed-dataset example sums three integers, adds a problem-owned offset, and
evaluates the answer independently. Its workers use Python's standard library
and implement the protocol directly.

Validate the manifest and inspect its execution plan:

```sh
target/release/metewand check --manifest examples/fixed-dataset/metewand.yaml
target/release/metewand plan --manifest examples/fixed-dataset/metewand.yaml
```

These commands do not launch workers or create run files. Add `--output json` to
read their versioned machine output.

## Run the benchmark

Choose a new output directory whose parent already exists:

```sh
target/release/metewand run \
  --manifest examples/fixed-dataset/metewand.yaml \
  --output-dir /tmp/metewand-first-run \
  --output jsonl
```

Metewand launches the implementation and independent evaluator, validates their
outputs, and publishes a durable run. The example produces the canonical result
`{"answer":8}` and evaluator metrics `{"absolute_error":0}`.

The output directory must be new. Use another path for a separate run. To resume
an existing run without repeating accepted slots, pass the same output directory
with `--resume`.

See [running benchmarks](../reference/run-cli.md) for execution policies and
guarantees, [status and explanations](../reference/inspection-cli.md) for
inspection, and [exporting results](../reference/exports.md) for analysis.

## Write your own benchmark

Create a minimal manifest with:

```sh
target/release/metewand init benchmark
```

The manifest starts with a version and a name. Add your environments, scientific
contracts, datasets, implementations, and experiments explicitly. The [authoring
reference](../reference/authoring-cli.md) explains initialization and component
scaffolding. A [runnable example](examples.md) is a useful starting point for a
complete benchmark.

Use the [Python SDK](../reference/python-sdk.md) or [R
SDK](../reference/r-sdk.md) to write workers in those languages. Any executable
can instead implement the [worker protocol](../reference/worker-protocol.md).
