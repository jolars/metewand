use std::path::Path;

use metewand_core::{manifest::parse_manifest, manifest_hash::hash_manifest};

#[test]
fn uv_selections_are_sets_and_empty_selections_preserve_legacy_hashes() {
    let manifest = "version: 1\nname: uv\nenvironments: {python: {kind: uv, project: ., lockfile: uv.lock%s}}\n";
    let hash = |selections| {
        let parsed = parse_manifest(
            Path::new("metewand.yaml"),
            &manifest.replace("%s", selections),
        )
        .unwrap();
        hash_manifest(&parsed).unwrap()
    };
    assert_eq!(hash(""), hash(", groups: [], extras: []"));
    assert_eq!(
        hash(", groups: [test, bench, test], extras: [gpu, cpu]"),
        hash(", extras: [cpu, gpu], groups: [bench, test]")
    );
    assert_ne!(hash(""), hash(", groups: [bench]"));
}
