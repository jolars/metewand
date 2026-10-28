# Metewand R package

Define benchmarks with ordinary R functions and small scientific result objects.
This package contains no native code. It requires R 4.2 or later and `jsonlite`.
The CLI supplies worker adapters, measures execution, validates outputs,
computes file hashes, and writes canonical artifacts.

Install `jsonlite` in the selected R environment, then install the package:

```sh
Rscript --vanilla -e 'install.packages("jsonlite", repos="https://cloud.r-project.org")'
R CMD INSTALL packages/r
```

Metewand itself needs no compiler or Rtools. Standard dependencies may contain
native code; installing those dependencies from source follows their own
requirements. Linux, macOS, and native Windows x64 are supported.

Use `metewand init --language r` for ordinary `generate(context)`,
`solve(data, context)`, and `evaluate(data, result, context)` functions. Select
functions in YAML with `function: benchmark.R:solve`. Install packages in the
worker interpreter's site library as described in the [installation
guide](../../docs/guide/getting-started.md#start-with-r).

For separate preparation and solving, register named callbacks:

```r
serve_implementation(
  prepare = function(context) read_json(file.path(context$dataset_dir, "data.json")),
  execute = function(state, context) list(answer = sum(unlist(state$values))),
  reset = function(state, context) NULL
)
```

Select `protocol-transport: socket` for lifecycle workers. Context contains role
parameters, problem parameters, seeds, scientific rules, assigned directories,
and optional observation controls. Additional context fields and optional
callbacks do not change existing function signatures. Callbacks remain
responsible for scientific semantics and seeding their random generators.

Return named lists for ordinary JSON output. Use `dataset(data, files)`,
`result(data, files)`, and `evaluation(metrics, verdict)` for explicit outputs.
Write binary files beneath `context$output_dir`, close them, and declare them
with `artifact(path, media_type, representation)`. The CLI computes inventories
and hashes. Named lists represent objects; unnamed lists represent arrays. Use
`json_object()` for an empty object and `json_array()` for a singleton array.

The package implements **binding API version 1**, independently of package and
CLI release numbers. Compatible CLI upgrades do not require reinstalling the
package. Imports, output constructors, and JSON utilities work without the CLI;
serving requires a compatible CLI and never downloads an adapter.

The pre-MVP migration replaces positional callbacks with context callbacks,
`result_data` with `result(output, context)`, and manual manifest writers with
output values and artifact declarations. Canonicalization helpers, raw string
wrappers, and descriptor arguments have been removed. R's optional in-worker
solve timing is unavailable; the CLI's timing remains authoritative.

See [package help](man/workers.Rd), the [authoring
guide](../../docs/guide/authoring/index.md), and the runnable [R
example](../../examples/r-package). CI checks source installation and the worker
chain on Linux, macOS, and Windows.
