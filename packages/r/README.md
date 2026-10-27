# Metewand R package

Define Metewand benchmarks with ordinary R functions for data generation,
methods, and independent evaluation. The package handles worker communication
and standard result files. It requires R 4.2
or later, POSIX, the `digest` package, and a C++17 compiler with floating-point
`std::to_chars` support, such as GCC 11 or later.

Install `digest` into the selected worker environment, then install this package:

```sh
R CMD INSTALL packages/r
```

The package dispatches callbacks, writes canonical JSON and output envelopes, and
reports structured failures. Metewand owns environments, source acquisition,
seeds, scheduling, validation, and publication. Benchmark code owns scientific
semantics and its random generators.

For a simple benchmark, use `metewand init --language r` and functions selected
with `function: benchmark.R:solve`. Contexts, JSON data, and standard envelopes
are handled by the package. Plain metrics imply `not_required` when no scientific
rules are declared. Add explicit `evaluation(metrics, verdict)` when adding
validity or completion rules.

Lifecycle implementations return scientific data from `execute`; optional
`result_data` converts solver state after diagnostic timing. Evaluators return
`evaluation(metrics, verdict)`. The package writes standard envelopes, while custom
`result` writers and evaluator-written metrics remain supported.

See the [authoring guide](../../docs/guide/authoring/index.md),
[package help](man/workers.Rd), and the runnable
[R package example](../../examples/r-package). The normal Rust workspace tests install
the package into temporary libraries, run its contracts, and exercise all three
roles through Rust's transport and output validator. For focused checks, run:

```sh
cargo test -p metewand-runtime --test r_sdk --locked
cargo test -p metewand-cli --test r_sdk --locked
```
