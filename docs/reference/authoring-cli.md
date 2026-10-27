# Benchmark authoring

Start with `metewand init`, ordinary Python or R functions, and a short manifest.
Keep scientific declarations in YAML and numerical code in source files. Add
validation, documentation, and execution controls as the comparison grows.
[Separate contracts](problem-contracts.md), reusable schemas, raw process
workers, and lifecycle SDK APIs remain available.

## Initialization

`metewand init [PATH] [--language python|r]` creates `metewand.yaml` and
`benchmark.py` or `benchmark.R`. Python is the default. The destination defaults
to the current directory. Missing directories are created, and existing
unrelated files are preserved. The command is noninteractive.

```sh
metewand init benchmark --dry-run
metewand init benchmark
cd benchmark
metewand check
metewand plan
metewand check --workers
metewand run --output-dir run
```

Install the matching SDK in the interpreter environment on `PATH` first; see
[getting started](../guide/getting-started.md). Initialization reports this setup
step and the next commands. It never installs dependencies or launches workers.
The starter generates seeded integers, compares two sum methods, and evaluates
their error independently. Its neutral name is `benchmark`, independent of the
destination directory. Edit the functions and declarations to define your own
comparison.

Use `metewand init --empty` for only the following manifest, which passes
`check` and produces an empty `plan`:

```yaml
version: 1
name: benchmark
```

`--empty` cannot be combined with `--language`. Component scaffolding can extend
this manifest incrementally, including with native Rust workers.

`--dry-run` checks every destination and lists the paths it would create without
writing. Both modes reject any existing target, including a directory, symbolic
link, or dangling symbolic link. All target paths are checked before any file
is created. Exclusive creation also protects files that appear after preflight.
A dry run does not reserve paths or guarantee future filesystem permissions.

Paths may be relative or absolute and must be nonempty UTF-8. Use `--` before a
destination beginning with a hyphen, for example `metewand init -- --benchmark`.
`PATH` always names a directory, not a custom manifest filename.

## Function declarations

A dataset, implementation, or evaluator can select an ordinary function:

```yaml
datasets:
  observations:
    function: benchmark.py:generate
    parameters:
      size: {type: integer, minimum: 1, default: 100}
problems:
  fit:
    result: {estimate: number}
    metrics: {error: number}
    evaluator: {function: benchmark.py:evaluate}
implementations:
  candidate:
    function: benchmark.py:solve
experiments:
  - name: comparison
    problem: fit
    datasets: [observations]
    implementations: [candidate]
    seed: 7
```

This fragment belongs beneath `version: 1` and `name: ...`. Use `.R` source files
for R. The selected source file is automatically included in the worker bundle;
add imported modules, data files, and native lockfiles through `sources`.
`function` cannot be combined with `runner`, `entrypoint`, `program`, or `args`.
It accepts a repository-relative file and a simple function name, not an
expression. `check` and `plan` do not import the source. `check --workers` and
`run` authorize imports inside worker processes.

