#![cfg(unix)]

use metewand_core::{
    canonical::CanonicalValue,
    identity::run_attempt_id,
    records::{ContentDigest, RecordId, ResolvedAttemptSlotRecord},
};
use metewand_runtime::run_store::{
    ArtifactRegistration, AttemptPhase, ClaimDisposition, LeaseRequest, RetryAuthorization,
    RunStore, TerminalOutcome,
};
use rusqlite::Connection;
use serde_json::json;
use tempfile::TempDir;

fn slot() -> RecordId<ResolvedAttemptSlotRecord> {
    RecordId::from_digest([1; 32])
}

fn setup() -> (TempDir, RunStore, Connection) {
    setup_slot("one_shot", "warmup")
}

fn setup_slot(kind: &str, role: &str) -> (TempDir, RunStore, Connection) {
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
        "INSERT INTO attempt_slots VALUES (?1, 'logical', 'run', ?2, ?3, '{}')",
        (slot().to_string(), kind, role),
    )
    .unwrap();
    (root, store, db)
}

fn lease(id: &str) -> LeaseRequest {
    LeaseRequest {
        id: id.into(),
        owner_token: "owner".into(),
        expires_at_ns: 100,
    }
}

fn artifact(id: &str) -> ArtifactRegistration {
    ArtifactRegistration {
        id: id.into(),
        relative_path: format!("attempts/{id}"),
        content_sha256: ContentDigest::new([2; 32]),
        manifest_sha256: ContentDigest::new([3; 32]),
        verified_at_ns: 20,
    }
}

fn details() -> CanonicalValue {
    CanonicalValue::try_from(json!({"message":"worker exited"})).unwrap()
}

fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn reserves_identity_history_and_lease_atomically_and_requires_explicit_retries() {
    let (_root, mut store, db) = setup();
    let attempt = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("reservation"),
            10,
        )
        .unwrap();
    assert_eq!(attempt.id(), run_attempt_id(slot(), 0).unwrap());
    assert_eq!(attempt.retry_index(), 0);
    assert_eq!(attempt.revision(), 0);
    assert_eq!(count(&db, "attempts"), 1);
    assert_eq!(count(&db, "attempt_transitions"), 1);
    assert_eq!(count(&db, "events"), 1);
    assert_eq!(count(&db, "leases"), 1);
    assert!(
        store
            .reserve(slot(), RetryAuthorization::FirstAttempt, lease("other"), 11)
            .is_err()
    );
    assert!(
        store
            .reserve(
                slot(),
                RetryAuthorization::Retry {
                    next_index: 2,
                    max_index: 2
                },
                lease("other"),
                11
            )
            .is_err()
    );
    let retry = store
        .reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 1,
                max_index: 1,
            },
            lease("other"),
            11,
        )
        .unwrap();
    assert_eq!(retry.id(), run_attempt_id(slot(), 1).unwrap());
    assert_ne!(retry.id(), attempt.id());
}

#[test]
fn executes_all_phases_clamps_clock_and_rejects_stale_guards() {
    let (_root, mut store, db) = setup();
    let original = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("reservation"),
            10,
        )
        .unwrap();
    assert!(
        store
            .advance(&original, AttemptPhase::Executing, 11)
            .is_err()
    );
    let mut attempt = original.clone();
    for phase in [
        AttemptPhase::Materializing,
        AttemptPhase::Starting,
        AttemptPhase::Preparing,
        AttemptPhase::Executing,
        AttemptPhase::Checkpointing,
        AttemptPhase::Validating,
        AttemptPhase::Evaluating,
        AttemptPhase::Executing,
        AttemptPhase::Validating,
        AttemptPhase::Evaluating,
    ] {
        attempt = store.advance(&attempt, phase, 9).unwrap();
    }
    assert_eq!(attempt.updated_at_ns(), 10);
    assert!(
        store
            .advance(&original, AttemptPhase::Materializing, 11)
            .is_err()
    );
    assert_eq!(count(&db, "events"), 11);
    assert_eq!(count(&db, "attempt_transitions"), 11);
    let payload: String = db
        .query_row(
            "SELECT payload_json FROM events ORDER BY sequence LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        payload,
        r#"{"from":null,"retry_index":0,"revision":0,"slot":"mw1-resolved-attempt-slot-0101010101010101010101010101010101010101010101010101010101010101","to":"reserved"}"#
    );
}

