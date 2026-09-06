# Source bundles

`metewand-runtime` expands the repository sources declared by fixed datasets
and process workers. The caller supplies an established repository root and one
typed `sources` list from the manifest:

```rust
use std::path::Path;

use metewand_core::manifest::{DatasetDefinition, parse_manifest};
use metewand_runtime::source_bundle::expand_source_bundle;

fn expand_fixed_dataset(
    repository_root: &Path,
    manifest_source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = parse_manifest(Path::new("metewand.toml"), manifest_source)?;
    let DatasetDefinition::Fixed(dataset) = &manifest.datasets["fixed"] else {
        return Err("expected a fixed dataset".into());
    };
    let bundle = expand_source_bundle(repository_root, &dataset.sources)?;
    assert!(bundle.paths().iter().all(|path| path.is_relative()));
    Ok(())
}
# let _ = expand_fixed_dataset;
```

This work belongs in runtime rather than core because it reads filesystem state.
The manifest parser remains pure.

`identify_source_bundle` applies the same expansion and containment checks, then
derives a typed `source-bundle` identity from the expanded logical paths and
their contents. Regular files contribute their SHA-256 content hash and
portable executable bit, directories contribute the versioned local-tree hash,
and symbolic links contribute their exact UTF-8 target spelling. Host paths,
timestamps, ownership, and non-executable permission bits do not contribute.
The expanded path order makes declaration order irrelevant, while any selected
content change invalidates the source bundle and its transitive definition and
plan identities.

## Path and glob contract

Manifest paths must be in normalized form before expansion: they are nonempty,
relative UTF-8 paths written with `/`, without empty, `.`, or `..` components.
The repository root itself may have any spelling; expansion canonicalizes it
and requires it to resolve to a directory. Absolute host paths never appear in
a successful bundle.

Version 1 supports these case-sensitive glob forms:

- `?` matches one character other than `/`;
- `*` matches zero or more characters other than `/`;
- `**` matches recursively across path components;
- `[abc]` and `[!abc]` match character classes; and
- `{a,b}` matches alternatives.

`**` is valid alone, at the start followed by `/`, at the end preceded by `/`,
or between two `/` separators. Metacharacters can be matched literally with a
character class—for example, `[*]` matches `*`.

Backslash escaping is unavailable because backslashes are not valid repository
path separators. Dotfiles have no special exclusion. Wildcard traversal does
not follow symlinked directories that it discovers, so a recursive pattern
cannot loop through the filesystem or silently acquire another tree.

Each declaration is required independently: if any literal path or pattern
matches no filesystem entry, expansion fails. Matches from all declarations are
then deduplicated and sorted lexicographically by their UTF-8 bytes. This order
does not depend on declaration order, directory iteration order, locale, or
platform collation.

## Tree containment

A matched file or symlink is checked directly. A matched directory selects a
tree, so every descendant is checked even though the expanded result retains
the directory as one match. This makes a literal directory declaration as safe
as an equivalent recursive glob.

Every selected path is resolved against the canonical repository root. A
symlink is accepted only when its target exists and remains beneath that root;
broken links and escaping links are errors. An escaping symlink in an unrelated,
unselected tree does not invalidate a narrower source bundle.
