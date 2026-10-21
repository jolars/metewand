use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use metewand_core::{
    manifest::{DatasetDefinition, WorkerDefinition, parse_manifest},
    public_schemas::{PublicSchema, public_schema_catalog},
    schema::SchemaCatalog,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const MANIFEST: &str = "# Keep the author's formatting.\nversion=1\nname = 'benchmark' # Keep this comment.\n\n[environments.local]\nkind='local'\n";

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metewand"));
    command.current_dir(root);
    command.env("PATH", root.join("no-programs"));
    command
}

fn scaffold(root: &Path, component: &str, name: &str, extra: &[&str]) -> Output {
    command(root)
        .args(["--output", "json", "scaffold", component, name])
        .args(extra)
        .output()
        .unwrap()
}

fn envelope(output: &Output, success: bool) -> Value {
    assert_eq!(output.status.success(), success, "{output:?}");
    let value = serde_json::from_slice(&output.stdout).unwrap();
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::MachineOutput.repository_path(), &value)
        .unwrap();
    value
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn scaffolds_all_three_components_incrementally_without_scientific_defaults() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
    fs::write(root.path().join("uv.lock"), "preserve native lock\n").unwrap();
    fs::write(root.path().join("renv.lock"), "preserve native lock\n").unwrap();
    for (component, name, extra, expected_files) in [
        (
            "dataset",
            "sample.data",
            vec!["--environment", "local"],
            vec![
                "parameters.schema.json",
                "dataset.schema.json",
                "materializer.sh",
            ],
        ),
        (
            "problem",
            "sample.problem",
            vec!["--environment", "local"],
            vec![
                "contract.toml",
                "parameters.schema.json",
                "result.schema.json",
                "metrics.schema.json",
                "semantics.schema.json",
                "evaluator.sh",
            ],
        ),
        (
            "implementation",
            "sample.impl",
            vec!["--environment", "local", "--problem", "sample.problem"],
            vec!["parameters.schema.json", "worker.sh"],
        ),
    ] {
        let before = files(root.path());
        let result = scaffold(root.path(), component, name, &extra);
        assert!(result.stderr.is_empty(), "{result:?}");
        let report = envelope(&result, true);
        assert_eq!(report["data"]["command"], "scaffold");
        assert_eq!(report["data"]["component"], component);
        assert_eq!(report["data"]["name"], name);
        assert_eq!(report["data"]["dry_run"], false);
        let paths = report["data"]["paths"].as_array().unwrap();
        assert_eq!(paths.len(), expected_files.len() + 3);
        assert_eq!(
            paths.last().unwrap(),
            &json!({"path":"metewand.toml", "kind":"file", "action":"modify"})
        );
        assert!(
            paths[..paths.len() - 1]
                .iter()
                .all(|path| path["action"] == "create")
        );
        let directory = format!("{component}s/{name}");
        for file in expected_files {
            assert!(
                root.path().join(&directory).join(file).is_file(),
                "missing {directory}/{file}"
            );
        }
        let after = files(root.path());
        for (path, contents) in &before {
            if path != Path::new("metewand.toml") {
                assert_eq!(&after[path], contents, "modified unrelated {path:?}");
            }
        }
        let changed = after
            .iter()
            .filter(|(path, contents)| before.get(*path) != Some(*contents))
            .map(|(path, _)| path.clone())
            .collect::<BTreeSet<_>>();
        let reported_files = paths
            .iter()
            .filter(|entry| entry["kind"] == "file")
            .map(|entry| PathBuf::from(entry["path"].as_str().unwrap()))
            .collect::<BTreeSet<_>>();
        assert_eq!(changed, reported_files);
    }
    let source = fs::read_to_string(root.path().join("metewand.toml")).unwrap();
    assert!(source.starts_with(MANIFEST));
    let manifest = parse_manifest(Path::new("metewand.toml"), &source).unwrap();
    assert_eq!(manifest.datasets.len(), 1);
    assert_eq!(manifest.problems.len(), 1);
    assert_eq!(manifest.implementations.len(), 1);
    assert_eq!(manifest.environments.len(), 1);
    assert!(manifest.experiments.is_empty());
    assert!(manifest.execution_policies.is_empty());
    assert!(manifest.observation_policies.is_empty());
    let DatasetDefinition::Generated(dataset) = &manifest.datasets["sample.data"] else {
        panic!("expected materializer");
    };
    let WorkerDefinition::Command(worker) = &dataset.worker else {
        panic!("expected raw worker");
    };
    assert_eq!(worker.program, "sh");
    assert_eq!(worker.environment.as_str(), "local");
    assert_eq!(
        manifest.implementations["sample.impl"].problem_contracts[0].as_str(),
        "sample.problem"
    );
    let documents = files(root.path())
        .into_iter()
        .filter(|(path, _)| path.to_string_lossy().ends_with(".schema.json"))
        .map(|(path, contents)| (path, serde_json::from_slice(&contents).unwrap()))
        .collect::<Vec<_>>();
    let schemas = SchemaCatalog::try_new(documents.clone()).unwrap();
    for (path, _) in documents {
        for value in [json!({}), json!(null), json!([]), json!(1), json!("text")] {
            assert!(schemas.validate(&path, &value).is_err());
        }
    }
    let contract =
        fs::read_to_string(root.path().join("problems/sample.problem/contract.toml")).unwrap();
    assert!(
        metewand_core::problem_contract::parse_problem_contract_document(
            Path::new("contract.toml"),
            &contract
        )
        .is_err()
    );
    assert!(
        !root
            .path()
            .join("problems/sample.problem/reference")
            .exists()
    );
}

