#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;

use crate::native::rust_worker as worker;

#[test]
fn native_implementation_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        worker::serve("implementation").unwrap();
    }
}

#[test]
fn native_evaluator_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        worker::serve("problem_evaluator").unwrap();
    }
}

fn invoke(runner: &Path, root: &Path, args: &[&str]) -> Value {
    let output = Command::new(runner)
        .current_dir(root)
        .args(args)
        .args(["--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
}

#[test]
fn durable_records_and_both_exports_keep_build_and_host_evidence_separate() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let root = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .current_dir(root.path())
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        root.path().join(".gitignore"),
        "build/\nrun/\njsonl/\ncsv/\n",
    )
    .unwrap();
    fs::create_dir(root.path().join("reference")).unwrap();
    for file in [
        "data.json",
        "reference/result.json",
        "reference/metrics.json",
    ] {
        fs::copy(
            repository.join("examples/rust-worker").join(file),
            root.path().join(file),
        )
        .unwrap();
    }
    let manifest = fs::read_to_string(repository.join("examples/rust-worker/metewand.yaml"))
        .unwrap()
        .replace("program: ./worker", "program: ./build/worker")
        .replace("sources: [worker]", "sources: [build/worker]")
        .replace(
            "args: [implementation]",
            "args: [--exact, cli_tests::build_provenance::native_implementation_entry, --nocapture]",
        )
        .replace(
            "args: [problem_evaluator]",
            "args: [--exact, cli_tests::build_provenance::native_evaluator_entry, --nocapture]",
        );
    fs::write(root.path().join("metewand.yaml"), manifest).unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    for (name, source) in [
        (
            "runner",
            Path::new(env!("CARGO_BIN_EXE_metewand")).to_path_buf(),
        ),
        ("worker", std::env::current_exe().unwrap()),
    ] {
        let output = Command::new("python3")
            .current_dir(root.path())
            .arg(repository.join("tools/build_evidence.py"))
            .args([
                "--executable",
                &format!("build/{name}"),
                "--source",
                "adapter=.",
                "--compiler",
                "rustc",
                "--profile",
                "test",
                "--",
                "python3",
                "-c",
                "import shutil,sys; shutil.copy2(sys.argv[1],sys.argv[2])",
            ])
            .arg(source)
            .arg(format!("build/{name}"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let runner = root.path().join("build/runner");
    let report = invoke(&runner, root.path(), &["run", "--output-dir", "run"]);
    let attempt = &report["attempts"][0];
    let document: Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("run")
                .join(attempt["attempt_directory"].as_str().unwrap())
                .join("attempt.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let context = &document["provenance"]["data"]["context"];
    for executable in [
        &context["runner"],
        &context["implementation"]["executable"],
        &context["evaluator"]["executable"],
    ] {
        assert_eq!(executable["build"]["status"], "available");
        assert_eq!(
            executable["executable_sha256"],
            executable["build"]["evidence"]["executable_sha256"]
        );
        assert_eq!(executable["build"]["evidence"]["build"]["profile"], "test");
    }
    assert_eq!(context["host_observation"]["phase"], "run_preflight");
    assert!(context["host_observation"].get("compiler").is_none());
    let observation_id = attempt["observation_id"].as_str().unwrap();
    invoke(
        &runner,
        root.path(),
        &["explain", observation_id, "--output-dir", "run"],
    );
    let original = Path::new(env!("CARGO_BIN_EXE_metewand"));
    // Export must preserve the recorded runner even when its binary is gone.
    fs::remove_dir_all(root.path().join("build")).unwrap();
    fs::remove_file(root.path().join("metewand.yaml")).unwrap();
    for format in ["jsonl", "csv"] {
        invoke(
            original,
            root.path(),
            &[
                "export",
                "--output-dir",
                "run",
                "--format",
                format,
                "--destination",
                format,
            ],
        );
    }
    let rows: Vec<Value> = fs::read_to_string(root.path().join("jsonl/results.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let csv = Command::new("python3").args(["-c", "import csv,json,sys; print(json.dumps(list(csv.DictReader(open(sys.argv[1],newline='')))))"]).arg(root.path().join("csv/results.csv")).output().unwrap();
    assert!(csv.status.success(), "{csv:?}");
    let csv: Vec<Value> = serde_json::from_slice(&csv.stdout).unwrap();
    for (row, csv) in rows
        .iter()
        .zip(&csv)
        .filter(|(r, _)| r["record_type"] == "observation" || r["record_type"] == "attempt")
    {
        for field in ["runner", "implementation", "evaluator", "host_observation"] {
            assert_eq!(row["provenance"]["data"]["context"][field], context[field]);
        }
        let csv_provenance: Value =
            serde_json::from_str(csv["provenance"].as_str().unwrap()).unwrap();
        assert_eq!(csv_provenance, row["provenance"]);
        assert_ne!(row["logical_run_id"], row["resolved_run_id"]);
        assert_eq!(row["compatibility"]["environment_class"], "development");
        assert_eq!(row["compatibility"]["timing_scope"], "prepare_and_execute");
    }
    assert_eq!(rows.len(), csv.len());
}
