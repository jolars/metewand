# Manifest parsing

`metewand-core` parses a version-1 `metewand.toml` document into domain types
for datasets, problems, implementations, environments, experiments, execution
policies, and observation policies. The parser is pure: the caller supplies the
source text and its logical path, and parsing performs no filesystem or network
access.

```rust
use std::path::Path;

use metewand_core::manifest::{DatasetDefinition, parse_manifest};

let source = r#"
version = 1
name = "example"

[datasets.fixed]
output_schema = "schemas/dataset.json"
sources = ["datasets/fixed/**"]
"#;

let manifest = parse_manifest(Path::new("metewand.toml"), source)?;
assert!(matches!(
    manifest.datasets["fixed"],
    DatasetDefinition::Fixed(_)
));

# Ok::<(), metewand_core::manifest::ManifestParseError>(())
```

The typed boundary enforces the version-1 manifest shape: required fields,
runner and environment variants, version constants, name and path syntax,
nonempty and unique lists required by the public schema, safe integer bounds,
and the one-shot capability and observation-policy restrictions. Every
Metewand-owned object rejects unknown fields, including nested workers,
parameter axes, and experiment cases.

Literal parameter defaults and axis values become `CanonicalValue`s as they are
parsed. TOML dates, times, and non-finite numbers are rejected because they are
outside Metewand's language-neutral JSON domain.

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
structural ownership checks; the remaining operations require repository
context. `metewand-runtime` performs the filesystem-sensitive
[source-bundle expansion](source-bundles.md).

The complete version-1 TOML fixture is
[`fixtures/manifest/v1/complete.toml`](../fixtures/manifest/v1/complete.toml).
