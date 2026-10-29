# 6. Extend the benchmark

Ordinary functions and compact declarations cover small one-shot comparisons.
Use more explicit interfaces when the benchmark needs additional data formats,
worker state, or observations.

## Add components

Scaffolding adds a component and an adapter to an existing manifest:

```sh
metewand scaffold dataset observations --language python --dry-run
metewand scaffold dataset observations --language python
metewand scaffold problem target --language python
metewand scaffold implementation candidate --language python --problem target
```

The adapters go under `datasets/`, `problems/`, and `implementations/`. Fill in
their functions, declare result and metric fields, and add an experiment that
selects them. Scaffolding does not add experiments or install dependencies.
It preserves existing manifest comments and refuses to overwrite adapter files.
Use `--dry-run` to inspect the proposed paths first.

`--manifest PATH` selects a different manifest. `--language r` selects R
functions; `--language rust` generates native worker adapters. Rust adapters need
to be added to a Cargo project and built. Copy each executable beside its
adapter with the same basename and no extension. The crates currently require
local path dependencies; scaffolding does not create a Cargo project.

## Use R or native workers

R functions have the same roles as Python:

```r
generate <- function(context) {
  set.seed(context$seed %% .Machine$integer.max)
  list(values = as.list(sample.int(10L, context$parameters$size, replace = TRUE)))
}

solve <- function(data, context) {
  list(total = sum(unlist(data$values)))
}

evaluate <- function(data, result, context) {
  list(absolute_error = abs(result$total - sum(unlist(data$values))))
}
```

Select them with references such as `function: benchmark.R:solve`. The generator
expects a declared integer parameter `size`. Return named lists for JSON
objects and unnamed lists for arrays. Use `metewand::evaluation(metrics, verdict)`
when scientific rules require an explicit verdict. The
[R starter](../getting-started.md#start-with-r) supplies a complete manifest.

The function context supplies `parameters` for the current role,
`problem_parameters` for implementations and evaluators, `seed` for generators
and implementations, and `semantics` for evaluators. Evaluators have no supplied
random seed. Install dependencies in each selected worker environment.

Native Rust helpers run without a Python or R host. Other languages can
implement the process protocol directly. Start with the
[Rust example](https://github.com/jolars/metewand/tree/main/examples/rust-worker)
or [raw worker example](https://github.com/jolars/metewand/tree/main/examples/fixed-dataset).

## Separate preparation and solving

Lifecycle workers let you load data in `prepare`, run a method in `execute`,
convert its output, and reset state before reuse. They also support custom
artifact writers and configuration-dependent applicability checks.

Python and R both provide `serve_implementation` with named callbacks.
Use `prepare(context)` to return state and `execute(state, context)` to return
solver output. Optional `result(output, context)` converts that output, and
`reset(state, context)` clears slot state. Select `protocol-transport: socket`
for these lifecycle workers. The CLI supplies the adapter; package and CLI
release numbers need not match when they support binding API version 1. Native Rust workers
implement the `Implementation` trait and use the same serving lifecycle.
The runnable [Python](https://github.com/jolars/metewand/tree/main/examples/python-package),
[R](https://github.com/jolars/metewand/tree/main/examples/r-package), and
[Rust](https://github.com/jolars/metewand/tree/main/examples/rust-worker) examples
show complete adapters and declarations. The package source and R help pages
describe their callback interfaces.

## Include source and data deliberately

Function declarations automatically include the selected source file. Add
imported modules, data files, and lockfiles through `sources`:

```yaml
function: benchmark.py:gd
sources: [benchmark.py, solvers.py, requirements.txt]
```

Metewand hashes declared source content. Workers receive private copies of
their bundles, and undeclared sibling modules are not automatically available.
A lockfile in `sources` records its bytes; the local runner does not install or
provision its dependencies.

Function workers exchange JSON objects. For large matrices, sparse structures,
or native formats, use a materializer and custom result writers. Write all files
under `context.output_dir` (or `context$output_dir` in R), close them, and return
a `Dataset` or `Result` with `Artifact` declarations (`dataset`, `result`, and
`artifact` in R). Declare paths, media types, and optional representation
metadata. The CLI computes inventories and hashes, writes canonical envelopes,
and validates schemas before publication. Native workers continue to return
completed manifests directly. See the
[generated-dataset example](https://github.com/jolars/metewand/tree/main/examples/generated-dataset).

## Move from final answers to profiles

Use a profile when the question concerns progress toward a common accuracy
target. Profile selection is explicit and requires lifecycle workers with the
corresponding capability.

| Approach | Use it when | Recorded coordinate |
| --- | --- | --- |
| Fresh sequence | You can rerun a solver from clean state at a finite schedule of iteration limits or tolerances. | Executor-observed time for each independent point. |
| Buffered profile | You can instrument one trajectory and capture bounded local measurements and result snapshots. | An instrumented clock with a problem-authorized scope, plus executor time. |
| Streaming profile | Your solver can pause for independent evaluation at bounded checkpoints. | Executor-observed active time, excluding checkpoint pauses, plus elapsed and evaluation time. |

Fresh sequences work with black-box solvers and stop according to independent
evaluation and configured limits. Buffered profiles retain provisional local
trace values and independently evaluated snapshots separately. Instrumented
time and wall time remain different measurements.

Streaming profiles publish each evaluated checkpoint before the worker receives
`continue` or `stop`. Their active clock includes snapshot serialization and
excludes validation, evaluation, and durable publication. Checkpoints remain
available after a later failure. A clean-state retry preserves that evidence
and selects a successful curve from one producing attempt.

The [fresh-sequence example](https://github.com/jolars/metewand/tree/main/examples/fresh-sequence)
contains Python and R adapters, explicit and geometric schedules, repetitions,
and resume commands. The
[buffered-profile example](https://github.com/jolars/metewand/tree/main/examples/buffered-profile)
contains Python and native Rust workers, bounded traces, selected snapshots,
and exports. Start from these complete manifests rather than adding profile
fields to a function worker.

The [streaming-profile example](https://github.com/jolars/metewand/tree/main/examples/streaming-profile)
contains Python and native Rust checkpoint helpers, evaluator-controlled
stopping, active-time coordinates, and partial-curve exports. Python registers
`execute_stream(state, context)` and uses `context.stream.checkpoint(...)` to
block for a decision. Stop all solver work at that barrier, including work in
child processes or background threads.

Find further integrations in [examples](../examples.md), or read the
[design](../../design.md) for the underlying model.
