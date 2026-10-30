# Metewand <img src="https://raw.githubusercontent.com/jolars/metewand/refs/heads/main/assets/logo.png" align="right" width="160" alt="A green measuring rod with brass ends" />

[![DOI](https://img.shields.io/badge/DOI-10.5281%2Fzenodo.23699794-blue)](https://doi.org/10.5281/zenodo.23699794)

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

Starting with the next release, Rust users can also install the CLI from
crates.io with Rust 1.98.0 or later:

```sh
cargo install metewand --locked
```

## Try a benchmark

With the CLI and Python worker package
[installed](docs/guide/getting-started.md#install), create and run a starter:

```sh
metewand init my-benchmark
cd my-benchmark
metewand check --workers
metewand run --output-dir run
```

The generated `benchmark.py` compares Python's built-in sum with a loop on seeded
integers and independently checks each answer. The experiment in `metewand.yaml`
declares the comparison:

```yaml
experiments:
  - name: comparison
    problem: sum
    datasets: [numbers]
    implementations: [builtin, loop]
    seed: 7
```

Metewand prints timings and absolute errors and saves the results in `run/`.
Replace the generated functions and edit the configuration to compare your own
methods. For R, start with `metewand init my-benchmark --language r`.

Follow the [authoring guide](docs/guide/authoring/index.md) or adapt the
[least-squares example](examples/least-squares) for a fuller comparison. See
[running and analyzing comparisons](docs/guide/authoring/running.md) for repeated
measurements and exports.

## Project status and development

Metewand is under active development. The current runner executes trusted local
code and supports locked Nix and uv environments, streaming profiles, and
portable result archives. Isolated execution and the Julia package remain
planned. The CLI, Python package, and R package have separate release workflows.

The [design](docs/design.md) describes the general model.
[CONTRIBUTING.md](CONTRIBUTING.md) covers contributor setup, implementation
contracts, verification, releases, and website development.
[TODO.md](TODO.md) lists remaining work by area.

## Citation

If you use Metewand in your research, please cite the release you used. Select
that release in the [Zenodo archive](https://doi.org/10.5281/zenodo.23699794)
and export its citation. To cite Metewand generally, use the concept DOI
`10.5281/zenodo.23699794`.

Machine-readable metadata is available in [CITATION.cff](CITATION.cff).

## License

Metewand is licensed under either the Apache License, Version 2.0, or the MIT
license, at your option.
