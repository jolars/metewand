use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
};

use crate::core::public_schemas::{PublicSchema, public_schema_catalog};
use serde_json::Value;
use tempfile::{Builder, TempDir};

use crate::native::buffered_worker as buffered;
use crate::native::rust_worker as native;

fn inherited_protocol() -> bool {
    std::env::var_os("METEWAND_PROTOCOL_READ_FD").is_some()
        || std::env::var_os("METEWAND_PROTOCOL_READ_HANDLE").is_some()
}

#[test]
fn portable_native_implementation_entry() {
    if inherited_protocol() {
        native::serve("implementation").unwrap();
    }
}

#[test]
fn portable_native_evaluator_entry() {
    if inherited_protocol() {
        native::serve("problem_evaluator").unwrap();
    }
}

#[test]
fn portable_native_buffered_entry() {
    if inherited_protocol() {
        buffered::serve().unwrap();
    }
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn copy(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn stage(example: &str) -> TempDir {
    let directory = Builder::new()
        .prefix("metewand portable é ")
        .tempdir()
        .unwrap();
    copy(
        &repository().join("examples").join(example),
        directory.path(),
    );
    directory
}

fn python_package(directory: &Path) {
    copy(
        &repository().join("packages/python/src/metewand"),
        &directory.join("metewand"),
    );
}

fn r_package(directory: &Path) {
    static LIBRARY: OnceLock<TempDir> = OnceLock::new();
    let installed = LIBRARY.get_or_init(|| {
        let temporary = Builder::new()
            .prefix("metewand-r-installed-")
            .tempdir()
            .unwrap();
        copy(
            &repository().join("packages/r"),
            &temporary.path().join("package"),
        );
        let library = temporary.path().join("library");
        fs::create_dir(&library).unwrap();
        let output = Command::new("R")
            .args(["CMD", "INSTALL", "--preclean", "--clean"])
            .arg(format!("--library={}", library.display()))
            .arg(temporary.path().join("package"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let jsonlite = Command::new("Rscript")
            .args(["--vanilla", "-e", "cat(find.package('jsonlite'))"])
            .output()
            .unwrap();
        assert!(jsonlite.status.success(), "{jsonlite:?}");
        copy(
            Path::new(std::str::from_utf8(&jsonlite.stdout).unwrap().trim()),
            &library.join("jsonlite"),
        );
        temporary
    });
    copy(
        &installed.path().join("library"),
        &directory.join("library"),
    );
}

fn edit(directory: &Path, file: &str, update: impl FnOnce(String) -> String) {
    let file = directory.join(file);
    fs::write(&file, update(fs::read_to_string(&file).unwrap())).unwrap();
}

fn invoke(directory: &Path, arguments: &[&str]) -> Output {
    invoke_with_path(directory, arguments, None)
}

fn invoke_with_path(directory: &Path, arguments: &[&str], path: Option<&OsStr>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metewand"));
    command
        .current_dir(directory)
        .args(arguments)
        .args(["--output", "json"]);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().unwrap()
}

fn decode(arguments: &[&str], output: Output, exit: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{arguments:?}: {output:?}"
    );
    let envelope: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {output:?}"));
    let schema = match arguments[0] {
        "run" => PublicSchema::RunOutput,
        "status" | "explain" => PublicSchema::InspectionOutput,
        "export" => PublicSchema::ExportOutput,
        "cache" => PublicSchema::CacheOutput,
        _ => PublicSchema::MachineOutput,
    };
    public_schema_catalog()
        .unwrap()
        .validate(schema.repository_path(), &envelope)
        .unwrap_or_else(|error| panic!("{error}: {envelope}"));
    envelope["data"].clone()
}

fn successful(directory: &Path, arguments: &[&str]) -> Value {
    decode(arguments, invoke(directory, arguments), 0)
}

fn assert_accepted(report: &Value, expected: usize) {
    let attempts = report["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), expected, "{report}");
    for attempt in attempts {
        assert_eq!(attempt["status"], "accepted", "{attempt}");
    }
}

fn assert_logical_inventory_paths(directory: &Path) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            assert_logical_inventory_paths(&entry.path());
        } else if matches!(
            entry.file_name().to_str(),
            Some("result.json" | "dataset-manifest.json" | "attempt.json" | "observation.json")
        ) {
            let document: Value = serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
            if let Some(files) = document["files"].as_array() {
                for file in files {
                    let path = file["path"].as_str().unwrap();
                    assert!(
                        !path.contains('\\') && Path::new(path).is_relative(),
                        "{path}"
                    );
                }
            }
        }
    }
}

fn inspect_and_export(directory: &Path) {
    assert_logical_inventory_paths(&directory.join("run"));
    let status = successful(directory, &["status", "--output-dir", "run"]);
    assert!(
        !status["attempts"].as_array().unwrap().is_empty(),
        "{status}"
    );
    for (format, destination, file) in [
        ("csv", "tables", "results.csv"),
        ("jsonl", "records", "results.jsonl"),
    ] {
        successful(
            directory,
            &[
                "export",
                "--output-dir",
                "run",
                "--format",
                format,
                "--destination",
                destination,
            ],
        );
        assert!(directory.join(destination).join(file).is_file());
    }
}

#[test]
fn python_generated_datasets_cache_resume_and_exports_are_portable() {
    let directory = stage("python-package");
    python_package(directory.path());
    successful(directory.path(), &["check", "--workers"]);
    successful(directory.path(), &["plan"]);
    let first = successful(
        directory.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--cache-dir",
            "cache",
            "--max-attempts",
            "1",
        ],
    );
    assert_accepted(&first, 1);
    let resumed = successful(
        directory.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--cache-dir",
            "cache",
            "--resume",
        ],
    );
    assert_accepted(&resumed, 1);
    assert_ne!(
        first["attempts"][0]["attempt_id"],
        resumed["attempts"][0]["attempt_id"]
    );
    let complete = successful(
        directory.path(),
        &[
            "run",
            "--output-dir",
            "run",
            "--cache-dir",
            "cache",
            "--resume",
        ],
    );
    assert_accepted(&complete, 0);
    successful(
        directory.path(),
        &["cache", "roots", "--cache-dir", "cache"],
    );
    successful(
        directory.path(),
        &["cache", "gc", "--cache-dir", "cache", "--dry-run"],
    );
    inspect_and_export(directory.path());
}

