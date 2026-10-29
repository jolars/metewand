use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

fn copy(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &destination.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
}

struct Benchmark {
    temporary: TempDir,
    root: PathBuf,
    data: PathBuf,
}
impl Benchmark {
    fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("benchmark space é");
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/locked-uv"),
            &root,
        );
        let data = temporary.path().join("data space é");
        Self {
            temporary,
            root,
            data,
        }
    }
    fn invoke(&self, args: &[&str], offline: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_metewand"));
        command
            .current_dir(&self.root)
            .env("METEWAND_DATA_DIR", &self.data)
            .env("UV_OFFLINE", "1")
            .args(["--output", "json"])
            .args(args);
        if offline {
            command.env("PATH", self.temporary.path().join("no-programs"));
        }
        command.output().unwrap()
    }
    fn lock(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.join("metewand.lock")).unwrap()).unwrap()
    }
    fn environment(&self) -> PathBuf {
        self.data
            .join("environments/uv")
            .join(
                self.lock()["environments"]["local"]["evidence"]["preparation"]
                    .as_str()
                    .unwrap(),
            )
            .join("environment")
    }
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    metewand_core::public_schemas::public_schema_catalog()
        .unwrap()
        .validate(
            metewand_core::public_schemas::PublicSchema::MachineOutput.repository_path(),
            &value,
        )
        .unwrap();
    value["data"].clone()
}
fn failure(output: Output) -> Value {
    assert!(!output.status.success());
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
}
fn find(root: &Path, filename: &str) -> PathBuf {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == filename {
            return entry.path();
        }
        if entry.file_type().unwrap().is_dir() {
            let path = find(&entry.path(), filename);
            if path.is_file() {
                return path;
            }
        }
    }
    root.join("absent")
}

#[test]
#[ignore = "requires installed uv and Python"]
fn prepared_workers_run_and_resume_without_package_managers_and_detect_tampering() {
    let benchmark = Benchmark::new();
    let native = fs::read(benchmark.root.join("uv.lock")).unwrap();
    let plan = success(benchmark.invoke(&["plan"], true));
    success(benchmark.invoke(&["lock"], false));
    let bytes = fs::read(benchmark.root.join("metewand.lock")).unwrap();
    assert_eq!(
        success(benchmark.invoke(&["lock"], false))["operation"],
        "unchanged"
    );
    assert_eq!(success(benchmark.invoke(&["plan"], true)), plan);
    success(benchmark.invoke(&["lock", "--check"], true));
    let output = benchmark.temporary.path().join("run");
    success(benchmark.invoke(
        &[
            "run",
            "--locked",
            "--output-dir",
            output.to_str().unwrap(),
            "--max-attempts",
            "1",
        ],
        true,
    ));
    success(benchmark.invoke(
        &[
            "run",
            "--locked",
            "--output-dir",
            output.to_str().unwrap(),
            "--resume",
        ],
        true,
    ));
    success(benchmark.invoke(&["lock", "--check"], true));
    assert_eq!(
        fs::read(benchmark.root.join("metewand.lock")).unwrap(),
        bytes
    );
    assert_eq!(fs::read(benchmark.root.join("uv.lock")).unwrap(), native);
    let installed = find(&benchmark.environment(), "metewand_example_arithmetic.py");
    fs::write(&installed, "raise RuntimeError('tampered')\n").unwrap();
    assert_eq!(
        failure(benchmark.invoke(&["lock", "--check"], true))["code"],
        "uv_environment_changed"
    );
    let destination = benchmark.temporary.path().join("not-created");
    assert_eq!(
        failure(benchmark.invoke(
            &[
                "run",
                "--locked",
                "--output-dir",
                destination.to_str().unwrap()
            ],
            true
        ))["code"],
        "uv_environment_changed"
    );
    assert!(!destination.exists());
    success(benchmark.invoke(&["lock"], false));
    assert_ne!(
        benchmark.environment(),
        installed
            .ancestors()
            .find(|p| p.file_name().is_some_and(|s| s == "environment"))
            .unwrap()
    );
    success(benchmark.invoke(&["lock", "--check"], true));
    let injected = find(&benchmark.environment(), "metewand_example_arithmetic.py")
        .with_file_name("injected.py");
    fs::write(&injected, "pass\n").unwrap();
    assert_eq!(
        failure(benchmark.invoke(&["lock", "--check"], true))["code"],
        "uv_environment_changed"
    );
}

#[test]
#[ignore = "requires installed uv and Python"]
fn source_only_changes_rebuild_and_locations_do_not_define_worker_identity() {
    let first = Benchmark::new();
    let second = Benchmark::new();
    success(first.invoke(&["lock"], false));
    success(second.invoke(&["lock"], false));
    let first_lock = first.lock();
    let second_lock = second.lock();
    assert_eq!(
        first_lock["environments"]["local"]["evidence"]["fingerprint"],
        second_lock["environments"]["local"]["evidence"]["fingerprint"]
    );
    assert_eq!(first_lock["workers"], second_lock["workers"]);
    let source = first.root.join("arithmetic/metewand_example_arithmetic.py");
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    fs::write(
        &source,
        fs::read_to_string(&source)
            .unwrap()
            .replace("sum(values)", "sum(values) + 0"),
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        failure(first.invoke(&["lock", "--check"], true))["code"],
        "lockfile_outdated"
    );
    success(first.invoke(&["lock"], false));
    assert!(
        fs::read_to_string(find(&first.environment(), "metewand_example_arithmetic.py"))
            .unwrap()
            .contains("+ 0")
    );
    assert_ne!(
        first.lock()["environments"]["local"]["evidence"]["fingerprint"],
        first_lock["environments"]["local"]["evidence"]["fingerprint"]
    );
}

#[test]
#[ignore = "requires installed uv and Python"]
fn failed_preparation_retains_locks_and_author_environment() {
    let benchmark = Benchmark::new();
    let author = benchmark.root.join(".venv");
    fs::create_dir(&author).unwrap();
    fs::write(author.join("marker"), "author").unwrap();
    success(benchmark.invoke(&["lock"], false));
    let old = fs::read(benchmark.root.join("metewand.lock")).unwrap();
    let project = benchmark.root.join("pyproject.toml");
    fs::write(
        &project,
        fs::read_to_string(&project).unwrap().replace(
            "dependencies = []",
            "dependencies = [\"definitely-missing-metewand-dependency\"]",
        ),
    )
    .unwrap();
    let native = fs::read(benchmark.root.join("uv.lock")).unwrap();
    failure(benchmark.invoke(&["lock"], false));
    assert_eq!(fs::read(benchmark.root.join("metewand.lock")).unwrap(), old);
    assert_eq!(fs::read(benchmark.root.join("uv.lock")).unwrap(), native);
    assert_eq!(fs::read_to_string(author.join("marker")).unwrap(), "author");
}
