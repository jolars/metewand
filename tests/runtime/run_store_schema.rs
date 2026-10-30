use crate::runtime::run_store::{RUN_STORE_SCHEMA_SQL, RUN_STORE_SCHEMA_VERSION};
use rusqlite::{Connection, ErrorCode};

fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys = ON; PRAGMA recursive_triggers = ON;")
        .unwrap();
    db.execute_batch(RUN_STORE_SCHEMA_SQL).unwrap();
    db
}

fn reject(db: &Connection, sql: &str) {
    let error = db.execute_batch(sql).expect_err(sql);
    assert_eq!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation),
        "{error}: {sql}"
    );
}

#[test]
fn schema_is_versioned_strict_and_has_valid_foreign_keys() {
    let db = database();
    assert_eq!(
        db.pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        RUN_STORE_SCHEMA_VERSION
    );
    let nonstrict: i64 = db.query_row("SELECT count(*) FROM pragma_table_list WHERE schema = 'main' AND name NOT LIKE 'sqlite_%' AND strict != 1", [], |row| row.get(0)).unwrap();
    assert_eq!(nonstrict, 0);
    assert!(
        !db.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn event_rows_are_insert_only_including_replace() {
    let db = database();
    db.execute_batch("INSERT INTO events(sequence, occurred_at_ns, kind, payload_json) VALUES (1, 0, 'test', '{}');").unwrap();
    for sql in [
        "UPDATE events SET kind = 'changed' WHERE sequence = 1;",
        "DELETE FROM events WHERE sequence = 1;",
        "INSERT OR REPLACE INTO events(sequence, occurred_at_ns, kind, payload_json) VALUES (1, 1, 'changed', '{}');",
        "INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (0, 'bad', '[]');",
    ] {
        reject(&db, sql);
    }
}

fn seed(db: &Connection) {
    db.execute_batch(
        "INSERT INTO applicability_decisions VALUES ('applicable', 'candidate', 1, '{}');
         INSERT INTO applicability_decisions VALUES ('excluded', 'other-candidate', 0, '{}');
         INSERT INTO resolved_runs VALUES ('run', 'logical-run', 'candidate', 'applicable', 1, '{}');
         INSERT INTO resolved_runs VALUES ('other-run', 'logical-run', 'candidate', 'applicable', 1, '{}');
         INSERT INTO observation_slots VALUES ('obs-slot', 'logical-obs', 'run', '{}');
         INSERT INTO observation_slots VALUES ('checkpoint-slot', 'logical-checkpoint', 'run', '{}');
         INSERT INTO observation_slots VALUES ('other-obs-slot', 'logical-obs', 'other-run', '{}');
         INSERT INTO attempt_slots VALUES ('slot', 'logical-slot', 'run', 'stream', 'measurement', '{}');
         INSERT INTO attempt_slots VALUES ('other-slot', 'logical-slot', 'other-run', 'one_shot', 'measurement', '{}');
         INSERT INTO attempt_observation_slots VALUES ('slot', 'obs-slot', 'run');
         INSERT INTO attempt_observation_slots VALUES ('slot', 'checkpoint-slot', 'run');
         INSERT INTO attempt_observation_slots VALUES ('other-slot', 'other-obs-slot', 'other-run');",
    )
    .unwrap();
}

fn reserve(db: &mut Connection, id: &str, slot: &str, retry: i64) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO attempts(id, slot_id, retry_index, state, revision, started_at_ns, updated_at_ns)
         VALUES (?1, ?2, ?3, 'reserved', 0, 0, 0)",
        (id, slot, retry),
    )?;
    tx.execute(
        "INSERT INTO events(attempt_id, occurred_at_ns, kind, payload_json) VALUES (?1, 0, 'attempt_reserved', '{}')",
        [id],
    )?;
    tx.execute(
        "INSERT INTO attempt_transitions VALUES (?1, 0, NULL, NULL, 'reserved', 0, ?2)",
        (id, tx.last_insert_rowid()),
    )?;
    tx.commit()
}

