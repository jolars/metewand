# Extend the benchmark

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
functions. `--language rust` generates callback templates for the internal Rust
helpers used by Metewand's repository examples. Build these templates within a
source checkout, then copy each executable beside its adapter with the same
basename and no extension. Metewand currently publishes only the CLI; standalone
native workers implement the versioned process protocol directly. Scaffolding
does not create a Cargo project.

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
release numbers need not match when they support binding API version 1. The native Rust
examples implement the internal `Implementation` trait and use the same serving
lifecycle. These helpers are not a published Rust library API.
The runnable [Python](https://github.com/jolars/metewand/tree/main/examples/python-package),
[R](https://github.com/jolars/metewand/tree/main/examples/r-package), and
[Rust](https://github.com/jolars/metewand/tree/main/examples/rust-worker) examples
show complete adapters and declarations. The package source and R help pages
describe their callback interfaces.

## Share problem-dependent preparation

Add `preparation` to a problem when shared inputs depend on problem parameters.
For example, a solution-path comparison needs one alpha grid even when the
reference method stops its path early:

```yaml
problems:
  path:
    parameters:
      path_length: {type: integer, default: 100}
      q: {type: number, default: 0.1}
      fit_intercept: {type: boolean, default: true}
    preparation:
      function: benchmark.py:prepare_path
    # Declare result fields, metrics, and an evaluator as usual.
```

The Python or R function takes `(data, context)`. `data` contains the original
JSON dataset, and `context.problem_parameters` (`context$problem_parameters` in
R) contains the resolved problem settings. Return a JSON object or a `Dataset`
with declared files. Implementations and evaluators read the prepared artifact
through `context.problem_dir`, while `context.dataset_dir` continues to contain
the original dataset. For example, use
`read_json(context.problem_dir / "data.json")` to read a prepared JSON grid.

Preparation runs once per dataset and problem configuration, before any
implementation timing scope, warmup, or applicability check. Repetitions and
methods share its output. `run --cache-dir cache` also reuses it across runs.
The cache identity includes the verified dataset instance, complete problem
configuration, preparation sources, launch, environment, and output schema.
Changing problem settings does not rematerialize the raw dataset. Keep solver
initialization in each implementation's own `prepare` callback.
Use `worker-timeouts.materialize` to adjust the preparation deadline.

For binary datasets, register `serve_problem_preparer(materialize)` and read the
verified `context.dataset_dir` directly. Write shared files beneath
`context.output_dir` and return a `Dataset` with `Artifact` declarations. Select
`protocol-transport: socket` and declare `output-schema` beneath `preparation`.
Raw workers serve `problem_preparer`, receive a `materialize` request with
`dataset_dir`, `problem_contract_id`, `problem_parameters`, and `output_dir`, and
return the standard dataset manifest. Consumers negotiate `problem_preparation`
before receiving `problem_dir`. Existing benchmarks without preparation keep
their original request shapes and identities.

The [shared preparation example](https://github.com/jolars/metewand/tree/main/examples/problem-preparation)
compares two methods over two path configurations and retains an early-stopped
grid for independent evaluation.

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

Fresh bindings accept compact `controls` instead of a full `control-schema`:

```yaml
delivery: fresh_sequence
controls:
  tolerance: {type: number, exclusiveMinimum: 0}
schedule:
  kind: geometric
  field: tolerance
  start: 0.1
  ratio: 0.1
  count: 12
  prefix: [{initial: true}]
```

The prefix runs first and its exact values are included in the compact control
schema. `count` counts generated controls; observation limits include the
prefix. Linear schedules also accept a prefix. Streaming and buffered bindings
can use
`check-schedule: {kind: geometric, start: 1, ratio: 2, count: 10, prefix: [0]}`
instead of listing indices. Checkpoint ratios are integers of at least two;
expanded indices must be strictly increasing safe integers.

Use a profile when the question concerns progress toward a common accuracy
target. Profile selection is explicit and requires lifecycle workers with the
corresponding capability.

| Approach | Use it when | Recorded coordinate |
| --- | --- | --- |
| Fresh sequence | You can rerun a solver from clean state at a finite schedule of iteration limits or tolerances. | The selected primary clock for each independent point, with executor wall time retained. |
| Buffered profile | You can instrument one trajectory and capture bounded local measurements and result snapshots. | An instrumented clock with a problem-authorized scope, plus executor time. |
| Streaming profile | Your solver can pause for independent evaluation at bounded checkpoints. | Executor-observed active time, excluding checkpoint pauses, plus elapsed and evaluation time. |

Fresh sequences work with black-box solvers and stop according to independent
evaluation and configured limits. Buffered profiles retain provisional local
trace values and independently evaluated snapshots separately. Instrumented
time and wall time remain different measurements.

Fresh sequences can select `primary-time: implementation_time` with authorized
execute-only timing to exclude framework result serialization. Those observations
use `measurement_basis: worker_reported`. Their cumulative budget still charges
executor wall time, including result writing, across resume and retries.

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

Checkpoint schedules accept periodic checks, such as `{every: 10, start: 0}`,
or explicit indices, such as `{indices: [0, 1, 2, 4, 8, 16]}`. Explicit indices
must be nonempty, strictly increasing, nonnegative safe integers. Both streaming
and buffered profiles use the same schedule semantics. A terminal snapshot may
occur between scheduled checks; unscheduled intermediate checks do not serialize
a snapshot. Existing periodic schedules retain their representation and identity.

## Run workers in a pinned image

On Linux, select a prebuilt image with an immutable registry digest:

```yaml
environments:
  container:
    kind: oci
    image: registry.example.org/bench@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
```

Set `environment: container` on the workers that use it. Install the language
binding package and scientific dependencies in the image. Run `metewand lock`
to prepare images, `metewand lock --check` to verify them, and `metewand run
--locked` to execute. Checks and locked runs never pull images.

The first OCI backend uses a local, rootful Linux Docker Engine and authenticated
protocol sockets over host networking. It records pinned software evidence and
control limitations; it does not provide archival isolation or container CPU and
memory accounting. Required controls that it cannot enforce cause an explicit
failure. See the [Docker example](https://github.com/jolars/metewand/tree/main/examples/locked-oci)
for image preparation and the supported configuration.

## Bundle the whole benchmark

Use `metewand bundle` to generate a portable Docker build context for a complete
benchmark. All workers run as processes inside one image, with results written
to `/output/run`. Mount an existing host output parent at `/output` when running
that image. Append `--resume` to resume the same stored run.

```sh
metewand bundle --destination ../benchmark-context \
  --base-image REGISTRY/IMAGE@sha256:DIGEST \
  --recipe install.sh --include requirements.txt --dry-run
```

Remove `--dry-run` to create the context, then run `docker build` explicitly.
The destination must be new, outside the manifest directory, with an existing
parent. Recipe paths and repeated `--include` patterns are relative to the
manifest. The optional recipe runs as root with `/bin/sh -eu` in `/benchmark`
during the build. Install bindings and scientific dependencies where isolated
workers can find them; ambient Python and R search paths are not inherited.
Without a recipe, the supplied base image must already contain those dependencies.

The command copies declared source bundles, datasets, contracts, schemas, and
reference artifacts. It preserves manifest bytes and logical identities. Include
additional dependency files explicitly. Symlinks, special files, remote dataset
sources, and nonlocal environments are unsupported. Supply a local-worker
manifest rather than flattening existing Nix, uv, renv, or OCI environments.

The runner defaults to this CLI's exact package version. Use `--runner-version`
with an exact `MAJOR.MINOR.PATCH` release when bundling from an unreleased
checkout. Generation performs no downloads or worker execution. Docker builds
may fetch dependencies; pin them in your recipe. The context inventory and
command output have separate version-1 schemas. Bundling retains the current
trusted execution guarantees.

See the [bundled benchmark example](https://github.com/jolars/metewand/tree/main/examples/bundled-benchmark)
for build, mounted-output, resume, and inspection commands.

### Local Apptainer images

On Linux with Apptainer 1.3 or newer, use an existing SIF file:

```yaml
environments:
  container:
    kind: apptainer
    image: images/worker.sif
```

Run `metewand lock`, `metewand lock --check`, and `metewand run --locked` as with
other prepared environments. The image can remain Git-ignored. Metewand verifies
its complete SHA-256 digest separately from the source snapshot and executes it
in place. It never pulls, builds, converts, or repairs images. Supported images
have a native Linux amd64 or arm64 unencrypted SquashFS system partition.
Image paths must remain inside the repository without symlinks. Keep SIF files
outside worker source globs.

All worker roles use authenticated sockets and host networking, with private
working directories. Explicit worker transport declarations must select `socket`.
CPU and memory accounting remain unavailable, and required controls still fail
when the backend cannot enforce them. See the
[Apptainer example](https://github.com/jolars/metewand/tree/main/examples/locked-apptainer).
