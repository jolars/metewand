# Metewand

[![Software
DOI](https://img.shields.io/badge/Software_DOI-10.5281%2Fzenodo.23699794-blue)](https://doi.org/10.5281/zenodo.23699794)

Metewand runs reproducible computational benchmarks across languages. It executes
methods as separate processes, evaluates answers independently, and retains
settings, timings, metrics, failures, and provenance.

Start with the Metewand Python or R package: write ordinary functions and declare
your comparison in YAML. The `metewand` CLI checks and runs the benchmark on
Linux, macOS, and native Windows x64.

Read the [overview](docs/guide/overview.md), follow
[getting started](docs/guide/getting-started.md), or work through the multipart
[authoring guide](docs/guide/authoring/index.md). The documentation is also
available at <https://metewand.org>.

## Install

Starting with the next release, install the prebuilt CLI on Linux or macOS with:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://metewand.org/install | sh
```

On Windows, run this in PowerShell:

```powershell
irm https://metewand.org/install.ps1 | iex
```

Installers place `metewand` in Cargo's binary directory, normally `~/.cargo/bin`.
Archives are also available from [GitHub Releases](https://github.com/jolars/metewand/releases).
Install the Python or R worker package separately. See
[getting started](docs/guide/getting-started.md) for package installation and
source builds.

## Try a benchmark

From this checkout, with Rust 1.98.0 and Python 3.11 or later:

```sh
cargo install --locked --path crates/metewand-cli
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install ./packages/python
metewand init my-benchmark
cd my-benchmark
metewand check --workers
metewand plan
metewand run --output-dir run
metewand status --output-dir run
metewand export --output-dir run --format csv --destination tables
```

Windows source builds use Rust's MSVC toolchain and Visual Studio C++ build
tools. See [getting started](docs/guide/getting-started.md) for PowerShell
installation and the local NTFS storage requirement. The R package contains no
native code and requires only `jsonlite` beyond standard R packages.

The starter has two files: `metewand.yaml` and `benchmark.py`. Use
`metewand init --language r` for R. Choose a new output directory, or explicitly
resume an existing run with `--resume`. See
[running and analyzing comparisons](docs/guide/authoring/running.md) for timing,
retries, and exports.

The [least-squares example](examples/least-squares) compares gradient descent
with coordinate descent and adds scientific validation incrementally. The
[example collection](examples) also covers native Rust workers, lifecycle callbacks,
parameter matrices, fresh sequences, and buffered profiles.

## Project status and development

Metewand is under active development. The current runner executes trusted local
code and records development-environment evidence. Locked environments,
isolation, streaming profiles, the Julia package, and portable archives are later
work. The crates and packages are not yet published to registries.

[DESIGN.md](DESIGN.md) describes the general model.
[CONTRIBUTING.md](CONTRIBUTING.md) covers contributor setup, implementation
contracts, verification, releases, and website development.
[TODO.md](TODO.md) tracks the active roadmap.

## Citation

If you use Metewand in your research, please cite the release you used. Select
that release in the [Zenodo archive](https://doi.org/10.5281/zenodo.23699794)
and export its citation. To cite Metewand generally, use the concept DOI
`10.5281/zenodo.23699794`.

Machine-readable metadata is available in [CITATION.cff](CITATION.cff).

## License

Metewand is licensed under either the Apache License, Version 2.0, or the MIT
license, at your option.
