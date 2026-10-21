# Benchmark initialization

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

## Compatibility

The generated manifest uses `MANIFEST_VERSION = 1`, and machine results use
`PUBLIC_SCHEMA_VERSION = 1`. This adds the initial `init` result to the existing
machine envelope without changing other command records, manifest semantics,
identities, execution semantics, or the worker protocol. The published
`machine-output` schema defines its result fields, and conformance fixtures cover
both creation and dry-run records.
