# Metewand

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

Windows uses Rust's MSVC toolchain and Visual Studio C++ build tools. See
[getting started](docs/guide/getting-started.md) for PowerShell installation,
Rtools, and the local NTFS storage requirement.

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

## License

Metewand is licensed under either the Apache License, Version 2.0, or the MIT
license, at your option.