#[test]
fn dry_runs_are_deterministic_and_report_exactly_the_later_changes() {
    for component in ["dataset", "problem", "implementation"] {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
        if component == "implementation" {
            envelope(
                &scaffold(
                    root.path(),
                    "problem",
                    "existing",
                    &["--environment", "local"],
                ),
                true,
            );
        }
        let before = files(root.path());
        let mut options = vec!["--environment", "local"];
        if component == "implementation" {
            options.extend(["--problem", "existing"]);
        }
        let mut preview_options = options.clone();
        preview_options.push("--dry-run");
        let first = scaffold(root.path(), component, "new", &preview_options);
        let second = scaffold(root.path(), component, "new", &preview_options);
        assert_eq!(first.stdout, second.stdout);
        let preview = envelope(&first, true);
        assert_eq!(preview["data"]["dry_run"], true);
        assert_eq!(files(root.path()), before);
        for entry in preview["data"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["action"] == "create")
        {
            assert!(!root.path().join(entry["path"].as_str().unwrap()).exists());
        }
        let report = envelope(&scaffold(root.path(), component, "new", &options), true);
        assert_eq!(preview["data"]["paths"], report["data"]["paths"]);
        for entry in report["data"]["paths"].as_array().unwrap() {
            assert!(root.path().join(entry["path"].as_str().unwrap()).exists());
        }
    }
}

#[test]
fn supports_custom_manifests_and_inline_component_tables_without_rewriting_other_fields() {
    let root = TempDir::new().unwrap();
    let directory = root.path().join("nested");
    fs::create_dir(&directory).unwrap();
    let source = "# Author's header.\nversion=1\nname='benchmark'\ndatasets = {} # Dataset choices.\nenvironments = { local = { kind = 'local' } }\n";
    fs::write(directory.join("custom.toml"), source).unwrap();
    envelope(
        &scaffold(
            root.path(),
            "dataset",
            "with.dots",
            &["--environment", "local", "--manifest", "nested/custom.toml"],
        ),
        true,
    );
    assert!(
        directory
            .join("datasets/with.dots/materializer.sh")
            .exists()
    );
    assert!(!root.path().join("datasets").exists());
    let changed = fs::read_to_string(directory.join("custom.toml")).unwrap();
    assert!(changed.starts_with("# Author's header.\nversion=1\nname='benchmark'\n"));
    assert!(changed.contains("environments = { local = { kind = 'local' } }\n"));
    assert!(changed.contains("# Dataset choices."));
    let manifest = parse_manifest(Path::new("custom.toml"), &changed).unwrap();
    assert!(manifest.datasets.contains_key("with.dots"));
}

