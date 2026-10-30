#![cfg(any(unix, windows))]

use std::{
    fs, io,
    path::Path,
    sync::{Mutex, MutexGuard, PoisonError},
};

use crate::core::records::{RecordId, ResolvedAttemptSlotRecord};
use crate::runtime::run_store::{
    ArtifactKind, AttemptPhase, LeaseRequest, OwnedAttempt, RetryAuthorization, RunStore,
    RunStoreError, TerminalOutcome,
};
use rusqlite::Connection;
use tempfile::TempDir;

static PROBE_ISOLATION: Mutex<()> = Mutex::new(());

fn isolate_probes() -> MutexGuard<'static, ()> {
    // Preflight forks briefly inherit every live file descriptor in this
    // process. Isolate unrelated cases so those children cannot retain another
    // case's coordination or staging locks. Concurrent-owner cases still
    // create their own competing threads while holding this guard.
    PROBE_ISOLATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn slot() -> RecordId<ResolvedAttemptSlotRecord> {
    RecordId::from_digest([1; 32])
}

fn setup() -> (TempDir, RunStore, Connection, OwnedAttempt) {
    setup_role("warmup")
}

fn setup_role(role: &str) -> (TempDir, RunStore, Connection, OwnedAttempt) {
    let root = TempDir::new().unwrap();
    let mut store = RunStore::open(root.path()).unwrap();
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
    let attempt = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("reservation", 100),
            10,
        )
        .unwrap();
    (root, store, db, attempt)
}

fn lease(id: &str, expires_at_ns: u64) -> LeaseRequest {
    LeaseRequest {
        id: id.into(),
        owner_token: "owner".into(),
        expires_at_ns,
    }
}

fn state(db: &Connection) -> String {
    db.query_row(
        "SELECT state FROM attempts WHERE retry_index = 0",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn payload(path: &Path) -> io::Result<()> {
    fs::write(path.join("data"), b"verified bytes")
}

fn orphan(store: &mut RunStore, attempt: &OwnedAttempt, outcome: TerminalOutcome) {
    let mut times = [20, 21, 100].into_iter();
    assert!(
        store
            .publish_finalization(attempt, outcome, &[], payload, || times.next().unwrap())
            .is_err()
    );
}

#[test]
fn expired_reservations_fail_durably_and_require_an_explicit_new_retry() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    assert!(store.recover(|| 99).unwrap().terminals.is_empty());
    assert_eq!(state(&db), "reserved");
    let recovered = store.recover(|| 100).unwrap();
    assert_eq!(recovered.terminals.len(), 1);
    assert!(
        matches!(&recovered.terminals[0].outcome, TerminalOutcome::Failed { code, .. } if code == "interrupted_attempt")
    );
    assert_eq!(state(&db), "failed");
    assert_eq!(count(&db, "attempt_transitions"), 3);
    assert!(matches!(
        store.advance(&attempt, AttemptPhase::Finalizing, 101),
        Err(RunStoreError::StaleAttempt)
    ));
    let final_path: String = db
        .query_row(
            "SELECT relative_path FROM artifacts WHERE kind = 'attempt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let directory = root.path().join(final_path);
    crate::runtime::publication::verify_finalized_directory(&directory).unwrap();
    assert_eq!(
        fs::read_to_string(directory.join("outcome.json")).unwrap(),
        include_str!("../../src/runtime/run_store/fixtures/recovered-failure-v1.json").trim_end()
    );
    assert!(store.recover(|| 101).unwrap().terminals.is_empty());
    assert!(
        store
            .reserve(
                slot(),
                RetryAuthorization::FirstAttempt,
                lease("invalid", 200),
                101
            )
            .is_err()
    );
    assert_eq!(
        store
            .reserve(
                slot(),
                RetryAuthorization::Retry {
                    next_index: 1,
                    max_index: 1
                },
                lease("retry", 200),
                101
            )
            .unwrap()
            .retry_index(),
        1
    );
}

#[test]
fn a_completed_claimed_orphan_is_imported_once_and_satisfies_its_slot() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 100), 11)
        .unwrap()
        .attempt;
    orphan(&mut store, &attempt, TerminalOutcome::Accepted);
    drop(store);
    let mut store = RunStore::open(root.path()).unwrap();
    assert_eq!(state(&db), "finalizing");
    let recovered = store.recover(|| 100).unwrap();
    assert_eq!(recovered.terminals.len(), 1);
    assert!(matches!(
        recovered.terminals[0].outcome,
        TerminalOutcome::Accepted
    ));
    assert_eq!(recovered.terminals[0].finished_at_ns, 21);
    assert_eq!(state(&db), "accepted");
    assert_eq!(count(&db, "finalization_claims"), 0);
    assert_eq!(count(&db, "artifacts"), 1);
    assert!(store.recover(|| 101).unwrap().terminals.is_empty());
    assert!(matches!(
        store.reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 1,
                max_index: 1
            },
            lease("retry", 200),
            101
        ),
        Err(RunStoreError::SatisfiedSlot)
    ));
}