#[test]
fn finalizes_every_terminal_outcome_and_releases_leases_without_rewriting_history() {
    for outcome in [
        TerminalOutcome::Accepted,
        TerminalOutcome::Censored {
            reason: "bound".into(),
        },
        TerminalOutcome::Superseded,
        TerminalOutcome::Failed {
            code: "worker_crash".into(),
            details: details(),
        },
    ] {
        let (_root, mut store, db) = setup();
        let attempt = store
            .reserve(
                slot(),
                RetryAuthorization::FirstAttempt,
                lease("reservation"),
                10,
            )
            .unwrap();
        let attempt = if matches!(outcome, TerminalOutcome::Accepted) {
            let result = store
                .begin_finalization(&attempt, lease("finalization"), 11)
                .unwrap();
            assert_eq!(result.disposition, ClaimDisposition::Claimed);
            result.attempt
        } else {
            store
                .advance(&attempt, AttemptPhase::Finalizing, 11)
                .unwrap()
        };
        let finished = store
            .commit_finalization(&attempt, outcome, artifact("final"), &[], 20)
            .unwrap();
        assert_eq!(finished.finished_at_ns, 20);
        assert_eq!(count(&db, "finalization_claims"), 0);
        assert_eq!(count(&db, "artifacts"), 1);
        assert_eq!(count(&db, "attempt_artifacts"), 1);
        assert_eq!(count(&db, "attempt_transitions"), 3);
        assert!(
            store
                .advance(&attempt, AttemptPhase::Executing, 21)
                .is_err()
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM leases WHERE released_at_ns IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert!(
            !db.prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
    }
}

#[test]
fn expiry_and_released_ownership_cannot_renew_advance_or_finalize() {
    let (_root, mut store, db) = setup();
    let attempt = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("reservation"),
            10,
        )
        .unwrap();
    assert!(store.renew(&attempt, 200, 100).is_err());
    assert!(
        store
            .advance(&attempt, AttemptPhase::Materializing, 100)
            .is_err()
    );
    assert!(
        store
            .begin_finalization(&attempt, lease("finalization"), 100)
            .is_err()
    );
    assert_eq!(count(&db, "events"), 1);
    store.renew(&attempt, 200, 99).unwrap();
    assert!(store.renew(&attempt, 150, 100).is_err());
    let result = store
        .begin_finalization(
            &attempt,
            LeaseRequest {
                expires_at_ns: 120,
                ..lease("finalization")
            },
            100,
        )
        .unwrap();
    assert!(
        store
            .commit_finalization(
                &result.attempt,
                TerminalOutcome::Accepted,
                artifact("final"),
                &[],
                120
            )
            .is_err()
    );
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "finalization_claims"), 1);
    store.renew(&result.attempt, 200, 119).unwrap();
    store
        .commit_finalization(
            &result.attempt,
            TerminalOutcome::Accepted,
            artifact("final"),
            &[],
            120,
        )
        .unwrap();
    assert!(store.renew(&result.attempt, 300, 121).is_err());
}