#[test]
fn leaves_existing_manifest_bytes_in_place_including_interleaved_dotted_keys() {
    for source in [
        include_str!("../../../fixtures/manifest/v1/complete.toml"),
        "version=1\nname='benchmark'\nenvironments.local.kind='local'\ndatasets.first.output_schema='schema.json'\ndatasets.second.output_schema='schema.json'\ndatasets.first.sources=['first.json']\ndatasets.second.sources=['second.json'] # Preserve order and comments.",
        "version=1\nname='benchmark'\nenvironments.local.kind='local'\n[datasets]\n",
    ] {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("metewand.toml"), source).unwrap();
        envelope(
            &scaffold(root.path(), "dataset", "new", &["--environment", "local"]),
            true,
        );
        let updated = fs::read_to_string(root.path().join("metewand.toml")).unwrap();
        assert!(
            updated.starts_with(source),
            "rewrote existing manifest bytes: {updated}"
        );
    }
}

#[test]
fn adds_each_component_to_nonempty_inline_tables_with_independent_namespaces() {
    let root = TempDir::new().unwrap();
    let source = "version=1\nname='benchmark'\ndatasets = {} # Preserve this comment.\nproblems = {}\nimplementations = {}\nenvironments = { local = { kind = 'local' } }\n";
    fs::write(root.path().join("metewand.toml"), source).unwrap();
    for component in ["dataset", "problem", "implementation"] {
        for name in ["first", "second.with.dots"] {
            let mut extra = vec!["--environment", "local"];
            if component == "implementation" {
                extra.extend(["--problem", "first"]);
            }
            envelope(&scaffold(root.path(), component, name, &extra), true);
        }
    }
    let changed = fs::read_to_string(root.path().join("metewand.toml")).unwrap();
    assert!(changed.starts_with("version=1\nname='benchmark'\ndatasets = {"));
    assert!(changed.ends_with("environments = { local = { kind = 'local' } }\n"));
    assert!(changed.contains("# Preserve this comment."));
    let manifest = parse_manifest(Path::new("metewand.toml"), &changed).unwrap();
    assert_eq!(manifest.datasets.len(), 2);
    assert_eq!(manifest.problems.len(), 2);
    assert_eq!(manifest.implementations.len(), 2);
    assert!(manifest.problems.contains_key("second.with.dots"));
}

#[test]
fn rejects_duplicate_components_and_every_existing_target_before_mutating() {
    for dry_run in [false, true] {
        for collision in [
            "datasets/new/dataset.schema.json",
            "datasets/new/materializer.sh",
        ] {
            let root = TempDir::new().unwrap();
            fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
            fs::create_dir_all(root.path().join("datasets/new")).unwrap();
            fs::write(root.path().join(collision), "preserve\n").unwrap();
            let before = files(root.path());
            let mut extra = vec!["--environment", "local"];
            if dry_run {
                extra.push("--dry-run");
            }
            let output = scaffold(root.path(), "dataset", "new", &extra);
            assert_eq!(output.status.code(), Some(74), "{output:?}");
            let diagnostic = envelope(&output, false);
            assert_eq!(diagnostic["data"]["code"], "scaffold_path_exists");
            assert_eq!(diagnostic["data"]["details"]["changed_paths"], json!([]));
            assert_eq!(files(root.path()), before);
        }
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
        envelope(
            &scaffold(root.path(), "dataset", "same", &["--environment", "local"]),
            true,
        );
        let before = files(root.path());
        let mut extra = vec!["--environment", "local"];
        if dry_run {
            extra.push("--dry-run");
        }
        let diagnostic = envelope(&scaffold(root.path(), "dataset", "same", &extra), false);
        assert_eq!(diagnostic["data"]["code"], "scaffold_component_exists");
        assert_eq!(files(root.path()), before);
    }
}

