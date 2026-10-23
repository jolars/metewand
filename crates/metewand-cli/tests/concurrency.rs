#![cfg(unix)]

mod support;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use metewand_core::canonical::MAX_SAFE_INTEGER;
use metewand_runtime::publication::verify_finalized_directory;
use serde_json::{Value, json};
use tempfile::TempDir;

const PROBE: &str = r#"
import fcntl
import time

def probe_state(change=None):
    with open(COORDINATOR_PATH, "r+") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        state = json.load(stream)
        if change is not None:
            change(state)
            stream.seek(0)
            json.dump(state, stream)
            stream.truncate()
        return state

def concurrency_probe(offset):
    width = int(sys.argv[2])
    stop = int(sys.argv[3]) + 2
    start = ((offset - 2) // width) * width + 2
    peers = set(range(start, min(start + width, stop)))
    assert not Path("attempt-dirty").exists()
    Path("attempt-dirty").write_text(str(offset))
    def enter(state):
        state["active"] += 1
        state["peak"] = max(state["peak"], state["active"])
        state["entered"].append(offset)
    probe_state(enter)
    deadline = time.monotonic() + 20
    while not peers.issubset(probe_state()["entered"]):
        if time.monotonic() >= deadline:
            raise RuntimeError("concurrent peers never entered execute")
        time.sleep(0.005)
    if offset == start:
        while not (peers - {offset}).issubset(probe_state()["completed"]):
            if time.monotonic() >= deadline:
                raise RuntimeError("concurrent peers never completed execute")
            time.sleep(0.005)
    def leave(state):
        state["active"] -= 1
        state["completed"].append(offset)
    probe_state(leave)
    if offset == int(sys.argv[4]):
        os._exit(17)
"#;

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

fn repository(max_concurrency: Option<u64>, slots: usize, failed_offset: Option<u64>) -> TempDir {
    let root = TempDir::new().unwrap();
    let benchmark = root.path().join("benchmark");
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset"),
        &benchmark,
    );
    // The host-side witness stays outside the benchmark so probe updates
    // cannot mutate declared execution inputs.
    let state = root.path().join("state.json");
    fs::write(
        &state,
        json!({"active": 0, "peak": 0, "entered": [], "completed": []}).to_string(),
    )
    .unwrap();
    let width = max_concurrency.unwrap_or(1).min(slots as u64);
    let worker = fs::read_to_string(benchmark.join("worker.py")).unwrap()
        .replace("def main():", &format!("COORDINATOR_PATH = {}\n{PROBE}\ndef main():", serde_json::to_string(state.to_str().unwrap()).unwrap()))
        .replace("prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]", "prepared = sum(values) + request[\"problem_parameters\"][\"offset\"]\n            prepared_offset = request[\"problem_parameters\"][\"offset\"]")
        .replace("assert role == \"implementation\" and prepared is not None", "assert role == \"implementation\" and prepared is not None\n            concurrency_probe(prepared_offset)")
        .replace("statistics={},", "statistics={\"cwd\": os.getcwd(), \"pid\": os.getpid()},");
    fs::write(benchmark.join("worker.py"), worker).unwrap();
    let mut manifest = fs::read_to_string(benchmark.join("metewand.yaml"))
        .unwrap()
        .replace(
            "args: [\"{sources}/worker.py\", \"implementation\"]",
            &format!(
                r#"args: ["{{sources}}/worker.py", "implementation", "{width}", "{slots}", "{}"]"#,
                failed_offset.map_or(-1, |offset| offset as i64)
            ),
        );
    manifest = support::replace_yaml(
        &manifest,
        "seed: 7",
        &format!(
            "seed: 7\nproblem_parameters:\n  offset: {{ grid: {:?} }}",
            (2..slots + 2).collect::<Vec<_>>()
        ),
    );
    if let Some(limit) = max_concurrency {
        manifest = support::replace_yaml(
            &manifest,
            "timing_scope:",
            &format!("max_concurrency: {limit}\ntiming_scope:"),
        );
    }
    fs::write(benchmark.join("metewand.yaml"), manifest).unwrap();
    root
}

fn metewand(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metewand"))
        .current_dir(root.join("benchmark"))
        .args(args)
        .output()
        .unwrap()
}