#[test]
fn competing_successes_claim_once_and_loser_commits_superseded() {
    let (_root, mut store, db) = setup();
    let first = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("reservation1"),
            10,
        )
        .unwrap();
    let second = store
        .reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 1,
                max_index: 1,
            },
            lease("reservation2"),
            10,
        )
        .unwrap();
    let first = store
        .begin_finalization(&first, lease("finalization1"), 11)
        .unwrap();
    let second = store
        .begin_finalization(&second, lease("finalization2"), 11)
        .unwrap();
    assert_eq!(first.disposition, ClaimDisposition::Claimed);
    assert_eq!(second.disposition, ClaimDisposition::Superseded);
    assert!(
        store
            .commit_finalization(
                &second.attempt,
                TerminalOutcome::Accepted,
                artifact("loser"),
                &[],
                20
            )
            .is_err()
    );
    store
        .commit_finalization(
            &second.attempt,
            TerminalOutcome::Superseded,
            artifact("loser"),
            &[],
            20,
        )
        .unwrap();
    store
        .commit_finalization(
            &first.attempt,
            TerminalOutcome::Accepted,
            artifact("winner"),
            &[],
            20,
        )
        .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM attempts WHERE state = 'accepted'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(
        store
            .reserve(
                slot(),
                RetryAuthorization::Retry {
                    next_index: 2,
                    max_index: 2
                },
                lease("reservation3"),
                21
            )
            .is_err()
    );
    assert_eq!(count(&db, "events"), 6);
}

#[test]
fn failed_transaction_leaves_no_artifact_transition_or_event() {
    let (_root, mut store, db) = setup();
    let attempt = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("reservation"),
            10,
        )
        .unwrap();
    let attempt = store
        .begin_finalization(&attempt, lease("finalization"), 11)
        .unwrap()
        .attempt;
    let before = count(&db, "events");
    assert!(
        store
            .commit_finalization(
                &attempt,
                TerminalOutcome::Accepted,
                artifact("final"),
                &[("dependency".into(), "missing".into())],
                20
            )
            .is_err()
    );
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "events"), before);
    assert_eq!(count(&db, "finalization_claims"), 1);
    store
        .commit_finalization(
            &attempt,
            TerminalOutcome::Accepted,
            artifact("final"),
            &[],
            20,
        )
        .unwrap();
}

#[test]
fn reservation_and_claim_errors_roll_back_their_partial_writes() {
    let (_root, mut store, db) = setup();
    let first = store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("used"), 10)
        .unwrap();
    assert!(
        store
            .reserve(
                slot(),
                RetryAuthorization::Retry {
                    next_index: 1,
                    max_index: 1
                },
                lease("used"),
                11
            )
            .is_err()
    );
    assert_eq!(count(&db, "attempts"), 1);
    assert_eq!(count(&db, "events"), 1);
    assert!(store.begin_finalization(&first, lease("used"), 11).is_err());
    assert_eq!(count(&db, "events"), 1);
    assert_eq!(count(&db, "attempt_transitions"), 1);
    assert_eq!(count(&db, "finalization_claims"), 0);
    assert_eq!(
        db.query_row("SELECT state FROM attempts", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "reserved"
    );
}

