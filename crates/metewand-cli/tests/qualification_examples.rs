#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;

#[allow(dead_code)]
#[path = "../../../examples/overhead/main.rs"]
mod overhead;

#[test]
fn overhead_implementation_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        overhead::serve("implementation").unwrap();
    }
}

#[test]
fn overhead_evaluator_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        overhead::serve("problem_evaluator").unwrap();
    }
}

#[test]
fn adopter_evaluators_check_analytic_references_and_malformed_results() {
    let output = Command::new("python3")
        .args(["-B", "-m", "unittest", "discover", "-s"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/adopters"))
        .args(["-p", "test_science.py"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn overhead_worker_keeps_primary_time_and_local_diagnostics_in_all_scopes() {
    let root = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/overhead");
    fs::create_dir(root.path().join("reference")).unwrap();
    for file in [
        "problem.yaml",
        "reference/result.json",
        "reference/metrics.json",
    ] {
        fs::copy(source.join(file), root.path().join(file)).unwrap();
    }
    let manifest = fs::read_to_string(source.join("metewand.yaml"))
        .unwrap()
        .lines()
        .filter(|line| line.trim() != "sources: [worker]")
        .collect::<Vec<_>>()
        .join("\n")
        .replace(
            "program: ./worker",
            &format!(
                "program: {}",
                serde_json::to_string(&std::env::current_exe().unwrap()).unwrap()
            ),
        )
        .replace(
            "args: [implementation]",
            "args: [--exact, overhead_implementation_entry, --nocapture]",
        )
        .replace(
            "args: [problem_evaluator]",
            "args: [--exact, overhead_evaluator_entry, --nocapture]",
        )
        .replace("measurement-repetitions: 11", "measurement-repetitions: 1")
        .replace("grid: [0, 1000000, 10000000]", "grid: [1000000]")
        .replace("grid: [0, 1000000]", "grid: [4096]");
    fs::write(root.path().join("metewand.yaml"), manifest).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root.path())
        .args(["run", "--output-dir", "run", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempts = report["data"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 3);
    let mut scopes = Vec::new();
    for attempt in attempts {
        let document: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("run")
                    .join(attempt["observation_directory"].as_str().unwrap())
                    .join("observation.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let statistics = &document["provenance"]["data"]["implementation_statistics"];
        let work = statistics["work_time_ns"].as_u64().unwrap();
        let encoding = statistics["result_encoding_time_ns"].as_u64().unwrap();
        let writing = statistics["result_write_time_ns"].as_u64().unwrap();
        let total = statistics["worker_execute_time_ns"].as_u64().unwrap();
        assert!(work >= 1000000);
        assert!(encoding > 0 && writing > 0);
        assert!(total >= work + encoding + writing);
        assert!(document["timing"]["timed_wall_time_ns"].as_u64().unwrap() >= total);
        assert_eq!(document["timing"]["implementation_time_ns"], work);
        scopes.push(
            document["provenance"]["data"]["context"]["timing_scope"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    scopes.sort();
    assert_eq!(
        scopes,
        ["cold_end_to_end", "execute_only", "prepare_and_execute"]
    );
}
