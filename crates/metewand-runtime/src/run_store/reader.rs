//! Inspection never opens the authoritative SQLite files through SQLite's VFS.

use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{RUN_STORE_SCHEMA_VERSION, RunStoreError, durable::verify_schema};

pub(crate) struct StoreSnapshot {
    pub root: PathBuf,
    pub tables: BTreeMap<&'static str, Vec<Value>>,
}

impl StoreSnapshot {
    pub fn read(root: &Path) -> Result<Self, RunStoreError> {
        if !fs::symlink_metadata(root)?.is_dir() {
            return Err(RunStoreError::UnsafePath(root.to_owned()));
        }
        let root = fs::canonicalize(root)?;
        let private = tempfile::Builder::new()
            .prefix("metewand-inspect-")
            .tempdir()?;
        copy_database_snapshot(&root, private.path(), "run.sqlite3")?;
        let connection = Connection::open_with_flags(
            private.path().join("run.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.pragma_update(None, "query_only", true)?;
        let version = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != RUN_STORE_SCHEMA_VERSION {
            return Err(RunStoreError::UnsupportedSchema {
                version,
                reason: "inspection requires an initialized supported store",
            });
        }
        verify_schema(&connection)?;
        let mut tables = BTreeMap::new();
        for (kind, sql) in QUERIES {
            let mut query = connection.prepare(sql)?;
            let records = query
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| {
                    serde_json::from_str(&row?).map_err(|_| {
                        RunStoreError::InvalidAttempt("stored inspection record is not JSON")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            tables.insert(*kind, records);
        }
        Ok(Self { root, tables })
    }
}

// Even mode=ro can create WAL sidecars. A private copy lets SQLite reconstruct
// its WAL index without touching the source, including after a writer's crash.
// Both files must remain unchanged throughout the copy so a checkpoint cannot
// mix main-file pages from different commits. A busy store fails boundedly.
pub(crate) fn copy_database_snapshot(
    root: &Path,
    target: &Path,
    name: &str,
) -> Result<(), RunStoreError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let database = open_source(&root.join(name))?;
        let wal_name = format!("{name}-wal");
        let wal = match open_source(&root.join(&wal_name)) {
            Ok(file) => Some(file),
            Err(RunStoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let database_before = signature(&database.metadata()?);
        let wal_before = wal
            .as_ref()
            .map(File::metadata)
            .transpose()?
            .as_ref()
            .map(signature);
        io::copy(&mut &database, &mut File::create(target.join(name))?)?;
        if let Some(wal) = &wal {
            io::copy(&mut &*wal, &mut File::create(target.join(&wal_name))?)?;
        } else {
            match fs::remove_file(target.join(&wal_name)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        let unchanged = source_signature(&root.join(name))? == Some(database_before)
            && source_signature(&root.join(&wal_name))? == wal_before;
        if unchanged {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(RunStoreError::Busy);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open_source(path: &Path) -> Result<File, RunStoreError> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(RunStoreError::UnsafePath(path.to_owned()));
    }
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?)
}

fn source_signature(path: &Path) -> Result<Option<[u64; 7]>, RunStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(signature(&metadata))),
        Ok(_) => Err(RunStoreError::UnsafePath(path.to_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn signature(metadata: &Metadata) -> [u64; 7] {
    [
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
    ]
}

const QUERIES: &[(&str, &str)] = &[
    (
        "applicability",
        "SELECT json_object('id', id, 'candidate_id', candidate_id, 'applicable', json(CASE applicable WHEN 1 THEN 'true' ELSE 'false' END), 'definition', json(record_json)) FROM applicability_decisions ORDER BY id",
    ),
    (
        "resolved-specification",
        "SELECT json_object('id', id, 'logical_specification_id', logical_specification_id, 'candidate_id', candidate_id, 'applicability_id', applicability_id, 'definition', json(record_json)) FROM resolved_runs ORDER BY id",
    ),
    (
        "resolved-attempt-slot",
        "SELECT json_object('id', id, 'logical_slot_id', logical_slot_id, 'resolved_run_id', resolved_run_id, 'role', role, 'kind', kind, 'definition', json(record_json)) FROM attempt_slots ORDER BY id",
    ),
    (
        "resolved-observation-slot",
        "SELECT json_object('id', id, 'logical_slot_id', logical_slot_id, 'resolved_run_id', resolved_run_id, 'definition', json(record_json)) FROM observation_slots ORDER BY id",
    ),
    (
        "attempt",
        "SELECT json_object('id', a.id, 'slot_id', a.slot_id, 'retry_index', a.retry_index, 'state', a.state, 'revision', a.revision, 'started_at_ns', CAST(a.started_at_ns AS TEXT), 'updated_at_ns', CAST(a.updated_at_ns AS TEXT), 'finished_at_ns', CAST(a.finished_at_ns AS TEXT), 'failure_code', a.failure_code, 'artifact_id', a.final_artifact_id, 'lease_expires_at_ns', CAST((SELECT max(expires_at_ns) FROM leases l WHERE l.attempt_id = a.id AND l.purpose = 'reservation' AND l.released_at_ns IS NULL) AS TEXT)) FROM attempts a ORDER BY a.slot_id, a.retry_index, a.id",
    ),
    (
        "observation",
        "SELECT json_object('id', o.id, 'slot_id', o.slot_id, 'attempt_id', o.producing_attempt_id, 'attempt_slot_id', o.attempt_slot_id, 'artifact_id', o.artifact_id, 'valid', json(CASE o.valid WHEN 1 THEN 'true' ELSE 'false' END), 'completion', o.completion, 'selected', json(CASE WHEN s.observation_id IS NULL THEN 'false' ELSE 'true' END), 'definition', json(o.record_json)) FROM observations o LEFT JOIN selected_observations s ON s.observation_id = o.id ORDER BY o.id",
    ),
    (
        "stored-artifact",
        "SELECT json_object('id', id, 'kind', kind, 'path', relative_path, 'sha256', content_sha256, 'storage_kind', storage_kind, 'manifest_sha256', manifest_sha256, 'verified_at_ns', CAST(verified_at_ns AS TEXT)) FROM artifacts ORDER BY id",
    ),
    (
        "attempt-artifact",
        "SELECT json_object('attempt_id', attempt_id, 'role', role, 'artifact_id', artifact_id) FROM attempt_artifacts ORDER BY attempt_id, role, artifact_id",
    ),
    (
        "observation-artifact",
        "SELECT json_object('observation_id', observation_id, 'role', role, 'artifact_id', artifact_id) FROM observation_artifacts ORDER BY observation_id, role, artifact_id",
    ),
    (
        "event",
        "SELECT json_object('sequence', sequence, 'attempt_id', attempt_id, 'occurred_at_ns', CAST(occurred_at_ns AS TEXT), 'kind', kind, 'payload', json(payload_json)) FROM events ORDER BY sequence",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_store::RunStore;

    fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn a_closed_wal_store_is_inspected_without_creating_sidecars() {
        let root = tempfile::tempdir().unwrap();
        drop(RunStore::open(root.path()).unwrap());
        assert!(!root.path().join("run.sqlite3-wal").exists());
        let before = files(root.path());
        let snapshot = StoreSnapshot::read(root.path()).unwrap();
        assert!(snapshot.tables["attempt"].is_empty());
        assert_eq!(files(root.path()), before);
    }

    #[test]
    fn live_wal_commits_are_visible_and_uncommitted_changes_are_excluded() {
        let root = tempfile::tempdir().unwrap();
        let store = RunStore::open(root.path()).unwrap();
        store.connection.execute("INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (0, 'committed', '{}')", []).unwrap();
        store.connection.execute_batch("BEGIN IMMEDIATE; INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (1, 'uncommitted', '{}');").unwrap();
        let before = files(root.path());
        let snapshot = StoreSnapshot::read(root.path()).unwrap();
        assert_eq!(snapshot.tables["event"].len(), 1);
        assert_eq!(snapshot.tables["event"][0]["kind"], "committed");
        assert_eq!(files(root.path()), before);
        store.connection.execute_batch("COMMIT").unwrap();
        assert_eq!(
            StoreSnapshot::read(root.path()).unwrap().tables["event"].len(),
            2
        );
    }

    #[test]
    fn missing_uninitialized_incompatible_and_symlinked_databases_are_not_changed() {
        let root = tempfile::tempdir().unwrap();
        assert!(StoreSnapshot::read(root.path()).is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        let path = root.path().join("run.sqlite3");
        let connection = Connection::open(&path).unwrap();
        for version in [0, 99] {
            connection
                .pragma_update(None, "user_version", version)
                .unwrap();
            let before = files(root.path());
            assert!(matches!(
                StoreSnapshot::read(root.path()),
                Err(RunStoreError::UnsupportedSchema { .. })
            ));
            assert_eq!(files(root.path()), before);
        }
        drop(connection);
        fs::rename(&path, root.path().join("other")).unwrap();
        std::os::unix::fs::symlink("other", &path).unwrap();
        assert!(matches!(
            StoreSnapshot::read(root.path()),
            Err(RunStoreError::UnsafePath(_))
        ));
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