#[test]
fn invalid_inputs_and_integer_boundaries_do_not_leave_rows() {
    let (_root, mut store, db) = setup();
    for now in [i64::MAX as u64 + 1, u64::MAX] {
        assert!(
            store
                .reserve(slot(), RetryAuthorization::FirstAttempt, lease("bad"), now)
                .is_err()
        );
    }
    for request in [
        LeaseRequest {
            id: String::new(),
            ..lease("bad")
        },
        LeaseRequest {
            owner_token: String::new(),
            ..lease("bad")
        },
        LeaseRequest {
            expires_at_ns: 10,
            ..lease("bad")
        },
    ] {
        assert!(
            store
                .reserve(slot(), RetryAuthorization::FirstAttempt, request, 10)
                .is_err()
        );
    }
    assert!(
        store
            .reserve(
                RecordId::from_digest([9; 32]),
                RetryAuthorization::FirstAttempt,
                lease("missing"),
                10
            )
            .is_err()
    );
    assert_eq!(count(&db, "attempts"), 0);
    assert_eq!(count(&db, "events"), 0);
    let original = store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("owner"), 10)
        .unwrap();
    assert!(
        store
            .begin_finalization(
                &original,
                LeaseRequest {
                    owner_token: "thief".into(),
                    ..lease("claim")
                },
                11
            )
            .is_err()
    );
    assert!(
        store
            .begin_finalization(
                &original,
                LeaseRequest {
                    expires_at_ns: 9,
                    ..lease("claim")
                },
                1
            )
            .is_err()
    );
    let attempt = store
        .advance(&original, AttemptPhase::Finalizing, 11)
        .unwrap();
    for outcome in [
        TerminalOutcome::Failed {
            code: String::new(),
            details: details(),
        },
        TerminalOutcome::Failed {
            code: "crash".into(),
            details: CanonicalValue::try_from(json!([])).unwrap(),
        },
        TerminalOutcome::Censored {
            reason: String::new(),
        },
    ] {
        assert!(
            store
                .commit_finalization(&attempt, outcome, artifact("bad"), &[], 20)
                .is_err()
        );
    }
    for path in [
        "../escape",
        "/absolute",
        "a//b",
        "a/./b",
        "a\\b",
        "trailing/",
    ] {
        assert!(
            store
                .commit_finalization(
                    &attempt,
                    TerminalOutcome::Superseded,
                    ArtifactRegistration {
                        relative_path: path.into(),
                        ..artifact("bad")
                    },
                    &[],
                    20
                )
                .is_err()
        );
    }
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "events"), 2);
}

#[test]
fn database_owner_fencing_rejects_released_reservations_and_expired_claims() {
    let (_root, mut store, db) = setup();
    let attempt = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("reservation"),
            10,
        )
        .unwrap();
    db.execute(
        "UPDATE leases SET released_at_ns = 11 WHERE id = 'reservation'",
        [],
    )
    .unwrap();
    assert!(
        store
            .advance(&attempt, AttemptPhase::Materializing, 12)
            .is_err()
    );
    assert!(store.renew(&attempt, 200, 12).is_err());
    assert_eq!(count(&db, "events"), 1);
}

#[test]
fn expired_competing_claim_is_never_stolen_and_failure_releases_a_live_claim() {
    let (_root, mut store, db) = setup();
    let first = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            LeaseRequest {
                expires_at_ns: 300,
                ..lease("owner1")
            },
            10,
        )
        .unwrap();
    let first = store
        .begin_finalization(&first, lease("claim1"), 11)
        .unwrap();
    let second = store
        .reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 1,
                max_index: 2,
            },
            LeaseRequest {
                expires_at_ns: 300,
                ..lease("owner2")
            },
            101,
        )
        .unwrap();
    let second = store
        .begin_finalization(
            &second,
            LeaseRequest {
                expires_at_ns: 300,
                ..lease("claim2")
            },
            102,
        )
        .unwrap();
    assert_eq!(second.disposition, ClaimDisposition::Superseded);
    assert_eq!(
        db.query_row("SELECT attempt_id FROM finalization_claims", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        first.attempt.id().to_string()
    );
    assert!(
        store
            .commit_finalization(
                &first.attempt,
                TerminalOutcome::Failed {
                    code: "crash".into(),
                    details: details()
                },
                artifact("expired"),
                &[],
                102
            )
            .is_err()
    );
    assert_eq!(count(&db, "artifacts"), 0);

    let (_root, mut store, db) = setup();
    let first = store
        .reserve(
            slot(),
            RetryAuthorization::FirstAttempt,
            lease("owner1"),
            10,
        )
        .unwrap();
    let first = store
        .begin_finalization(&first, lease("claim1"), 11)
        .unwrap();
    store
        .commit_finalization(
            &first.attempt,
            TerminalOutcome::Failed {
                code: "publication_failed".into(),
                details: details(),
            },
            artifact("failed"),
            &[],
            20,
        )
        .unwrap();
    let next = store
        .reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 1,
                max_index: 1,
            },
            lease("owner2"),
            21,
        )
        .unwrap();
    let next = store
        .begin_finalization(&next, lease("claim2"), 22)
        .unwrap();
    assert_eq!(next.disposition, ClaimDisposition::Claimed);
    assert_eq!(count(&db, "artifacts"), 1);
}

