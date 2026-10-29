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
use rusqlite::{Connection, params};
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
    seed_kind(store, measurement, false);
}

fn seed_kind(store: &RunStore, measurement: bool, streaming: bool) {
    store
        .connection
        .execute_batch(
            "INSERT INTO applicability_decisions VALUES ('decision', 'candidate', 1, '{}');
         INSERT INTO resolved_runs VALUES ('run', 'logical', 'candidate', 'decision', 1, '{}');",
        )
        .unwrap();
    let version = store
        .connection
        .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
        .unwrap();
    let buffered = version == 2;
    for index in [1, 2] {
        store
            .connection
            .execute(
                "INSERT INTO attempt_slots VALUES (?1, ?1, 'run', ?3, ?2, '{}')",
                (
                    slot(index).to_string(),
                    if measurement { "measurement" } else { "warmup" },
                    if streaming {
                        "stream"
                    } else if buffered {
                        "buffered"
                    } else if version == 3 {
                        "fresh_observation"
                    } else {
                        "one_shot"
                    },
                ),
            )
            .unwrap();
    }
    if version == 3 && !streaming {
        let policy=json!({"metric":"gap","direction":"at_most","threshold":0,"max-observations":2,"max-active-time":"1 ms"}).to_string();
        store
            .connection
            .execute("INSERT INTO fresh_sequences VALUES ('run',0,?1)", [policy])
            .unwrap();
        for index in [1u8, 2] {
            store
                .connection
                .execute(
                    "INSERT INTO fresh_members VALUES (?1,'run',0,?2,?3)",
                    params![
                        slot(index).to_string(),
                        i64::from(index - 1),
                        json!({"iterations":index}).to_string()
                    ],
                )
                .unwrap();
        }
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
    let buffered = tx
        .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
        .unwrap()
        == 2;
    if buffered {
        tx.execute("INSERT OR IGNORE INTO observation_slots VALUES ('observation-slot-2','logical-2','run','{}')",[]).unwrap();
        tx.execute("INSERT OR IGNORE INTO attempt_observation_slots VALUES (?1,'observation-slot-2','run')",[slot(1).to_string()]).unwrap();
        tx.execute("INSERT INTO observations VALUES (?1,'observation-slot-2',?2,?3,1,'reached',?4,'observation','{}')",(format!("observation-2-{}",attempt.retry_index()),attempt.id().to_string(),slot(1).to_string(),&artifact.id)).unwrap();
    } else if tx
        .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
        .unwrap()
        != 3
    {
        tx.execute(
            "INSERT OR IGNORE INTO selected_observations VALUES ('observation-slot', ?1, 1, ?2)",
            (&id, now as i64),
        )
        .unwrap();
    }
    let streaming: bool = tx
        .query_row(
            "SELECT kind='stream' FROM attempt_slots WHERE id=?1",
            [attempt.slot().to_string()],
            |r| r.get(0),
        )
        .unwrap();
    if streaming {
        crate::fault_injection::hit("stream.checkpoint.before_commit");
        tx.execute("INSERT INTO events(attempt_id,occurred_at_ns,kind,payload_json) VALUES (?1,?2,'stream_checkpoint',?3)",params![attempt.id().to_string(),now as i64,json!({"observation_id":id,"sequence":0,"decision":"stop","reason":"fixed_count"}).to_string()]).unwrap();
    }
    tx.commit().unwrap();
    if streaming {
        crate::fault_injection::hit("stream.checkpoint.after_commit");
    }
    artifact.id
}

fn prepare(root: &Path, scenario: &str) {
    if scenario.starts_with("initialize") {
        return;
    }
    let mut store = if scenario.starts_with("fresh-") || scenario.starts_with("stream-") {
        RunStore::open_fresh(root)
    } else if scenario.starts_with("buffered-") {
        RunStore::open_buffered(root)
    } else {
        RunStore::open(root)
    }
    .unwrap();
    seed_kind(
        &store,
        !scenario.starts_with("concurrent"),
        scenario.starts_with("stream-"),
    );
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
    let fresh = scenario.starts_with("fresh-");
    let streaming = scenario.starts_with("stream-");
    let scenario = scenario
        .strip_prefix("buffered-")
        .or_else(|| scenario.strip_prefix("fresh-"))
        .or_else(|| scenario.strip_prefix("stream-"))
        .unwrap_or(scenario);
    let mut attempt = reserve(store);
    if fresh {
        store.reserve_fresh_time(&attempt, None, 10).unwrap();
    }
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
    if streaming {
        // Recovery must not infer delivery or acknowledgment from the saved decision.
        crate::fault_injection::hit("stream.decision.before_send");
        crate::fault_injection::hit("stream.decision.after_receive");
    }
    if fresh {
        store
            .record_fresh_time(
                &attempt,
                if scenario == "censored" {
                    1_000_000
                } else {
                    25
                },
                22,
            )
            .unwrap();
    }
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
    if scenario.starts_with("initialize") {
        arm(trace, target);
        if scenario == "initialize-fresh" {
            RunStore::open_fresh(Path::new(&root)).unwrap();
        } else if scenario == "initialize-buffered" {
            RunStore::open_buffered(Path::new(&root)).unwrap();
        } else {
            RunStore::open(Path::new(&root)).unwrap();
        }
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
    if scenario.starts_with("initialize") {
        seed(&store, true);
    }
    let successes = published_successes(root);
    let before_selections = selections(&store.connection);
    let accepted_before = count_where(&store.connection, "state = 'accepted'");
    store.recover(|| 200).unwrap();
    if store
        .connection
        .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
        .unwrap()
        >= 2
    {
        let after = selections(&store.connection);
        assert!(
            before_selections
                .iter()
                .all(|selection| after.contains(selection))
        );
        let invalid:i64=store.connection.query_row("SELECT count(*) FROM selected_observations s JOIN observations o ON o.id=s.observation_id JOIN attempts a ON a.id=o.producing_attempt_id WHERE a.state!='accepted'",[],|r|r.get(0)).unwrap();
        assert_eq!(
            invalid, 0,
            "buffered selection must commit with its accepted attempt"
        );
    } else {
        assert_eq!(selections(&store.connection), before_selections);
    }
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
    let mut tables = vec![
        "events",
        "attempt_transitions",
        "attempts",
        "artifacts",
        "attempt_artifacts",
        "leases",
        "observations",
        "selected_observations",
    ];
    if super::fresh::supported(&store.connection).unwrap()
        && count(&store.connection, "fresh_sequences") > 0
    {
        tables.extend([
            "fresh_sequences",
            "fresh_members",
            "fresh_budget",
            "fresh_dispositions",
        ]);
        let progress = store.fresh_progress().unwrap();
        assert_eq!(progress.len(), 1);
        assert!(progress[0].consumed_time_ns <= 1_000_000);
        if !successes.is_empty() {
            assert_eq!(progress[0].status, "accepted");
            let skipped: i64 = store
                .connection
                .query_row(
                    "SELECT count(*) FROM fresh_dispositions WHERE reason='skipped_target_reached'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(skipped, 1);
        }
        let unknown: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM fresh_budget WHERE charged_ns IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if unknown > 0 {
            assert_eq!(progress[0].conservative_time_ns, 1_000_000);
            assert_eq!(progress[0].status, "censored");
        }
    }
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
    let stopped = super::fresh::supported(&store.connection).unwrap()
        && matches!(reservation, Err(RunStoreError::SatisfiedSlot));
    if accepted == 1 || stopped {
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
    assert_eq!(
        count_where(&store.connection, "state = 'accepted'"),
        if stopped { accepted } else { 1 }
    );
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
    if scenario.starts_with("initialize") {
        let version = if scenario == "initialize-fresh" {
            3
        } else if scenario == "initialize-buffered" {
            2
        } else {
            1
        };
        for phase in ["before", "after"] {
            assert!(
                boundaries.contains(&format!(
                    "database.{phase} PRAGMA user_version = {version};"
                )),
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
#[ignore = "exhaustive crash campaign; see CONTRIBUTING.md#crash-conformance"]
fn every_durability_boundary_survives_process_death_and_resume() {
    let scenarios = [
        "initialize",
        "initialize-buffered",
        "initialize-fresh",
        "fresh-accepted",
        "fresh-censored",
        "fresh-failed",
        "buffered-accepted",
        "buffered-censored",
        "stream-accepted",
        "stream-censored",
        "stream-failed",
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
        #[cfg(unix)]
        "directory_sync.before",
        #[cfg(unix)]
        "directory_sync.after",
        #[cfg(windows)]
        "namespace_change.before",
        #[cfg(windows)]
        "namespace_change.after",
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

#[test]
fn buffered_selection_commits_with_the_terminal_attempt_and_survives_recovery() {
    let fixture = tempfile::tempdir().unwrap();
    prepare(fixture.path(), "buffered-accepted");
    let baseline = tempfile::tempdir().unwrap();
    copy_fixture(fixture.path(), baseline.path());
    let (output, boundaries) = run(baseline.path(), "buffered-accepted", 0);
    assert!(output.status.success(), "{output:?}");
    for boundary in [
        "finalization.outcome.after",
        "finalization.projection.before",
        "finalization.after",
    ] {
        let target = boundaries.iter().position(|p| p == boundary).unwrap() + 1;
        let root = tempfile::tempdir().unwrap();
        copy_fixture(fixture.path(), root.path());
        let (output, _) = run(root.path(), "buffered-accepted", target);
        assert_eq!(output.status.code(), Some(86));
        audit(root.path(), "buffered-accepted");
    }
}

#[test]
fn fresh_sequence_budget_and_target_survive_publication_crashes() {
    for scenario in ["fresh-accepted", "fresh-censored"] {
        let fixture = tempfile::tempdir().unwrap();
        prepare(fixture.path(), scenario);
        let baseline = tempfile::tempdir().unwrap();
        copy_fixture(fixture.path(), baseline.path());
        let (output, boundaries) = run(baseline.path(), scenario, 0);
        assert!(output.status.success(), "{output:?}");
        audit(baseline.path(), scenario);
        for needle in [
            "INSERT INTO fresh_budget",
            "UPDATE fresh_budget",
            "INSERT INTO fresh_dispositions",
            "finalization.projection.after",
        ] {
            let target = boundaries.iter().position(|b| b.contains(needle)).unwrap() + 1;
            let root = tempfile::tempdir().unwrap();
            copy_fixture(fixture.path(), root.path());
            let (output, _) = run(root.path(), scenario, target);
            assert!(!output.status.success());
            audit(root.path(), scenario);
        }
    }
}

#[test]
fn fresh_time_reservation_refunds_only_on_terminal_commit() {
    let root = tempfile::tempdir().unwrap();
    let mut store = RunStore::open_fresh(root.path()).unwrap();
    seed(&store, true);
    let attempt = reserve(&mut store);
    assert_eq!(
        store.reserve_fresh_time(&attempt, None, 10).unwrap(),
        Some(1_000_000)
    );
    store.record_fresh_time(&attempt, 25, 11).unwrap();
    let progress = store.fresh_progress().unwrap().remove(0);
    assert_eq!(progress.consumed_time_ns, 1_000_000);
    assert_eq!(progress.conservative_time_ns, 999_975);
    assert!(matches!(
        store.reserve(
            slot(2),
            RetryAuthorization::FirstAttempt,
            lease("competing", 100),
            11
        ),
        Err(RunStoreError::SequenceBusy)
    ));
    let attempt = store
        .advance(&attempt, AttemptPhase::Finalizing, 12)
        .unwrap();
    store
        .publish_finalization(
            &attempt,
            TerminalOutcome::Failed {
                code: "failed_point".into(),
                details: CanonicalValue::try_from(json!({})).unwrap(),
            },
            &[],
            payload,
            || 20,
        )
        .unwrap();
    drop(store);
    let mut store = RunStore::open(root.path()).unwrap();
    let progress = store.fresh_progress().unwrap().remove(0);
    assert_eq!(progress.consumed_time_ns, 25);
    assert_eq!(progress.conservative_time_ns, 0);
    let second = store
        .reserve(
            slot(2),
            RetryAuthorization::FirstAttempt,
            lease("second", 100),
            30,
        )
        .unwrap();
    assert_eq!(
        store.reserve_fresh_time(&second, None, 30).unwrap(),
        Some(999_975)
    );
    store.recover(|| 200).unwrap();
    let progress = store.fresh_progress().unwrap().remove(0);
    assert_eq!(progress.status, "censored");
    assert_eq!(progress.consumed_time_ns, 1_000_000);
    assert_eq!(progress.conservative_time_ns, 999_975);
    assert!(store.recover(|| 201).unwrap().terminals.is_empty());
}

#[test]
fn streaming_publication_and_decisions_survive_process_death_without_splicing() {
    let fixture = tempfile::tempdir().unwrap();
    prepare(fixture.path(), "stream-accepted");
    let baseline = tempfile::tempdir().unwrap();
    copy_fixture(fixture.path(), baseline.path());
    let (output, boundaries) = run(baseline.path(), "stream-accepted", 0);
    assert!(output.status.success(), "{output:?}");
    for boundary in [
        "stream.checkpoint.before_commit",
        "stream.checkpoint.after_commit",
        "stream.decision.before_send",
        "stream.decision.after_receive",
        "finalization.outcome.after",
        "finalization.projection.before",
        "finalization.after",
    ] {
        let target = boundaries.iter().position(|p| p == boundary).unwrap() + 1;
        let root = tempfile::tempdir().unwrap();
        copy_fixture(fixture.path(), root.path());
        let (output, _) = run(root.path(), "stream-accepted", target);
        assert_eq!(output.status.code(), Some(86), "{boundary}: {output:?}");
        audit(root.path(), "stream-accepted");
        let store = RunStore::open(root.path()).unwrap();
        let producers:i64 = store.connection.query_row("SELECT count(DISTINCT o.producing_attempt_id) FROM selected_observations s JOIN observations o ON o.id=s.observation_id",[],|r|r.get(0)).unwrap();
        assert_eq!(producers, 1);
    }
}