#[test]
fn r_generated_dataset_and_fresh_sequences_are_portable() {
    let one_shot = stage("r-package");
    r_package(one_shot.path());
    edit(one_shot.path(), "metewand.yaml", |source| {
        source.replace("library/metewand/**", "library/**")
    });
    successful(one_shot.path(), &["check", "--workers"]);
    assert_accepted(
        &successful(one_shot.path(), &["run", "--output-dir", "run"]),
        2,
    );
    inspect_and_export(one_shot.path());

    let fresh = stage("fresh-sequence");
    python_package(fresh.path());
    r_package(fresh.path());
    successful(fresh.path(), &["check", "--workers"]);
    let partial = successful(
        fresh.path(),
        &["run", "--output-dir", "run", "--max-attempts", "1"],
    );
    assert_accepted(&partial, 1);
    let finished = successful(fresh.path(), &["run", "--output-dir", "run", "--resume"]);
    assert!(
        finished["fresh_sequences"]
            .as_array()
            .unwrap()
            .iter()
            .all(|sequence| sequence["status"] == "accepted"),
        "{finished}"
    );
    inspect_and_export(fresh.path());
}

#[test]
fn python_and_rust_buffered_profiles_are_portable() {
    for native in [false, true] {
        let directory = stage("buffered-profile");
        python_package(directory.path());
        if native {
            let executable = serde_json::to_string(&std::env::current_exe().unwrap()).unwrap();
            edit(directory.path(), "metewand.yaml", |source| {
                let worker = "runner: python\n    protocol-transport: socket\n    entrypoint: worker.py\n    sources: [worker.py, \"metewand/*.py\"]";
                assert_eq!(
                    source.matches(worker).count(),
                    2,
                    "expected both buffered worker definitions to match"
                );
                source.replace(
                    worker,
                    &format!("runner: command\n    program: {executable}\n    args: [--exact, cli_tests::cross_platform::portable_native_buffered_entry, --nocapture]"),
                )
            });
        }
        successful(directory.path(), &["check", "--workers"]);
        let report = successful(
            directory.path(),
            &["run", "--output-dir", "run", "--max-attempts", "1"],
        );
        assert_accepted(&report, 1);
        assert_eq!(
            report["attempts"][0]["observations"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_accepted(
            &successful(
                directory.path(),
                &["run", "--output-dir", "run", "--resume"],
            ),
            4,
        );
        inspect_and_export(directory.path());
    }
}

#[test]
fn rust_workers_and_raw_python_workers_are_portable() {
    let directory = stage("rust-worker");
    let executable = if cfg!(windows) {
        "worker.exe"
    } else {
        "worker"
    };
    fs::copy(
        std::env::current_exe().unwrap(),
        directory.path().join(executable),
    )
    .unwrap();
    edit(directory.path(), "metewand.yaml", |source| {
        source
            .replace("sources: [worker]", &format!("sources: [{executable}]"))
            .replace("program: ./worker", &format!("program: ./{executable}"))
            .replace(
                "args: [implementation]",
                "args: [--exact, cli_tests::cross_platform::portable_native_implementation_entry, --nocapture]",
            )
            .replace(
                "args: [problem_evaluator]",
                "args: [--exact, cli_tests::cross_platform::portable_native_evaluator_entry, --nocapture]",
            )
    });
    successful(directory.path(), &["check", "--workers"]);
    assert_accepted(
        &successful(directory.path(), &["run", "--output-dir", "run"]),
        2,
    );
    inspect_and_export(directory.path());
    let raw = stage("generated-dataset");
    successful(raw.path(), &["check", "--workers"]);
    assert_accepted(&successful(raw.path(), &["run", "--output-dir", "run"]), 2);
}

#[test]
fn python_starter_uses_the_selected_virtual_environment() {
    let directory = Builder::new()
        .prefix("metewand starter é ")
        .tempdir()
        .unwrap();
    successful(directory.path(), &["init", "benchmark"]);
    let project = directory.path().join("benchmark");
    let venv = project.join(".venv");
    let python = if cfg!(windows) { "python" } else { "python3" };
    let output = Command::new(python)
        .args(["-m", "venv", "--without-pip"])
        .arg(&venv)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
    let interpreter = bin.join(if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    });
    let output = Command::new(&interpreter)
        .args([
            "-X",
            "utf8",
            "-c",
            "import sysconfig; print(sysconfig.get_path('purelib'))",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let site = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    copy(
        &repository().join("packages/python/src/metewand"),
        &site.join("metewand"),
    );
    fs::write(site.join("portable_venv_marker.py"), "selected = True\n").unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let run = |project: &Path, arguments: &[&str]| {
        decode(
            arguments,
            invoke_with_path(project, arguments, Some(&path)),
            0,
        )
    };
    edit(&project, "benchmark.py", |source| {
        format!("import portable_venv_marker\n{source}")
    });
    run(&project, &["check", "--workers"]);
    assert_accepted(&run(&project, &["run", "--output-dir", "run"]), 2);
    assert_accepted(
        &run(&project, &["run", "--output-dir", "run", "--resume"]),
        0,
    );
    run(&project, &["status", "--output-dir", "run"]);
    run(
        &project,
        &[
            "export",
            "--output-dir",
            "run",
            "--format",
            "csv",
            "--destination",
            "tables",
        ],
    );
}

#[test]
fn worker_failures_and_logs_preserve_machine_output() {
    let directory = stage("python-package");
    python_package(directory.path());
    edit(directory.path(), "worker.py", |source| {
        source.replace(
            "self.answer = sum(self.values) + self.offset",
            "raise RuntimeError('portable worker failure')",
        )
    });
    let arguments = ["run", "--output-dir", "run"];
    let report = decode(&arguments, invoke(directory.path(), &arguments), 5);
    assert_eq!(report["summary"]["failed_attempts"], 2, "{report}");
    assert!(
        report.to_string().contains("portable worker failure"),
        "{report}"
    );
    let resumed = successful(
        directory.path(),
        &["run", "--output-dir", "run", "--resume"],
    );
    assert!(resumed["attempts"].as_array().unwrap().is_empty());
    let arguments = ["run", "--output-dir", "run", "--resume", "--retry"];
    let retried = decode(&arguments, invoke(directory.path(), &arguments), 5);
    assert_eq!(retried["summary"]["failed_attempts"], 2, "{retried}");
    assert_ne!(
        report["attempts"][0]["attempt_id"],
        retried["attempts"][0]["attempt_id"]
    );
    let status = successful(directory.path(), &["status", "--output-dir", "run"]);
    assert!(
        status.to_string().contains("portable worker failure"),
        "{status}"
    );
    inspect_and_export(directory.path());
}