#[test]
fn rejects_invalid_or_incomplete_requests_without_writes() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
    let before = files(root.path());
    for arguments in [
        vec![],
        vec!["dataset"],
        vec!["experiment", "new"],
        vec!["dataset", "../escape", "--environment", "local"],
        vec!["dataset", "new"],
        vec!["dataset", "new", "--environment", "missing"],
        vec!["implementation", "new", "--environment", "local"],
        vec![
            "implementation",
            "new",
            "--environment",
            "local",
            "--problem",
            "missing",
        ],
        vec![
            "dataset",
            "new",
            "--environment",
            "local",
            "--problem",
            "other",
        ],
        vec![
            "dataset",
            "new",
            "--environment",
            "local",
            "--dry-run",
            "--dry-run",
        ],
        vec![
            "dataset",
            "new",
            "--environment",
            "local",
            "--environment",
            "local",
        ],
        vec!["dataset", "new", "--template", "lasso"],
    ] {
        let output = command(root.path())
            .args(["--output", "json", "scaffold"])
            .args(arguments)
            .output()
            .unwrap();
        envelope(&output, false);
        assert_eq!(files(root.path()), before);
    }
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_manifests_and_generated_paths_including_dangling_links() {
    use std::os::unix::fs::symlink;
    for target in [
        "metewand.toml",
        "datasets",
        "datasets/new",
        "datasets/new/materializer.sh",
        "datasets/new/parameters.schema.json",
    ] {
        for dangling in [false, true] {
            let root = TempDir::new().unwrap();
            let outside = TempDir::new().unwrap();
            fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
            let path = root.path().join(target);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if target == "metewand.toml" {
                fs::remove_file(&path).unwrap();
            }
            let destination = outside.path().join("target");
            if !dangling {
                if target.ends_with(".toml") || target.ends_with(".sh") || target.ends_with(".json")
                {
                    fs::write(&destination, MANIFEST).unwrap();
                } else {
                    fs::create_dir(&destination).unwrap();
                }
            }
            symlink(&destination, &path).unwrap();
            let outside_before = files(outside.path());
            for extra in [
                vec!["--environment", "local"],
                vec!["--environment", "local", "--dry-run"],
            ] {
                envelope(&scaffold(root.path(), "dataset", "new", &extra), false);
            }
            assert!(path.is_symlink());
            assert_eq!(files(outside.path()), outside_before);
            if target != "metewand.toml" {
                assert_eq!(
                    fs::read_to_string(root.path().join("metewand.toml")).unwrap(),
                    MANIFEST
                );
            }
        }
    }
}

