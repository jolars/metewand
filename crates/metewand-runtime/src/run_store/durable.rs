use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use thiserror::Error;

use super::{RUN_STORE_SCHEMA_SQL, RUN_STORE_SCHEMA_VERSION};
use crate::publication::rename_no_replace;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A verified WAL/FULL connection to an explicitly authorized run-store root.
///
/// Opening the store mutates disk. The root must already exist. Opening never
/// infers acceptance from artifacts. The connection stays private so callers
/// cannot weaken its durability settings.
#[derive(Debug)]
pub struct RunStore {
    database_path: PathBuf,
    pub(super) connection: Connection,
}

impl RunStore {
    /// Initializes or reopens `run.sqlite3` in an existing absolute directory.
    ///
    /// Every open checks filesystem capabilities using disposable objects in
    /// that root, serializes initialization, verifies schema compatibility, and
    /// configures and verifies all connection-local settings. Lock contention
    /// waits at most five seconds per lock acquisition.
    ///
    /// # Errors
    ///
    /// Rejects unsafe paths, incompatible schemas, unavailable locking,
    /// synchronization, or atomic no-replace rename, and SQLite or I/O errors.
    pub fn open(root: &Path) -> Result<Self, RunStoreError> {
        Self::open_checked(root, &|_| Ok(()))
    }

    /// Absolute path of the database, for inspection and backup tooling.
    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) fn reconnect(&self) -> Result<Self, RunStoreError> {
        let connection = open_database(&self.database_path)?;
        configure(&connection)?;
        Ok(Self {
            database_path: self.database_path.clone(),
            connection,
        })
    }

    pub fn open_fresh(root: &Path) -> Result<Self, RunStoreError> {
        Self::open_version(root, &|_| Ok(()), 3)
    }

    pub fn open_buffered(root: &Path) -> Result<Self, RunStoreError> {
        Self::open_version(root, &|_| Ok(()), 2)
    }

    fn open_checked(root: &Path, check: &Check<'_>) -> Result<Self, RunStoreError> {
        Self::open_version(root, check, 1)
    }

    fn open_version(root: &Path, check: &Check<'_>, layout: u32) -> Result<Self, RunStoreError> {
        if crate::archive::is_collection(root) {
            return Err(RunStoreError::InvalidAttempt(
                "archive collections are read-only",
            ));
        }
        if !root.is_absolute()
            || !crate::platform_fs::is_real_directory(&fs::symlink_metadata(root)?)
        {
            return Err(RunStoreError::UnsafePath(root.to_owned()));
        }
        let root_file = crate::platform_fs::open_directory(root)?;
        reject_network_filesystem(&root_file)?;
        crate::platform_fs::check_storage_root(root)?;
        #[cfg(windows)]
        crate::platform_fs::set_private_directory(root)?;
        let lock = open_lock(&root.join(".run-store-init.lock"))?;
        let _guard = acquire_lock(lock, check)?;
        preflight(root, check)?;

        let database_path = root.join("run.sqlite3");
        for path in [
            database_path.clone(),
            root.join("run.sqlite3-wal"),
            root.join("run.sqlite3-shm"),
            root.join("run.sqlite3-journal"),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if !crate::platform_fs::is_regular_file(&metadata) => {
                    return Err(RunStoreError::UnsafePath(path));
                }
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        let mut connection = open_database(&database_path)?;
        // Check compatibility before changing persistent journal settings.
        verify_schema(&connection)?;
        configure(&connection)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 {
            verify_schema(&tx)?;
            tx.execute_batch(match layout {
                3 => super::FRESH_RUN_STORE_SCHEMA_SQL,
                2 => super::BUFFERED_RUN_STORE_SCHEMA_SQL,
                _ => RUN_STORE_SCHEMA_SQL,
            })?;
        }
        tx.commit()?;
        check(Operation::FileSync)?;
        crate::platform_fs::sync_owned_file(&database_path)?;
        sync_directory(root, check)?;
        Ok(Self {
            database_path,
            connection,
        })
    }
}

