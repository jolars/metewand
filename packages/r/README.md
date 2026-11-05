# Metewand for R <img src="https://raw.githubusercontent.com/jolars/metewand/refs/heads/main/assets/logo.png" align="right" width="160" alt="A green measuring rod with brass ends" />

Write reproducible computational benchmarks with ordinary R functions. Define
how to generate data, run your methods, and evaluate their answers, then declare
the comparison in YAML. The Metewand CLI runs each method in a separate process
and saves timings, metrics, failures, and provenance.

This package provides the R functions and result objects you need to write a
benchmark. The CLI checks and runs it on Linux, macOS, and native Windows x64.

## Install

You need R 4.2 or later. Install the package and its dependencies from GitHub
with [pak](https://pak.r-lib.org/):

```r
install.packages("pak") # Run once if pak is not installed.
pak::pak("jolars/metewand/packages/r", lib = .Library)
```

Install into an R installation whose library (`.Library`) you can write to.
Benchmark workers use a private user library, so they cannot see packages
installed only in your personal R library. Install other packages your benchmark
needs into the same installation library.

Metewand's R package contains no native code and needs no compiler or Rtools.
Its `jsonlite` dependency follows its own requirements when installed from
source.

Install the separate **Metewand CLI** by following [getting
started](https://metewand.org/guide/getting-started#install). Installing the R
package does not install the CLI. On Windows, put `R.exe` and `Rscript.exe` on
`PATH`.

The command above installs the development version. For a reproducible
benchmark, append `@<commit-hash>` to `jolars/metewand/packages/r` and pin your
scientific dependencies.

## Try a benchmark

With the CLI and R package installed, run these commands in your terminal:

```sh
metewand init my-benchmark --language r
cd my-benchmark
metewand check --workers
metewand run --output-dir run
```

The starter creates `benchmark.R` and `metewand.yaml`. It generates seeded
integers, compares R's built-in sum with a loop, and independently checks each
answer. Metewand prints timings and absolute errors and saves the results in
`run/`.

## Write your own comparison

Replace the generated functions with your data, methods, and metrics. A method
and its evaluator can be as small as:

```r
builtin_sum <- function(data, context) {
  list(total = sum(unlist(data$values)))
}

evaluate <- function(data, result, context) {
  expected <- sum(unlist(data$values))
  list(absolute_error = abs(result$total - expected))
}
```

Select functions in YAML with entries such as
`function: benchmark.R:builtin_sum`. The `context` argument supplies your
parameters, a seed, and input and output directories. For randomized code, seed
R explicitly with `set.seed(context$seed %% .Machine$integer.max)`.

Return named lists for ordinary JSON data and metrics; unnamed lists represent
arrays. For benchmarks with an accuracy target, use
`evaluation(metrics, verdict)` to report whether the method reached it. Use
`dataset()`, `result()`, and `artifact()` when you also need to return files.
The CLI records and checks those files alongside the JSON output.

Follow the [authoring guide](https://metewand.org/guide/authoring/) for
parameter sweeps, repeated measurements, reference checks, and result exports.
If your methods need separate setup and solve steps, see the [R
example](https://github.com/jolars/metewand/tree/main/examples/r-package).
Explore the package's functions with `help(package = "metewand")` in R.

## Project status

Metewand is under active development. The R package and CLI have separate
releases; their version numbers do not need to match. This package uses binding
API version 1, so compatible CLI upgrades do not require a package update.

See the [main repository](https://github.com/jolars/metewand) for project news,
citation information, and contributor instructions.
