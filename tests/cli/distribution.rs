use std::{path::Path, process::Command};

use serde_json::Value;

#[test]
fn package_has_one_binary_and_no_library_target() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let output = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["metadata", "--no-deps", "--locked", "--format-version", "1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"], "metewand");
    let targets = packages[0]["targets"].as_array().unwrap();
    let binaries = targets
        .iter()
        .filter(|target| target["kind"] == serde_json::json!(["bin"]))
        .collect::<Vec<_>>();
    assert_eq!(binaries.len(), 1);
    assert_eq!(binaries[0]["name"], "metewand");
    assert!(targets.iter().all(|target| {
        target["crate_types"]
            .as_array()
            .unwrap()
            .iter()
            .all(|kind| !matches!(kind.as_str(), Some("lib" | "rlib" | "cdylib" | "dylib")))
    }));
}
