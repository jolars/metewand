use serde_json::Value;
use std::{fs, path::Path, process::Command};
fn cli(root: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(["--output", "json"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
}
fn docker(args: &[&str]) -> String {
    let output = Command::new("docker").args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
struct Registry(String);
impl Drop for Registry {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", "--volumes", &self.0])
            .output();
    }
}
#[test]
#[ignore = "requires rootful local Linux Docker and fixture image preparation"]
fn pinned_workers_run_and_resume_after_registry_shutdown() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let registry = Registry(docker(&[
        "run",
        "--detach",
        "--publish",
        "127.0.0.1::5000",
        "registry@sha256:88604dfe2f457935aafbb96755d0511e8216eb1b8df6b94329d530706eb8b906",
    ]));
    let port = docker(&["port", &registry.0, "5000/tcp"]);
    let tag = format!("{port}/metewand-fixture:oci-test");
    docker(&[
        "build",
        "--file",
        "examples/locked-oci/Dockerfile",
        "--tag",
        &tag,
        repository.to_str().unwrap(),
    ]);
    docker(&["push", &tag]);
    let references = docker(&[
        "image",
        "inspect",
        "--format",
        "{{json .RepoDigests}}",
        &tag,
    ]);
    let image = serde_json::from_str::<Vec<String>>(&references)
        .unwrap()
        .into_iter()
        .find(|r| r.starts_with(&format!("{port}/metewand-fixture@")))
        .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("benchmark space é");
    fs::create_dir(&root).unwrap();
    for file in ["benchmark.py", "metewand.yaml"] {
        fs::copy(
            repository.join("examples/locked-oci").join(file),
            root.join(file),
        )
        .unwrap();
    }
    let manifest = root.join("metewand.yaml");
    for file in ["functions.py", "preparation.py"] {
        fs::copy(
            repository.join("examples/problem-preparation").join(file),
            root.join(file),
        )
        .unwrap();
    }
    let source =
        fs::read_to_string(repository.join("examples/problem-preparation/metewand.yaml")).unwrap();
    let prepared_manifest = source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("function:") {
                let indent = &line[..line.len() - line.trim_start().len()];
                format!("{indent}environment: container\n{line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let environment = fs::read_to_string(&manifest)
        .unwrap()
        .split_once("\nenvironments:")
        .unwrap()
        .1
        .to_owned();
    fs::write(
        &manifest,
        format!("{prepared_manifest}\nenvironments:{environment}"),
    )
    .unwrap();
    fs::write(
        &manifest,
        fs::read_to_string(&manifest).unwrap().replace(
            &format!("localhost:5000/metewand-example@sha256:{}", "0".repeat(64)),
            &image,
        ),
    )
    .unwrap();
    let plan = cli(&root, &["plan"]);
    cli(&root, &["lock"]);
    let lock = fs::read(root.join("metewand.lock")).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&lock).unwrap()["schema_version"],
        3
    );
    drop(registry);
    cli(&root, &["lock", "--check"]);
    assert_eq!(plan, cli(&root, &["plan"]));
    let run = temporary.path().join("run");
    let first = cli(
        &root,
        &[
            "run",
            "--locked",
            "--output-dir",
            run.to_str().unwrap(),
            "--max-attempts",
            "1",
        ],
    );
    assert_eq!(first["summary"]["accepted_attempts"], 1);
    let resumed = cli(
        &root,
        &[
            "run",
            "--locked",
            "--output-dir",
            run.to_str().unwrap(),
            "--resume",
        ],
    );
    assert_eq!(resumed["summary"]["accepted_attempts"], 7);
    cli(&root, &["lock", "--check"]);
    assert_eq!(lock, fs::read(root.join("metewand.lock")).unwrap());
    let original: Value = serde_json::from_slice(&lock).unwrap();
    let image_id = original["environments"]["container"]["evidence"]["image_id"]
        .as_str()
        .unwrap();
    check_failure_cleanup(image_id, temporary.path());
    for changed in ["image_id", "image"] {
        let mut tampered = original.clone();
        tampered["environments"]["container"]["evidence"][changed] =
            Value::from(if changed == "image_id" {
                format!("sha256:{}", "0".repeat(64))
            } else {
                format!("missing.invalid/image@sha256:{}", "0".repeat(64))
            });
        fs::write(
            root.join("metewand.lock"),
            serde_json::to_vec(&tampered).unwrap(),
        )
        .unwrap();
        let destination = temporary.path().join(format!("failed-{changed}"));
        let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(&root)
            .args([
                "--output",
                "json",
                "run",
                "--locked",
                "--output-dir",
                destination.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("oci_environment_changed"));
        assert!(!destination.exists());
    }
    fs::write(root.join("metewand.lock"), &lock).unwrap();
    assert_eq!(check_observations(&run), 8);
    let status = cli(&root, &["status", "--output-dir", run.to_str().unwrap()]);
    assert!(!status.is_null());
    for format in ["csv", "jsonl"] {
        cli(
            &root,
            &[
                "export",
                "--output-dir",
                run.to_str().unwrap(),
                "--format",
                format,
                "--destination",
                temporary.path().join(format).to_str().unwrap(),
            ],
        );
    }
}

fn check_failure_cleanup(image: &str, root: &Path) {
    use crate::{
        core::records::{RecordId, ResolvedLaunchRecord, WorkerWorkingDirectory},
        protocol::WorkerRole,
        runtime::{
            local_worker::launch_trusted_local_worker,
            worker_process::WorkerLogLimits,
            worker_session::{PhaseTimeouts, WorkerSession},
        },
    };
    let before = docker(&[
        "ps",
        "--all",
        "--quiet",
        "--filter",
        "label=org.metewand.worker=true",
    ]);
    for program in ["/usr/local/bin/python3", "/missing-metewand-worker"] {
        let launch=ResolvedLaunchRecord {environment:RecordId::from_digest([0;32]),program:program.into(),args:vec!["-c".into(),"import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'],start_new_session=True); time.sleep(30)".into()],environment_variables:std::collections::BTreeMap::from([("METEWAND_OCI_IMAGE".into(),image.into()),("METEWAND_PROTOCOL_TRANSPORT".into(),"socket".into())]),working_directory:WorkerWorkingDirectory::PrivateRunDirectory};
        let process =
            launch_trusted_local_worker(&launch, root, WorkerLogLimits::new(4096, 4096)).unwrap();
        assert!(
            WorkerSession::connect(
                process,
                WorkerRole::Implementation,
                format!("mw1-resolved-worker-{}", "0".repeat(64))
                    .parse()
                    .unwrap(),
                &[],
                PhaseTimeouts {
                    hello: std::time::Duration::from_millis(250),
                    ..PhaseTimeouts::default()
                }
            )
            .is_err()
        );
    }
    assert_eq!(
        before,
        docker(&[
            "ps",
            "--all",
            "--quiet",
            "--filter",
            "label=org.metewand.worker=true"
        ])
    );
}

fn check_observations(root: &Path) -> usize {
    let mut count = 0;
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            count += check_observations(&entry.path());
        } else if entry.file_name() == "observation.json" {
            let observation: Value =
                serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
            assert!(observation["timing"]["cpu_time_ns"].is_null());
            assert!(observation["timing"]["peak_rss_bytes"].is_null());
            for role in ["implementation", "evaluator"] {
                let environment =
                    &observation["provenance"]["data"]["execution"][role]["worker_environment"];
                assert_eq!(environment["version"], 3);
                assert!(environment["umask"].is_null());
                let variables = environment["environment_variables"].as_object().unwrap();
                assert!(!variables.contains_key("METEWAND_OCI_IMAGE"));
                assert!(!variables.contains_key("METEWAND_OCI_SOURCE_ROOT"));
            }
            count += 1;
        }
    }
    count
}
