# Manifest parsing

`metewand-core` parses a version-1 `metewand.yaml` document into domain types
for datasets, problems, implementations, environments, experiments, execution
policies, and observation policies. The parser is pure: the caller supplies the
source text and its logical path, and parsing performs no filesystem or network
access.

```rust
use std::path::Path;

use metewand_core::manifest::{DatasetDefinition, parse_manifest};

let source = r#"
version: 1
name: "example"
datasets:
  fixed:
    output-schema: "schemas/dataset.json"
    sources: ["datasets/fixed/**"]
"#;

let manifest = parse_manifest(Path::new("metewand.yaml"), source)?;
assert!(matches!(
    manifest.datasets["fixed"],
    DatasetDefinition::Fixed(_)
));

# Ok::<(), metewand_core::manifest::ManifestParseError>(())
```

Metewand-owned YAML option keys use kebab-case, including `output-schema`,
`execution-policies`, and `max-concurrency`. Underscore spellings are rejected.
Enum values such as `one_shot` and `prepare_and_execute`, benchmark-defined
names, parameter keys, and family-owned semantics keep their declared spelling.
This unreleased version-1 spelling change preserves canonical internal records
and content identities.

The typed boundary enforces the version-1 manifest shape: required fields,
runner and environment variants, version constants, name and path syntax,
nonempty and unique lists required by the public schema, safe integer bounds,
and the one-shot capability and observation-policy restrictions. Every
Metewand-owned object rejects unknown fields, including nested workers,
parameter axes, and experiment cases.

Execution policies accept `max-concurrency` as a positive safe integer. Its
default is `1`, which keeps performance measurements serial. Zero, negative,
fractional, string, and out-of-range values fail parsing before execution.

Literal parameter defaults and axis values become `CanonicalValue`s as they are
parsed. Values must fit Metewand's language-neutral JSON domain: strings,
booleans, nulls, safe integers, finite numbers, arrays, and mappings with string
keys. Non-finite numbers and unsupported YAML tags are rejected. An untagged
date or time remains a string. Nulls are preserved in parameter values, while
nulls in typed fields such as `datasets` fail the required mapping shape.
Optional typed fields may be omitted, but an explicit null, including an empty
YAML value, is rejected. For example, omitting `enforcement` uses its default;
`enforcement: null` and `enforcement:` are invalid.

## Authoring defaults

Generated datasets, problem contracts, and implementations may omit both
`parameters` and `parameter-schema`. Metewand supplies the same closed
empty-object schema as `parameters: {}`. Defaults and overrides still undergo
schema validation; an unexpected parameter is an error.

A worker may omit `environment` when exactly one declared environment has
`kind: local`. If the manifest declares no environments, Metewand creates the
ordinary `local: {kind: local}` definition when needed. A manifest with several
local environments or only managed environments must select an environment
explicitly. Explicit selections always take precedence, and managed backends
are never inferred from a worker language or project files.

Nested execution and observation policies default their `version` to `1`.
The manifest and standalone or wrapped problem contracts still require their
own version. Flat problem declarations inherit contract version `1` and their
mapping name; see [problem contracts](problem-contracts.md). Both
`implementation-repetitions` and `measurement-repetitions` default to `1`;
`seed` remains explicit. One-shot execution also resolves these mechanical
policy defaults:

| Field | Effective default |
| --- | --- |
| `network` | `false` |
| `worker-reuse` | `false` |
| `warmup-runs` | `0` |
| `timing-scope` | `prepare_and_execute` |
| `primary-time` | `timed_wall_time` |
| `run-order` | `sequential` |
| `max-concurrency` | `1` |
| `enforcement` | `best_effort` |
| `cpus`, `threads`, `memory`, `timeout` | No requested constraint |

Omitted experiment policies select built-in local execution and one final
observation. They do not select from custom named policies. Explicit policies
can override the defaults, and profiles require explicit selection. A named
one-shot policy declares `kind: one_shot`. For example:

```yaml
execution-policies:
  serial:
    timing-scope: prepare_and_execute
    primary-time: timed_wall_time
observation-policies:
  final:
    kind: one_shot
```

Metewand resolves defaults before constructing identities. Equivalent omitted
and explicit defaults share identities. `metewand plan --output json` exposes
effective environments and worker selections in `requirements`, all control
requests in `requirements.controls`, and resolved parameter mappings and slots
in `experiments`. The human plan renders these same records. Runs retain the
resolved settings in policy records, specifications, and provenance. Defaults
never infer scientific tolerances or convergence. Missing scientific rule groups
normalize to empty objects, and reference cases are optional. See
[function declarations](authoring-cli.md#function-declarations) for source,
artifact-schema, capability, and sole-problem defaults.

## YAML syntax

Manifests and problem contracts use a single YAML mapping document. Explicit
mapping keys must be unique strings. Anchors, aliases, and merge keys are
supported, with bounded alias expansion. After alias and merge expansion,
mappings and sequences may nest at most 24 levels. Explicit fields override
merged fields, and earlier mappings in a merge sequence take precedence. Tags
that introduce non-JSON types and external includes are unsupported.

```yaml
version: 1
name: "shared-settings"
environments:
  local: { kind: "local" }
execution-policies:
  serial: &serial
    version: 1
    timing-scope: prepare_and_execute
    primary-time: timed_wall_time
    max-concurrency: 1
  parallel:
    <<: *serial
    max-concurrency: 2
```

The CLI defaults to `metewand.yaml`. An explicit `--manifest` accepts any
filename, including `.yml`, and always reads YAML. TOML configuration is no
longer accepted. Native ecosystem files such as `Cargo.toml`, `pyproject.toml`,
and Julia's `Manifest.toml` retain their own formats.

## Diagnostics

`ManifestParseError` retains the logical source path and the decoder's byte
range as a one-indexed `SourceSpan`. Columns count Unicode scalar values, and
the end position is exclusive. This representation can feed the shared typed
diagnostic envelope without reparsing human-readable error text.

Repository paths cross the typed boundary only in normalized lexical form. They
are relative UTF-8 paths with `/` separators and no empty, `.`, or `..`
components; absolute paths, backslashes, and trailing separators are rejected.
Parsing does not resolve those paths against a repository root, inspect
referenced schemas or problem contracts, expand source bundles, or follow
symlinks. The pure
[parameter-namespace validator](parameter-namespaces.md) performs the first
structural ownership checks. After the caller loads problem contracts, the pure
[manifest compatibility validator](manifest-compatibility.md) checks their
relationships with datasets, implementations, and policies. `metewand-runtime`
performs the filesystem-sensitive
[source-bundle expansion](source-bundles.md).

The complete version-1 YAML fixture is
[`fixtures/manifest/v1/complete.yaml`](https://github.com/jolars/metewand/blob/main/fixtures/manifest/v1/complete.yaml).