#[test]
fn abandoned_stages_are_quarantined_but_complete_staging_never_implies_success() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 100), 11)
        .unwrap()
        .attempt;
    assert!(
        store
            .publish_finalization(
                &attempt,
                TerminalOutcome::Accepted,
                &[],
                |path| {
                    payload(path)?;
                    fs::write(path.join("manifest.json"), b"{\"complete\":true}")?;
                    Err(io::Error::other("interrupted"))
                },
                || 20
            )
            .is_err()
    );
    let report = store.recover(|| 100).unwrap();
    assert_eq!(state(&db), "failed");
    assert_eq!(report.quarantined_stages.len(), 1);
    assert!(
        root.path()
            .join(&report.quarantined_stages[0])
            .join("object/data")
            .is_file()
    );
    assert_eq!(count(&db, "finalization_claims"), 0);
}

#[test]
fn recovery_skips_locked_stages_even_when_their_owner_has_expired() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let mut recovery = RunStore::open(root.path()).unwrap();
    assert!(
        store
            .publish_artifact(
                &attempt,
                ArtifactKind::Dataset,
                |path| {
                    payload(path)?;
                    let report = recovery.recover(|| 100).unwrap();
                    assert!(report.terminals.is_empty());
                    assert_eq!(report.busy_stages.len(), 1);
                    assert!(path.exists());
                    assert_eq!(state(&db), "reserved");
                    Err(io::Error::other("owner stopped"))
                },
                || 20
            )
            .is_err()
    );
    assert_eq!(recovery.recover(|| 100).unwrap().terminals.len(), 1);
}

#[test]
fn an_expired_claim_is_resolved_even_while_its_reservation_is_live() {
    let _isolation = isolate_probes();
    let (_root, mut store, db, attempt) = setup();
    store
        .begin_finalization(&attempt, lease("claim", 30), 11)
        .unwrap();
    assert!(store.recover(|| 29).unwrap().terminals.is_empty());
    assert_eq!(store.recover(|| 30).unwrap().terminals.len(), 1);
    assert_eq!(state(&db), "failed");
    assert_eq!(count(&db, "finalization_claims"), 0);
}

#[test]
fn corrupted_orphan_is_rejected_without_manufacturing_acceptance() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 100), 11)
        .unwrap()
        .attempt;
    orphan(&mut store, &attempt, TerminalOutcome::Accepted);
    let completed = fs::read_dir(root.path().join("artifacts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("attempt-")
        })
        .unwrap();
    fs::write(completed.join("data"), b"corruption").unwrap();
    let recovered = store.recover(|| 100).unwrap();
    assert_eq!(state(&db), "failed");
    assert_eq!(recovered.rejected_artifacts.len(), 1);
    assert_eq!(count(&db, "finalization_claims"), 0);
}

#[test]
fn independent_orphans_register_bytes_without_selecting_observations() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let mut times = [20, 100].into_iter();
    assert!(
        store
            .publish_artifact(&attempt, ArtifactKind::Observation, payload, || times
                .next()
                .unwrap())
            .is_err()
    );
    let report = store.recover(|| 100).unwrap();
    assert_eq!(report.imported_artifacts.len(), 1);
    assert_eq!(count(&db, "observations"), 0);
    assert_eq!(count(&db, "selected_observations"), 0);
    assert_eq!(state(&db), "failed");
    assert_eq!(count(&db, "artifacts"), 2);
    assert!(
        root.path()
            .join(&report.imported_artifacts[0].relative_path)
            .is_dir()
    );
}