Functions have matching signatures in [Python](python-sdk.md#function-workers)
and [R](r-sdk.md#function-workers): `generate(context)`, `solve(data, context)`,
and `evaluate(data, result, context)`. The initial convenience interface uses
canonical JSON objects. Helpers own dataset inventories and result and metric
envelopes. Lifecycle workers and custom artifact writers support arrays in
binary files, separate preparation, profiles, and other advanced requirements.

### Resolved defaults

- Omitted parameters form a closed empty object. Compact declarations provide
  types, literal defaults, and default grids.
- Omitted environments select the sole declared local environment, or an
  implicit local environment when none are declared. Ambiguous selections fail.
- Function datasets use a standard `data.json` inventory schema. A flat problem
  with a function evaluator accepts it by default. Use `dataset-schemas: []`
  explicitly for a dataset-free problem.
- A flat problem's family defaults to its name. Allowed timing scopes default
  to `[prepare_and_execute]`, and supported budgets default to `[none]`.
- A function implementation defaults to `one_shot` and binds to the sole
  problem. With multiple problems, declare `problem-contracts` explicitly.
- Omitted experiment policies select built-in local execution and one final
  observation. Repetition counts default to one. The primary time is wall time
  over preparation, execution, and result serialization. Named policies remain
  available for explicit controls; profile selection is always explicit.
- Scientific rules and reference cases are optional. A plain metric object
  receives completion `not_required`. Declared validity or completion rules
  require an explicit evaluator verdict. Metrics never imply convergence.

Plans expose effective settings before execution. The
[least-squares example](https://github.com/jolars/metewand/tree/main/examples/least-squares)
shows a basic GD/CD comparison and adds scientific rules and inline references
to the same functions. See [problem contracts](problem-contracts.md) for
stronger checks and [running benchmarks](run-cli.md) for execution controls.

## Reported paths and failures

Human output reports every created path in creation order, including missing
parent directories. A dry run uses the same records with “Would create” wording.
Both [`json` and `jsonl`](machine-output.md) emit one version-1 `machine-output`
result. Its `data` object contains:

| Field | Meaning |
| --- | --- |
| `command` | Always `init`. |
| `dry_run` | Whether changes were only proposed. |
| `manifest` | The destination manifest path. |
| `paths` | Ordered created or proposed paths, each with `path` and `kind` (`directory` or `file`). |
| `next_steps` | Setup and command suggestions for a runnable starter; omitted for `--empty`. |

Paths address the supplied destination, and relative paths are relative to the
command's working directory. Preexisting directories do not appear in the
change list.

Invalid arguments exit with `2`. Path conflicts (`init_path_exists`) and other
filesystem failures (`init_io_failed`) exit with `74`. A diagnostic identifies
the failing path in `details.path` and reports changes already made in
`details.created_paths`; human diagnostics show those paths too. Initialization
does not roll back created directories or partially written files after an
I/O failure. Inspect the reported paths before retrying.

## Component scaffolding

`metewand scaffold <dataset|problem|implementation> NAME` adds one named
component to an existing manifest. Select `--language python|r|rust`; the default
is `python`. Implementation scaffolds require `--problem NAME`, naming an
existing problem. Add components incrementally after initialization:

```sh
metewand scaffold dataset observations --language python --dry-run
metewand scaffold dataset observations --language python
metewand scaffold problem target --language python
metewand scaffold implementation candidate --language python --problem target
```

An omitted `--environment` uses the sole declared local environment, or the
implicit `local` environment if none are declared. Multiple local environments
or exclusively managed environments require an explicit selection. Use
`--environment NAME` to select an existing environment. Scaffolding does not
write environment definitions, other components, experiments, or policies.

`--manifest PATH` selects a custom manifest. Generated paths are relative to
that manifest's parent directory, including when the manifest is outside the
working directory. Names use the manifest's
`^[A-Za-z0-9][A-Za-z0-9._-]*$` syntax. A dot belongs to the name; it does not
introduce a nested component or directory. Generated component keys are quoted
so names such as `100_10` remain strings when the manifest is read.

Scaffolding preserves existing YAML comments and formatting, including block
and flow mappings. An anchored document root, an anchored or aliased target
namespace, merge keys inside that namespace, or a namespace inherited through
a root merge causes `scaffold_invalid_request` before any files are created.
Expand the affected mapping manually, then retry. Anchors on unrelated
components or environments and unrelated root merges are supported. Successful
edits are reparsed and checked to change only the requested component.

### Generated files and author responsibilities

Each scaffold creates one adapter beneath its component directory and updates
the manifest. The extension is `.py`, `.R`, or `.rs` for Python, R, or Rust:

| Component | Directory | Adapter basename |
| --- | --- | --- |
| Dataset | `datasets/NAME/` | `materializer` |
| Problem | `problems/NAME/` | `evaluator` |
| Implementation | `implementations/NAME/` | `worker` |

Parameters start as `parameters: {}`, a closed empty object. Add typed
parameters, defaults, and grids as needed. Python and R scaffolds use function
references with `generate`, `solve`, or `evaluate` stubs. Their callbacks raise
clear TODO errors until implemented. Problem scaffolds start with empty compact
`result` and `metrics` mappings; declare the fields your callbacks return. No
separate contract, schema, or reference files are required.

Choose `dataset-schemas: []` for a dataset-free problem. Add scientific rules
and independently justified reference results when useful. `check` and `plan`
validate declarations without importing source, while `check --workers` verifies
callables and executes any declared references. Zero references means no
scientific reference checks were performed.

Install the matching [Python](python-sdk.md) or [R](r-sdk.md) SDK into the
selected environment. The [Rust](rust-sdk.md) adapters use native helpers through
the command runner. Add them as binaries to your Cargo project with local path
dependencies on `metewand-runtime` and `metewand-protocol`, plus `serde_json`.
Build and copy each executable beside its `.rs` adapter with the same basename
and no extension. The generated manifest includes its source and executable.
Rust datasets retain an explicit artifact schema placeholder for the author to
replace. Scaffolding creates no Cargo project and invokes no package manager.

Finish by adding an experiment. Policies may be omitted for the basic local
comparison or selected explicitly for other execution and observation controls.
Lifecycle callbacks and custom writers remain available for more involved data
and timing boundaries.

### Dry runs, path reports, and conflicts

`--dry-run` performs the same request, manifest, and path checks without
writing. Its path report matches a subsequent invocation if filesystem state
does not change. Existing directories are reused, and missing directories are
reported before their files. A duplicate component name or any existing target
file fails in both modes. Symbolic links, including dangling links, are rejected
at the manifest and generated paths. Unrelated files remain untouched.

Human output reports `Created` and `Modified` paths, or `Would create` and
`Would modify` in a dry run. Both machine formats emit one version-1
`machine-output` result with these fields:

| Field | Meaning |
| --- | --- |
| `command` | Always `scaffold`. |
| `component` | `dataset`, `problem`, or `implementation`. |
| `name` | The new manifest-local component name. |
| `dry_run` | Whether changes were only proposed. |
| `manifest` | The selected manifest path. |
| `paths` | Ordered changes, each with `path`, `kind` (`directory` or `file`), and `action` (`create` or `modify`). |

The manifest is the only modified file and appears last. Existing manifest
values, comments, and source text are preserved when adding the requested
definition, including flow component mappings. Relative reported paths are
relative to the command's working directory.

Scaffolding takes a nonblocking manifest lock, shared for dry runs and exclusive
for writes. A competing operation fails with `scaffold_manifest_busy` and may
be retried after the lock is released. No lock or temporary files are created.
Generated files use exclusive creation to protect files that appear after
preflight. The manifest edit checks the original file and contents again before
writing, so detected intervening edits fail with `scaffold_manifest_changed`.

Invalid arguments or component choices exit with `2`; malformed existing
manifests exit with `3`; path conflicts and filesystem failures exit with `74`.
Diagnostics include `details.path` and `details.changed_paths`, listing changes
already made in execution order. Human diagnostics show the same changes. A
failure leaves those changes for inspection, including any partially written
new file or manifest. The manifest retains its existing permissions. A dry run
does not reserve paths or guarantee that a later write will have permission.

## Compatibility

Function declarations and the new defaults extend the pre-MVP version-1
authoring boundary. They lower into existing worker launches and typed policies.
Existing explicit declarations retain their canonical representations and
persisted records remain readable. Inline references have distinct identities
from file-backed fixtures. The comparison view and initialization suggestions
are additive version-1 machine fields. These changes do not alter identity
transcripts, the run-store format, wire protocol, or execution-semantics version.
See [compatibility versions](compatibility-versions.md).