fn advance(
    db: &mut Connection,
    id: &str,
    state: &str,
    artifact: Option<&str>,
) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    let (revision, previous, time): (i64, String, i64) = tx.query_row(
        "SELECT revision, state, updated_at_ns FROM attempts WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    tx.execute(
        "INSERT INTO events(attempt_id, occurred_at_ns, kind, payload_json) VALUES (?1, ?2, 'attempt_transition', '{}')",
        (id, time + 1),
    )?;
    tx.execute(
        "INSERT INTO attempt_transitions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            id,
            revision + 1,
            revision,
            previous,
            state,
            time + 1,
            tx.last_insert_rowid(),
        ),
    )?;
    let terminal = matches!(state, "accepted" | "censored" | "superseded" | "failed");
    tx.execute(
        "UPDATE attempts SET state = ?2, revision = ?3, updated_at_ns = ?4,
         finished_at_ns = ?5, failure_code = ?6, final_artifact_id = ?7 WHERE id = ?1",
        (
            id,
            state,
            revision + 1,
            time + 1,
            terminal.then_some(time + 1),
            (state == "failed").then_some("worker_crash"),
            artifact,
        ),
    )?;
    tx.commit()
}

fn artifact(db: &Connection, id: &str, kind: &str) {
    db.execute(
        "INSERT INTO artifacts VALUES (?1, ?2, ?1, ?3, 'directory', ?3, 0)",
        (id, kind, "a".repeat(64)),
    )
    .unwrap();
}

fn observation(db: &Connection, id: &str, slot: &str, attempt: &str, valid: bool) {
    artifact(db, id, "observation");
    db.execute(
        "INSERT INTO observations VALUES (?1, ?2, ?3, 'slot', ?4, 'not_reached', ?1, 'observation', '{}')",
        (id, slot, attempt, valid),
    )
    .unwrap();
}

fn claim(db: &Connection, attempt: &str, slot: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO leases(id, owner_token, attempt_id, purpose, acquired_at_ns, expires_at_ns)
         VALUES (?1, 'owner', ?1, 'finalization', 0, 10)",
        [attempt],
    )?;
    db.execute(
        "INSERT INTO finalization_claims(slot_id, attempt_id, lease_id) VALUES (?1, ?2, ?2)",
        (slot, attempt),
    )?;
    Ok(())
}

fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn applicability_slot_membership_and_retry_constraints_bind_resolved_runs() {
    let mut db = database();
    seed(&db);
    for sql in [
        "INSERT INTO resolved_runs VALUES ('bad', 'logical-run', 'other-candidate', 'excluded', 1, '{}');",
        "INSERT INTO resolved_runs VALUES ('bad', 'logical-run', 'wrong-candidate', 'applicable', 1, '{}');",
        "INSERT INTO observation_slots VALUES ('duplicate', 'logical-obs', 'run', '{}');",
        "INSERT INTO attempt_slots VALUES ('duplicate', 'logical-slot', 'run', 'one_shot', 'measurement', '{}');",
        "INSERT INTO attempt_observation_slots VALUES ('other-slot', 'checkpoint-slot', 'other-run');",
        "INSERT INTO attempt_observation_slots VALUES ('slot', 'other-obs-slot', 'run');",
    ] {
        reject(&db, sql);
    }
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    assert!(reserve(&mut db, "duplicate", "slot", 0).is_err());
    assert!(reserve(&mut db, "negative", "slot", -1).is_err());
    reserve(&mut db, "retry", "slot", 1).unwrap();
    reserve(&mut db, "independent", "other-slot", 0).unwrap();
    assert_eq!(count(&db, "attempts"), 3);
}