fn completed_attempt(root: &Path) -> std::path::PathBuf {
    fs::read_dir(root.join("artifacts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("attempt-")
        })
        .unwrap()
}

fn variant(root: &Path, change: impl FnOnce(&mut serde_json::Value)) -> std::path::PathBuf {
    use crate::core::canonical::CanonicalValue;
    use sha2::{Digest, Sha256};
    let source = completed_attempt(root);
    let stage = TempDir::new_in(root).unwrap();
    for entry in fs::read_dir(&source).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), stage.path().join(entry.file_name())).unwrap();
    }
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(stage.path().join("outcome.json")).unwrap()).unwrap();
    change(&mut document);
    fs::write(
        stage.path().join("outcome.json"),
        CanonicalValue::try_from(document)
            .unwrap()
            .to_canonical_bytes()
            .unwrap(),
    )
    .unwrap();
    let mut files: Vec<_> = fs::read_dir(stage.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap() != "manifest.json")
        .collect();
    files.sort();
    let files: Vec<_> = files.into_iter().map(|path| {
        let bytes = fs::read(&path).unwrap();
        serde_json::json!({
            "path":path.file_name().unwrap().to_str().unwrap(),
            "sha256":Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "size_bytes":bytes.len(),
            "media_type":if path.extension().is_some_and(|ext| ext == "json") { "application/json" } else { "application/octet-stream" },
        })
    }).collect();
    fs::write(
        stage.path().join("manifest.json"),
        CanonicalValue::try_from(
            serde_json::json!({"schema_version":1,"complete":true,"files":files}),
        )
        .unwrap()
        .to_canonical_bytes()
        .unwrap(),
    )
    .unwrap();
    crate::runtime::publication::verify_finalized_directory(stage.path()).unwrap();
    let digest = crate::runtime::local_tree::hash_local_tree(stage.path()).unwrap();
    let hash: String = digest.bytes().iter().map(|b| format!("{b:02x}")).collect();
    let destination = root.join(format!("artifacts/attempt-{hash}"));
    fs::rename(stage.path(), &destination).unwrap();
    destination
}

#[test]
fn complete_unpublished_stages_are_quarantined_without_importing_their_outcome() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 100), 11)
        .unwrap()
        .attempt;
    orphan(&mut store, &attempt, TerminalOutcome::Accepted);
    let stage: String = db
        .query_row(
            "SELECT staging_path FROM leases WHERE purpose = 'staging'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    fs::rename(
        completed_attempt(root.path()),
        root.path().join(&stage).join("object"),
    )
    .unwrap();
    let report = store.recover(|| 100).unwrap();
    assert_eq!(state(&db), "failed");
    assert_eq!(report.quarantined_stages.len(), 1);
    assert_eq!(count(&db, "artifacts"), 1);
    assert!(
        root.path()
            .join(&report.quarantined_stages[0])
            .join("object/outcome.json")
            .is_file()
    );
}

#[test]
fn recovery_quarantines_unregistered_stages_even_before_lock_creation() {
    let _isolation = isolate_probes();
    let (root, mut store, db, _attempt) = setup();
    let path = root.path().join("artifacts");
    fs::create_dir(&path).unwrap();
    fs::create_dir(path.join(".staging-unregistered")).unwrap();
    fs::write(path.join(".staging-unregistered/partial"), b"incomplete").unwrap();
    let report = store.recover(|| 20).unwrap();
    assert_eq!(report.quarantined_stages.len(), 1);
    assert_eq!(state(&db), "reserved");
    assert_eq!(
        fs::read(
            root.path()
                .join(&report.quarantined_stages[0])
                .join("partial")
        )
        .unwrap(),
        b"incomplete"
    );
}

#[test]
fn expired_staging_can_be_quarantined_after_reservation_renewal() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    assert!(
        store
            .publish_artifact(
                &attempt,
                ArtifactKind::Dataset,
                |path| {
                    payload(path)?;
                    Err(io::Error::other("interrupted copy"))
                },
                || 20
            )
            .is_err()
    );
    db.execute(
        "UPDATE leases SET expires_at_ns = 200 WHERE purpose = 'reservation'",
        [],
    )
    .unwrap();
    let report = store.recover(|| 100).unwrap();
    assert_eq!(report.quarantined_stages.len(), 1);
    assert_eq!(state(&db), "reserved");
    store
        .advance(&attempt, AttemptPhase::Finalizing, 101)
        .unwrap();
    assert!(root.path().join(&report.quarantined_stages[0]).exists());
}