fn observation(
    db: &Connection,
    id: &str,
    attempt: &str,
    valid: bool,
    completion: &str,
    select: bool,
) {
    db.execute(
        "INSERT INTO artifacts VALUES (?1, 'observation', ?1, ?2, 'directory', ?2, 15)",
        (id, "a".repeat(64)),
    )
    .unwrap();
    db.execute("INSERT INTO observations VALUES (?1, 'observation-slot', ?2, ?3, ?4, ?5, ?1, 'observation', '{}')", (id, attempt, slot().to_string(), valid, completion)).unwrap();
    if select {
        db.execute("INSERT INTO selected_observations(slot_id, observation_id, selected_at_ns) VALUES ('observation-slot', ?1, 15)", [id]).unwrap();
    }
}

fn add_measurement(db: &Connection) {
    db.execute_batch("INSERT INTO observation_slots VALUES ('observation-slot', 'logical-observation', 'run', '{}');").unwrap();
    db.execute(
        "INSERT INTO attempt_observation_slots VALUES (?1, 'observation-slot', 'run')",
        [slot().to_string()],
    )
    .unwrap();
}

#[test]
fn acceptance_requires_selected_complete_valid_observations_from_this_one_shot() {
    for case in ["absent", "unselected", "invalid", "incomplete", "complete"] {
        let (_root, mut store, db) = setup_slot("one_shot", "measurement");
        add_measurement(&db);
        let attempt = store
            .reserve(slot(), RetryAuthorization::FirstAttempt, lease("owner"), 10)
            .unwrap();
        let attempt = store
            .begin_finalization(&attempt, lease("claim"), 11)
            .unwrap()
            .attempt;
        if case != "absent" {
            observation(
                &db,
                "observation",
                &attempt.id().to_string(),
                case != "invalid",
                if case == "incomplete" {
                    "not_reached"
                } else {
                    "reached"
                },
                matches!(case, "incomplete" | "complete"),
            );
        }
        let result = store.commit_finalization(
            &attempt,
            TerminalOutcome::Accepted,
            artifact("attempt"),
            &[],
            20,
        );
        assert_eq!(result.is_ok(), case == "complete", "{case}");
        if result.is_err() {
            assert_eq!(count(&db, "finalization_claims"), 1);
        }
    }
}

