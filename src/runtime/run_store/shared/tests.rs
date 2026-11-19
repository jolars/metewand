use super::*;
use crate::runtime::run_store::reader::StoreSnapshot;
use serde_json::{Value, json};

fn event(store: &mut RunStore, time: i64, kind: &str) -> Result<(), RunStoreError> {
    let tx = store.transaction()?;
    tx.execute(
        "INSERT INTO events(occurred_at_ns,kind,payload_json) VALUES (?1,?2,'{}')",
        (time, kind),
    )?;
    tx.commit()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect()
}

#[test]
fn canonical_initialization_and_schema_columns_are_frozen_for_version_one() {
    let root = tempfile::tempdir().unwrap();
    let store = RunStore::open_shared(root.path()).unwrap();
    let expected = include_bytes!("../../../../schemas/v1/fixtures/durable-initialization-v1.json");
    assert_eq!(
        files(&root.path().join(JOURNAL)).values().next().unwrap(),
        expected
    );
    let schema: Value =
        serde_json::from_str(crate::core::public_schemas::PublicSchema::DurableRecord.source())
            .unwrap();
    for (name, columns) in collections(&store.connection).unwrap() {
        let actual = columns
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        let expected = schema["$defs"][&name]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            actual, expected,
            "{name}: changing persisted row fields requires an explicit compatibility decision"
        );
    }
}

#[test]
fn interrupted_initialization_can_resume_in_a_pristine_run_home() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(JOURNAL);
    fs::create_dir(&directory).unwrap();
    let pending = directory.join(".pending-initialization");
    fs::write(&pending, b"unpublished initialization").unwrap();

    drop(RunStore::open(root.path()).unwrap());

    assert_eq!(files(&directory).len(), 2);
    assert_eq!(fs::read(pending).unwrap(), b"unpublished initialization");
    assert!(StoreSnapshot::read(root.path()).is_ok());
}

#[test]
fn empty_journals_cannot_reset_populated_run_homes() {
    for entry in ["artifacts", ".cache-owner.json", "index.jsonl"] {
        let root = tempfile::tempdir().unwrap();
        let mut store = RunStore::open_shared(root.path()).unwrap();
        event(&mut store, 1, "committed").unwrap();
        drop(store);
        let directory = root.path().join(JOURNAL);
        for path in files(&directory).into_keys() {
            fs::remove_file(path).unwrap();
        }
        if entry == "artifacts" {
            fs::create_dir(root.path().join(entry)).unwrap();
        } else {
            fs::write(root.path().join(entry), b"persisted run data").unwrap();
        }

        assert!(StoreSnapshot::read(root.path()).is_err(), "{entry}");
        assert!(RunStore::open(root.path()).is_err(), "{entry}");
        assert!(RunStore::open_shared(root.path()).is_err(), "{entry}");
        assert!(files(&directory).is_empty(), "{entry}");
    }
}

#[test]
fn readers_are_read_only_and_ownership_outlives_reconnected_stores() {
    let root = tempfile::tempdir().unwrap();
    let mut store = RunStore::open_shared(root.path()).unwrap();
    let timestamp = 9_007_199_254_741_000_i64;
    event(&mut store, timestamp, "committed").unwrap();
    assert!(matches!(
        RunStore::open_shared(root.path()),
        Err(RunStoreError::Busy)
    ));
    let mut heartbeat = store.reconnect().unwrap();
    drop(store);
    assert!(matches!(
        RunStore::open(root.path()),
        Err(RunStoreError::Busy)
    ));
    event(&mut heartbeat, timestamp + 1, "heartbeat").unwrap();
    let saved = files(&root.path().join(JOURNAL));
    let snapshot = StoreSnapshot::read(root.path()).unwrap();
    assert_eq!(snapshot.tables["event"].len(), 2);
    assert_eq!(
        snapshot.tables["event"][0]["occurred_at_ns"],
        timestamp.to_string()
    );
    assert_eq!(saved, files(&root.path().join(JOURNAL)));
    let catalog = crate::core::public_schemas::public_schema_catalog().unwrap();
    for bytes in saved.values() {
        catalog
            .validate(
                crate::core::public_schemas::PublicSchema::DurableRecord.repository_path(),
                &serde_json::from_slice::<Value>(bytes).unwrap(),
            )
            .unwrap();
    }
    let scratch = heartbeat.database_path().parent().unwrap().to_owned();
    drop(heartbeat);
    assert!(!scratch.exists());
    let store = RunStore::open(root.path()).unwrap();
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT occurred_at_ns FROM events WHERE kind='committed'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        timestamp
    );
}

#[test]
fn failed_publication_revokes_further_local_commits_until_replay() {
    let root = tempfile::tempdir().unwrap();
    let mut store = RunStore::open_shared(root.path()).unwrap();
    event(&mut store, 1, "committed").unwrap();
    fs::rename(root.path().join(JOURNAL), root.path().join("saved")).unwrap();
    fs::write(root.path().join(JOURNAL), b"publication unavailable").unwrap();
    assert!(event(&mut store, 2, "unpublished").is_err());
    fs::remove_file(root.path().join(JOURNAL)).unwrap();
    fs::rename(root.path().join("saved"), root.path().join(JOURNAL)).unwrap();
    assert!(event(&mut store, 3, "must_reopen").is_err());
    drop(store);
    let mut store = RunStore::open(root.path()).unwrap();
    event(&mut store, 4, "reopened").unwrap();
    let kinds: Vec<String> = store
        .connection
        .prepare("SELECT kind FROM events ORDER BY sequence")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(kinds, ["committed", "reopened"]);
}

