# Whole-manifest hashing

Metewand hashes a manifest only after it has crossed the typed parsing boundary
and every manifest-local name has resolved. The version-one digest is:

```text
SHA-256(canonical version-one manifest bytes)
```

The digest is 64 lowercase hexadecimal digits. It is a lockfile binding, not a
typed component identity, so it does not use an `mw1-<kind>-` prefix or become a
field of component identity records.

`metewand-core::manifest_hash` exposes both operations:

```rust
use std::path::Path;

use metewand_core::{
    manifest::parse_manifest,
    manifest_hash::{canonical_manifest_bytes, hash_manifest},
};

# fn example(source: &str) -> Result<(), Box<dyn std::error::Error>> {
let manifest = parse_manifest(Path::new("metewand.yaml"), source)?;
let canonical = canonical_manifest_bytes(&manifest)?;
let digest = hash_manifest(&manifest)?;

assert!(!canonical.is_empty());
assert_eq!(digest.to_string().len(), 64);
# Ok(())
# }
```

Both functions are pure. They read neither the manifest path nor the repository
filesystem, and they perform no mutation.

## Canonical representation

The representation is constructed explicitly from typed fields; it is not a
serialization of the original YAML. Consequently, comments, whitespace, YAML
key order, mapping order, aliases, equivalent merge expansions, and the logical
source path do not affect the hash.
Repository paths have already been restricted to normalized, relative UTF-8
paths by the parser.

Inline schemas contribute their normalized mappings, including the inherited
Draft 2020-12 `$schema`. Inline contracts contribute their complete typed
envelopes and normalized schema declarations. Omitting the inherited dialect
or spelling it explicitly produces identical bytes. Referenced schemas and
contracts retain their path representation here; their contents and resolved
dependencies belong to resource and component identities.

Compact fields contribute their generated inline schemas. Literal problem
defaults in an inline contract normalize to the owning problem definition's
`parameter_defaults`, matching equivalent explicit declarations. Nonempty
declaration grids contribute `parameter_grids` beside their owning definition;
they do not change generated schemas or component identities. Definitions
without declaration grids retain their existing canonical representation.

All top-level collections are present, including empty collections. Optional
scalar and resource fields are represented as JSON `null`; omitted arguments,
selections, parameter mappings, and cases are represented by their typed empty
collections. Map keys are ordered by canonical JSON. Semantic sequences remain
arrays in their declared order, including experiments, cases, worker arguments,
dataset and implementation selections, source declarations, and parameter
grids.

The complete version-one execution-policy defaults are applied before hashing:

- `network: false`;
- `worker-reuse: false`;
- `warmup-runs: 0`;
- `timing-scope: "prepare_and_execute"`;
- `primary-time: "timed_wall_time"`;
- `run-order: "sequential"`;
- `max-concurrency: 1`; and
- `enforcement: "best_effort"`.

Therefore, omitting one of these fields produces the same hash as spelling its
default explicitly. CPU, thread, memory, and timeout values have no implicit
request; their omitted representation is `null`.

Every worker environment, implementation problem declaration, experiment
selection, and named dataset or implementation parameter namespace must name a
definition in the corresponding manifest namespace. Hashing fails with
`ManifestHashError::UndefinedReference` if any name is unresolved. Resolved
references retain their validated local spelling in the typed field—the entire
target definition is already present in the same hashed manifest.

## Separation from component identities

The whole-manifest hash binds `metewand.lock` to the complete current manifest.
It is deliberately absent from `IdentityRecord` representations. Adding an
unrelated definition or experiment changes the manifest hash and invalidates an
old lockfile, but it does not change an unaffected dataset, problem,
implementation, policy, configuration, or run identity.

For an empty version-one benchmark named `empty`, the exact canonical bytes are:

```json
{"datasets":{},"environments":{},"execution_policies":{},"experiments":[],"implementations":{},"name":"empty","observation_policies":{},"problems":{},"version":1}
```

Their SHA-256 digest is:

```text
6f1ccf8407899e4f01fac61cd309f6837327482f6b61613d129d8d3d07372a4b
```

The representation is part of manifest compatibility version 1. Changing its
fields, defaults, sequence semantics, or digest construction requires an
explicit review of `MANIFEST_VERSION` and updated conformance vectors.

The switch from TOML to YAML retains the unreleased version-1 typed manifest,
public-schema, identity, and wire contracts and version-2 execution semantics.
Canonical hashing consumes the resolved typed values. Renaming a referenced
resource from `.toml` to `.yaml` changes that path value and can change the
manifest hash or an identity that contains the path. Existing run-store records remain readable.
