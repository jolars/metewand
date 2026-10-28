# Getting started

This guide installs Metewand and runs a complete benchmark. You need Linux,
macOS, or native Windows x64 and Python 3.11 or later. Put Python on `PATH`.

## Install

### Prebuilt CLI

Starting with the next release, install the latest prebuilt CLI on Linux or
macOS with:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://metewand.org/install | sh
```

On Windows, run this in PowerShell:

```powershell
irm https://metewand.org/install.ps1 | iex
```

The installers support x64 and ARM64 on Linux and macOS, and x64 on Windows.
They place `metewand` in Cargo's binary directory, normally `~/.cargo/bin`.
Restart your shell if `metewand` is not on `PATH`, then run
`metewand --version` to check the installation. You can also download archives
and checksums from [GitHub Releases](https://github.com/jolars/metewand/releases).

Install worker packages separately. Python, R, and the CLI have independent
release numbers; compatibility follows binding API version 1. The CLI supplies
its worker adapters, so compatible CLI upgrades do not require package updates.
For development from a checkout, build and install the components below.

### From source

Source builds require Git and Rust 1.98.0. Windows builds use the MSVC Rust
toolchain and the Visual Studio C++ build tools, including the Windows SDK.

```sh
git clone https://github.com/jolars/metewand.git
cd metewand
cargo install --locked --path crates/metewand-cli
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install ./packages/python
```

On Windows, run these commands in PowerShell instead:

```powershell
git clone https://github.com/jolars/metewand.git
cd metewand
cargo install --locked --path crates/metewand-cli
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install ./packages/python
```

If your PowerShell policy prevents activation, put the environment on `PATH`
for the current session with `$env:Path = "$PWD\.venv\Scripts;$env:Path"`.

Keep this environment active. Workers use its Python interpreter and installed
dependencies. On Windows, the runner resolves the default `python3` launch to
`python.exe` when needed. The CLI and package are not yet published to package
registries. If you use Nix, `devenv shell` provides the repository's development
tools.

## Create a benchmark

Choose a new directory:

```sh
metewand init my-benchmark
cd my-benchmark
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

Use local disks for run and cache directories. Windows currently requires
local NTFS. Recovery covers abrupt process death; it does not promise recovery
from a machine losing power. The recorded storage evidence identifies the
platform's synchronization and publication behavior. Network filesystems are
unsupported.

## Start with R

Use R 4.2 or later and the `jsonlite` package. Metewand's R package contains no
native code and needs no compiler or Rtools. Standard dependencies may contain
native code and follow their own source-installation requirements. On Windows,
put `R.exe` and `Rscript.exe` on `PATH`.

Install `jsonlite` and Metewand into the selected R interpreter's site library;
workers use a private user library. Use a writable R installation or run the
installation commands with permission to write that library.

From the Metewand checkout:

```sh
Rscript --vanilla -e 'install.packages("jsonlite", lib=.Library, repos="https://cloud.r-project.org")'
R CMD INSTALL --library=SITE_LIBRARY packages/r
metewand init my-r-benchmark --language r
cd my-r-benchmark
metewand check --workers
metewand run --output-dir run
```

Replace `SITE_LIBRARY` with the path printed by `Rscript --vanilla -e 'cat(.Library)'`.
Quote that path if it contains spaces. These R commands also work in PowerShell.

The R starter creates `metewand.yaml` and `benchmark.R`, with the same roles and
defaults as Python. See the [R package](https://github.com/jolars/metewand/tree/main/packages/r)
for installation details.

## Write your own comparison

Continue with [authoring a benchmark](authoring/index.md) to build a
least-squares comparison step by step. It starts with two methods and adds
parameter sweeps, reference checks, repeated measurements, and analysis.

Use `metewand init --empty` if you prefer a blank manifest. Component
[scaffolding](authoring/extending.md#add-components) helps extend it.
