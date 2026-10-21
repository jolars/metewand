# Benchmark authoring

## Initialization

`metewand init [PATH]` creates `PATH/metewand.toml`. The destination defaults to
the current directory. Missing directories are created, and existing directories
and unrelated files are preserved. The command is noninteractive.

```sh
metewand init benchmark --dry-run
metewand init benchmark
metewand check --manifest benchmark/metewand.toml
metewand plan --manifest benchmark/metewand.toml
```

The manifest contains exactly:

```toml
version = 1
name = "benchmark"
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

`metewand scaffold <dataset|problem|implementation> NAME --environment NAME`
adds one named component to an existing manifest. The environment must already
be defined. Implementation scaffolds also require `--problem NAME`, naming an
existing problem. These choices are explicit so scaffolding does not add or
change environments, other components, experiments, or policies.

For example, after initialization, add your chosen environment to the manifest:

```toml
[environments.host]
kind = "local"
```

Then add components incrementally:

```sh
metewand scaffold dataset observations --environment host --dry-run
metewand scaffold dataset observations --environment host
metewand scaffold problem target --environment host
metewand scaffold implementation candidate --environment host --problem target
```

`--manifest PATH` selects a custom manifest. Generated paths are relative to
that manifest's parent directory, including when the manifest is outside the
working directory. Names use the manifest's
`^[A-Za-z0-9][A-Za-z0-9._-]*$` syntax. A dot belongs to the name; it does not
introduce a nested component or directory.

### Generated files and author responsibilities

Each scaffold creates only the following files beneath its component directory:

| Component | Directory | Files |
| --- | --- | --- |
| Dataset | `datasets/NAME/` | `parameters.schema.json`, `dataset.schema.json`, `materializer.sh` |
| Problem | `problems/NAME/` | `contract.toml`, `parameters.schema.json`, `result.schema.json`, `metrics.schema.json`, `semantics.schema.json`, `evaluator.sh` |
| Implementation | `implementations/NAME/` | `parameters.schema.json`, `worker.sh` |

The dataset scaffold declares a generated dataset with a materializer. The
problem scaffold declares a contract and an independent evaluator. The
implementation scaffold declares support for the selected problem and the
version-1 `one_shot` capability. Each worker uses the raw-command runner,
`program = "sh"`, its explicitly declared source file, and the selected
environment. The shell file only prints a TODO diagnostic and exits with `1`.
Replace it with your protocol worker or use a
[Python](python-sdk.md) or [R](r-sdk.md) SDK adapter, updating the launch and
complete source bundle as needed.

The generated Draft 2020-12 schemas contain `"not": {}`, which rejects every
instance. Define the schema yourself, then remove that rejecting clause.
Scaffolding supplies no parameter meanings, dataset values, result fields,
metrics, or library translations.

The problem contract deliberately omits a family, accepted dataset schemas,
allowed timing scopes, validity rules, completion rules, and reference cases.
Its comments identify each author responsibility. Choose these explicitly,
including preprocessing rules and independently justified reference results
and metrics. `dataset_schemas = []` is an author decision for a dataset-free
problem. No reference data or results are generated, and the contract fails
`check` until its required definitions are complete. Schema and worker stubs
also require author edits before they can be used for execution. See
[problem contracts](problem-contracts.md) for the full contract.

Scaffolding never launches a worker or package manager, initializes a native
environment, or changes a native lockfile. Use the ecosystem's own tools for
environment setup. Complete runnable benchmarks belong in `examples/`.

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
definition, including inline component tables. Relative reported paths are
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
The compatibility constants remain unchanged. The published `machine-output`
schema defines both results, and conformance fixtures cover creation, dry runs,
and malformed records.
