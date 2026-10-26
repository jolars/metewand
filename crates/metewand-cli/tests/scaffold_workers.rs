#![cfg(unix)]

use std::{fs, path::Path, process::Command, str::FromStr as _, time::Duration};

use metewand_protocol::{Capability, WorkerIdentity, WorkerRole};
use metewand_runtime::{
    worker_process::{PosixWorkerProcess, WorkerLogLimits},
    worker_session::{PhaseTimeouts, WorkerSession},
};
use serde_json::json;
use tempfile::TempDir;

#[path = "../../metewand-runtime/src/authoring/templates/evaluator.rs"]
mod evaluator;
#[path = "../../metewand-runtime/src/authoring/templates/materializer.rs"]
mod materializer;
#[path = "../../metewand-runtime/src/authoring/templates/worker.rs"]
mod worker;

#[test]
fn native_implementation_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        worker::main().unwrap();
    }
}

#[test]
fn native_materializer_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        materializer::main().unwrap();
    }
}

#[test]
fn native_evaluator_entry() {
    if std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some() {
        evaluator::main().unwrap();
    }
}

#[test]
fn generated_adapters_negotiate_and_report_unfinished_science() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let r_package = TempDir::new().unwrap();
    copy_tree(&repository.join("sdk/r"), &r_package.path().join("package"));
    let r_library = r_package.path().join("library");
    fs::create_dir(&r_library).unwrap();
    let output = Command::new("R")
        .args(["CMD", "INSTALL", "--preclean", "--clean"])
        .arg(format!("--library={}", r_library.display()))
        .arg(r_package.path().join("package"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    for (language, extension) in [("python", "py"), ("r", "R"), ("rust", "rs")] {
        let benchmark = TempDir::new().unwrap();
        fs::write(
            benchmark.path().join("metewand.yaml"),
            "version: 1\nname: scaffold\n",
        )
        .unwrap();
        fs::write(benchmark.path().join("result.json"), br#"{"data":{}}"#).unwrap();
        for (component, basename, role, entry) in [
            (
                "dataset",
                "materializer",
                WorkerRole::DatasetMaterializer,
                "native_materializer_entry",
            ),
            (
                "problem",
                "evaluator",
                WorkerRole::ProblemEvaluator,
                "native_evaluator_entry",
            ),
            (
                "implementation",
                "worker",
                WorkerRole::Implementation,
                "native_implementation_entry",
            ),
        ] {
            let mut scaffold = Command::new(env!("CARGO_BIN_EXE_metewand"));
            scaffold.current_dir(benchmark.path()).args([
                "scaffold",
                component,
                "sample",
                "--language",
                language,
            ]);
            if component == "implementation" {
                scaffold.args(["--problem", "sample"]);
            }
            let output = scaffold.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let source = benchmark
                .path()
                .join(format!("{component}s/sample/{basename}.{extension}"));
            let mut command = match language {
                "python" => {
                    let mut command = Command::new("python3");
                    command
                        .arg(&source)
                        .env("PYTHONPATH", repository.join("sdk/python/src"))
                        .env("PYTHONDONTWRITEBYTECODE", "1");
                    command
                }
                "r" => {
                    let mut command = Command::new("Rscript");
                    command.arg(&source).env("R_LIBS_USER", &r_library);
                    command
                }
                "rust" => {
                    let template = repository.join(format!(
                        "crates/metewand-runtime/src/authoring/templates/{basename}.rs"
                    ));
                    assert_eq!(fs::read(&source).unwrap(), fs::read(template).unwrap());
                    let mut command = Command::new(std::env::current_exe().unwrap());
                    command.args(["--exact", entry, "--nocapture"]);
                    command
                }
                _ => unreachable!(),
            };
            command.current_dir(benchmark.path());
            let process =
                PosixWorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
            let limit = Duration::from_secs(10);
            let required = match role {
                WorkerRole::Implementation => &[Capability::OneShot][..],
                WorkerRole::ProblemEvaluator => &[Capability::ProblemSemantics][..],
                WorkerRole::DatasetMaterializer => &[],
            };
            let mut session = WorkerSession::connect(
                process,
                role,
                WorkerIdentity::from_str(&format!("mw1-resolved-worker-{}", "a".repeat(64)))
                    .unwrap(),
                required,
                PhaseTimeouts {
                    hello: limit,
                    materialize: limit,
                    prepare: limit,
                    execute: limit,
                    reset: limit,
                    evaluate: limit,
                    shutdown: limit,
                },
            )
            .unwrap();
            assert_eq!(
                session.negotiated().sdk().unwrap().name(),
                format!("metewand-{language}")
            );
            let directory = benchmark.path().to_str().unwrap();
            let error = match role {
                WorkerRole::DatasetMaterializer => session
                    .materialize(directory, json!({}), 7, directory)
                    .unwrap_err(),
                WorkerRole::Implementation => session
                    .prepare(directory, "contract", json!({}), json!({}), 7)
                    .unwrap_err(),
                WorkerRole::ProblemEvaluator => session
                    .evaluate(
                        directory,
                        "contract",
                        json!({}),
                        json!({}),
                        directory,
                        benchmark.path().join("metrics.json").to_str().unwrap(),
                    )
                    .unwrap_err(),
            };
            assert!(error.to_string().contains("TODO:"), "{language}: {error}");
            assert!(session.shutdown().unwrap().status.success());
            assert!(!benchmark.path().join("metrics.json").exists());
        }
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
