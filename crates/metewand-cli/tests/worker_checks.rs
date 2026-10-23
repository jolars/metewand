#![cfg(unix)]

mod support;

use serde_json::Value;
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn repository() -> TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    copy_tree(&root.join("examples/python-sdk"), benchmark.path());
    copy_tree(
        &root.join("sdk/python/src/metewand"),
        &benchmark.path().join("metewand"),
    );
    benchmark
}

fn check(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args([
            "check",
            "--manifest",
            "metewand.yaml",
            "--workers",
            "--output",
            "json",
        ])
        .output()
        .unwrap()
}

#[test]
fn verifies_workers_and_executes_reference_cases_without_creating_a_run() {
    let repository = repository();
    let output = check(repository.path());
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let catalog = metewand_core::public_schemas::public_schema_catalog().unwrap();
    catalog
        .validate(
            metewand_core::public_schemas::PublicSchema::MachineOutput.repository_path(),
            &report,
        )
        .unwrap();
    assert_eq!(report["data"]["worker_launches"], 3);
    assert_eq!(report["data"]["workers"][0]["evidence"], "verified_now");
    assert_eq!(
        report["data"]["reference_cases"][0]["metrics"],
        serde_json::json!({"absolute_error": 0})
    );
    assert!(!repository.path().join("run").exists());
    let repeated = check(repository.path());
    assert!(repeated.status.success(), "{repeated:?}");
    assert_eq!(output.stdout, repeated.stdout);
}

#[test]
fn rejects_reference_metric_disagreement() {
    let repository = repository();
    fs::write(
        repository.path().join("reference/metrics.json"),
        "{\"absolute_error\":1}",
    )
    .unwrap();
    let output = check(repository.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("reference metrics disagree"),
        "{output:?}"
    );
}

#[test]
fn rejects_an_unobserved_declared_capability() {
    let repository = repository();
    // The raw worker makes its hello advertisement explicit for this test.
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset/worker.py");
    let worker = fs::read_to_string(raw).unwrap().replace(
        "capabilities=[\"one_shot\"] if role == \"implementation\" else [],",
        "capabilities=[],",
    );
    assert!(worker.contains("capabilities=[],"));
    fs::write(repository.path().join("worker.py"), worker).unwrap();
    let output = check(repository.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("OneShot"),
        "{output:?}"
    );
}

#[test]
fn rejects_invalid_reference_results_and_evaluator_input_mutation() {
    for mutation in [false, true] {
        let repository = repository();
        if mutation {
            let path = repository.path().join("worker.py");
            let worker = fs::read_to_string(&path).unwrap().replace(
                "\n    values = read_json(dataset_instance / \"data.json\")[\"values\"]",
                "\n    values = read_json(dataset_instance / \"data.json\")[\"values\"]\n    write_json(result_dir / \"extra.json\", {})",
            );
            fs::write(path, worker).unwrap();
        } else {
            fs::write(
                repository.path().join("reference/result.json"),
                "{\"answer\":\"wrong type\"}",
            )
            .unwrap();
        }
        let output = check(repository.path());
        assert!(!output.status.success(), "{output:?}");
        if mutation {
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("mutated reference inputs"),
                "{output:?}"
            );
        }
    }
}

#[test]
fn accepts_enveloped_reference_results_and_metrics() {
    let repository = repository();
    for (file, schema, data) in [
        ("result", "result", serde_json::json!({"answer": 8})),
        (
            "metrics",
            "metrics",
            serde_json::json!({"absolute_error": 0}),
        ),
    ] {
        let mut value = serde_json::json!({"schema_version": 1, "schema": format!("schemas/{schema}.json"), "data": data});
        if file == "result" {
            value["files"] = serde_json::json!([]);
        }
        let directory = repository.path().join(format!("reference/{file}"));
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join(format!("{file}.json")),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let contract = repository.path().join("problem.yaml");
        let text = fs::read_to_string(&contract).unwrap().replace(
            &format!("reference/{file}.json"),
            &format!("reference/{file}"),
        );
        fs::write(contract, text).unwrap();
    }
    let output = check(repository.path());
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn checks_dataset_free_problem_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repository = TempDir::new().unwrap();
    copy_tree(&root.join("examples/fixed-dataset"), repository.path());
    let manifest_path = repository.path().join("metewand.yaml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let manifest = support::replace_yaml(
        &manifest,
        "datasets:\n  numbers:\n    output_schema: \"schemas/dataset.json\"\n    sources: [\"data.json\"]\n",
        "",
    )
    .replace("    datasets: [\"numbers\"]\n", "");
    fs::write(manifest_path, manifest).unwrap();
    let contract_path = repository.path().join("problem.yaml");
    let contract = fs::read_to_string(&contract_path)
        .unwrap()
        .replace(
            "dataset_schemas: [\"schemas/dataset.json\"]",
            "dataset_schemas: []",
        )
        .replace("  - dataset: \"data.json\"\n    result:", "  - result:");
    fs::write(contract_path, contract).unwrap();
    let worker_path = repository.path().join("worker.py");
    let worker = fs::read_to_string(&worker_path).unwrap().replace(
        "values = read_json(Path(request[\"dataset_dir\"]) / \"data.json\")[\"values\"]",
        "values = [1, 2, 3]",
    );
    fs::write(worker_path, worker).unwrap();
    let output = check(repository.path());
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn accepts_existing_dataset_inventory_and_checks_each_case() {
    let repository = repository();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/problem-contract/v1/suites/sum/fixtures/mixed/dataset");
    copy_tree(&source, &repository.path().join("reference/dataset"));
    let path = repository.path().join("problem.yaml");
    let contract = fs::read_to_string(&path)
        .unwrap()
        .replace("dataset: \"data.json\"", "dataset: \"reference/dataset\"");
    let case = &contract[contract.find("  - dataset:").unwrap()..];
    fs::write(path, format!("{contract}\n{case}")).unwrap();
    let output = check(repository.path());
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["data"]["reference_cases"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn json_lines_and_human_output_include_worker_evidence() {
    let repository = repository();
    for format in ["human", "jsonl"] {
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(repository.path())
            .args(["check", "--workers", "--output", format])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("verified_now"));
        if format == "jsonl" {
            assert_eq!(text.lines().count(), 1);
            let _: Value = serde_json::from_str(&text).unwrap();
        } else {
            assert!(text.contains("reference case: sum[0]"));
        }
    }
}

#[test]
fn records_additional_capabilities_without_selecting_or_executing_them() {
    let repository = repository();
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset/worker.py");
    let worker = fs::read_to_string(raw).unwrap()
        .replace("capabilities=[\"one_shot\"] if role == \"implementation\" else [],", "capabilities=[\"one_shot\", \"fresh_sequence\"] if role == \"implementation\" else [],")
        .replace("assert role == \"implementation\" and prepared is not None", "raise AssertionError(\"check must not execute implementations\")");
    fs::write(repository.path().join("worker.py"), worker).unwrap();
    let output = check(repository.path());
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let implementation = report["data"]["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["role"] == "implementation")
        .unwrap();
    assert_eq!(
        implementation["selected_capabilities"],
        serde_json::json!(["one_shot"])
    );
    assert_eq!(
        implementation["reported_capabilities"],
        serde_json::json!(["one_shot", "fresh_sequence"])
    );
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "__pycache__" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
