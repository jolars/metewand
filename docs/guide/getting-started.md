# Getting started

This guide installs Metewand from source and runs a complete benchmark. You
need Linux, macOS, or native Windows x64, Git, Rust 1.98.0, and Python 3.11 or
later. Windows uses the MSVC Rust toolchain and the Visual Studio C++ build
tools, including the Windows SDK. Put Python on `PATH`.

## Install

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

Use R 4.2 or later, the `digest` package, and a C++17 compiler with floating-point
`std::to_chars` support. On Windows, use R 4.3 or later with the matching Rtools
release, and put `R.exe` and `Rscript.exe` on `PATH`.
[Rtools43](https://developer.r-project.org/WindowsBuilds/Rtools43/rtools.html)
and later supply a sufficiently recent compiler. Rtools supplies
the compiler for the R package; the CLI uses MSVC separately. Install `digest`
and the package into the selected R environment's site library; workers use a
private user library. Set up a writable R installation or run the installation
commands with permission to write that library.

From the Metewand checkout:

```sh
Rscript --vanilla -e 'install.packages("digest", lib=.Library, repos="https://cloud.r-project.org")'
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
