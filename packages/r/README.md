# Metewand R package

Define Metewand benchmarks with ordinary R functions for data generation,
methods, and independent evaluation. The package handles worker communication
and standard result files. It requires R 4.2
or later, the `digest` package, and a C++17 compiler with floating-point
`std::to_chars` support, such as GCC 11 or later. Linux, macOS, and native
Windows x64 are supported. Windows requires R 4.3 or later and the matching
Rtools release; building the separate CLI also requires Rust's MSVC toolchain and
Visual Studio C++ build tools.

Install `digest` into the selected worker environment, then install this package:

```sh
R CMD INSTALL packages/r
```

For function workers created by `metewand init --language r`, install both
packages in the selected interpreter's site library. Workers use a private user
library and do not inherit ambient R library paths. See the
[installation guide](../../docs/guide/getting-started.md#start-with-r) for commands.
The [R package example](../../examples/r-package) instead bundles its installed
package beside the worker. Windows runs and caches require local NTFS.

The package dispatches callbacks, writes canonical JSON and output envelopes, and
reports structured failures. Metewand owns environments, source acquisition,
seeds, scheduling, validation, and publication. Benchmark code owns scientific
semantics and its random generators.

Helpers automatically select inherited Unix descriptors or native Windows
handles, preserve binary protocol frames, and keep standard output and standard
error separate for logs. Explicit descriptor arguments remain caller-owned.

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
[R package example](../../examples/r-package). CI checks the R package and runs
its contracts on Linux, macOS, and Windows. The portable Rust integration suite
installs the package into temporary libraries and exercises all three roles
through the native transport and output validator. For a focused check, run:

```sh
cargo test -p metewand-cli --test cross_platform --locked r_generated_dataset_and_fresh_sequences_are_portable
```
