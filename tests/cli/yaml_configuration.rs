use std::{fs, process::Command};

#[test]
fn reads_yaml_by_default_and_accepts_explicit_yml_paths() {
    let root = tempfile::tempdir().unwrap();
    let source = "version: 1\nname: yaml\nenvironments:\n  host: &host {kind: local}\n  other: {<<: *host}\n";
    for (filename, arguments) in [
        ("metewand.yaml", vec!["check"]),
        (
            "benchmark.yml",
            vec!["check", "--manifest", "benchmark.yml"],
        ),
    ] {
        fs::write(root.path().join(filename), source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(root.path())
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        fs::remove_file(root.path().join(filename)).unwrap();
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn rejects_legacy_toml_syntax_without_rewriting_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("metewand.yaml");
    let source = "version = 1\nname = \"legacy\"\n";
    fs::write(&path, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root.path())
        .args(["check", "--output", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diagnostic["data"]["causes"][0]["code"], "invalid_manifest");
    assert_eq!(fs::read_to_string(path).unwrap(), source);
}