#[test]
fn concurrent_component_additions_preserve_both_successful_updates() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
    let children = (0..6)
        .map(|index| {
            command(root.path())
                .args([
                    "--output",
                    "jsonl",
                    "scaffold",
                    "dataset",
                    &format!("data{index}"),
                    "--environment",
                    "local",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let mut successful = Vec::new();
    for child in children {
        let output = child.wait_with_output().unwrap();
        let report = envelope(&output, output.status.success());
        if output.status.success() {
            successful.push(report["data"]["name"].as_str().unwrap().to_owned());
        }
    }
    assert!(!successful.is_empty());
    let source = fs::read_to_string(root.path().join("metewand.toml")).unwrap();
    let manifest = parse_manifest(Path::new("metewand.toml"), &source).unwrap();
    assert_eq!(manifest.datasets.len(), successful.len());
    for name in successful {
        assert!(manifest.datasets.contains_key(name.as_str()));
    }
}

#[test]
fn compact_and_human_reports_list_the_same_paths_in_execution_order() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
    let arguments = [
        "scaffold",
        "dataset",
        "new",
        "--environment",
        "local",
        "--dry-run",
    ];
    let compact = command(root.path())
        .args(["--output", "jsonl"])
        .args(arguments)
        .output()
        .unwrap();
    envelope(&compact, true);
    assert_eq!(
        String::from_utf8(compact.stdout).unwrap(),
        concat!(
            "{\"schema_version\":1,\"kind\":\"result\",\"data\":{\"command\":\"scaffold\",\"component\":\"dataset\",\"name\":\"new\",\"dry_run\":true,\"manifest\":\"metewand.toml\",\"paths\":[",
            "{\"path\":\"datasets\",\"kind\":\"directory\",\"action\":\"create\"},",
            "{\"path\":\"datasets/new\",\"kind\":\"directory\",\"action\":\"create\"},",
            "{\"path\":\"datasets/new/parameters.schema.json\",\"kind\":\"file\",\"action\":\"create\"},",
            "{\"path\":\"datasets/new/dataset.schema.json\",\"kind\":\"file\",\"action\":\"create\"},",
            "{\"path\":\"datasets/new/materializer.sh\",\"kind\":\"file\",\"action\":\"create\"},",
            "{\"path\":\"metewand.toml\",\"kind\":\"file\",\"action\":\"modify\"}]}}\n"
        )
    );
    let human = command(root.path()).args(arguments).output().unwrap();
    assert!(human.status.success(), "{human:?}");
    assert_eq!(
        String::from_utf8(human.stdout).unwrap(),
        concat!(
            "Would create directory: datasets\n",
            "Would create directory: datasets/new\n",
            "Would create file: datasets/new/parameters.schema.json\n",
            "Would create file: datasets/new/dataset.schema.json\n",
            "Would create file: datasets/new/materializer.sh\n",
            "Would modify file: metewand.toml\n"
        )
    );
    let actual = command(root.path())
        .args(&arguments[..arguments.len() - 1])
        .output()
        .unwrap();
    assert!(actual.status.success(), "{actual:?}");
    assert_eq!(
        String::from_utf8(actual.stdout).unwrap(),
        concat!(
            "Created directory: datasets\n",
            "Created directory: datasets/new\n",
            "Created file: datasets/new/parameters.schema.json\n",
            "Created file: datasets/new/dataset.schema.json\n",
            "Created file: datasets/new/materializer.sh\n",
            "Modified file: metewand.toml\n"
        )
    );
}

#[test]
fn a_busy_manifest_fails_without_changing_any_path() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("metewand.toml"), MANIFEST).unwrap();
    let file = fs::File::open(root.path().join("metewand.toml")).unwrap();
    file.try_lock().unwrap();
    for extra in [
        vec!["--environment", "local"],
        vec!["--environment", "local", "--dry-run"],
    ] {
        let output = scaffold(root.path(), "dataset", "new", &extra);
        assert_eq!(output.status.code(), Some(74));
        let diagnostic = envelope(&output, false);
        assert_eq!(diagnostic["data"]["code"], "scaffold_manifest_busy");
        assert_eq!(diagnostic["data"]["details"]["changed_paths"], json!([]));
    }
    assert_eq!(
        fs::read_to_string(root.path().join("metewand.toml")).unwrap(),
        MANIFEST
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn validates_the_existing_manifest_and_reports_its_source_location() {
    let root = TempDir::new().unwrap();
    fs::write(
        root.path().join("metewand.toml"),
        "version = 99\nname = 'benchmark'\n",
    )
    .unwrap();
    let before = files(root.path());
    let output = scaffold(root.path(), "dataset", "new", &["--environment", "local"]);
    assert_eq!(output.status.code(), Some(3));
    let diagnostic = envelope(&output, false);
    assert_eq!(diagnostic["data"]["code"], "scaffold_invalid_manifest");
    assert_eq!(diagnostic["data"]["source_span"]["path"], "metewand.toml");
    assert_eq!(diagnostic["data"]["source_span"]["line"], 1);
    assert_eq!(files(root.path()), before);
}