#[test]
fn replay_uses_primary_keys_for_every_collection() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(super::super::FRESH_RUN_STORE_SCHEMA_SQL)
        .unwrap();
    for (table, columns) in collections(&db).unwrap() {
        let collection = ReplayCollection::new(&db, &table, columns).unwrap();
        for sql in [collection.delete, collection.verify] {
            let plan: String = db
                .query_row(
                    &format!("EXPLAIN QUERY PLAN {sql}"),
                    params_from_iter(collection.columns.iter().map(|_| Option::<String>::None)),
                    |row| row.get(3),
                )
                .unwrap();
            assert!(
                plan.starts_with(&format!("SEARCH {table} USING ")),
                "{plan}"
            );
        }
    }
}

#[test]
fn replay_rejects_mismatched_before_images_for_existing_keys() {
    for (field, replacement) in [
        ("kind", json!("wrong kind")),
        ("occurred_at_ns", json!(null)),
        ("sequence", json!("01")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut store = RunStore::open_shared(root.path()).unwrap();
        event(&mut store, 1, "first").unwrap();
        event(&mut store, 2, "second").unwrap();
        drop(store);

        let directory = root.path().join(JOURNAL);
        let mut committed = files(&directory);
        let (path, bytes) = committed.pop_last().unwrap();
        let (_, previous) = committed.pop_last().unwrap();
        let first: Value = serde_json::from_slice(&previous).unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        let mut before = first["changes"][0]["after"].clone();
        before[field] = replacement;
        value["changes"][0]["before"] = before;
        let bytes = CanonicalValue::try_from(value)
            .unwrap()
            .to_canonical_bytes()
            .unwrap();
        fs::remove_file(path).unwrap();
        fs::write(directory.join(filename(2, &digest(&bytes))), bytes).unwrap();

        let error = RunStore::open_shared(root.path()).err().unwrap();
        assert!(
            error.to_string().contains("before-image differs"),
            "{field}: {error}"
        );
    }
}

#[test]
fn replay_rejects_corruption_gaps_unknown_versions_and_bad_row_shapes() {
    for failure in [
        "hash",
        "gap",
        "version",
        "row",
        "predecessor",
        "before_image",
        "integer_key_normalization",
        "integer_field_normalization",
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut store = RunStore::open_shared(root.path()).unwrap();
        event(&mut store, 1, "first").unwrap();
        event(&mut store, 2, "second").unwrap();
        drop(store);
        let directory = root.path().join(JOURNAL);
        let paths = files(&directory).keys().cloned().collect::<Vec<_>>();
        let path = &paths[1];
        let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        match failure {
            "hash" => {
                fs::write(path, b"{}").unwrap();
            }
            "gap" => {
                fs::remove_file(path).unwrap();
            }
            other => {
                match other {
                    "version" => value["schema_version"] = json!(2),
                    "row" => value["changes"][0]["after"]["unknown"] = json!("field"),
                    "predecessor" => value["previous_sha256"] = json!("0".repeat(64)),
                    "integer_key_normalization" => {
                        value["changes"][0]["after"]["sequence"] = json!("01")
                    }
                    "integer_field_normalization" => {
                        value["changes"][0]["after"]["occurred_at_ns"] = json!("01")
                    }
                    "before_image" => {
                        value["changes"][0]["before"] = value["changes"][0]["after"].clone()
                    }
                    _ => unreachable!(),
                }
                let bytes = CanonicalValue::try_from(value)
                    .unwrap()
                    .to_canonical_bytes()
                    .unwrap();
                fs::remove_file(path).unwrap();
                fs::write(directory.join(filename(1, &digest(&bytes))), bytes).unwrap();
            }
        }
        let before = files(&directory);
        assert!(StoreSnapshot::read(root.path()).is_err(), "{failure}");
        let error = RunStore::open_shared(root.path()).err().unwrap();
        if failure.ends_with("_normalization") {
            assert!(
                error.to_string().contains("after-image changes"),
                "{failure}: {error}"
            );
        }
        assert_eq!(before, files(&directory));
    }
}

#[cfg(unix)]
#[test]
fn shared_readers_reject_symlinked_journals_and_records() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    drop(RunStore::open_shared(root.path()).unwrap());
    let directory = root.path().join(JOURNAL);
    let path = files(&directory).into_keys().next().unwrap();
    let target = root.path().join("record");
    fs::rename(&path, &target).unwrap();
    symlink(&target, &path).unwrap();
    assert!(StoreSnapshot::read(root.path()).is_err());
    fs::remove_file(&path).unwrap();
    fs::rename(&target, &path).unwrap();
    let actual = root.path().join("actual");
    fs::rename(&directory, &actual).unwrap();
    symlink(&actual, &directory).unwrap();
    assert!(StoreSnapshot::read(root.path()).is_err());
    assert!(RunStore::open_shared(root.path()).is_err());
}
