use std::{
    collections::BTreeSet,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use metewand_core::{
    canonical::CanonicalValue,
    records::{RecordId, ResolvedAttemptSlotRecord},
};
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::fault_injection::arm;
use crate::{
    local_tree::hash_local_tree,
    publication::verify_finalized_directory,
    run_store::{
        ArtifactKind, AttemptPhase, LeaseRequest, OwnedAttempt, RetryAuthorization, RunStore,
        RunStoreError, TerminalOutcome,
    },
};

fn slot(index: u8) -> RecordId<ResolvedAttemptSlotRecord> {
    RecordId::from_digest([index; 32])
}

fn lease(name: &str, expiry: u64) -> LeaseRequest {
    LeaseRequest {
        id: name.into(),
        owner_token: "owner".into(),
        expires_at_ns: expiry,
    }
}

fn payload(path: &Path) -> io::Result<()> {
    fs::create_dir(path.join("nested"))?;
    fs::write(path.join("nested/payload"), b"verified bytes\0")
}

fn reserve(store: &mut RunStore) -> OwnedAttempt {
    store
        .reserve(
            slot(1),
            RetryAuthorization::FirstAttempt,
            lease("reservation", 100),
            10,
        )
        .unwrap()
}

fn claim(store: &mut RunStore, attempt: &OwnedAttempt) -> OwnedAttempt {
    store
        .begin_finalization(attempt, lease("claim", 100), 15)
        .unwrap()
        .attempt
}

fn seed(store: &RunStore, measurement: bool) {
    store
        .connection
        .execute_batch(
            "INSERT INTO applicability_decisions VALUES ('decision', 'candidate', 1, '{}');
         INSERT INTO resolved_runs VALUES ('run', 'logical', 'candidate', 'decision', 1, '{}');",
        )
        .unwrap();
    for index in [1, 2] {
        store
            .connection
            .execute(
                "INSERT INTO attempt_slots VALUES (?1, ?1, 'run', 'one_shot', ?2, '{}')",
                (
                    slot(index).to_string(),
                    if measurement { "measurement" } else { "warmup" },
                ),
            )
            .unwrap();
    }
    if measurement {
        store
            .connection
            .execute_batch(
                "INSERT INTO observation_slots VALUES ('observation-slot', 'logical', 'run', '{}')",
            )
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO attempt_observation_slots VALUES (?1, 'observation-slot', 'run')",
                [slot(1).to_string()],
            )
            .unwrap();
    }
}

fn observe(store: &mut RunStore, attempt: &OwnedAttempt, now: u64) -> String {
    let artifact = store
        .publish_artifact(attempt, ArtifactKind::Observation, payload, || now)
        .unwrap()
        .artifact;
    let id = format!("observation-{}", attempt.retry_index());
    // The orchestrator's record and selection registration must be atomic too.
    let tx = store.connection.transaction().unwrap();
    tx.execute("INSERT INTO observations VALUES (?1, 'observation-slot', ?2, ?3, 1, 'reached', ?4, 'observation', '{}')",
        (&id, attempt.id().to_string(), slot(1).to_string(), &artifact.id)).unwrap();
    tx.execute(
        "INSERT OR IGNORE INTO selected_observations VALUES ('observation-slot', ?1, 1, ?2)",
        (&id, now as i64),
    )
    .unwrap();
    tx.commit().unwrap();
    artifact.id
}

fn prepare(root: &Path, scenario: &str) {
    if scenario == "initialize" {
        return;
    }
    let mut store = RunStore::open(root).unwrap();
    seed(&store, !scenario.starts_with("concurrent"));
    if !scenario.starts_with("recover-") {
        return;
    }
    let attempt = reserve(&mut store);
    if scenario == "recover-artifact" {
        let mut times = [20, 100].into_iter();
        assert!(
            store
                .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || times
                    .next()
                    .unwrap())
                .is_err()
        );
        return;
    }
    let observation = observe(&mut store, &attempt, 11);
    let attempt = claim(&mut store, &attempt);
    if scenario == "recover-interrupted" {
        assert!(
            store
                .publish_artifact(
                    &attempt,
                    ArtifactKind::Other,
                    |path| {
                        payload(path)?;
                        Err(io::Error::other("interrupted population"))
                    },
                    || 20
                )
                .is_err()
        );
    } else {
        let mut times = [20, 20, 100].into_iter();
        assert!(
            store
                .publish_finalization(
                    &attempt,
                    TerminalOutcome::Accepted,
                    &[("observation".into(), observation)],
                    payload,
                    || times.next().unwrap()
                )
                .is_err()
        );
    }
}

