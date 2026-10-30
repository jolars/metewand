#![cfg(unix)]

use std::{collections::BTreeMap, fs, path::Path};

use crate::core::{
    inspection::SlotState,
    records::{RecordId, ResolvedAttemptSlotRecord},
};
use crate::runtime::{
    inspection::Inspection,
    run_store::{
        ArtifactKind, AttemptPhase, LeaseRequest, RetryAuthorization, RunStore, TerminalOutcome,
    },
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

fn setup(
    role: &str,
) -> (
    tempfile::TempDir,
    RunStore,
    RecordId<ResolvedAttemptSlotRecord>,
) {
    let root = tempfile::tempdir().unwrap();
    let store = RunStore::open(root.path()).unwrap();
    let db = Connection::open(store.database_path()).unwrap();
    let candidate = format!("mw1-logical-candidate-{}", "a".repeat(64));
    let run = format!("mw1-resolved-specification-{}", "b".repeat(64));
    let logical_run = format!("mw1-logical-specification-{}", "c".repeat(64));
    let logical_slot = format!("mw1-logical-attempt-slot-{}", "d".repeat(64));
    let slot = RecordId::from_digest([1; 32]);
    db.execute(
        "INSERT INTO applicability_decisions VALUES ('decision', ?1, 1, '{}')",
        [&candidate],
    )
    .unwrap();
    db.execute(
        "INSERT INTO resolved_runs VALUES (?1, ?2, ?3, 'decision', 1, '{}')",
        params![run, logical_run, candidate],
    )
    .unwrap();
    db.execute(
        "INSERT INTO attempt_slots VALUES (?1, ?2, ?3, 'one_shot', ?4, '{}')",
        params![slot.to_string(), logical_slot, run, role],
    )
    .unwrap();
    (root, store, slot)
}

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if entry.file_type().unwrap().is_dir() {
            for (path, bytes) in files(&entry.path()) {
                result.insert(format!("{name}/{path}"), bytes);
            }
        } else {
            result.insert(
                name,
                Sha256::digest(fs::read(entry.path()).unwrap()).to_vec(),
            );
        }
    }
    result
}

#[test]
fn expired_reservations_remain_nonterminal_and_orphans_are_not_imported() {
    let (root, mut store, slot) = setup("measurement");
    let owned = store
        .reserve(
            slot,
            RetryAuthorization::FirstAttempt,
            LeaseRequest {
                id: "reservation".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            10,
        )
        .unwrap();
    fs::create_dir(root.path().join("artifacts")).unwrap();
    fs::write(root.path().join("artifacts/orphan"), "not committed").unwrap();
    let before = files(root.path());
    let mut inspection = Inspection::default();
    inspection.add_store(root.path()).unwrap();
    let report = inspection.status();
    assert_eq!(report.slots[0].state, SlotState::RecoveryRequired);
    assert_eq!(report.summary.recovery_required_slots, 1);
    assert_eq!(report.attempts[0].state, "reserved");
    assert_eq!(report.attempts[0].finished_at_ns, None);
    assert_eq!(report.attempts[0].id, owned.id().to_string());
    assert_eq!(files(root.path()), before);
    assert!(store.slot_progress(slot).unwrap().active);
    let before = files(root.path());
    let mut inspection = Inspection::default();
    inspection.add_store(root.path()).unwrap();
    let explanation = inspection.explain(&owned.id().to_string()).unwrap();
    assert_eq!(
        explanation.subject.record["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(files(root.path()), before);
}

#[test]
fn censored_history_is_retained_without_selecting_an_observation() {
    let (root, mut store, slot) = setup("measurement");
    let owned = store
        .reserve(
            slot,
            RetryAuthorization::FirstAttempt,
            LeaseRequest {
                id: "reservation".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            10,
        )
        .unwrap();
    let dependency = store
        .publish_artifact(
            &owned,
            ArtifactKind::Other,
            |path| fs::write(path.join("attempt.json"), b"opaque benchmark data"),
            || 11,
        )
        .unwrap();
    let owned = store.advance(&owned, AttemptPhase::Finalizing, 12).unwrap();
    let published = store
        .publish_finalization(
            &owned,
            TerminalOutcome::Censored {
                reason: "scientific budget exhausted".into(),
            },
            &[("retained".into(), dependency.artifact.id)],
            |_| Ok(()),
            || 13,
        )
        .unwrap();
    let before = files(root.path());
    let mut inspection = Inspection::default();
    inspection.add_store(root.path()).unwrap();
    let report = inspection.status();
    assert_eq!(report.summary.censored_attempts, 1);
    assert_eq!(report.summary.retry_required_slots, 1);
    assert_eq!(report.summary.selected_observations, 0);
    let mut inspection = Inspection::default();
    inspection.add_store(root.path()).unwrap();
    let explanation = inspection.explain(&published.artifact.id).unwrap();
    assert_eq!(explanation.subject.kind, "stored-artifact");
    assert_eq!(files(root.path()), before);
}