/// Store operations never fall back to a weaker durability mode.
#[derive(Debug, Error)]
pub enum RunStoreError {
    #[error("another attempt owns this fresh-sequence repetition")]
    SequenceBusy,
    #[error(transparent)]
    Publication(#[from] crate::publication::PublicationError),
    #[error("invalid attempt operation: {0}")]
    InvalidAttempt(&'static str),
    #[error("attempt ownership, lease, or expected revision is stale")]
    StaleAttempt,
    #[error("attempt slot is already satisfied")]
    SatisfiedSlot,
    #[error("slot has a previous attempt; retry requires explicit authorization")]
    RetryNotAuthorized,
    #[error("attempt identity failed: {0}")]
    Identity(#[from] metewand_core::identity::IdentityError),
    #[error("attempt event is not canonical JSON: {0}")]
    Canonical(#[from] metewand_core::canonical::CanonicalJsonError),
    #[error("run store requires an existing absolute directory and regular files: {0}")]
    UnsafePath(PathBuf),
    #[error("unsupported run-store schema version {version}: {reason}")]
    UnsupportedSchema { version: u32, reason: &'static str },
    #[error("required run-store capability is unavailable: {0}")]
    Capability(&'static str),
    #[error("run-store coordination lock is busy")]
    Busy,
    #[error("run-store I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("run-store SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

// This hook permits deterministic refusal tests before each required capability
// operation without exposing a production option that bypasses checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Lock,
    FileSync,
    DirectorySync,
    Rename,
}
type Check<'a> = dyn Fn(Operation) -> io::Result<()> + 'a;

fn open_lock(path: &Path) -> Result<File, RunStoreError> {
    let file = crate::platform_fs::open_regular(
        path,
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false),
    )?;
    if !file.metadata()?.is_file() {
        return Err(RunStoreError::UnsafePath(path.to_owned()));
    }
    Ok(file)
}

fn acquire_lock(file: File, check: &Check<'_>) -> Result<File, RunStoreError> {
    check(Operation::Lock)?;
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if start.elapsed() < BUSY_TIMEOUT => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => return Err(RunStoreError::Busy),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn sync_file(file: &File, check: &Check<'_>) -> Result<(), RunStoreError> {
    check(Operation::FileSync)?;
    crate::durability::sync_all(file)?;
    Ok(())
}

fn sync_directory(path: &Path, check: &Check<'_>) -> Result<(), RunStoreError> {
    check(Operation::DirectorySync)?;
    crate::platform_fs::complete_namespace_change(path)?;
    Ok(())
}

fn preflight(root: &Path, check: &Check<'_>) -> Result<(), RunStoreError> {
    let probe = tempfile::Builder::new()
        .prefix(".run-store-probe-")
        .tempdir_in(root)?;
    let path = probe.path();
    let first = open_lock(&path.join("lock"))?;
    let second = open_lock(&path.join("lock"))?;
    check(Operation::Lock)?;
    first
        .try_lock()
        .map_err(|_| RunStoreError::Capability("exclusive file locking"))?;
    match second.try_lock() {
        Err(TryLockError::WouldBlock) => (),
        Err(TryLockError::Error(error)) => return Err(error.into()),
        Ok(()) => {
            return Err(RunStoreError::Capability(
                "conflicting file locks were both granted",
            ));
        }
    }
    first.unlock()?;
    second
        .try_lock()
        .map_err(|_| RunStoreError::Capability("file lock release"))?;
    second.unlock()?;
    check(Operation::Lock)?;
    #[cfg(unix)]
    probe_record_lock(&first)?;
    drop((first, second));

    let source = path.join("source");
    let destination = path.join("destination");
    fs::create_dir(&source)?;
    let mut payload = File::create(source.join("payload"))?;
    payload.write_all(b"metewand durable initialization\n")?;
    sync_file(&payload, check)?;
    drop(payload);
    sync_directory(&source, check)?;
    sync_directory(path, check)?;
    check(Operation::Rename)?;
    rename_no_replace(&source, &destination)?;
    sync_directory(path, check)?;
    if source.exists()
        || fs::read(destination.join("payload"))? != b"metewand durable initialization\n"
    {
        return Err(RunStoreError::Capability(
            "directory rename did not preserve contents",
        ));
    }
    fs::create_dir(&source)?;
    // Empty destinations are replaceable by ordinary rename, so test both empty
    // and populated collisions to catch an accidental weaker implementation.
    for target in [&destination, &path.join("empty")] {
        if !target.exists() {
            fs::create_dir(target)?;
        }
        check(Operation::Rename)?;
        match rename_no_replace(&source, target) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
            Ok(()) => {
                return Err(RunStoreError::Capability(
                    "rename replaced an existing directory",
                ));
            }
        }
        if !source.is_dir() || !target.is_dir() {
            return Err(RunStoreError::Capability(
                "failed rename changed directory ownership",
            ));
        }
    }
    if fs::read(destination.join("payload"))? != b"metewand durable initialization\n" {
        return Err(RunStoreError::Capability(
            "rename collision changed published contents",
        ));
    }
    probe_sqlite(&path.join("probe.sqlite3"))?;
    probe.close()?;
    sync_directory(root, check)?;
    Ok(())
}

// POSIX record locks belong to a process, so independent descriptors in the
// parent cannot verify exclusion. The child performs only async-signal-safe
// fcntl and _exit operations and never touches SQLite or Rust-owned state.
#[cfg(unix)]
fn probe_record_lock(file: &File) -> Result<(), RunStoreError> {
    use rustix::fs::{FlockOperation, fcntl_lock};

    fcntl_lock(file, FlockOperation::NonBlockingLockExclusive).map_err(io::Error::from)?;
    let result = record_lock_child(file, true);
    fcntl_lock(file, FlockOperation::NonBlockingUnlock).map_err(io::Error::from)?;
    result?;
    record_lock_child(file, false)
}

#[cfg(unix)]
fn record_lock_child(file: &File, should_conflict: bool) -> Result<(), RunStoreError> {
    // SAFETY: The child calls only the async-signal-safe fcntl and _exit
    // operations, using a descriptor opened before fork. It never unwinds,
    // allocates, drops inherited objects, or returns into the caller.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if pid == 0 {
        let result =
            rustix::fs::fcntl_lock(file, rustix::fs::FlockOperation::NonBlockingLockExclusive);
        let valid = if should_conflict {
            matches!(
                result,
                Err(rustix::io::Errno::AGAIN | rustix::io::Errno::ACCESS)
            )
        } else {
            result.is_ok()
        };
        // SAFETY: Exit immediately without running inherited destructors.
        unsafe { libc::_exit(if valid { 0 } else { 1 }) }
    }
    let start = Instant::now();
    loop {
        let mut status = 0;
        // SAFETY: pid identifies our unreaped child and status is writable.
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if waited == pid {
            return if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                Ok(())
            } else {
                Err(RunStoreError::Capability(
                    "cross-process POSIX record locking",
                ))
            };
        }
        if waited < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if start.elapsed() >= BUSY_TIMEOUT {
            // SAFETY: Only our own unreaped child is terminated. Reap it even
            // if interrupted so failed preflight does not leave a zombie.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                while libc::waitpid(pid, &mut status, 0) < 0 {
                    if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                        break;
                    }
                }
            }
            return Err(RunStoreError::Capability(
                "POSIX record-lock probe timed out",
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn reject_network_filesystem(file: &File) -> Result<(), RunStoreError> {
    #[cfg(target_os = "linux")]
    {
        let kind = rustix::fs::fstatfs(file).map_err(io::Error::from)?.f_type;
        // Linux UAPI magic.h identifies known network filesystems. Successful
        // local probes cannot establish WAL shared-memory safety across hosts.
        if matches!(
            kind as u32,
            0x6969
                | 0x517b
                | 0xff534d42
                | 0xfe534d42
                | 0x5346414f
                | 0x6b414653
                | 0x00c36400
                | 0x73757245
                | 0x564c
                | 0x01021997
        ) {
            return Err(RunStoreError::Capability(
                "SQLite WAL on a network filesystem",
            ));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = file;
    Ok(())
}

fn open_database(path: &Path) -> Result<Connection, RunStoreError> {
    #[cfg(test)]
    crate::fault_injection::hit("database.open.before");
    // Exclude URI interpretation and VFS overrides that can disable locking.
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    #[cfg(test)]
    crate::fault_injection::hit("database.open.after");
    #[cfg(test)]
    crate::fault_injection::trace_database(&connection);
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(connection)
}

fn configure(connection: &Connection) -> Result<(), RunStoreError> {
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA recursive_triggers = ON;
         PRAGMA locking_mode = NORMAL;
         PRAGMA synchronous = FULL;
         PRAGMA fullfsync = ON;
         PRAGMA checkpoint_fullfsync = ON;
         PRAGMA journal_mode = WAL;",
    )?;
    for (pragma, expected) in [
        ("foreign_keys", 1),
        ("recursive_triggers", 1),
        ("synchronous", 2),
        ("fullfsync", 1),
        ("checkpoint_fullfsync", 1),
    ] {
        if connection.pragma_query_value::<u32, _>(None, pragma, |r| r.get(0))? != expected {
            return Err(RunStoreError::Capability(pragma));
        }
    }
    for (pragma, expected) in [("journal_mode", "wal"), ("locking_mode", "normal")] {
        if connection.pragma_query_value::<String, _>(None, pragma, |r| r.get(0))? != expected {
            return Err(RunStoreError::Capability(pragma));
        }
    }
    Ok(())
}

type SchemaObject = (String, String, String, Option<String>);

fn schema_objects(connection: &Connection) -> rusqlite::Result<Vec<SchemaObject>> {
    connection.prepare(
        "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name"
    )?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect()
}

pub(super) fn verify_schema(connection: &Connection) -> Result<(), RunStoreError> {
    let version = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let objects = schema_objects(connection)?;
    if version == 0 && objects.is_empty() {
        return Ok(());
    }
    if [
        RUN_STORE_SCHEMA_VERSION,
        super::BUFFERED_RUN_STORE_SCHEMA_VERSION,
        super::FRESH_RUN_STORE_SCHEMA_VERSION,
    ]
    .contains(&version)
    {
        let expected = Connection::open_in_memory()?;
        expected.execute_batch(match version {
            3 => super::FRESH_RUN_STORE_SCHEMA_SQL,
            2 => super::BUFFERED_RUN_STORE_SCHEMA_SQL,
            _ => RUN_STORE_SCHEMA_SQL,
        })?;
        if objects == schema_objects(&expected)? {
            return Ok(());
        }
    }
    Err(RunStoreError::UnsupportedSchema {
        version,
        reason: "expected an empty database or the exact supported layout",
    })
}

fn probe_sqlite(path: &Path) -> Result<(), RunStoreError> {
    let mut writer = open_database(path)?;
    configure(&writer)?;
    writer.execute_batch(
        "CREATE TABLE probe(value INTEGER NOT NULL CHECK(json_valid('{}'))) STRICT;",
    )?;
    let reader = open_database(path)?;
    configure(&reader)?;
    reader.busy_timeout(Duration::ZERO)?;
    let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("INSERT INTO probe VALUES (1)", [])?;
    match reader.execute_batch("BEGIN IMMEDIATE;") {
        Err(error) if error.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) => (),
        Err(error) => return Err(error.into()),
        Ok(()) => {
            return Err(RunStoreError::Capability(
                "SQLite granted conflicting write transactions",
            ));
        }
    }
    let count = || reader.query_row("SELECT count(*) FROM probe", [], |r| r.get::<_, u32>(0));
    if count()? != 0 {
        return Err(RunStoreError::Capability("SQLite exposed uncommitted data"));
    }
    tx.commit()?;
    if count()? != 1 {
        return Err(RunStoreError::Capability("SQLite commit was not visible"));
    }
    reader.execute_batch("BEGIN IMMEDIATE; INSERT INTO probe VALUES (2); ROLLBACK;")?;
    if count()? != 1 {
        return Err(RunStoreError::Capability(
            "SQLite rollback did not discard writes",
        ));
    }
    let busy: u32 = reader.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    if busy != 0 {
        return Err(RunStoreError::Capability("SQLite WAL checkpoint"));
    }
    reader.close().map_err(|(_, error)| error)?;
    writer.close().map_err(|(_, error)| error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_connection_enforces_full_durability_and_integrity_guards() {
        let root = tempfile::tempdir().unwrap();
        for _ in 0..2 {
            let store = RunStore::open(root.path()).unwrap();
            for (pragma, expected) in [
                ("synchronous", 2),
                ("foreign_keys", 1),
                ("recursive_triggers", 1),
                ("busy_timeout", 5000),
                ("fullfsync", 1),
                ("checkpoint_fullfsync", 1),
            ] {
                assert_eq!(
                    store
                        .connection
                        .pragma_query_value::<u32, _>(None, pragma, |r| r.get(0))
                        .unwrap(),
                    expected
                );
            }
            assert!(store.connection.execute("INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (0, 'test', '{}')", []).is_ok());
            assert!(store.connection.execute("DELETE FROM events", []).is_err());
        }
    }

    #[test]
    fn unavailable_capabilities_refuse_initialization_before_creating_the_database() {
        for operation in [
            Operation::Lock,
            Operation::FileSync,
            Operation::DirectorySync,
            Operation::Rename,
        ] {
            for kind in [
                io::ErrorKind::Unsupported,
                io::ErrorKind::PermissionDenied,
                io::ErrorKind::Other,
            ] {
                let root = tempfile::tempdir().unwrap();
                let result = RunStore::open_checked(root.path(), &|current| {
                    if current == operation {
                        Err(io::Error::new(kind, "injected failure"))
                    } else {
                        Ok(())
                    }
                });
                assert!(
                    matches!(result, Err(RunStoreError::Io(error)) if error.kind() == kind),
                    "{operation:?}"
                );
                assert!(!root.path().join("run.sqlite3").exists());
                assert!(fs::read_dir(root.path()).unwrap().all(|e| {
                    !e.unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".run-store-probe-")
                }));
            }
        }
    }

    #[test]
    fn errors_at_later_sync_and_rename_boundaries_never_return_a_store() {
        use std::cell::Cell;
        let root = tempfile::tempdir().unwrap();
        let total = Cell::new(0);
        drop(
            RunStore::open_checked(root.path(), &|_| {
                total.set(total.get() + 1);
                Ok(())
            })
            .unwrap(),
        );
        for boundary in 0..total.get() {
            let root = tempfile::tempdir().unwrap();
            let count = Cell::new(0);
            let result = RunStore::open_checked(root.path(), &|_| {
                let current = count.get();
                count.set(current + 1);
                if current == boundary {
                    Err(io::Error::other("injected boundary failure"))
                } else {
                    Ok(())
                }
            });
            assert!(
                matches!(result, Err(RunStoreError::Io(_))),
                "boundary {boundary}"
            );
            // A late failure may leave an initialized database, but retry must
            // inspect it rather than overwrite its schema or invent history.
            let reopened = RunStore::open(root.path()).unwrap();
            assert_eq!(
                reopened
                    .connection
                    .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn record_lock_probe_rejects_missing_exclusion_and_missing_release() {
        let file = tempfile::tempfile().unwrap();
        assert!(matches!(
            record_lock_child(&file, true),
            Err(RunStoreError::Capability(_))
        ));
        rustix::fs::fcntl_lock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .unwrap();
        assert!(matches!(
            record_lock_child(&file, false),
            Err(RunStoreError::Capability(_))
        ));
        rustix::fs::fcntl_lock(&file, rustix::fs::FlockOperation::NonBlockingUnlock).unwrap();
        probe_record_lock(&file).unwrap();
    }

    #[test]
    fn a_vfs_without_wal_support_is_rejected() {
        let connection = Connection::open_in_memory().unwrap();
        assert!(matches!(
            configure(&connection),
            Err(RunStoreError::Capability("journal_mode"))
        ));
    }
}
