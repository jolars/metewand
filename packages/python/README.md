# Metewand for Python <img src="https://raw.githubusercontent.com/jolars/metewand/refs/heads/main/assets/logo.png" align="right" width="160" alt="A green measuring rod with brass ends" />

Write reproducible computational benchmarks with ordinary Python functions.
Define how to generate data, run your methods, and evaluate their answers, then
declare the comparison in YAML. The Metewand CLI runs each method in a separate
process and saves timings, metrics, failures, and provenance.

This package provides the Python functions and result objects you need to write
a benchmark. The CLI checks and runs it on Linux, macOS, and native Windows x64.

## Install

You need Python 3.11 or later and Git. Install the package from GitHub in a
virtual environment:

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install "metewand @ git+https://github.com/jolars/metewand.git@main#subdirectory=packages/python"
```

On Windows, use `python` in place of `python3` and activate the environment with
`.\.venv\Scripts\Activate.ps1` in PowerShell. Keep the environment active when
running benchmarks so workers can find your installed packages.

Install the separate **Metewand CLI** by following [getting
started](https://metewand.org/guide/getting-started#install). Installing the
Python package does not install the CLI.

The base package has no third-party runtime dependencies. To include NumPy and
SciPy helpers for dense and sparse arrays, install the `arrays` extra:

```sh
python3 -m pip install "metewand[arrays] @ git+https://github.com/jolars/metewand.git@main#subdirectory=packages/python"
```

These commands install the development version. For a reproducible benchmark,
replace `main` with a commit hash and pin your scientific dependencies.

## Try a benchmark

With the CLI and Python package installed, create and run a starter:

```sh
metewand init my-benchmark

cd my-benchmark

metewand check --workers
metewand run --output-dir run
```

The starter creates `benchmark.py` and `metewand.yaml`. It generates seeded
integers, compares Python's built-in sum with a loop, and independently checks
each answer. Metewand prints timings and absolute errors and saves the results
in `run/`.

## Write your own comparison

Replace the generated functions with your data, methods, and metrics. A method
and its evaluator can be as small as:

```python
def builtin_sum(data, context):
    return {"total": sum(data["values"])}

def evaluate(data, result, context):
    expected = sum(data["values"])
    return {"absolute_error": abs(result["total"] - expected)}
```

Select functions in YAML with entries such as
`function: benchmark.py:builtin_sum`. The `context` argument supplies your
parameters, a seed, and input and output directories. Use `context.seed` to seed
your random generator when generating data or running randomized methods.

Return dictionaries for ordinary JSON data and metrics. For benchmarks with an
accuracy target, use `Evaluation(metrics, verdict)` to report whether the method
reached it. Use `Dataset`, `Result`, and `Artifact` when you also need to return
files. The CLI records and checks those files alongside the JSON output.

Follow the [authoring guide](https://metewand.org/guide/authoring/) for
parameter sweeps, repeated measurements, reference checks, and result exports.
If your methods need separate setup and solve steps, see the [Python
example](https://github.com/jolars/metewand/tree/main/examples/python-package).
For progress measurements during a solve, see the [streaming
example](https://github.com/jolars/metewand/tree/main/examples/streaming-profile)
or [buffered profile
example](https://github.com/jolars/metewand/tree/main/examples/buffered-profile).

## Project status

Metewand is under active development. The Python package and CLI have separate
releases; their version numbers do not need to match.

See the [main repository](https://github.com/jolars/metewand) for project news,
citation information, and contributor instructions.