fn publish(store: &mut RunStore, scenario: &str) {
    let mut attempt = reserve(store);
    if scenario == "accepted" {
        for (offset, phase) in [
            AttemptPhase::Materializing,
            AttemptPhase::Starting,
            AttemptPhase::Preparing,
            AttemptPhase::Executing,
            AttemptPhase::Checkpointing,
            AttemptPhase::Validating,
            AttemptPhase::Evaluating,
        ]
        .into_iter()
        .enumerate()
        {
            attempt = store.advance(&attempt, phase, 11 + offset as u64).unwrap();
        }
        store.renew(&attempt, 120, 18).unwrap();
    }
    let dependency = store
        .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 20)
        .unwrap();
    // Exercise the no-replace collision and independent synchronization path.
    store
        .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 21)
        .unwrap();
    let observation = observe(store, &attempt, 22);
    let outcome = match scenario {
        "accepted" => TerminalOutcome::Accepted,
        "censored" => TerminalOutcome::Censored {
            reason: "bound".into(),
        },
        "superseded" => TerminalOutcome::Superseded,
        "failed" => TerminalOutcome::Failed {
            code: "injected_failure".into(),
            details: CanonicalValue::try_from(json!({})).unwrap(),
        },
        _ => panic!("unknown scenario {scenario}"),
    };
    let attempt = if scenario == "accepted" {
        claim(store, &attempt)
    } else {
        store
            .advance(&attempt, AttemptPhase::Finalizing, 23)
            .unwrap()
    };
    store
        .publish_finalization(
            &attempt,
            outcome,
            &[
                ("dataset".into(), dependency.artifact.id),
                ("observation".into(), observation),
            ],
            payload,
            || 25,
        )
        .unwrap();
}

fn command(root: &Path, scenario: &str, trace: &Path, target: usize) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "run_store::fault_tests::crash_child",
            "--nocapture",
        ])
        .env("METEWAND_FAULT_ROOT", root)
        .env("METEWAND_FAULT_SCENARIO", scenario)
        .env("METEWAND_FAULT_TRACE", trace)
        .env("METEWAND_FAULT_TARGET", target.to_string());
    command
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("METEWAND_FAULT_ROOT") else {
        return;
    };
    let scenario = std::env::var("METEWAND_FAULT_SCENARIO").unwrap();
    let trace = File::create(std::env::var_os("METEWAND_FAULT_TRACE").unwrap()).unwrap();
    let target = std::env::var("METEWAND_FAULT_TARGET")
        .unwrap()
        .parse()
        .unwrap();
    if scenario == "initialize" {
        arm(trace, target);
        RunStore::open(Path::new(&root)).unwrap();
        return;
    }
    let mut store = RunStore::open(Path::new(&root)).unwrap();
    arm(trace, target);
    if scenario.starts_with("recover-") {
        store.recover(|| 200).unwrap();
    } else if let Some(index) = scenario.strip_prefix("concurrent-") {
        let index: u8 = index.parse().unwrap();
        let attempt = store
            .reserve(
                slot(index),
                RetryAuthorization::FirstAttempt,
                lease(&format!("writer-{index}"), 100),
                10,
            )
            .unwrap();
        store
            .publish_artifact(
                &attempt,
                ArtifactKind::Dataset,
                |path| {
                    payload(path)?;
                    let root = Path::new(&root);
                    fs::write(root.join(format!("ready-{index}")), b"")?;
                    wait_for(&root.join(format!("go-{index}")));
                    Ok(())
                },
                || 20,
            )
            .unwrap();
    } else {
        publish(&mut store, &scenario);
    }
}

fn run(root: &Path, scenario: &str, target: usize) -> (Output, Vec<String>) {
    let trace = tempfile::NamedTempFile::new().unwrap();
    let output = command(root, scenario, trace.path(), target)
        .output()
        .unwrap();
    let boundaries = fs::read_to_string(trace.path())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    (output, boundaries)
}

fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn selections(db: &Connection) -> Vec<(String, String, i64)> {
    db.prepare("SELECT slot_id, observation_id, selected_at_ns FROM selected_observations ORDER BY slot_id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

fn published_successes(root: &Path) -> BTreeSet<String> {
    let mut successes = BTreeSet::new();
    if let Ok(entries) = fs::read_dir(root.join("artifacts")) {
        for entry in entries {
            let path = entry.unwrap().path();
            if !path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("attempt-")
            {
                continue;
            }
            verify_finalized_directory(&path).unwrap();
            let document: Value =
                serde_json::from_slice(&fs::read(path.join("outcome.json")).unwrap()).unwrap();
            if document["state"] == "accepted" {
                successes.insert(document["attempt_id"].as_str().unwrap().to_owned());
            }
        }
    }
    successes
}

fn assert_terminal_artifacts(root: &Path, db: &Connection, successes: &BTreeSet<String>) {
    let mut query = db
        .prepare("SELECT relative_path, content_sha256, manifest_sha256 FROM artifacts")
        .unwrap();
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (relative, content, manifest) = row.unwrap();
        let directory = root.join(relative);
        verify_finalized_directory(&directory).unwrap();
        assert_eq!(
            Sha256::digest(fs::read(directory.join("manifest.json")).unwrap())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            manifest
        );
        assert_eq!(
            hash_local_tree(&directory)
                .unwrap()
                .bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            content
        );
    }
    let mut query = db.prepare("SELECT a.id, a.state, a.finished_at_ns, r.relative_path FROM attempts a JOIN artifacts r ON r.id = a.final_artifact_id WHERE a.finished_at_ns IS NOT NULL").unwrap();
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (id, state, finished, relative) = row.unwrap();
        let directory = root.join(relative);
        let document: Value =
            serde_json::from_slice(&fs::read(directory.join("outcome.json")).unwrap()).unwrap();
        assert_eq!(document["attempt_id"], id);
        assert_eq!(document["state"], state);
        assert_eq!(document["finished_at_ns"], finished.to_string());
        if state == "accepted" {
            assert!(successes.contains(&id), "accepted unpublished attempt {id}");
        }
    }
}

fn audit(root: &Path, scenario: &str) {
    let mut store = RunStore::open(root).unwrap();
    if scenario == "initialize" {
        seed(&store, true);
    }
    let successes = published_successes(root);
    let before_selections = selections(&store.connection);
    let accepted_before = count_where(&store.connection, "state = 'accepted'");
    store.recover(|| 200).unwrap();
    assert_eq!(selections(&store.connection), before_selections);
    let accepted = count_where(&store.connection, "state = 'accepted'");
    assert!(accepted >= accepted_before);
    assert!(accepted <= 1);
    let accepted_ids: BTreeSet<String> = store
        .connection
        .prepare("SELECT id FROM attempts WHERE state = 'accepted'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        accepted_ids, successes,
        "recovery must import every verified claimed success"
    );
    assert_eq!(count_where(&store.connection, "finished_at_ns IS NULL"), 0);
    assert_eq!(count(&store.connection, "finalization_claims"), 0);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT count(*) FROM leases WHERE released_at_ns IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_terminal_artifacts(root, &store.connection, &successes);
    let tables = [
        "events",
        "attempt_transitions",
        "attempts",
        "artifacts",
        "attempt_artifacts",
        "leases",
        "observations",
        "selected_observations",
    ];
    let before: Vec<_> = tables
        .iter()
        .map(|table| count(&store.connection, table))
        .collect();
    let report = store.recover(|| 201).unwrap();
    assert!(report.terminals.is_empty());
    assert!(report.imported_artifacts.is_empty());
    assert!(report.quarantined_stages.is_empty());
    assert!(report.busy_stages.is_empty());
    assert_eq!(
        tables
            .iter()
            .map(|table| count(&store.connection, table))
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(
        !store
            .connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert!(fs::read_dir(root.join("artifacts")).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_str()
            .unwrap()
            .starts_with(".staging-")
    }));

    // Resume uses the next authorized retry and cannot displace a selection.
    let next: Option<i64> = store
        .connection
        .query_row("SELECT max(retry_index) FROM attempts", [], |r| r.get(0))
        .unwrap();
    let authorization = next.map_or(RetryAuthorization::FirstAttempt, |n| {
        RetryAuthorization::Retry {
            next_index: n as u64 + 1,
            max_index: n as u64 + 1,
        }
    });
    let reservation = store.reserve(slot(1), authorization, lease("resume", 400), 210);
    if accepted == 1 {
        assert!(matches!(reservation, Err(RunStoreError::SatisfiedSlot)));
    } else {
        let attempt = reservation.unwrap();
        let observation = observe(&mut store, &attempt, 220);
        let attempt = store
            .begin_finalization(&attempt, lease("resume-claim", 400), 230)
            .unwrap()
            .attempt;
        store
            .publish_finalization(
                &attempt,
                TerminalOutcome::Accepted,
                &[("observation".into(), observation)],
                payload,
                || 240,
            )
            .unwrap();
    }
    assert_eq!(count_where(&store.connection, "state = 'accepted'"), 1);
    for selected in before_selections {
        assert!(selections(&store.connection).contains(&selected));
    }
}