#[test]
fn stream_can_reuse_selected_checkpoint_but_one_shot_retry_cannot() {
    for kind in ["stream", "one_shot", "fresh_observation"] {
        let (_root, mut store, db) = setup_slot(kind, "measurement");
        add_measurement(&db);
        let first = store
            .reserve(
                slot(),
                RetryAuthorization::FirstAttempt,
                lease("owner1"),
                10,
            )
            .unwrap();
        observation(
            &db,
            "retained",
            &first.id().to_string(),
            true,
            "reached",
            true,
        );
        let first = store.advance(&first, AttemptPhase::Finalizing, 16).unwrap();
        store
            .commit_finalization(
                &first,
                TerminalOutcome::Failed {
                    code: "worker_crash".into(),
                    details: details(),
                },
                artifact("first"),
                &[],
                20,
            )
            .unwrap();
        let next = store
            .reserve(
                slot(),
                RetryAuthorization::Retry {
                    next_index: 1,
                    max_index: 1,
                },
                lease("owner2"),
                21,
            )
            .unwrap();
        let next = store
            .begin_finalization(&next, lease("claim2"), 22)
            .unwrap()
            .attempt;
        let result =
            store.commit_finalization(&next, TerminalOutcome::Accepted, artifact("next"), &[], 23);
        assert_eq!(result.is_ok(), kind == "stream", "{kind}");
        assert_eq!(count(&db, "observations"), 1);
        assert_eq!(count(&db, "selected_observations"), 1);
        assert_eq!(
            db.query_row("SELECT producing_attempt_id FROM observations", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            first.id().to_string()
        );
    }
}

#[test]
fn simultaneous_writers_allocate_one_retry_and_one_success_claim() {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };
    let (root, mut store, db) = setup();
    store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("first"), 10)
        .unwrap();
    let second_store = RunStore::open(root.path()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let writers: Vec<_> = [
        (RunStore::open(root.path()).unwrap(), "owner1"),
        (second_store, "owner2"),
    ]
    .into_iter()
    .map(|(mut store, id)| {
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            store.reserve(
                slot(),
                RetryAuthorization::Retry {
                    next_index: 1,
                    max_index: 1,
                },
                lease(id),
                11,
            )
        })
    })
    .collect();
    let results: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(count(&db, "attempts"), 2);
    let retry = results.into_iter().find_map(Result::ok).unwrap();
    let third = store
        .reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 2,
                max_index: 2,
            },
            lease("third"),
            12,
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let writers: Vec<_> = [retry, third]
        .into_iter()
        .enumerate()
        .map(|(i, attempt)| {
            let mut store = RunStore::open(root.path()).unwrap();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store
                    .begin_finalization(&attempt, lease(&format!("claim-{i}")), 13)
                    .unwrap()
            })
        })
        .collect();
    let outcomes: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| r.disposition == ClaimDisposition::Claimed)
            .count(),
        1
    );
    assert_eq!(count(&db, "finalization_claims"), 1);
}

#[test]
fn terminal_receipt_uses_event_sequence_and_committed_state_survives_reopening() {
    let (root, mut store, db) = setup();
    let attempt = store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("owner"), 10)
        .unwrap();
    db.execute(
        "INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (10, 'other', '{}')",
        [],
    )
    .unwrap();
    let attempt = store
        .begin_finalization(&attempt, lease("claim"), 11)
        .unwrap()
        .attempt;
    let finished = store
        .commit_finalization(
            &attempt,
            TerminalOutcome::Accepted,
            artifact("final"),
            &[],
            20,
        )
        .unwrap();
    assert_eq!(finished.event_sequence, 4);
    drop(store);
    let reopened = RunStore::open(root.path()).unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM attempts", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "accepted"
    );
    assert_eq!(count(&db, "attempt_transitions"), 3);
    assert_eq!(count(&db, "artifacts"), 1);
    assert_eq!(count(&db, "finalization_claims"), 0);
    assert_eq!(reopened.database_path(), db.path().unwrap());
}

#[test]
fn successful_retry_retains_earlier_selection_and_produces_its_own_result() {
    let (_root, mut store, db) = setup_slot("one_shot", "measurement");
    add_measurement(&db);
    let first = store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("first"), 10)
        .unwrap();
    observation(
        &db,
        "earlier",
        &first.id().to_string(),
        true,
        "reached",
        true,
    );
    let first = store.advance(&first, AttemptPhase::Finalizing, 16).unwrap();
    store
        .commit_finalization(
            &first,
            TerminalOutcome::Failed {
                code: "crash".into(),
                details: details(),
            },
            artifact("first"),
            &[],
            20,
        )
        .unwrap();
    let retry = store
        .reserve(
            slot(),
            RetryAuthorization::Retry {
                next_index: 1,
                max_index: 1,
            },
            lease("retry"),
            21,
        )
        .unwrap();
    observation(
        &db,
        "recomputed",
        &retry.id().to_string(),
        true,
        "reached",
        false,
    );
    let retry = store
        .begin_finalization(&retry, lease("claim"), 22)
        .unwrap()
        .attempt;
    store
        .commit_finalization(
            &retry,
            TerminalOutcome::Accepted,
            artifact("retry"),
            &[],
            23,
        )
        .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT observation_id FROM selected_observations",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "earlier"
    );
    assert_eq!(count(&db, "observations"), 2);
}

