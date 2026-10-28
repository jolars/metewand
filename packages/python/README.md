# Metewand Python package

Define benchmarks with ordinary functions and small scientific result objects.
The package requires Python 3.11 or later and has no third-party runtime
dependencies. The separate CLI supplies worker adapters, measures execution,
validates outputs, computes file hashes, and publishes canonical artifacts.

## Install and run

From a checkout, install the CLI and package in the selected worker environment:

```sh
cargo install --locked --path crates/metewand-cli
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install ./packages/python
metewand init my-benchmark
cd my-benchmark
metewand check --workers
metewand run --output-dir run
```

On Windows, activate the environment with `.\.venv\Scripts\Activate.ps1`.
Building the CLI from source requires Rust's MSVC toolchain and Visual Studio
C++ build tools; installing the Python package does not. See
[getting started](https://metewand.org/guide/getting-started) for binary CLI
installation, platform requirements, and local NTFS storage requirements.

The package and CLI have independent releases. Compatibility follows **binding
API version 1**, rather than equal release numbers. Upgrade the CLI without
reinstalling a compatible package. Pin scientific dependencies and source
revisions when retaining benchmark evidence.

## Author benchmarks

Ordinary functions use `generate(context)`, `solve(data, context)`, and
`evaluate(data, result, context)`. Select them in YAML with
`function: benchmark.py:solve`. Evaluators may return plain metric dictionaries
when no scientific rules are declared; otherwise return
`Evaluation(metrics, verdict)`.

For separate preparation and solving, register functions:

```python
from metewand import read_json, serve_implementation


def prepare(context):
    return read_json(context.dataset_dir / "data.json")


def execute(state, context):
    return {"answer": sum(state["values"])}


serve_implementation(prepare=prepare, execute=execute,
                     reset=lambda state, context: None)
```

Select `protocol-transport: socket` for lifecycle workers. Context supplies
`parameters`, `problem_parameters`, `seed`, `semantics`, assigned directories,
and optional `observation_control`. Optional fields can be added without
changing signatures. Path fields are `pathlib.Path` values. The CLI never sets
scientific random generators automatically.

Optional `result(output, context)` converts solver output after diagnostic solve
timing. `statistics(output, context)` returns diagnostics. `reset(state,
context)` releases slot state, and `shutdown(state, context)` releases resources.
Execution consumes preparation even when it fails; the original preparation
state remains available for cleanup.

`Dataset(data, files)` and `Result(data, files)` describe JSON data and completed
files. Write files beneath `context.output_dir`, close them, and declare them
with `Artifact(path, media_type, representation)`. The CLI owns envelopes,
inventories, canonicalization, and hashes. Native workers may still implement
the public process protocol and write completed artifacts directly.

For configuration-dependent support, register `check_applicability(context)`
and declare `capabilities: [one_shot, applicability]`. Return a dictionary with
`outcome` (`applicable` or `not_applicable`), nonempty `code` and `reason`, and
optional `details`. The callback can inspect `context.dataset_dir` and
`context.dataset_manifest` before preparation and must preserve those inputs.

Buffered workers register `execute_buffered(state, context)` and use the
CLI-provided `context.observer` for `observe`, `record`, `finish`, or native
buffer adoption. Observer implementations and profile policy handling ship with
the CLI. No per-point process exchange is added.

The pre-MVP migration replaces implementation subclasses with named callbacks,
positional arguments with contexts, and manifest writers with output values.
Protocol, canonicalization, and observer implementation classes are no longer
exported from the package. Constructors and JSON utilities work without the CLI;
serving requires its adapter and never downloads code.

See the [authoring guide](https://metewand.org/guide/authoring/),
[Python example](../../examples/python-package), and
[buffered profile example](../../examples/buffered-profile). Package tests run with:

```sh
python -m unittest discover -s packages/python/tests -v
```

The runtime's adapter tests retain canonicalization, identity, lifecycle, and
profile conformance coverage. CI checks installed wheels and source distributions
on Linux, macOS, and Windows.