#[test]
fn altered_coordination_records_versions_identities_and_times_cannot_establish_success() {
    let _isolation = isolate_probes();
    for field in [
        "attempt_id",
        "slot_id",
        "retry_index",
        "finalizing_revision",
        "run_store_version",
        "started_at_ns",
        "finished_at_ns",
        "coordination",
        "references",
    ] {
        let (root, mut store, db, attempt) = setup();
        let attempt = store
            .begin_finalization(&attempt, lease("claim", 100), 11)
            .unwrap()
            .attempt;
        orphan(&mut store, &attempt, TerminalOutcome::Accepted);
        let original = completed_attempt(root.path());
        variant(root.path(), |v| match field {
            "attempt_id" | "slot_id" => v[field] = serde_json::json!("unrelated"),
            "retry_index" | "finalizing_revision" | "run_store_version" => {
                v[field] = serde_json::json!(2)
            }
            "started_at_ns" => v[field] = serde_json::json!("0"),
            "finished_at_ns" => v[field] = serde_json::json!("100"),
            "coordination" => {
                v[field]["attempt_slots"][0]["record"] = serde_json::json!({"changed":true})
            }
            "references" => v[field] = serde_json::json!([{"id":"missing", "role":"dataset"}]),
            _ => unreachable!(),
        });
        fs::remove_dir_all(original).unwrap();
        store.recover(|| 100).unwrap();
        assert_eq!(state(&db), "failed", "{field}");
        assert_eq!(count(&db, "finalization_claims"), 0);
    }
}

#[test]
fn a_complete_acceptance_without_a_claim_is_rejected() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let attempt = store
        .advance(&attempt, AttemptPhase::Finalizing, 11)
        .unwrap();
    orphan(&mut store, &attempt, TerminalOutcome::Superseded);
    let original = completed_attempt(root.path());
    variant(root.path(), |v| v["state"] = serde_json::json!("accepted"));
    fs::remove_dir_all(original).unwrap();
    let report = store.recover(|| 100).unwrap();
    assert_eq!(state(&db), "failed");
    assert_eq!(report.rejected_artifacts.len(), 1);
    assert!(report.rejected_artifacts[0].reason.contains("claim"));
}

#[test]
fn ambiguous_completed_outcomes_fail_instead_of_choosing_a_success() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 100), 11)
        .unwrap()
        .attempt;
    orphan(&mut store, &attempt, TerminalOutcome::Accepted);
    variant(root.path(), |v| {
        v["finished_at_ns"] = serde_json::json!("22")
    });
    let report = store.recover(|| 100).unwrap();
    assert_eq!(state(&db), "failed");
    assert_eq!(report.rejected_artifacts.len(), 2);
    assert!(
        report
            .rejected_artifacts
            .iter()
            .all(|r| r.reason.contains("ambiguous"))
    );
}

