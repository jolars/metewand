//! Inspection never opens the authoritative SQLite files through SQLite's VFS.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io,
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
        if !crate::platform_fs::is_real_directory(&fs::symlink_metadata(root)?) {
            return Err(RunStoreError::UnsafePath(root.to_owned()));
        }
        let root = fs::canonicalize(root)?;
        let private = tempfile::Builder::new()
            .prefix("metewand-inspect-")
            .tempdir()?;
        crate::platform_fs::set_private_directory(private.path())?;
        copy_database_snapshot(&root, private.path(), "run.sqlite3")?;
        let connection = Connection::open_with_flags(
            private.path().join("run.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.pragma_update(None, "query_only", true)?;
        let version = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != RUN_STORE_SCHEMA_VERSION
            && version != super::BUFFERED_RUN_STORE_SCHEMA_VERSION
            && version != super::FRESH_RUN_STORE_SCHEMA_VERSION
        {
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
        if version == super::FRESH_RUN_STORE_SCHEMA_VERSION {
            for (kind, sql) in [
                (
                    "fresh-member",
                    "SELECT json_object('slot_id',slot_id,'resolved_run_id',resolved_run_id,'measurement_index',measurement_index,'schedule_index',schedule_index,'control',json(control_json),'observation_slot_id',(SELECT observation_slot_id FROM attempt_observation_slots WHERE attempt_slot_id=fresh_members.slot_id)) FROM fresh_members ORDER BY slot_id",
                ),
                (
                    "fresh-budget",
                    "SELECT json_object('attempt_id',attempt_id,'reserved_ns',reserved_ns,'charged_ns',charged_ns) FROM fresh_budget ORDER BY attempt_id",
                ),
                (
                    "fresh-disposition",
                    "SELECT json_object('slot_id',slot_id,'reason',reason,'causing_attempt_id',causing_attempt_id) FROM fresh_dispositions ORDER BY slot_id",
                ),
            ] {
                let mut query = connection.prepare(sql)?;
                let rows = query
                    .query_map([], |r| r.get::<_, String>(0))?
                    .map(|r| {
                        serde_json::from_str(&r?)
                            .map_err(|_| super::attempts::invalid("invalid fresh evidence"))
                    })
                    .collect::<Result<Vec<Value>, _>>()?;
                tables.insert(kind, rows);
            }
            tables.insert(
                "fresh-sequence",
                super::fresh::progress(&connection)?
                    .into_iter()
                    .map(|p| serde_json::to_value(p).expect("typed progress"))
                    .collect(),
            );
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
        #[cfg(windows)]
        let shared_memory = match windows_snapshot_locks(root, name, &database) {
            Ok(file) => file,
            Err(RunStoreError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(error),
        };
        let wal_name = format!("{name}-wal");
        let wal = match open_source(&root.join(&wal_name)) {
            Ok(file) => Some(file),
            Err(RunStoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let database_before = crate::platform_fs::file_signature(&database)?;
        let wal_before = wal
            .as_ref()
            .map(crate::platform_fs::file_signature)
            .transpose()?;
        io::copy(
            &mut io::Read::take(&database, database.metadata()?.len()),
            &mut File::create(target.join(name))?,
        )?;
        if let Some(wal) = &wal {
            io::copy(
                &mut io::Read::take(wal, wal.metadata()?.len()),
                &mut File::create(target.join(&wal_name))?,
            )?;
        } else {
            match fs::remove_file(target.join(&wal_name)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        let unchanged = source_signature(&root.join(name))? == Some(database_before)
            && source_signature(&root.join(&wal_name))? == wal_before;
        #[cfg(windows)]
        let unchanged = unchanged && {
            // A newly created WAL index needs its locks acquired on the next
            // pass. Existing indexes must retain the identity we locked.
            let current = source_signature(&root.join(format!("{name}-shm")))?;
            match &shared_memory {
                Some(file) => current.is_some_and(|signature| {
                    crate::platform_fs::file_signature(file)
                        .is_ok_and(|locked| signature[..3] == locked[..3])
                }),
                None => current.is_none(),
            }
        };
        if unchanged {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(RunStoreError::Busy);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn windows_snapshot_locks(
    root: &Path,
    name: &str,
    database: &File,
) -> Result<Option<File>, RunStoreError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::ERROR_LOCK_VIOLATION,
        Storage::FileSystem::{LOCKFILE_FAIL_IMMEDIATELY, LockFileEx},
        System::IO::OVERLAPPED,
    };

    fn lock(file: &File, offset: u32, length: u32) -> Result<(), RunStoreError> {
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = offset;
        // SAFETY: The owned file and synchronous OVERLAPPED stay live during
        // the call. Closing the file releases this shared byte-range lock.
        if unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_FAIL_IMMEDIATELY,
                0,
                length,
                0,
                &mut overlapped,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
                return Err(RunStoreError::Busy);
            }
            return Err(error.into());
        }
        Ok(())
    }

    // These are the bundled Windows SQLite VFS lock ranges. The database's
    // shared lock prevents mode changes and sidecar deletion. The WAL-index
    // checkpoint, recovery, and read locks prevent database backfill and WAL
    // reset while permitting an existing WAL writer to append. Windows file
    // timestamps alone cannot detect writes until their handles close.
    // https://sqlite.org/walformat.html#wal_locks
    lock(database, 0x4000_0002, 510)?;
    let shared_memory = match open_source(&root.join(format!("{name}-shm"))) {
        Ok(file) => file,
        Err(RunStoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    lock(&shared_memory, 121, 7)?;
    Ok(Some(shared_memory))
}

fn open_source(path: &Path) -> Result<File, RunStoreError> {
    if !crate::platform_fs::is_regular_file(&fs::symlink_metadata(path)?) {
        return Err(RunStoreError::UnsafePath(path.to_owned()));
    }
    Ok(crate::platform_fs::open_regular(
        path,
        OpenOptions::new().read(true),
    )?)
}

fn source_signature(path: &Path) -> Result<Option<[u64; 7]>, RunStoreError> {
    match open_source(path) {
        Ok(file) => Ok(Some(crate::platform_fs::file_signature(&file)?)),
        Err(RunStoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
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

    #[derive(Debug, Eq, PartialEq)]
    enum FileState {
        Bytes(Vec<u8>),
        #[cfg(windows)]
        LockedSharedMemory {
            len: u64,
            modified: std::time::SystemTime,
        },
    }

    fn file_state(path: &Path) -> FileState {
        match fs::read(path) {
            Ok(bytes) => FileState::Bytes(bytes),
            #[cfg(windows)]
            Err(error)
                if error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION as i32)
                    && path
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .ends_with(".sqlite3-shm") =>
            {
                // SQLite's mandatory Windows locks prevent separate handles
                // from reading its WAL index during a write transaction.
                let metadata = fs::metadata(path).unwrap();
                FileState::LockedSharedMemory {
                    len: metadata.len(),
                    modified: metadata.modified().unwrap(),
                }
            }
            Err(error) => panic!("cannot read {path:?}: {error}"),
        }
    }

    fn files(root: &Path) -> BTreeMap<String, FileState> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    file_state(&entry.path()),
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

    #[cfg(windows)]
    #[test]
    fn snapshot_locks_block_checkpoints_and_release_without_writing_source_files() {
        let root = tempfile::tempdir().unwrap();
        let store = RunStore::open(root.path()).unwrap();
        store.connection.execute("INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (0, 'committed', '{}')", []).unwrap();
        let checkpointer = Connection::open(store.database_path()).unwrap();
        checkpointer.busy_timeout(Duration::ZERO).unwrap();
        checkpointer
            .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, u32>(0))
            .unwrap();
        store.connection.execute_batch("BEGIN IMMEDIATE; INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (1, 'uncommitted', '{}');").unwrap();
        let before = files(root.path());
        let database = open_source(store.database_path()).unwrap();
        let shared_memory = windows_snapshot_locks(root.path(), "run.sqlite3", &database).unwrap();
        assert!(shared_memory.is_some());
        let busy: u32 = checkpointer
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| r.get(0))
            .unwrap();
        assert_eq!(busy, 1);
        assert_eq!(files(root.path()), before);
        store.connection.execute_batch("COMMIT").unwrap();
        drop((shared_memory, database));
        let busy: u32 = checkpointer
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
            .unwrap();
        assert_eq!(busy, 0);
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
        #[cfg(unix)]
        {
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
}
