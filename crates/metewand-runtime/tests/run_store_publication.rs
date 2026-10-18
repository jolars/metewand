#![cfg(unix)]

use std::{fs, io, path::Path};

use metewand_core::records::{RecordId, ResolvedAttemptSlotRecord};
use metewand_runtime::{
    local_tree::hash_local_tree,
    publication::verify_finalized_directory,
    run_store::{
        ArtifactKind, AttemptPhase, LeaseRequest, RetryAuthorization, RunStore, TerminalOutcome,
    },
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn setup() -> (TempDir, RunStore, Connection) {
    setup_role("warmup")
}

fn setup_role(role: &str) -> (TempDir, RunStore, Connection) {
    let root = TempDir::new().unwrap();
    let store = RunStore::open(root.path()).unwrap();
    let db = Connection::open(store.database_path()).unwrap();
    db.execute_batch(
        "PRAGMA foreign_keys = ON; PRAGMA recursive_triggers = ON;
        INSERT INTO applicability_decisions VALUES ('decision', 'candidate', 1, '{}');
        INSERT INTO resolved_runs VALUES ('run', 'logical', 'candidate', 'decision', 1, '{}');",
    )
    .unwrap();
    db.execute(
        "INSERT INTO attempt_slots VALUES (?1, 'logical', 'run', 'one_shot', ?2, '{}')",
        (slot().to_string(), role),
    )
    .unwrap();
    (root, store, db)
}

fn slot() -> RecordId<ResolvedAttemptSlotRecord> {
    RecordId::from_digest([1; 32])
}

fn reserve(store: &mut RunStore) -> metewand_runtime::run_store::OwnedAttempt {
    store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            LeaseRequest {
                id: "reservation".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            10,
        )
        .unwrap()
}

fn payload(path: &Path) -> io::Result<()> {
    fs::create_dir(path.join("nested"))?;
    fs::write(path.join("nested/payload"), b"data\0")
}

fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn terminal_publication_binds_exact_durable_bytes_and_commits_all_roots() {
    let (root, mut store, db) = setup();
    let attempt = reserve(&mut store);
    let dependency = store
        .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 11)
        .unwrap();
    let attempt = store
        .begin_finalization(
            &attempt,
            LeaseRequest {
                id: "claim".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            12,
        )
        .unwrap()
        .attempt;
    let published = store
        .publish_finalization(
            &attempt,
            TerminalOutcome::Accepted,
            &[("dataset".into(), dependency.artifact.id.clone())],
            payload,
            || 20,
        )
        .unwrap();
    let directory = root.path().join(&published.artifact.relative_path);
    verify_finalized_directory(&directory).unwrap();
    assert_eq!(
        published.artifact.content_sha256,
        hash_local_tree(&directory).unwrap()
    );
    assert_eq!(
        published.artifact.manifest_sha256.bytes().as_slice(),
        Sha256::digest(fs::read(directory.join("manifest.json")).unwrap()).as_slice()
    );
    assert_eq!(
        fs::read(directory.join("nested/payload")).unwrap(),
        b"data\0"
    );
    let document = fs::read_to_string(directory.join("outcome.json")).unwrap();
    assert!(document.contains(&format!("\"attempt_id\":\"{}\"", attempt.id())));
    assert!(document.contains("\"state\":\"accepted\""));
    assert_eq!(published.terminal.id, attempt.id());
    assert_eq!(published.terminal.finished_at_ns, 20);
    assert_eq!(count(&db, "artifacts"), 2);
    assert_eq!(count(&db, "attempt_artifacts"), 2);
    assert_eq!(count(&db, "finalization_claims"), 0);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM leases WHERE released_at_ns IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT final_artifact_id FROM attempts", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        published.artifact.id
    );
    drop(store);
    RunStore::open(root.path()).unwrap();
    assert_eq!(count(&db, "events"), 3);
}