#[test]
fn recovery_preserves_valid_selected_observations_even_when_a_dependency_changed() {
    let _isolation = isolate_probes();
    for corrupt in [false, true] {
        let (root, mut store, db, attempt) = setup_role("measurement");
        db.execute(
            "INSERT INTO observation_slots VALUES ('checkpoint', 'logical', 'run', '{}')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO attempt_observation_slots VALUES (?1, 'checkpoint', 'run')",
            [slot().to_string()],
        )
        .unwrap();
        let artifact = store
            .publish_artifact(&attempt, ArtifactKind::Observation, payload, || 11)
            .unwrap()
            .artifact;
        db.execute("INSERT INTO observations VALUES ('observation', 'checkpoint', ?1, ?2, 1, 'reached', ?3, 'observation', '{}')", (attempt.id().to_string(), slot().to_string(), &artifact.id)).unwrap();
        db.execute(
            "INSERT INTO selected_observations VALUES ('checkpoint', 'observation', 1, 12)",
            [],
        )
        .unwrap();
        let attempt = store
            .begin_finalization(&attempt, lease("claim", 100), 13)
            .unwrap()
            .attempt;
        orphan(&mut store, &attempt, TerminalOutcome::Accepted);
        if corrupt {
            fs::write(
                root.path().join(&artifact.relative_path).join("data"),
                b"changed",
            )
            .unwrap();
        }
        store.recover(|| 100).unwrap();
        assert_eq!(state(&db), if corrupt { "failed" } else { "accepted" });
        assert_eq!(count(&db, "observations"), 1);
        assert_eq!(count(&db, "selected_observations"), 1);
        assert_eq!(
            db.query_row(
                "SELECT observation_id FROM selected_observations",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "observation"
        );
    }
}

#[test]
fn recovery_rolls_back_the_terminal_transaction_and_reimports_the_same_orphan() {
    let _isolation = isolate_probes();
    let (_root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 100), 11)
        .unwrap()
        .attempt;
    orphan(&mut store, &attempt, TerminalOutcome::Accepted);
    db.execute_batch("CREATE TRIGGER injected_failure BEFORE UPDATE OF released_at_ns ON leases BEGIN SELECT RAISE(ABORT, 'injected release failure'); END;").unwrap();
    assert!(store.recover(|| 100).is_err());
    assert_eq!(state(&db), "finalizing");
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "attempt_artifacts"), 0);
    assert_eq!(count(&db, "events"), 2);
    assert_eq!(count(&db, "finalization_claims"), 1);
    db.execute_batch("DROP TRIGGER injected_failure;").unwrap();
    let report = store.recover(|| 101).unwrap();
    assert_eq!(report.terminals.len(), 1);
    assert_eq!(state(&db), "accepted");
    assert_eq!(count(&db, "events"), 3);
    assert_eq!(count(&db, "artifacts"), 1);
}

#[test]
fn concurrent_recoverers_cannot_duplicate_an_accepted_slot_or_terminal_event() {
    let _isolation = isolate_probes();
    use std::sync::{Arc, Barrier};
    let (root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 100), 11)
        .unwrap()
        .attempt;
    orphan(&mut store, &attempt, TerminalOutcome::Accepted);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let mut store = RunStore::open(root.path()).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                match store.recover(|| 100) {
                    Ok(report) => report.terminals.len(),
                    Err(RunStoreError::Busy) => 0,
                    Err(error) => panic!("{error}"),
                }
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .sum::<usize>(),
        1
    );
    assert!(store.recover(|| 101).unwrap().terminals.is_empty());
    assert_eq!(state(&db), "accepted");
    assert_eq!(count(&db, "events"), 3);
    assert_eq!(count(&db, "attempt_artifacts"), 1);
}

#[test]
fn failure_publication_is_recoverable_after_a_late_database_error() {
    let _isolation = isolate_probes();
    let (root, mut store, db, _attempt) = setup();
    db.execute_batch("CREATE TRIGGER injected_failure BEFORE UPDATE OF released_at_ns ON leases BEGIN SELECT RAISE(ABORT, 'injected release failure'); END;").unwrap();
    assert!(store.recover(|| 100).is_err());
    assert_eq!(state(&db), "finalizing");
    let directory = completed_attempt(root.path());
    crate::runtime::publication::verify_finalized_directory(&directory).unwrap();
    db.execute_batch("DROP TRIGGER injected_failure;").unwrap();
    let report = store.recover(|| 101).unwrap();
    assert_eq!(report.terminals.len(), 1);
    assert_eq!(report.terminals[0].finished_at_ns, 100);
    assert_eq!(state(&db), "failed");
    assert_eq!(count(&db, "artifacts"), 1);
}

#[test]
fn recovering_non_success_orphans_preserves_their_original_outcomes_and_roots() {
    let _isolation = isolate_probes();
    use crate::core::canonical::CanonicalValue;
    for outcome in [
        TerminalOutcome::Superseded,
        TerminalOutcome::Censored {
            reason: "budget".into(),
        },
        TerminalOutcome::Failed {
            code: "worker_crash".into(),
            details: CanonicalValue::try_from(serde_json::json!({"exit": 1})).unwrap(),
        },
    ] {
        let (root, mut store, db, attempt) = setup();
        let dependency = store
            .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 11)
            .unwrap()
            .artifact;
        let attempt = store
            .advance(&attempt, AttemptPhase::Finalizing, 12)
            .unwrap();
        let mut times = [20, 21, 100].into_iter();
        assert!(
            store
                .publish_finalization(
                    &attempt,
                    outcome.clone(),
                    &[("dataset".into(), dependency.id.clone())],
                    payload,
                    || times.next().unwrap()
                )
                .is_err()
        );
        let report = store.recover(|| 100).unwrap();
        assert_eq!(report.terminals.len(), 1);
        assert_eq!(
            format!("{:?}", report.terminals[0].outcome),
            format!("{outcome:?}")
        );
        assert_eq!(count(&db, "attempt_artifacts"), 2);
        assert_eq!(
            db.query_row(
                "SELECT artifact_id FROM attempt_artifacts WHERE role = 'dataset'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            dependency.id
        );
        assert!(root.path().join(&dependency.relative_path).exists());
    }
}