#[test]
fn rollback_after_terminal_projection_preserves_claim_and_lease() {
    let (_root, mut store, db) = setup();
    let attempt = store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("owner"), 10)
        .unwrap();
    let attempt = store
        .begin_finalization(&attempt, lease("claim"), 11)
        .unwrap()
        .attempt;
    db.execute_batch("CREATE TRIGGER fail_release BEFORE UPDATE OF released_at_ns ON leases BEGIN SELECT RAISE(ABORT, 'injected release failure'); END;").unwrap();
    assert!(
        store
            .commit_finalization(
                &attempt,
                TerminalOutcome::Accepted,
                artifact("final"),
                &[],
                20
            )
            .is_err()
    );
    assert_eq!(count(&db, "events"), 2);
    assert_eq!(count(&db, "attempt_transitions"), 2);
    assert_eq!(count(&db, "artifacts"), 0);
    assert_eq!(count(&db, "finalization_claims"), 1);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM leases WHERE released_at_ns IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT state FROM attempts", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "finalizing"
    );
    db.execute_batch("DROP TRIGGER fail_release;").unwrap();
    store
        .commit_finalization(
            &attempt,
            TerminalOutcome::Accepted,
            artifact("final"),
            &[],
            20,
        )
        .unwrap();
}

#[test]
fn stream_intermediate_checkpoint_need_not_reach_the_final_target() {
    let (_root, mut store, db) = setup_slot("stream", "measurement");
    add_measurement(&db);
    let attempt = store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("owner"), 10)
        .unwrap();
    observation(
        &db,
        "checkpoint",
        &attempt.id().to_string(),
        true,
        "not_reached",
        true,
    );
    let attempt = store
        .begin_finalization(&attempt, lease("claim"), 11)
        .unwrap()
        .attempt;
    // The trusted publisher separately validates the stream's contracted target.
    store
        .commit_finalization(
            &attempt,
            TerminalOutcome::Accepted,
            artifact("stream"),
            &[],
            20,
        )
        .unwrap();
    assert_eq!(count(&db, "selected_observations"), 1);
}

#[test]
fn terminal_clock_clamping_preserves_diagnostic_and_artifact_roots() {
    let (_root, mut store, db) = setup();
    let attempt = store
        .reserve(slot(), RetryAuthorization::FirstAttempt, lease("owner"), 10)
        .unwrap();
    let attempt = store
        .advance(&attempt, AttemptPhase::Finalizing, 30)
        .unwrap();
    db.execute("INSERT INTO artifacts VALUES ('dataset', 'dataset', 'datasets/object', ?1, 'directory', ?1, 15)", ["a".repeat(64)]).unwrap();
    let finished = store
        .commit_finalization(
            &attempt,
            TerminalOutcome::Failed {
                code: "dataset_mutation".into(),
                details: details(),
            },
            ArtifactRegistration {
                verified_at_ns: 30,
                ..artifact("failed")
            },
            &[("dataset".into(), "dataset".into())],
            20,
        )
        .unwrap();
    assert_eq!(finished.finished_at_ns, 30);
    let (code, payload): (String, String) = db.query_row("SELECT failure_code, payload_json FROM attempts JOIN events ON events.attempt_id = attempts.id WHERE events.kind = 'attempt_finished'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(code, "dataset_mutation");
    assert_eq!(
        payload,
        r#"{"diagnostic":{"message":"worker exited"},"failure_code":"dataset_mutation","from":"finalizing","revision":2,"to":"failed"}"#
    );
    assert_eq!(count(&db, "attempt_artifacts"), 2);
    assert_eq!(
        db.query_row("SELECT min(released_at_ns) FROM leases", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        30
    );
}
