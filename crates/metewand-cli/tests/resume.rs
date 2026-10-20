#![cfg(unix)]

use std::{fs, path::Path, process::Command};

use metewand_core::public_schemas::{PublicSchema, public_schema_catalog};
use metewand_runtime::publication::verify_finalized_directory;
use serde_json::Value;
use tempfile::TempDir;

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn repository() -> TempDir {
    let root = TempDir::new().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset"),
        root.path(),
    );
    let path = root.path().join("metewand.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        manifest.replace("measurement_repetitions = 1", "measurement_repetitions = 3"),
    )
    .unwrap();
    root
}

fn run(root: &Path, flags: &[&str]) -> (i32, Value) {
    run_to(root, "run", flags)
}

fn run_to(root: &Path, destination: &str, flags: &[&str]) -> (i32, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args(["run", "--output-dir", destination, "--output", "json"])
        .args(flags)
        .output()
        .unwrap();
    let document: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    public_schema_catalog()
        .unwrap()
        .validate(PublicSchema::RunOutput.repository_path(), &document)
        .unwrap();
    (output.status.code().unwrap(), document)
}

#[test]
fn candidate_run_observation_and_slot_filters_preserve_identities_and_form_a_union() {
    let root = repository();
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root.path())
        .args(["plan", "--output", "json"])
        .output()
        .unwrap();
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    let candidate = &plan["data"]["experiments"][0]["candidates"][0];
    let specification = &candidate["specifications"][0];
    let (code, baseline) = run(root.path(), &["--max-attempts", "1"]);
    assert_eq!(code, 0, "{baseline}");
    let baseline = &baseline["data"]["attempts"][0];
    let attempt: Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("run")
                .join(baseline["attempt_directory"].as_str().unwrap())
                .join("attempt.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let observation: Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("run")
                .join(baseline["observation_directory"].as_str().unwrap())
                .join("observation.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let selectors = [
        (candidate["id"].as_str().unwrap().to_owned(), 2),
        (specification["id"].as_str().unwrap().to_owned(), 2),
        (
            specification["observation_slots"][0]
                .as_str()
                .unwrap()
                .to_owned(),
            1,
        ),
        (
            attempt["provenance"]["data"]["context"]["resolved_attempt_slot"]["specification"]
                .as_str()
                .unwrap()
                .to_owned(),
            2,
        ),
        (observation["slot_id"].as_str().unwrap().to_owned(), 1),
        (baseline["resolved_slot_id"].as_str().unwrap().to_owned(), 1),
        (
            format!(
                "run-store-v1:unconditional:{}",
                candidate["id"].as_str().unwrap()
            ),
            2,
        ),
    ];
    for (index, (selector, expected)) in selectors.iter().enumerate() {
        let (code, selected) = run_to(
            root.path(),
            &format!("selection-{index}"),
            &["--select", selector, "--max-attempts", "2"],
        );
        assert_eq!(code, 0, "{selected}");
        assert_eq!(
            selected["data"]["attempts"].as_array().unwrap().len(),
            *expected
        );
        assert_eq!(
            selected["data"]["attempts"][0]["resolved_slot_id"],
            baseline["resolved_slot_id"]
        );
    }
    let slots = &specification["attempt_slots"];
    let (code, union) = run_to(
        root.path(),
        "union",
        &[
            "--select",
            slots[0].as_str().unwrap(),
            "--select",
            slots[1].as_str().unwrap(),
            "--select",
            slots[0].as_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{union}");
    assert_eq!(union["data"]["attempts"].as_array().unwrap().len(), 2);
    let (code, previous_observation) = run(
        root.path(),
        &[
            "--resume",
            "--select",
            baseline["observation_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{previous_observation}");
    assert_eq!(
        previous_observation["data"]["attempts"],
        serde_json::json!([])
    );
}

#[test]
fn bounded_execution_resumes_without_repeating_accepted_slots() {
    let root = repository();
    let (code, first) = run(root.path(), &["--max-attempts", "1"]);
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["data"]["attempts"].as_array().unwrap().len(), 1);
    let attempt = &first["data"]["attempts"][0];
    let directory = root
        .path()
        .join("run")
        .join(attempt["attempt_directory"].as_str().unwrap());
    let bytes = fs::read(directory.join("attempt.json")).unwrap();
    let (code, second) = run(root.path(), &["--resume"]);
    assert_eq!(code, 0, "{second}");
    assert_eq!(second["data"]["attempts"].as_array().unwrap().len(), 2);
    for next in second["data"]["attempts"].as_array().unwrap() {
        assert_ne!(next["resolved_slot_id"], attempt["resolved_slot_id"]);
    }
    assert_eq!(fs::read(directory.join("attempt.json")).unwrap(), bytes);
    verify_finalized_directory(&directory).unwrap();
    let (code, third) = run(root.path(), &["--resume"]);
    assert_eq!(code, 0, "{third}");
    assert_eq!(third["data"]["attempts"], serde_json::json!([]));
}

#[test]
fn invocation_bounds_shrink_concurrency_batches_without_changing_the_policy() {
    let root = repository();
    let path = root.path().join("metewand.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        manifest.replace("timing_scope =", "max_concurrency = 3\ntiming_scope ="),
    )
    .unwrap();
    for (flags, expected) in [
        (vec!["--max-attempts", "2"], 2),
        (vec!["--resume", "--max-attempts", "2"], 1),
    ] {
        let (code, report) = run(root.path(), &flags);
        assert_eq!(code, 0, "{report}");
        let attempts = report["data"]["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), expected);
        for (lane, attempt) in attempts.iter().enumerate() {
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
            assert_eq!(context["execution_policy"]["max_concurrency"], 3);
            assert_eq!(context["concurrency"]["batch_size"], expected);
            assert_eq!(context["concurrency"]["lane_index"], lane);
        }
    }
}

#[test]
fn a_live_worker_renews_its_lease_and_records_its_current_phase() {
    let root = gated_repository();
    let process = spawn_run(root.path(), &[]);
    wait_for_workers(root.path(), 1);
    std::thread::sleep(std::time::Duration::from_secs(6));
    let output = Command::new("python3").args(["-c", "import json, sqlite3, sys; db = sqlite3.connect(sys.argv[1]); print(json.dumps(db.execute(\"SELECT l.expires_at_ns - l.acquired_at_ns, a.state FROM leases l JOIN attempts a ON a.id = l.attempt_id WHERE l.purpose = 'reservation'\").fetchone()))"]).arg(root.path().join("run/run.sqlite3")).output().unwrap();
    fs::write(root.path().join("release"), "release").unwrap();
    let completed = process.wait_with_output().unwrap();
    assert!(completed.status.success(), "{completed:?}");
    assert!(output.status.success(), "{output:?}");
    let lease: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(lease[0].as_u64().unwrap() > 34_000_000_000);
    assert_eq!(lease[1], "executing");
}

#[test]
fn selected_logical_slot_executes_only_that_slot() {
    let root = repository();
    let output = Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root.path())
        .args(["plan", "--output", "json"])
        .output()
        .unwrap();
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    let slot =
        plan["data"]["experiments"][0]["candidates"][0]["specifications"][0]["attempt_slots"][1]
            .as_str()
            .unwrap();
    let (code, report) = run(root.path(), &["--select", slot]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["data"]["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(report["data"]["attempts"][0]["logical_slot_id"], slot);
}

#[test]
fn retries_require_authorization_and_preserve_failed_attempt_bytes() {
    let root = repository();
    let worker = root.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap();
    let marker = root.path().join("fail");
    fs::write(&marker, "fail").unwrap();
    fs::write(&worker, source.replace("elif method == \"execute\":", &format!("elif method == \"execute\":\n            if Path({:?}).exists():\n                os._exit(9)", marker.to_str().unwrap()))).unwrap();
    let (code, first) = run(root.path(), &["--max-attempts", "1"]);
    assert_eq!(code, 5, "{first}");
    let attempt = &first["data"]["attempts"][0];
    let directory = root
        .path()
        .join("run")
        .join(attempt["attempt_directory"].as_str().unwrap());
    let original = fs::read(directory.join("attempt.json")).unwrap();
    let (_, resumed) = run(root.path(), &["--resume"]);
    assert_eq!(resumed["data"]["summary"]["failed_attempts"], 2);
    fs::remove_file(marker).unwrap();
    let (code, retry) = run(
        root.path(),
        &[
            "--resume",
            "--retry",
            "--select",
            attempt["attempt_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{retry}");
    assert_eq!(retry["data"]["attempts"].as_array().unwrap().len(), 1);
    let next = &retry["data"]["attempts"][0];
    assert_ne!(next["attempt_id"], attempt["attempt_id"]);
    assert_eq!(next["resolved_slot_id"], attempt["resolved_slot_id"]);
    let document: Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("run")
                .join(next["attempt_directory"].as_str().unwrap())
                .join("attempt.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(document["retry_index"], 1);
    assert_eq!(fs::read(directory.join("attempt.json")).unwrap(), original);
    verify_finalized_directory(&directory).unwrap();
    let (code, no_op) = run(
        root.path(),
        &[
            "--resume",
            "--retry",
            "--select",
            attempt["attempt_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{no_op}");
    assert_eq!(no_op["data"]["attempts"], serde_json::json!([]));
}

#[test]
fn unknown_selection_and_invalid_bounds_do_not_create_a_run() {
    for flags in [
        vec!["--select", "unknown"],
        vec!["--max-attempts", "0"],
        vec!["--max-attempts", "9007199254740992"],
    ] {
        let root = repository();
        let (code, report) = run(root.path(), &flags);
        assert_eq!(code, 2, "{report}");
        assert!(!root.path().join("run").exists());
    }
}

fn gated_repository() -> TempDir {
    let root = repository();
    let worker = root.path().join("worker.py");
    let source = fs::read_to_string(&worker).unwrap();
    fs::write(
        &worker,
        source.replace(
            "elif method == \"execute\":",
            &format!(
                r#"elif method == "execute":
            import time
            gate = Path({:?})
            gate.joinpath("started-" + str(os.getpid())).write_text("started")
            while not gate.joinpath("release").exists():
                time.sleep(0.01)"#,
                root.path().to_str().unwrap()
            ),
        ),
    )
    .unwrap();
    root
}

fn spawn_run(root: &Path, flags: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root)
        .args([
            "run",
            "--output-dir",
            "run",
            "--output",
            "json",
            "--max-attempts",
            "1",
        ])
        .args(flags)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

fn wait_for_workers(root: &Path, count: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while fs::read_dir(root)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("started-")
        })
        .count()
        < count
    {
        assert!(
            std::time::Instant::now() < deadline,
            "workers did not reach the gate"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn concurrent_retry_candidates_publish_one_acceptance_and_one_superseded_outcome() {
    let root = gated_repository();
    let first = spawn_run(root.path(), &[]);
    wait_for_workers(root.path(), 1);
    let second = spawn_run(root.path(), &["--resume", "--retry"]);
    wait_for_workers(root.path(), 2);
    fs::write(root.path().join("release"), "release").unwrap();
    let mut statuses = Vec::new();
    let mut ids = Vec::new();
    for process in [first, second] {
        let output = process.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        public_schema_catalog()
            .unwrap()
            .validate(PublicSchema::RunOutput.repository_path(), &report)
            .unwrap();
        let attempt = &report["data"]["attempts"][0];
        let status = attempt["status"].as_str().unwrap().to_owned();
        let directory = root
            .path()
            .join("run")
            .join(attempt["attempt_directory"].as_str().unwrap());
        verify_finalized_directory(&directory).unwrap();
        let document: Value =
            serde_json::from_slice(&fs::read(directory.join("attempt.json")).unwrap()).unwrap();
        assert_eq!(document["outcome"]["kind"], status);
        let outcome: Value =
            serde_json::from_slice(&fs::read(directory.join("outcome.json")).unwrap()).unwrap();
        assert_eq!(outcome["state"], status);
        assert_eq!(
            outcome["coordination"]["observations"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|observation| !observation["selected_at_ns"].is_null())
                .count(),
            if status == "accepted" { 1 } else { 0 }
        );
        statuses.push(status);
        ids.push(attempt["attempt_id"].clone());
    }
    statuses.sort();
    assert_eq!(statuses, ["accepted", "superseded"]);
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn expired_interrupted_attempt_is_recovered_and_requires_an_explicit_retry() {
    let root = gated_repository();
    let mut first = spawn_run(root.path(), &[]);
    wait_for_workers(root.path(), 1);
    first.kill().unwrap();
    first.wait().unwrap();
    fs::write(root.path().join("release"), "release").unwrap();
    let mut store = metewand_runtime::run_store::RunStore::open(&root.path().join("run")).unwrap();
    let future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        + 31_000_000_000;
    let recovered = store.recover(|| future).unwrap();
    assert_eq!(recovered.terminals.len(), 1);
    assert!(
        matches!(&recovered.terminals[0].outcome, metewand_runtime::run_store::TerminalOutcome::Failed { code, .. } if code == "interrupted_attempt")
    );
    let interrupted = recovered.terminals[0].id.to_string();
    let (_, skipped) = run(root.path(), &["--resume", "--select", &interrupted]);
    assert_eq!(skipped["data"]["attempts"], serde_json::json!([]));
    let (code, retried) = run(
        root.path(),
        &["--resume", "--retry", "--select", &interrupted],
    );
    assert_eq!(code, 0, "{retried}");
    assert_eq!(retried["data"]["summary"]["accepted_attempts"], 1);
    assert_ne!(retried["data"]["attempts"][0]["attempt_id"], interrupted);
}