#[test]
fn live_unlocked_stages_are_preserved_until_expiration() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    assert!(
        store
            .publish_artifact(
                &attempt,
                ArtifactKind::Dataset,
                |path| {
                    payload(path)?;
                    Err(io::Error::other("copy interrupted"))
                },
                || 20
            )
            .is_err()
    );
    let stage: String = db
        .query_row(
            "SELECT staging_path FROM leases WHERE purpose = 'staging'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let report = store.recover(|| 99).unwrap();
    assert!(report.terminals.is_empty());
    assert!(report.quarantined_stages.is_empty());
    assert!(root.path().join(stage).join("object/data").exists());
    store.renew(&attempt, 200, 99).unwrap();
    assert!(store.recover(|| 100).unwrap().terminals.is_empty());
}

#[cfg(unix)]
#[test]
fn unsafe_staging_and_dependency_symlinks_never_cross_the_recovery_boundary() {
    let _isolation = isolate_probes();
    for kind in ["stage", "dependency"] {
        let (root, mut store, db, attempt) = setup();
        if kind == "stage" {
            fs::create_dir(root.path().join("artifacts")).unwrap();
            let external = TempDir::new().unwrap();
            fs::write(external.path().join("keep"), b"external").unwrap();
            std::os::unix::fs::symlink(
                external.path(),
                root.path().join("artifacts/.staging-unsafe"),
            )
            .unwrap();
            assert!(store.recover(|| 100).is_err());
            assert_eq!(fs::read(external.path().join("keep")).unwrap(), b"external");
            assert_eq!(state(&db), "reserved");
        } else {
            let dependency = store
                .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 11)
                .unwrap()
                .artifact;
            let attempt = store
                .begin_finalization(&attempt, lease("claim", 100), 12)
                .unwrap()
                .attempt;
            let mut times = [20, 21, 100].into_iter();
            assert!(
                store
                    .publish_finalization(
                        &attempt,
                        TerminalOutcome::Accepted,
                        &[("dataset".into(), dependency.id)],
                        payload,
                        || times.next().unwrap()
                    )
                    .is_err()
            );
            let data = root.path().join(dependency.relative_path).join("data");
            fs::remove_file(&data).unwrap();
            std::os::unix::fs::symlink("/dev/null", data).unwrap();
            let report = store.recover(|| 100).unwrap();
            assert_eq!(state(&db), "failed", "{report:?}");
            assert_eq!(report.rejected_artifacts.len(), 1);
        }
    }
}

