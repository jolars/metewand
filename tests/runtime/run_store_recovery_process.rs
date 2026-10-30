#![cfg(any(unix, windows))]

use crate::core::records::{RecordId, ResolvedAttemptSlotRecord};
use crate::runtime::run_store::{
    ArtifactKind, LeaseRequest, RetryAuthorization, RunStore, TerminalOutcome,
};
use rusqlite::Connection;
use std::{fs, io, path::Path};
use tempfile::TempDir;

fn slot() -> RecordId<ResolvedAttemptSlotRecord> {
    RecordId::from_digest([1; 32])
}

fn setup() -> (TempDir, RunStore, Connection) {
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
        "INSERT INTO attempt_slots VALUES (?1, 'logical', 'run', 'one_shot', 'warmup', '{}')",
        [slot().to_string()],
    )
    .unwrap();
    (root, store, db)
}

fn payload(path: &Path) -> io::Result<()> {
    fs::write(path.join("data"), b"verified bytes")
}

fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn recovery_child_process() {
    let Some(root) = std::env::var_os("METEWAND_RECOVERY_CHILD_ROOT") else {
        return;
    };
    let mut store = RunStore::open(Path::new(&root)).unwrap();
    let attempt = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            LeaseRequest {
                id: "child-reservation".into(),
                owner_token: "child".into(),
                expires_at_ns: 100,
            },
            10,
        )
        .unwrap();
    if std::env::var("METEWAND_RECOVERY_CHILD_MODE").unwrap() == "stage" {
        let _ = store.publish_artifact(
            &attempt,
            ArtifactKind::Dataset,
            |path| {
                payload(path)?;
                std::process::exit(77);
            },
            || 20,
        );
    } else {
        let attempt = store
            .begin_finalization(
                &attempt,
                LeaseRequest {
                    id: "child-claim".into(),
                    owner_token: "child".into(),
                    expires_at_ns: 100,
                },
                11,
            )
            .unwrap()
            .attempt;
        let mut samples = 0;
        let _ =
            store.publish_finalization(&attempt, TerminalOutcome::Accepted, &[], payload, || {
                samples += 1;
                if samples == 3 {
                    std::process::exit(77);
                }
                20
            });
    }
    panic!("child failed to reach its crash boundary");
}

#[test]
fn process_death_releases_staging_locks_and_preserves_only_published_success() {
    for mode in ["stage", "published"] {
        let (root, mut store, db) = setup();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime_tests::run_store_recovery_process::recovery_child_process",
                "--nocapture",
            ])
            .env("METEWAND_RECOVERY_CHILD_ROOT", root.path())
            .env("METEWAND_RECOVERY_CHILD_MODE", mode)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(77), "{output:?}");
        let report = store.recover(|| 100).unwrap();
        assert_eq!(report.terminals.len(), 1);
        assert_eq!(report.quarantined_stages.len(), 1);
        assert_eq!(
            db.query_row(
                "SELECT state FROM attempts WHERE retry_index = 0",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            if mode == "published" {
                "accepted"
            } else {
                "failed"
            }
        );
        assert_eq!(count(&db, "finalization_claims"), 0);
        assert!(report.busy_stages.is_empty());
    }
}