fn document(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn check_execution(root: &Path, slots: usize, max_concurrency: u64, failures: usize) {
    let output = metewand(root, &["plan", "--output", "json"]);
    assert!(output.status.success(), "{output:?}");
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    let experiment = &plan["data"]["experiments"][0];
    assert_eq!(experiment["max_concurrency"], max_concurrency);
    let expected = experiment["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|candidate| candidate["specifications"].as_array().unwrap())
        .flat_map(|specification| specification["attempt_slots"].as_array().unwrap())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), slots);
    let output = metewand(root, &["run", "--output-dir", "run", "--output", "jsonl"]);
    assert_eq!(
        output.status.code(),
        Some(if failures == 0 { 0 } else { 5 }),
        "{output:?}"
    );
    let records = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let report = &records.last().unwrap()["data"];
    assert_eq!(
        report["summary"],
        json!({"attempt_slots": slots, "accepted_attempts": slots - failures, "failed_attempts": failures, "superseded_attempts": 0})
    );
    let attempts = report["attempts"].as_array().unwrap();
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt["logical_slot_id"].clone())
            .collect::<Vec<_>>(),
        expected
    );
    let width = max_concurrency.min(slots as u64) as usize;
    let mut events = records[..records.len() - 1]
        .iter()
        .map(|record| &record["data"]);
    assert_eq!(events.next().unwrap()["event"], "run_started");
    for batch in attempts.chunks(width) {
        for attempt in batch {
            let event = events.next().unwrap();
            assert_eq!(event["event"], "attempt_started");
            assert_eq!(event["logical_slot_id"], attempt["logical_slot_id"]);
        }
        for attempt in batch {
            let event = events.next().unwrap();
            assert_eq!(event["event"], "attempt_finished");
            assert_eq!(event["attempt"], *attempt);
        }
    }
    assert_eq!(events.next().unwrap()["event"], "run_finished");
    assert!(events.next().is_none());
    assert_eq!(
        report["events"],
        Value::Array(
            records[..records.len() - 1]
                .iter()
                .map(|record| record["data"].clone())
                .collect()
        )
    );
    let mut directories = BTreeSet::new();
    for (index, attempt) in attempts.iter().enumerate() {
        let directory = root
            .join("benchmark/run")
            .join(attempt["attempt_directory"].as_str().unwrap());
        verify_finalized_directory(&directory).unwrap();
        let published = document(&directory.join("attempt.json"));
        let provenance = &published["provenance"];
        assert_eq!(provenance["execution_semantics_version"], 2);
        let context = &provenance["data"]["context"];
        assert_eq!(
            context["execution_policy"]["max_concurrency"],
            max_concurrency
        );
        assert_eq!(
            context["concurrency"],
            json!({
                "max_concurrency": max_concurrency, "dispatch": "fixed_batches",
                "batch_index": index / width, "lane_index": index % width,
                "batch_size": width.min(slots - (index / width) * width),
            })
        );
        if let Some(observation) = attempt["observation_directory"].as_str() {
            verify_finalized_directory(&root.join("benchmark/run").join(observation)).unwrap();
            assert!(
                directories.insert(
                    provenance["data"]["implementation_statistics"]["cwd"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                )
            );
        } else {
            assert_eq!(attempt["status"], "failed");
        }
    }
    assert_eq!(directories.len(), slots - failures);
    let state = document(&root.join("state.json"));
    assert_eq!(state["active"], 0);
    assert_eq!(state["peak"], width);
    assert_eq!(state["completed"].as_array().unwrap().len(), slots);
    for (index, completed) in state["completed"]
        .as_array()
        .unwrap()
        .chunks(width)
        .enumerate()
    {
        assert_eq!(completed.last().unwrap(), index * width + 2);
    }
}

#[test]
fn serial_defaults_and_explicit_bounds_enforce_real_overlap_and_deterministic_events() {
    for (limit, slots, scope) in [
        (None, 5, "prepare_and_execute"),
        (Some(2), 5, "cold_end_to_end"),
        (Some(3), 5, "execute_only"),
        (Some(MAX_SAFE_INTEGER), 1, "prepare_and_execute"),
    ] {
        let root = repository(limit, slots, None);
        for path in ["metewand.yaml", "problem.yaml"] {
            let path = root.path().join("benchmark").join(path);
            let source = fs::read_to_string(&path).unwrap();
            fs::write(path, source.replace("prepare_and_execute", scope)).unwrap();
        }
        check_execution(root.path(), slots, limit.unwrap_or(1), 0);
    }
}

#[test]
fn a_failed_concurrent_attempt_keeps_its_policy_and_does_not_skip_later_batches() {
    let root = repository(Some(2), 5, Some(3));
    check_execution(root.path(), 5, 2, 1);
}

#[test]
fn event_sink_failure_joins_dispatched_attempts_and_stops_before_the_next_batch() {
    use metewand_core::run_output::RunEventKind;
    use metewand_runtime::{
        repository::check_repository,
        run::{RunError, run_repository},
    };
    use std::{cell::Cell, io, rc::Rc};

    let root = repository(Some(2), 5, None);
    let repository = check_repository(&root.path().join("benchmark/metewand.yaml")).unwrap();
    let plan = repository.logical_plan().unwrap();
    let terminal_events = Rc::new(Cell::new(0));
    let error = run_repository(&repository, &plan, &root.path().join("run"), |event| {
        if matches!(event.kind, RunEventKind::AttemptFinished { .. }) {
            terminal_events.set(terminal_events.get() + 1);
            Err(io::Error::other("sink stopped"))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(matches!(error, RunError::Event(_)), "{error:?}");
    assert_eq!(terminal_events.get(), 1);
    let state = document(&root.path().join("state.json"));
    assert_eq!(state["active"], 0);
    assert_eq!(state["entered"].as_array().unwrap().len(), 2);
    let attempts = fs::read_dir(root.path().join("run/artifacts"))
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("attempt-"))
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    for attempt in attempts {
        verify_finalized_directory(&attempt.path()).unwrap();
    }
}
