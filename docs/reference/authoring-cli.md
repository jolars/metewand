# Benchmark authoring

Scaffolds create compact declarations in one manifest and small SDK adapters.
[Separate contracts](problem-contracts.md) and [schemas](schema-validation.md#inline-declarations)
remain supported for reuse. Implementations and generated datasets use
[compact parameter declarations](parameter-namespaces.md#compact-implementation-parameters)
with literal defaults or default grids. Problem contracts accept the same
`parameters` form and compact `result` and `metrics` declarations. Each boundary
can independently use a compact declaration or an explicit schema. Problem
fields may also live directly under `problems.<name>`, without a `contract` wrapper.
Implementation parameters keep their own mapping alongside worker settings.
Experiment axes override declaration grids, and named cases override experiment
axes. Experiment and case parameters accept plain scalar literals; arrays and
objects use `value`, and sweeps use `grid`. Parameter diagnostics point to the
authored declaration or effective supplied value in YAML. Local-environment and
one-shot policy defaults resolve before planning. A separate family
`semantics-schema` is optional; scientific rules and reference cases remain explicit.

## Initialization

`metewand init [PATH]` creates `PATH/metewand.yaml`. The destination defaults to
the current directory. Missing directories are created, and existing directories
and unrelated files are preserved. The command is noninteractive.

```sh
metewand init benchmark --dry-run
metewand init benchmark
metewand check --manifest benchmark/metewand.yaml
metewand plan --manifest benchmark/metewand.yaml
```

The manifest contains exactly:

```yaml
version: 1
name: benchmark
```

The neutral name does not depend on the destination directory. Edit it to name
your benchmark. This manifest passes `check` and produces an empty `plan`.
It contains no scientific definitions. Initialization creates no datasets,
problems, evaluators, fixtures, implementations, environments, policies, worker
code, or native lockfiles. It never launches workers or package managers.

`--dry-run` checks the destination and lists the paths it would create without
writing anything. Both modes fail if the manifest path already exists, including
a directory, a symbolic link, or a dangling symbolic link. Manifest creation is
exclusive: a file that appears after preflight is also protected. A dry run does
not reserve paths or promise that permissions and filesystem state will remain
unchanged before a later invocation.

Paths may be relative or absolute and must be nonempty UTF-8. Use `--` before a
destination beginning with a hyphen, for example `metewand init -- --benchmark`.
`PATH` always names a directory, not a custom manifest filename.

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

Paths address the supplied destination, and relative paths are relative to the
command's working directory. Preexisting directories do not appear in the
change list.

Invalid arguments exit with `2`. Path conflicts (`init_path_exists`) and other
filesystem failures (`init_io_failed`) exit with `74`. A diagnostic identifies
the failing path in `details.path` and reports changes already made in
`details.created_paths`; human diagnostics show those paths too. Initialization
does not roll back created directories or a partially written manifest after an
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

Parameters start as `parameters: {}`, a closed empty object. Add typed scientific
parameters, defaults, and grids as needed. The generated dataset has an inline
`output-schema: {not: {}}`; define the artifact shape and remove the rejecting
clause. Problem fields live directly under `problems.NAME`, with compact `result`
and `metrics` mappings. No separate contract, schema, or reference files are
created. You can replace any boundary with a reusable external declaration later.

The problem deliberately leaves its family, accepted dataset schemas, timing
scopes, preprocessing, validity, completion target, and reference cases for the
author. Choose `dataset-schemas: []` explicitly for a dataset-free problem.
Supply independently justified reference results and metrics. Scaffolding
checks YAML structure and worker declarations while allowing unfinished flat
scientific definitions between component additions. `check`, `plan`, and `run`
require complete declarations and reference fixtures. See
[problem contracts](problem-contracts.md) for the full contract.

Python and R adapters use the [Python](python-sdk.md) and [R](r-sdk.md) runners.
Install the matching SDK into the selected environment. The [Rust](rust-sdk.md)
adapters use native helpers through the command runner. Add them as binaries to
your Cargo project with local path dependencies on `metewand-runtime` and
`metewand-protocol`, plus `serde_json`. Build and copy each executable beside its
`.rs` adapter with the same basename and no extension. The generated manifest
includes both the source and executable in its source bundle. Add other solver
sources and native lockfiles to `sources` as needed. Scaffolding creates no
Cargo project and runs no build or package manager.

Adapters already serve handshakes, dispatch callbacks, and report structured
errors through the SDK. Their scientific callbacks raise TODO failures until
you implement them. Implementations own preparation, solving, and conversion
into scientific result data; the SDK writes the standard result envelope.
Evaluators independently compute metrics and choose an explicit verdict,
consuming declared rules through `problem_semantics`. Dataset callbacks write
scientific files and use SDK inventory helpers. Add lifecycle hooks or custom
artifact writers when needed, preserving the chosen timing boundary.

Finish the benchmark by declaring an experiment and choosing its execution and
observation policies. Keep `timing-scope`, `primary-time`, scientific rules, and
profile selection explicit. The
[Python SDK example](https://github.com/jolars/metewand/tree/main/examples/python-sdk)
shows a complete benchmark with one manifest, small adapters, and reference
fixtures. Use ecosystem tools to initialize environments and install the SDKs;
scaffolding never changes native lockfiles or launches workers.

The [Benchopt SLOPE comparison](https://github.com/jolars/metewand/tree/main/examples/benchopt-slope)
reuses an existing dataset generator, two solvers, and an independent evaluator
through small SDK adapters. Its validation script checks three repetitions per
configuration, completion misses, effective settings, metrics, and timings
through the public CLI. Human `run` output shows metrics and failure reasons;
use `plan` for settings and `explain` or `export` for recorded timing evidence.

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

The generated manifest uses `MANIFEST_VERSION = 1`, and machine results use
`PUBLIC_SCHEMA_VERSION = 1`. The `init` and `scaffold` results add command shapes
to the existing machine envelope without changing other command records,
manifest semantics, identities, execution semantics, or the worker protocol.
Compact scaffolds use existing authoring syntax and SDKs, so this change needs
no compatibility version bump. Existing external declarations remain supported.
The compatibility constants remain unchanged. The published `machine-output`
schema defines both results, and conformance fixtures cover creation, dry runs,
and malformed records.
