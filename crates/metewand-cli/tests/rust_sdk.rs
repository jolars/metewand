#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

#[allow(dead_code)]
#[path = "../../../examples/rust-sdk/main.rs"]
mod worker;

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

#[test]
fn native_helpers_publish_results_and_scientific_failures_through_cli() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let benchmark = TempDir::new().unwrap();
    fs::create_dir(benchmark.path().join("reference")).unwrap();
    for file in [
        "data.json",
        "reference/result.json",
        "reference/metrics.json",
    ] {
        fs::copy(
            root.join("examples/rust-sdk").join(file),
            benchmark.path().join(file),
        )
        .unwrap();
    }
    let manifest = fs::read_to_string(root.join("examples/rust-sdk/metewand.yaml"))
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
            "args: [--exact, native_implementation_entry, --nocapture]",
        )
        .replace(
            "args: [problem_evaluator]",
            "args: [--exact, native_evaluator_entry, --nocapture]",
        );
    let path = benchmark.path().join("metewand.yaml");
    fs::write(&path, &manifest).unwrap();
    for (bias, expected) in [(0, "accepted"), (1, "failed")] {
        fs::write(
            &path,
            manifest.replace(
                "bias: { type: integer, default: 0 }",
                &format!("bias: {{ type: integer, default: {bias} }}"),
            ),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(benchmark.path())
            .args([
                "run",
                "--output-dir",
                &format!("run-{bias}"),
                "--output",
                "json",
            ])
            .output()
            .unwrap();
        let report: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
        assert_eq!(output.status.success(), bias == 0, "{report}");
        let attempts = report["data"]["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2);
        for attempt in attempts {
            assert_eq!(attempt["status"], expected, "{attempt}");
            assert_eq!(attempt["result"], serde_json::json!({"answer":8+bias}));
            assert_eq!(
                attempt["metrics"],
                serde_json::json!({"absolute_error":bias})
            );
            let record: Value = serde_json::from_slice(
                &fs::read(
                    benchmark
                        .path()
                        .join(format!("run-{bias}"))
                        .join(attempt["attempt_directory"].as_str().unwrap())
                        .join("attempt.json"),
                )
                .unwrap(),
            )
            .unwrap();
            for role in ["implementation", "evaluator"] {
                assert_eq!(
                    record["provenance"]["data"]["execution"][role]["sdk"]["name"],
                    "metewand-rust"
                );
            }
        }
    }
}
