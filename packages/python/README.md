# Metewand Python package

Define Metewand benchmarks with ordinary Python functions for data generation,
methods, and independent evaluation. The package handles worker communication
and standard result files. It requires Python 3.11 or later and has no runtime
dependencies. Running benchmarks requires Linux, macOS, or native Windows x64
and the separate Metewand CLI, built with Rust 1.98.0. Installing this Python package does
not install the CLI.

## Install and run

Once the first PyPI release is published, install the package into the Python
environment that will run your workers:

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install metewand
```

Starting with the next binary release, install the CLI matching the package,
then create and run a benchmark:

```sh
metewand_version=$(python3 -c 'import metewand; print(metewand.__version__)')
curl --proto '=https' --tlsv1.2 -LsSf \
  "https://github.com/jolars/metewand/releases/download/v${metewand_version}/metewand-cli-installer.sh" | sh
metewand init my-benchmark
cd my-benchmark
metewand check --workers
metewand run --output-dir run
metewand status --output-dir run
```

Keep the Python environment active so workers use its interpreter and installed
dependencies. The CLI creates `metewand.yaml` and `benchmark.py`, with data
generation, two methods, and independent evaluation. Choose a new output
directory for each run, or explicitly resume with `--resume`.

On Windows, install the matching CLI in PowerShell:

```powershell
$metewand_version = python -c 'import metewand; print(metewand.__version__)'
irm "https://github.com/jolars/metewand/releases/download/v$metewand_version/metewand-cli-installer.ps1" | iex
```

Before the first binary release, or when developing from a checkout, install
both components from the same revision:

```sh
git clone https://github.com/jolars/metewand.git
cd metewand
cargo install --locked --path crates/metewand-cli
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install ./packages/python
```

On Windows, use PowerShell with the MSVC Rust toolchain and Visual Studio C++
build tools. After cloning, build and install from source with:

```powershell
cargo install --locked --path crates/metewand-cli
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install ./packages/python
```

Keep the environment active. Workers select its `python.exe` when the default
`python3` name is unavailable. Use local NTFS for run and cache directories.
See [getting started](https://metewand.org/guide/getting-started) for complete
platform requirements and recovery guarantees.

This is an experimental package. Use matching CLI and Python package versions,
and pin the version or source revision for retained benchmarks. Authoring syntax
and profile APIs may change before the MVP. Package versions are separate from
the versioned worker protocol and persisted record formats.

## Author benchmarks

The package implements Metewand's version-1 worker protocol, including one-shot
execution, fresh sequences, and buffered profiles. It serializes
messages, dispatches callbacks, reports structured errors, and measures the
implementation callback. The CLI owns source acquisition, seeds, scheduling,
worker lifecycle, artifact validation, and
durable publication. The package does not set random generators or infer scientific
semantics. Install scientific dependencies into the worker environment yourself.

Protocol pipes remain separate from standard output and standard error. Helpers
use inherited Unix descriptors or native Windows handles automatically and keep
frames binary on both platforms. Explicit descriptor arguments remain
caller-owned.

For a simple benchmark, use `metewand init` and ordinary functions selected in
YAML with `function: benchmark.py:solve`. The package supplies the context, JSON data,
and artifact envelopes. Evaluators can return plain metrics when no scientific
rules are declared; completion is then `not_required`. See the
[least-squares example](https://github.com/jolars/metewand/tree/main/examples/least-squares)
for incremental validation.

For lifecycle adapters, subclass `OneShotImplementation` and return scientific data
from `execute`. Evaluators return `Evaluation(metrics, verdict)`. The package writes
standard envelopes; custom result and metrics writers remain supported.

See the [authoring guide](https://metewand.org/guide/authoring/),
[callback interfaces](https://github.com/jolars/metewand/blob/main/packages/python/src/metewand/_worker.py),
and the runnable
[Python package example](https://github.com/jolars/metewand/tree/main/examples/python-package).
With the package installed in the active environment, run its tests from a
repository checkout with:

```sh
python -m unittest discover -s packages/python/tests -v
```

CI tests the installed wheel and source distribution on Linux, macOS, and
Windows. The Rust workspace tests exercise all three Python roles through the
native transport and output validator.

## Configuration-dependent support

Declare `capabilities: [one_shot, applicability]` and override
`check_applicability` to exclude configurations your implementation cannot
support intrinsically:

```python
def check_applicability(self, dataset_instance, dataset_manifest,
                        problem_contract, problem_parameters,
                        implementation_parameters):
    supported = not problem_parameters["intercept"]
    return {
        "outcome": "applicable" if supported else "not_applicable",
        "code": "supported" if supported else "unsupported_intercept",
        "reason": "Supported configuration." if supported else "Requires a zero intercept.",
    }
```

Both dataset arguments are `Path` objects. The callback runs in a separate
worker before measured preparation, may inspect the immutable dataset and its
manifest, and must return nonempty `code` and `reason` strings. Optional
`details` must be a dictionary. Exclusions produce no attempts; callback
exceptions fail resolution. `plan` records `unchecked` until the dataset and
worker are resolved. `explain` and `export` retain checked inputs and reasons.