fn count_where(db: &Connection, condition: &str) -> i64 {
    db.query_row(
        &format!("SELECT count(*) FROM attempts WHERE {condition}"),
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn campaign(scenario: &str) -> BTreeSet<String> {
    let fixture = tempfile::tempdir().unwrap();
    prepare(fixture.path(), scenario);
    let baseline = tempfile::tempdir().unwrap();
    copy_fixture(fixture.path(), baseline.path());
    let (output, boundaries) = run(baseline.path(), scenario, 0);
    assert!(output.status.success(), "{scenario}: {output:?}");
    assert!(!boundaries.is_empty(), "no crash points in {scenario}");
    if scenario == "initialize" {
        for phase in ["before", "after"] {
            assert!(
                boundaries.contains(&format!("database.{phase} PRAGMA user_version = 1;")),
                "leading SQL comments must not hide the schema version write"
            );
        }
    }
    audit(baseline.path(), scenario);
    for target in 1..=boundaries.len() {
        let root = tempfile::tempdir().unwrap();
        copy_fixture(fixture.path(), root.path());
        let (output, observed) = run(root.path(), scenario, target);
        assert_eq!(
            output.status.code(),
            Some(86),
            "{scenario} boundary {target} {}: {output:?}",
            boundaries[target - 1]
        );
        assert_eq!(
            observed,
            boundaries[..target],
            "{scenario} boundary {target}"
        );
        audit(root.path(), scenario);
    }
    eprintln!("{scenario}: verified {} crash boundaries", boundaries.len());
    boundaries
        .iter()
        .map(|label| label.split_whitespace().next().unwrap().to_owned())
        .collect()
}

fn copy_fixture(source: &Path, destination: &Path) {
    // A closed store has no live WAL writer or kernel lease. Preserve its
    // generated staging names so every crash replay uses the same ordering.
    let mut entries = fs::read_dir(source)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir(&target).unwrap();
            copy_fixture(&entry.path(), &target);
        } else {
            assert!(entry.file_type().unwrap().is_file());
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn campaign_child() {
    let Ok(scenario) = std::env::var("METEWAND_CAMPAIGN_SCENARIO") else {
        return;
    };
    let covered = campaign(&scenario);
    fs::write(
        std::env::var_os("METEWAND_CAMPAIGN_COVERAGE").unwrap(),
        serde_json::to_vec(&covered).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "exhaustive crash campaign; see docs/reference/run-store-schema.md#crash-conformance"]
fn every_durability_boundary_survives_process_death_and_resume() {
    let scenarios = [
        "initialize",
        "accepted",
        "censored",
        "superseded",
        "failed",
        "recover-interrupted",
        "recover-orphan",
        "recover-artifact",
    ];
    // Separate processes prevent probe forks from inheriting locks held by
    // another campaign while allowing independent filesystem syncs to overlap.
    let covered: BTreeSet<String> = thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let scenarios = &scenarios;
                scope.spawn(move || {
                    let mut covered = BTreeSet::new();
                    for scenario in scenarios.iter().skip(worker).step_by(4) {
                        let coverage = tempfile::NamedTempFile::new().unwrap();
                        let output = Command::new(std::env::current_exe().unwrap())
                            .args([
                                "--exact",
                                "run_store::fault_tests::campaign_child",
                                "--nocapture",
                            ])
                            .env("METEWAND_CAMPAIGN_SCENARIO", scenario)
                            .env("METEWAND_CAMPAIGN_COVERAGE", coverage.path())
                            .output()
                            .unwrap();
                        assert!(output.status.success(), "{scenario}: {output:?}");
                        eprint!("{}", String::from_utf8_lossy(&output.stderr));
                        covered.extend(
                            serde_json::from_slice::<BTreeSet<String>>(
                                &fs::read(coverage.path()).unwrap(),
                            )
                            .unwrap(),
                        );
                    }
                    covered
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    for kind in [
        "database.open.before",
        "database.open.after",
        "database.before",
        "database.after",
        "file_sync.before",
        "file_sync.after",
        "directory_sync.before",
        "directory_sync.after",
        "rename.before",
        "rename.after",
        "finalization.before",
        "finalization.after",
        "finalization.outcome.before",
        "finalization.outcome.after",
        "finalization.projection.before",
        "finalization.projection.after",
    ] {
        assert!(covered.contains(kind), "missing {kind}");
    }
}

fn wait_for(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn wait_child(mut child: Child, expected: i32) {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(
                status.code(),
                Some(expected),
                "publisher exited with {status}"
            );
            return;
        }
        if start.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("publisher failed to finish");
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn concurrent_processes_converge_even_when_a_publisher_dies_after_rename() {
    let mut crash_boundary = 0;
    for crash in [false, true] {
        let root = tempfile::tempdir().unwrap();
        prepare(root.path(), "concurrent-1");
        let traces: Vec<_> = (0..2)
            .map(|_| tempfile::NamedTempFile::new().unwrap())
            .collect();
        let children: Vec<_> = (1..=2)
            .map(|index| {
                command(
                    root.path(),
                    &format!("concurrent-{index}"),
                    traces[index - 1].path(),
                    if crash && index == 1 {
                        crash_boundary
                    } else {
                        0
                    },
                )
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
            })
            .collect();
        for index in 1..=2 {
            wait_for(&root.path().join(format!("ready-{index}")));
        }
        fs::write(root.path().join("go-1"), b"").unwrap();
        if !crash {
            fs::write(root.path().join("go-2"), b"").unwrap();
        }
        for (index, child) in children.into_iter().enumerate() {
            wait_child(child, if crash && index == 0 { 86 } else { 0 });
            if crash && index == 0 {
                // The first writer must win and die before parent sync, so the
                // survivor exercises reuse of that unsynchronized publication.
                fs::write(root.path().join("go-2"), b"").unwrap();
            }
        }
        let trace = fs::read_to_string(traces[0].path()).unwrap();
        if !crash {
            crash_boundary = trace
                .lines()
                .position(|label| label == "rename.after")
                .unwrap()
                + 1;
        } else {
            assert_eq!(trace.lines().count(), crash_boundary);
            assert_eq!(trace.lines().last(), Some("rename.after"));
        }
        let mut store = RunStore::open(root.path()).unwrap();
        assert_eq!(count(&store.connection, "artifacts"), 1);
        assert_eq!(
            count(&store.connection, "leased_artifacts"),
            if crash { 1 } else { 2 }
        );
        let relative: String = store
            .connection
            .query_row("SELECT relative_path FROM artifacts", [], |r| r.get(0))
            .unwrap();
        let directory: PathBuf = root.path().join(relative);
        verify_finalized_directory(&directory).unwrap();
        assert_eq!(
            fs::read(directory.join("nested/payload")).unwrap(),
            b"verified bytes\0"
        );
        let visible = fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .filter(|entry| {
                !entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_str()
                    .unwrap()
                    .starts_with('.')
            })
            .count();
        assert_eq!(visible, 1);
        if crash {
            store.recover(|| 200).unwrap();
            verify_finalized_directory(&directory).unwrap();
            assert_eq!(count_where(&store.connection, "state = 'accepted'"), 0);
            assert_eq!(count_where(&store.connection, "finished_at_ns IS NULL"), 0);
        } else {
            assert_eq!(
                fs::read_dir(root.path().join("artifacts")).unwrap().count(),
                1
            );
        }
    }
}