#[test]
fn reservation_requires_its_transition_and_event_in_the_same_transaction() {
    let mut db = database();
    seed(&db);
    {
        let tx = db.transaction().unwrap();
        tx.execute_batch("INSERT INTO attempts(id, slot_id, retry_index, state, revision, started_at_ns, updated_at_ns) VALUES ('incomplete', 'slot', 0, 'reserved', 0, 0, 0);").unwrap();
        assert!(tx.commit().is_err());
    }
    assert_eq!(count(&db, "attempts"), 0);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    reject(
        &db,
        "UPDATE attempts SET state = 'materializing', revision = 1, updated_at_ns = 1 WHERE id = 'attempt';",
    );
    assert_eq!(count(&db, "attempt_transitions"), 1);
    assert_eq!(count(&db, "events"), 1);
}

#[test]
fn transitions_cover_execution_checkpoints_and_every_terminal_outcome() {
    for terminal in ["accepted", "failed", "censored", "superseded"] {
        let mut db = database();
        seed(&db);
        reserve(&mut db, "attempt", "slot", 0).unwrap();
        assert!(advance(&mut db, "attempt", "executing", None).is_err());
        for state in [
            "materializing",
            "starting",
            "preparing",
            "executing",
            "checkpointing",
            "validating",
            "evaluating",
            "executing",
            "validating",
            "evaluating",
            "finalizing",
        ] {
            advance(&mut db, "attempt", state, None).unwrap();
        }
        artifact(&db, "attempt-directory", "attempt");
        if terminal == "accepted" {
            assert!(advance(&mut db, "attempt", terminal, Some("attempt-directory")).is_err());
            claim(&db, "attempt", "slot").unwrap();
        }
        assert!(advance(&mut db, "attempt", terminal, None).is_err());
        advance(&mut db, "attempt", terminal, Some("attempt-directory")).unwrap();
        assert!(advance(&mut db, "attempt", "finalizing", None).is_err());
        reject(
            &db,
            "UPDATE attempts SET revision = revision + 1 WHERE id = 'attempt';",
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
fn transitions_cannot_reuse_other_attempt_events_or_skip_history() {
    let mut db = database();
    seed(&db);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    reserve(&mut db, "retry", "slot", 1).unwrap();
    db.execute_batch("INSERT INTO events(sequence, attempt_id, occurred_at_ns, kind, payload_json) VALUES (3, 'retry', 1, 'transition', '{}');").unwrap();
    reject(
        &db,
        "INSERT INTO attempt_transitions VALUES ('attempt', 1, 0, 'reserved', 'materializing', 1, 3);",
    );
    db.execute_batch("INSERT INTO events(sequence, attempt_id, occurred_at_ns, kind, payload_json) VALUES (4, 'attempt', 1, 'transition', '{}');").unwrap();
    for sql in [
        "INSERT INTO attempt_transitions VALUES ('attempt', 2, 1, 'materializing', 'starting', 1, 4);",
        "INSERT INTO attempt_transitions VALUES ('attempt', 1, 0, 'reserved', 'materializing', 2, 4);",
        "INSERT INTO attempt_transitions VALUES ('attempt', 1, NULL, NULL, 'materializing', 1, 4);",
        "DELETE FROM attempt_transitions;",
    ] {
        reject(&db, sql);
    }
}

#[test]
fn valid_checkpoints_survive_failure_and_retry_cannot_replace_selection() {
    let mut db = database();
    seed(&db);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    observation(&db, "first", "obs-slot", "attempt", true);
    observation(&db, "second", "checkpoint-slot", "attempt", true);
    db.execute_batch("INSERT INTO selected_observations(slot_id, observation_id, selected_at_ns) VALUES ('obs-slot', 'first', 1), ('checkpoint-slot', 'second', 2);").unwrap();
    advance(&mut db, "attempt", "finalizing", None).unwrap();
    artifact(&db, "failed-directory", "attempt");
    advance(&mut db, "attempt", "failed", Some("failed-directory")).unwrap();
    reserve(&mut db, "retry", "slot", 1).unwrap();
    observation(&db, "recomputed", "obs-slot", "retry", true);
    reject(
        &db,
        "INSERT INTO selected_observations(slot_id, observation_id, selected_at_ns) VALUES ('obs-slot', 'recomputed', 3);",
    );
    reject(
        &db,
        "INSERT OR REPLACE INTO selected_observations(slot_id, observation_id, selected_at_ns) VALUES ('obs-slot', 'recomputed', 3);",
    );
    reject(
        &db,
        "UPDATE selected_observations SET observation_id = 'recomputed' WHERE slot_id = 'obs-slot';",
    );
    reject(&db, "DELETE FROM selected_observations;");
    reject(&db, "DELETE FROM observations;");
    reject(
        &db,
        "UPDATE observations SET producing_attempt_id = 'retry' WHERE id = 'first';",
    );
    assert_eq!(count(&db, "selected_observations"), 2);
    assert_eq!(count(&db, "observations"), 3);
    assert_eq!(
        db.query_row(
            "SELECT observation_id FROM selected_observations WHERE slot_id = 'obs-slot'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "first"
    );
}

#[test]
fn selection_requires_valid_published_observation_from_the_permitted_attempt_slot() {
    let mut db = database();
    seed(&db);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    reserve(&mut db, "other-attempt", "other-slot", 0).unwrap();
    observation(&db, "invalid", "obs-slot", "attempt", false);
    artifact(&db, "orphan", "observation");
    for sql in [
        "INSERT INTO selected_observations(slot_id, observation_id, selected_at_ns) VALUES ('obs-slot', 'invalid', 0);",
        "INSERT INTO selected_observations(slot_id, observation_id, selected_at_ns) VALUES ('obs-slot', 'orphan', 0);",
        "INSERT INTO observations VALUES ('wrong', 'obs-slot', 'other-attempt', 'other-slot', 1, 'reached', 'orphan', 'observation', '{}');",
        "INSERT INTO observations VALUES ('wrong', 'other-obs-slot', 'attempt', 'slot', 1, 'reached', 'orphan', 'observation', '{}');",
        "INSERT INTO observations VALUES ('missing', 'obs-slot', 'attempt', 'slot', 1, 'reached', 'missing-artifact', 'observation', '{}');",
    ] {
        reject(&db, sql);
    }
    assert_eq!(count(&db, "selected_observations"), 0);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM attempts WHERE state = 'accepted'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn claims_reserve_one_winner_without_accepting_it_or_allowing_expiry_theft() {
    let mut db = database();
    seed(&db);
    reserve(&mut db, "winner", "slot", 0).unwrap();
    reserve(&mut db, "loser", "slot", 1).unwrap();
    assert!(claim(&db, "winner", "slot").is_err());
    advance(&mut db, "winner", "finalizing", None).unwrap();
    db.execute_batch("INSERT INTO finalization_claims(slot_id, attempt_id, lease_id) VALUES ('slot', 'winner', 'winner');").unwrap();
    advance(&mut db, "loser", "finalizing", None).unwrap();
    assert!(claim(&db, "loser", "slot").is_err());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM attempts WHERE state = 'accepted'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    for sql in [
        "DELETE FROM finalization_claims WHERE slot_id = 'slot';",
        "UPDATE finalization_claims SET attempt_id = 'loser', lease_id = 'loser';",
        "UPDATE leases SET owner_token = 'thief' WHERE id = 'winner';",
        "UPDATE leases SET expires_at_ns = 9 WHERE id = 'winner';",
    ] {
        reject(&db, sql);
    }
    artifact(&db, "failed-directory", "attempt");
    advance(&mut db, "winner", "failed", Some("failed-directory")).unwrap();
    db.execute_batch("DELETE FROM finalization_claims WHERE slot_id = 'slot'; INSERT INTO finalization_claims(slot_id, attempt_id, lease_id) VALUES ('slot', 'loser', 'loser');").unwrap();
    artifact(&db, "accepted-directory", "attempt");
    advance(&mut db, "loser", "accepted", Some("accepted-directory")).unwrap();
    db.execute_batch("DELETE FROM finalization_claims;")
        .unwrap();
    reserve(&mut db, "late", "slot", 2).unwrap();
    advance(&mut db, "late", "finalizing", None).unwrap();
    assert!(claim(&db, "late", "slot").is_err());
    assert!(advance(&mut db, "late", "accepted", Some("accepted-directory")).is_err());
    let (unique, partial): (bool, bool) = db.query_row("SELECT \"unique\", partial FROM pragma_index_list('attempts') WHERE name = 'one_accepted_attempt_per_slot'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert!(unique && partial);
}

#[test]
fn claim_cannot_borrow_another_attempts_lease_or_a_released_lease() {
    let mut db = database();
    seed(&db);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    reserve(&mut db, "retry", "slot", 1).unwrap();
    advance(&mut db, "attempt", "finalizing", None).unwrap();
    db.execute_batch("INSERT INTO leases(id, owner_token, attempt_id, purpose, acquired_at_ns, expires_at_ns) VALUES ('lease', 'owner', 'retry', 'finalization', 0, 10);").unwrap();
    reject(
        &db,
        "INSERT INTO finalization_claims(slot_id, attempt_id, lease_id) VALUES ('slot', 'attempt', 'lease');",
    );
    db.execute_batch("INSERT INTO leases(id, owner_token, attempt_id, purpose, acquired_at_ns, expires_at_ns, released_at_ns) VALUES ('released', 'owner', 'attempt', 'finalization', 0, 10, 1);").unwrap();
    reject(
        &db,
        "INSERT INTO finalization_claims(slot_id, attempt_id, lease_id) VALUES ('slot', 'attempt', 'released');",
    );
    reject(
        &db,
        "UPDATE leases SET released_at_ns = NULL WHERE id = 'released';",
    );
}

#[test]
fn artifact_references_are_typed_and_preserve_gc_roots() {
    let mut db = database();
    seed(&db);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    observation(&db, "observation", "obs-slot", "attempt", true);
    artifact(&db, "dataset", "dataset");
    db.execute_batch("INSERT INTO attempt_artifacts VALUES ('attempt', 'dataset', 'dataset');
        INSERT INTO observation_artifacts VALUES ('observation', 'dataset', 'dataset');
        INSERT INTO leases(id, owner_token, purpose, acquired_at_ns, expires_at_ns) VALUES ('lease', 'owner', 'artifact', 0, 10);
        INSERT INTO leased_artifacts VALUES ('lease', 'dataset');").unwrap();
    reject(
        &db,
        "INSERT INTO attempt_artifacts VALUES ('attempt', 'missing', 'absent');",
    );
    reject(
        &db,
        "INSERT INTO observation_artifacts VALUES ('absent', 'dataset', 'dataset');",
    );
    reject(&db, "DELETE FROM attempt_artifacts;");
    reject(&db, "DELETE FROM artifacts WHERE id = 'dataset';");
    reject(
        &db,
        "INSERT INTO observations VALUES ('wrong-kind', 'checkpoint-slot', 'attempt', 'slot', 1, 'reached', 'dataset', 'observation', '{}');",
    );
    advance(&mut db, "attempt", "finalizing", None).unwrap();
    assert!(advance(&mut db, "attempt", "failed", Some("dataset")).is_err());
    assert_eq!(count(&db, "attempt_artifacts"), 1);
    assert_eq!(count(&db, "observation_artifacts"), 1);
    assert_eq!(count(&db, "leased_artifacts"), 1);
}

#[test]
fn artifact_and_staging_paths_are_normalized_relative_paths() {
    let db = database();
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        ".",
        "a/./b",
        "a//b",
        "trailing/",
        "a\\b",
        "nul\0byte",
    ] {
        let error = db
            .execute(
                "INSERT INTO artifacts VALUES ('bad', 'other', ?1, ?2, 'directory', ?2, 0)",
                (path, "a".repeat(64)),
            )
            .unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(ErrorCode::ConstraintViolation),
            "{path:?}"
        );
    }
    reject(
        &db,
        "INSERT INTO artifacts VALUES ('bad', 'other', 'ok', 'wrong-digest', 'file', NULL, 0);",
    );
}

#[test]
fn leases_preserve_reservation_and_staging_ownership_history() {
    let mut db = database();
    seed(&db);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    db.execute_batch("INSERT INTO leases(id, owner_token, attempt_id, purpose, acquired_at_ns, expires_at_ns) VALUES ('reservation', 'owner', 'attempt', 'reservation', 0, 10);").unwrap();
    reject(
        &db,
        "INSERT INTO leases(id, owner_token, attempt_id, purpose, acquired_at_ns, expires_at_ns) VALUES ('duplicate', 'owner', 'attempt', 'reservation', 0, 10);",
    );
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "a/./b",
        "a//b",
        "trailing/",
        "a\\b",
        "nul\0byte",
    ] {
        let error = db.execute(
            "INSERT INTO leases(id, owner_token, attempt_id, purpose, staging_path, acquired_at_ns, expires_at_ns) VALUES ('staging', 'owner', 'attempt', 'staging', ?1, 0, 10)",
            [path],
        ).unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(ErrorCode::ConstraintViolation),
            "{path:?}"
        );
    }
    db.execute_batch("UPDATE leases SET expires_at_ns = 20 WHERE id = 'reservation';
        INSERT INTO leases(id, owner_token, attempt_id, purpose, staging_path, acquired_at_ns, expires_at_ns) VALUES ('staging', 'owner', 'attempt', 'staging', 'staging/unique', 0, 10);").unwrap();
    reject(
        &db,
        "UPDATE leases SET staging_path = 'staging/changed' WHERE id = 'staging';",
    );
    reject(
        &db,
        "INSERT INTO leases(id, owner_token, purpose, acquired_at_ns, expires_at_ns) VALUES ('missing-attempt', 'owner', 'reservation', 0, 10);",
    );
    reject(
        &db,
        "INSERT INTO leases(id, owner_token, purpose, acquired_at_ns, expires_at_ns) VALUES ('bad-expiry', 'owner', 'artifact', 10, 10);",
    );
    db.execute_batch("UPDATE leases SET released_at_ns = 1 WHERE id = 'reservation';")
        .unwrap();
    reject(
        &db,
        "UPDATE leases SET expires_at_ns = 30 WHERE id = 'reservation';",
    );
    db.execute_batch("INSERT INTO leases(id, owner_token, attempt_id, purpose, acquired_at_ns, expires_at_ns) VALUES ('replacement', 'owner', 'attempt', 'reservation', 1, 11);").unwrap();
    assert_eq!(count(&db, "leases"), 3);
}

#[test]
fn competing_connections_cannot_select_two_observations_for_one_slot() {
    use std::{
        sync::{Arc, Barrier},
        thread,
        time::Duration,
    };

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys = ON; PRAGMA recursive_triggers = ON;")
        .unwrap();
    db.execute_batch(RUN_STORE_SCHEMA_SQL).unwrap();
    seed(&db);
    reserve(&mut db, "attempt", "slot", 0).unwrap();
    reserve(&mut db, "retry", "slot", 1).unwrap();
    observation(&db, "first", "obs-slot", "attempt", true);
    observation(&db, "second", "obs-slot", "retry", true);
    let barrier = Arc::new(Barrier::new(2));
    let writers: Vec<_> = ["first", "second"].into_iter().map(|id| {
        let path = path.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            let connection = Connection::open(path).unwrap();
            connection.busy_timeout(Duration::from_secs(5)).unwrap();
            connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA recursive_triggers = ON;").unwrap();
            barrier.wait();
            connection.execute("INSERT INTO selected_observations(slot_id, observation_id, selected_at_ns) VALUES ('obs-slot', ?1, 1)", [id])
        })
    }).collect();
    let outcomes: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let error = outcomes.into_iter().find_map(Result::err).unwrap();
    assert_eq!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
    assert_eq!(count(&db, "selected_observations"), 1);
}