#[test]
fn an_expired_owner_after_rename_leaves_an_orphan_without_terminal_database_state() {
    let (root, mut store, db) = setup();
    let attempt = reserve(&mut store);
    let attempt = store
        .advance(&attempt, AttemptPhase::Finalizing, 11)
        .unwrap();
    let mut times = [20, 20, 100].into_iter();
    assert!(
        store
            .publish_finalization(&attempt, TerminalOutcome::Superseded, &[], payload, || {
                times.next().unwrap()
            })
            .is_err()
    );
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "events"), 2);
    assert_eq!(
        db.query_row("SELECT state FROM attempts", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "finalizing"
    );
    let objects: Vec<_> = fs::read_dir(root.path().join("artifacts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| !p.file_name().unwrap().to_str().unwrap().starts_with('.'))
        .collect();
    assert_eq!(objects.len(), 1);
    verify_finalized_directory(&objects[0]).unwrap();
}

#[test]
fn unsafe_or_caller_supplied_completion_entries_never_get_database_references() {
    for case in ["symlink", "socket", "empty", "manifest", "outcome"] {
        let (_root, mut store, db) = setup();
        let attempt = reserve(&mut store);
        let attempt = store
            .advance(&attempt, AttemptPhase::Finalizing, 11)
            .unwrap();
        let result = store.publish_finalization(
            &attempt,
            TerminalOutcome::Superseded,
            &[],
            |path| {
                match case {
                    "symlink" => std::os::unix::fs::symlink("/etc/passwd", path.join("link"))?,
                    "socket" => {
                        std::os::unix::net::UnixListener::bind(path.join("socket"))?;
                    }
                    "empty" => fs::create_dir(path.join("empty"))?,
                    "manifest" => fs::write(path.join("manifest.json"), b"{}")?,
                    "outcome" => fs::write(path.join("outcome.json"), b"{}")?,
                    _ => unreachable!(),
                }
                Ok(())
            },
            || 20,
        );
        assert!(result.is_err(), "{case}");
        assert_eq!(count(&db, "artifacts"), 0);
        assert_eq!(count(&db, "events"), 2);
    }
}

#[test]
fn coordination_outcome_has_canonical_versioned_bytes_and_one_immutable_finish_time() {
    let (root, mut store, db) = setup();
    let attempt = reserve(&mut store);
    let attempt = store
        .begin_finalization(
            &attempt,
            LeaseRequest {
                id: "claim".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            11,
        )
        .unwrap()
        .attempt;
    let mut times = [20, 21, 22].into_iter();
    let published = store
        .publish_finalization(&attempt, TerminalOutcome::Accepted, &[], payload, || {
            times.next().unwrap()
        })
        .unwrap();
    let document = fs::read_to_string(
        root.path()
            .join(&published.artifact.relative_path)
            .join("outcome.json"),
    )
    .unwrap();
    let expected = include_str!("../src/run_store/fixtures/outcome-v1.json").trim_end();
    assert_eq!(document, expected);
    assert_eq!(published.terminal.finished_at_ns, 21);
    assert_eq!(published.artifact.verified_at_ns, 22);
    assert_eq!(
        db.query_row("SELECT min(released_at_ns) FROM leases", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        22
    );
    assert!(!document.contains("owner"));
    assert!(!document.contains("staging"));
}

#[test]
fn every_terminal_outcome_survives_publication_and_preserves_diagnostics() {
    use metewand_core::canonical::CanonicalValue;
    use serde_json::json;
    for (outcome, state, details) in [
        (TerminalOutcome::Accepted, "accepted", json!({})),
        (TerminalOutcome::Superseded, "superseded", json!({})),
        (
            TerminalOutcome::Censored {
                reason: "bound".into(),
            },
            "censored",
            json!({"reason":"bound"}),
        ),
        (
            TerminalOutcome::Failed {
                code: "worker_crash".into(),
                details: CanonicalValue::try_from(json!({"message":"worker exited"})).unwrap(),
            },
            "failed",
            json!({"failure_code":"worker_crash","diagnostic":{"message":"worker exited"}}),
        ),
    ] {
        let (root, mut store, db) = setup();
        let attempt = reserve(&mut store);
        let attempt = store
            .begin_finalization(
                &attempt,
                LeaseRequest {
                    id: "claim".into(),
                    owner_token: "owner".into(),
                    expires_at_ns: 100,
                },
                11,
            )
            .unwrap()
            .attempt;
        let published = store
            .publish_finalization(&attempt, outcome, &[], payload, || 20)
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join(&published.artifact.relative_path)
                    .join("outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(document["state"], state);
        assert_eq!(document["details"], details);
        assert_eq!(
            db.query_row("SELECT state FROM attempts", [], |r| r.get::<_, String>(0))
                .unwrap(),
            state
        );
        assert_eq!(count(&db, "finalization_claims"), 0);
    }
}

#[test]
fn current_epoch_nanoseconds_remain_exact_in_the_canonical_outcome() {
    let (root, mut store, _db) = setup();
    let now = 1_792_349_520_000_000_123;
    let attempt = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            LeaseRequest {
                id: "reservation".into(),
                owner_token: "owner".into(),
                expires_at_ns: now + 100,
            },
            now,
        )
        .unwrap();
    let attempt = store
        .advance(&attempt, AttemptPhase::Finalizing, now + 1)
        .unwrap();
    let published = store
        .publish_finalization(&attempt, TerminalOutcome::Superseded, &[], payload, || {
            now + 2
        })
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join(&published.artifact.relative_path)
                .join("outcome.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(document["started_at_ns"], now.to_string());
    assert_eq!(document["finished_at_ns"], (now + 2).to_string());
}

#[test]
fn late_sql_failure_rolls_back_completion_but_keeps_a_verified_orphan() {
    let (root, mut store, db) = setup();
    let attempt = reserve(&mut store);
    let attempt = store
        .begin_finalization(
            &attempt,
            LeaseRequest {
                id: "claim".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            11,
        )
        .unwrap()
        .attempt;
    db.execute_batch("CREATE TRIGGER fail_release BEFORE UPDATE OF released_at_ns ON leases BEGIN SELECT RAISE(ABORT, 'injected release failure'); END;").unwrap();
    let mut directory = None;
    let result = store.publish_finalization(
        &attempt,
        TerminalOutcome::Accepted,
        &[],
        |path| {
            directory = Some(path.parent().unwrap().to_owned());
            payload(path)
        },
        || 20,
    );
    assert!(result.is_err());
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "events"), 2);
    assert_eq!(count(&db, "finalization_claims"), 1);
    assert!(directory.unwrap().exists());
    let object = fs::read_dir(root.path().join("artifacts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| !p.file_name().unwrap().to_str().unwrap().starts_with('.'))
        .unwrap();
    verify_finalized_directory(&object).unwrap();
}

#[test]
fn staging_is_locked_leased_and_on_the_destination_filesystem_outside_database_transactions() {
    use std::os::unix::fs::MetadataExt;
    let (root, mut store, db) = setup();
    let attempt = reserve(&mut store);
    store.publish_artifact(&attempt, ArtifactKind::Other, |path| {
        assert_eq!(fs::metadata(path)?.dev(), fs::metadata(root.path())?.dev());
        let stage = path.parent().unwrap();
        let lock = fs::File::open(stage.join("lock"))?;
        assert!(matches!(lock.try_lock(), Err(fs::TryLockError::WouldBlock)));
        let relative = stage.strip_prefix(root.path()).unwrap().to_str().unwrap();
        assert_eq!(db.query_row("SELECT count(*) FROM leases WHERE purpose = 'staging' AND staging_path = ?1 AND released_at_ns IS NULL", [relative], |r| r.get::<_, i64>(0)).unwrap(), 1);
        // A second connection can commit while population is still in progress.
        db.execute("INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (11, 'test', '{}')", []).unwrap();
        assert_eq!(count(&db, "artifacts"), 0);
        payload(path)
    }, || 11).unwrap();
}

#[test]
fn concurrent_identical_content_has_one_object_and_two_committed_lease_roots() {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };
    let (root, mut store, db) = setup();
    let first = reserve(&mut store);
    let second = store
        .reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 1,
                max_index: 1,
            },
            LeaseRequest {
                id: "second".into(),
                owner_token: "other-owner".into(),
                expires_at_ns: 100,
            },
            10,
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let writers: Vec<_> = [first, second]
        .into_iter()
        .map(|attempt| {
            let mut store = RunStore::open(root.path()).unwrap();
            let barrier = barrier.clone();
            thread::spawn(move || {
                store
                    .publish_artifact(
                        &attempt,
                        ArtifactKind::Dataset,
                        |path| {
                            payload(path)?;
                            barrier.wait();
                            Ok(())
                        },
                        || 20,
                    )
                    .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results[0].artifact.id, results[1].artifact.id);
    assert_eq!(count(&db, "artifacts"), 1);
    assert_eq!(count(&db, "leased_artifacts"), 2);
    assert_eq!(
        fs::read_dir(root.path().join("artifacts")).unwrap().count(),
        1
    );
    verify_finalized_directory(&root.path().join(&results[0].artifact.relative_path)).unwrap();
}

#[test]
fn backward_clock_samples_cannot_predate_the_staging_lease() {
    let (root, mut store, _db) = setup();
    let attempt = reserve(&mut store);
    let attempt = store
        .advance(&attempt, AttemptPhase::Finalizing, 11)
        .unwrap();
    let mut times = [20, 19, 18].into_iter();
    let published = store
        .publish_finalization(&attempt, TerminalOutcome::Superseded, &[], payload, || {
            times.next().unwrap()
        })
        .unwrap();
    assert_eq!(published.terminal.finished_at_ns, 20);
    assert_eq!(published.artifact.verified_at_ns, 20);
    verify_finalized_directory(&root.path().join(published.artifact.relative_path)).unwrap();
}

#[test]
fn owner_renewal_extends_live_staging_without_holding_a_publication_transaction() {
    let (root, mut store, _db) = setup();
    let attempt = reserve(&mut store);
    let mut second = RunStore::open(root.path()).unwrap();
    let mut times = [20, 110].into_iter();
    store
        .publish_artifact(
            &attempt,
            ArtifactKind::Other,
            |path| {
                payload(path)?;
                second.renew(&attempt, 200, 99).unwrap();
                Ok(())
            },
            || times.next().unwrap(),
        )
        .unwrap();
}

#[test]
fn corrupt_existing_content_is_preserved_and_never_registered_again() {
    let (root, mut store, db) = setup();
    let attempt = reserve(&mut store);
    let first = store
        .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 20)
        .unwrap();
    let directory = root.path().join(&first.artifact.relative_path);
    fs::write(directory.join("nested/payload"), b"corrupt").unwrap();
    assert!(
        store
            .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 21)
            .is_err()
    );
    assert_eq!(
        fs::read(directory.join("nested/payload")).unwrap(),
        b"corrupt"
    );
    assert_eq!(count(&db, "artifacts"), 1);
    assert_eq!(count(&db, "leased_artifacts"), 1);
}

#[test]
fn stale_guards_missing_claims_and_missing_dependencies_cannot_publish_acceptance() {
    for case in ["stale", "no_claim", "missing_dependency"] {
        let (root, mut store, db) = setup();
        let attempt = reserve(&mut store);
        let original = attempt.clone();
        let attempt = if case == "no_claim" {
            store
                .advance(&attempt, AttemptPhase::Finalizing, 11)
                .unwrap()
        } else {
            store
                .begin_finalization(
                    &attempt,
                    LeaseRequest {
                        id: "claim".into(),
                        owner_token: "owner".into(),
                        expires_at_ns: 100,
                    },
                    11,
                )
                .unwrap()
                .attempt
        };
        let references = if case == "missing_dependency" {
            vec![("dataset".into(), "missing".into())]
        } else {
            vec![]
        };
        let guard = if case == "stale" { &original } else { &attempt };
        assert!(
            store
                .publish_finalization(
                    guard,
                    TerminalOutcome::Accepted,
                    &references,
                    payload,
                    || 20
                )
                .is_err()
        );
        assert_eq!(count(&db, "artifacts"), 0);
        assert_eq!(count(&db, "events"), 2);
        assert!(
            fs::read_dir(root.path().join("artifacts"))
                .unwrap()
                .all(|e| e.unwrap().file_name().to_str().unwrap().starts_with('.'))
        );
    }
}

#[test]
fn the_last_clock_sample_occurs_after_acquiring_the_final_database_write_lock() {
    let (_root, mut store, db) = setup();
    let attempt = reserve(&mut store);
    db.busy_timeout(std::time::Duration::ZERO).unwrap();
    let mut samples = 0;
    store
        .publish_artifact(&attempt, ArtifactKind::Other, payload, || {
            samples += 1;
            if samples == 2 {
                let error = db.execute_batch("BEGIN IMMEDIATE").unwrap_err();
                assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
            }
            20
        })
        .unwrap();
    assert_eq!(samples, 2);
}

#[test]
fn measured_acceptance_requires_published_observations_and_preserves_their_snapshot() {
    let (root, mut store, db) = setup_role("measurement");
    db.execute_batch("INSERT INTO observation_slots VALUES ('observation-slot', 'logical-observation', 'run', '{}')").unwrap();
    db.execute(
        "INSERT INTO attempt_observation_slots VALUES (?1, 'observation-slot', 'run')",
        [slot().to_string()],
    )
    .unwrap();
    let attempt = reserve(&mut store);
    let attempt = store
        .begin_finalization(
            &attempt,
            LeaseRequest {
                id: "claim".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            11,
        )
        .unwrap()
        .attempt;
    assert!(
        store
            .publish_finalization(&attempt, TerminalOutcome::Accepted, &[], payload, || 20)
            .is_err()
    );
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "finalization_claims"), 1);
    let observation = store
        .publish_artifact(&attempt, ArtifactKind::Observation, payload, || 21)
        .unwrap();
    db.execute("INSERT INTO observations VALUES ('observation', 'observation-slot', ?1, ?2, 1, 'reached', ?3, 'observation', '{}')",
        (attempt.id().to_string(), slot().to_string(), &observation.artifact.id)).unwrap();
    db.execute_batch(
        "INSERT INTO selected_observations VALUES ('observation-slot', 'observation', 1, 21)",
    )
    .unwrap();
    let published = store
        .publish_finalization(
            &attempt,
            TerminalOutcome::Accepted,
            &[("observation".into(), observation.artifact.id.clone())],
            payload,
            || 22,
        )
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join(published.artifact.relative_path)
                .join("outcome.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let recorded = &document["coordination"]["observations"][0];
    assert_eq!(recorded["id"], "observation");
    assert_eq!(recorded["producing_attempt_id"], attempt.id().to_string());
    assert_eq!(recorded["completion"], "reached");
    assert_eq!(recorded["artifact_id"], observation.artifact.id);
    assert_eq!(recorded["selected_at_ns"], "21");
    assert_eq!(count(&db, "selected_observations"), 1);
    assert_eq!(count(&db, "artifacts"), 2);
}