#[test]
fn released_stages_and_missing_stages_do_not_revoke_completed_artifacts() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    let artifact = store
        .publish_artifact(&attempt, ArtifactKind::Dataset, payload, || 20)
        .unwrap()
        .artifact;
    let stage: String = db
        .query_row(
            "SELECT staging_path FROM leases WHERE purpose = 'staging'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    fs::create_dir(root.path().join(&stage)).unwrap();
    fs::write(root.path().join(&stage).join("lock"), b"").unwrap();
    let report = store.recover(|| 21).unwrap();
    assert_eq!(report.quarantined_stages.len(), 1);
    assert!(report.terminals.is_empty());
    assert_eq!(state(&db), "reserved");
    assert!(root.path().join(artifact.relative_path).is_dir());
    assert!(store.recover(|| 22).unwrap().quarantined_stages.is_empty());
    assert!(
        store
            .publish_artifact(
                &attempt,
                ArtifactKind::Other,
                |path| {
                    payload(path)?;
                    Err(io::Error::other("copy interrupted"))
                },
                || 23
            )
            .is_err()
    );
    let missing: String = db
        .query_row(
            "SELECT staging_path FROM leases WHERE purpose = 'staging' AND released_at_ns IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    fs::remove_dir_all(root.path().join(missing)).unwrap();
    assert_eq!(store.recover(|| 100).unwrap().terminals.len(), 1);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM leases WHERE released_at_ns IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(count(&db, "attempt_artifacts"), 2);
}

#[test]
fn a_completed_claim_can_be_recovered_before_reservation_expiration() {
    let _isolation = isolate_probes();
    let (_root, mut store, db, attempt) = setup();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", 30), 11)
        .unwrap()
        .attempt;
    let mut times = [20, 21, 30].into_iter();
    assert!(
        store
            .publish_finalization(&attempt, TerminalOutcome::Accepted, &[], payload, || times
                .next()
                .unwrap())
            .is_err()
    );
    let report = store.recover(|| 30).unwrap();
    assert_eq!(report.terminals.len(), 1);
    assert_eq!(state(&db), "accepted");
    assert_eq!(count(&db, "finalization_claims"), 0);
}

#[test]
fn a_crash_between_quarantine_rename_and_lease_release_is_repeatable() {
    let _isolation = isolate_probes();
    let (root, mut store, db, attempt) = setup();
    assert!(
        store
            .publish_artifact(
                &attempt,
                ArtifactKind::Dataset,
                |path| {
                    payload(path)?;
                    Err(io::Error::other("interrupted"))
                },
                || 20
            )
            .is_err()
    );
    // Renew only the reservation, leaving its abandoned stage expired.
    db.execute(
        "UPDATE leases SET expires_at_ns = 200 WHERE purpose = 'reservation'",
        [],
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER injected_failure BEFORE UPDATE OF released_at_ns ON leases WHEN OLD.purpose = 'staging' BEGIN SELECT RAISE(ABORT, 'injected quarantine failure'); END;").unwrap();
    assert!(store.recover(|| 100).is_err());
    let stage: String = db
        .query_row(
            "SELECT staging_path FROM leases WHERE purpose = 'staging'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!root.path().join(&stage).exists());
    let quarantine = root
        .path()
        .join("artifacts/.quarantine")
        .join(Path::new(&stage).file_name().unwrap());
    assert!(quarantine.join("object/data").is_file());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM leases WHERE purpose = 'staging' AND released_at_ns IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    db.execute_batch("DROP TRIGGER injected_failure;").unwrap();
    assert!(store.recover(|| 101).unwrap().terminals.is_empty());
    assert!(quarantine.join("object/data").is_file());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM leases WHERE purpose = 'staging' AND released_at_ns IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn interruption_in_every_execution_phase_produces_a_typed_failure() {
    let _isolation = isolate_probes();
    let phases = [
        AttemptPhase::Materializing,
        AttemptPhase::Starting,
        AttemptPhase::Preparing,
        AttemptPhase::Executing,
        AttemptPhase::Checkpointing,
        AttemptPhase::Validating,
        AttemptPhase::Evaluating,
    ];
    for stop in 1..=phases.len() {
        let (_root, mut store, db, mut attempt) = setup();
        for phase in &phases[..stop] {
            attempt = store.advance(&attempt, *phase, 20).unwrap();
        }
        let report = store.recover(|| 100).unwrap();
        assert_eq!(report.terminals.len(), 1);
        assert_eq!(state(&db), "failed");
        assert_eq!(count(&db, "attempt_transitions"), stop as i64 + 3);
        assert_eq!(count(&db, "observations"), 0);
        assert_eq!(count(&db, "selected_observations"), 0);
    }
}

#[test]
fn recovery_retains_exact_current_epoch_finish_timestamps() {
    let _isolation = isolate_probes();
    let (_root, mut store, db, attempt) = setup();
    let now = 1_792_349_520_000_000_123;
    store.renew(&attempt, now + 100, 20).unwrap();
    let attempt = store
        .begin_finalization(&attempt, lease("claim", now + 100), now + 1)
        .unwrap()
        .attempt;
    let mut times = [now + 2, now + 3, now + 100].into_iter();
    assert!(
        store
            .publish_finalization(&attempt, TerminalOutcome::Accepted, &[], payload, || times
                .next()
                .unwrap())
            .is_err()
    );
    let report = store.recover(|| now + 100).unwrap();
    assert_eq!(report.terminals[0].finished_at_ns, now + 3);
    assert_eq!(state(&db), "accepted");
}
