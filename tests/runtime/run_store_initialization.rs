#![cfg(any(unix, windows))]

use std::{fs, sync::Arc, thread};

use crate::runtime::run_store::{RUN_STORE_SCHEMA_VERSION, RunStore, RunStoreError};
use rusqlite::Connection;
use tempfile::TempDir;

#[cfg(unix)]
#[test]
fn stores_and_snapshots_allow_symlinked_ancestors() {
    use std::process::Command;

    use crate::runtime::{artifact_store::ArtifactStore, run_store::reader::StoreSnapshot};

    if std::env::var_os("METEWAND_TEST_SYMLINKED_TEMP").is_some() {
        let root = TempDir::new().unwrap();
        let store = RunStore::open(root.path()).unwrap();
        assert!(StoreSnapshot::read(root.path()).unwrap().tables["attempt"].is_empty());
        drop(store.reconnect().unwrap());
        let cache = ArtifactStore::open(&root.path().join("cache")).unwrap();
        assert!(cache.roots().unwrap().targets.is_empty());
        return;
    }

    // macOS exposes its temporary directory through /var, a symlink to /private/var.
    let root = TempDir::new().unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(fs::canonicalize(root.path()).unwrap(), &alias).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime_tests::run_store_initialization::stores_and_snapshots_allow_symlinked_ancestors",
        ])
        .env("TMPDIR", alias)
        .env("METEWAND_TEST_SYMLINKED_TEMP", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn creates_and_reopens_a_versioned_wal_database_without_losing_history() {
    let root = TempDir::new().unwrap();
    let store = RunStore::open(root.path()).unwrap();
    let path = store.database_path().to_owned();
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value::<String, _>(None, "journal_mode", |r| r.get(0))
            .unwrap(),
        "wal"
    );
    assert_eq!(
        db.pragma_query_value::<u32, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        RUN_STORE_SCHEMA_VERSION
    );
    db.execute(
        "INSERT INTO events(occurred_at_ns, kind, payload_json) VALUES (0, 'test', '{}')",
        [],
    )
    .unwrap();
    drop(db);
    drop(store);
    let store = RunStore::open(root.path()).unwrap();
    let db = Connection::open(store.database_path()).unwrap();
    assert_eq!(
        db.query_row("SELECT kind FROM events", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "test"
    );
    assert!(
        !db.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    drop(db);
    drop(store);
    let mut names: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, [".run-store-init.lock", "run.sqlite3"]);
}

#[test]
fn rejects_unknown_versions_and_unversioned_nonempty_databases() {
    for sql in [
        "PRAGMA user_version = 999;",
        "CREATE TABLE unrelated(value);",
    ] {
        let root = TempDir::new().unwrap();
        let path = root.path().join("run.sqlite3");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(sql).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            RunStore::open(root.path()),
            Err(RunStoreError::UnsupportedSchema { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            db.pragma_query_value::<String, _>(None, "journal_mode", |r| r.get(0))
                .unwrap(),
            "delete"
        );
    }
}

#[test]
fn concurrent_initializers_share_one_complete_schema() {
    let root = Arc::new(TempDir::new().unwrap());
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let root = Arc::clone(&root);
            thread::spawn(move || RunStore::open(root.path()).unwrap())
        })
        .collect();
    let stores: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let db = Connection::open(stores[0].database_path()).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn requires_an_existing_absolute_directory_and_regular_database() {
    let root = TempDir::new().unwrap();
    for path in [
        std::path::Path::new("relative"),
        &root.path().join("missing"),
    ] {
        assert!(RunStore::open(path).is_err());
    }
    #[cfg(unix)]
    {
        let other = TempDir::new().unwrap();
        std::os::unix::fs::symlink(other.path(), root.path().join("link")).unwrap();
        assert!(RunStore::open(&root.path().join("link")).is_err());
        let target = other.path().join("target.sqlite3");
        fs::write(&target, b"do not change").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("run.sqlite3")).unwrap();
        assert!(RunStore::open(root.path()).is_err());
        assert_eq!(fs::read(target).unwrap(), b"do not change");
    }
}

#[test]
fn rejects_changes_to_the_versioned_schema() {
    let root = TempDir::new().unwrap();
    let store = RunStore::open(root.path()).unwrap();
    let db = Connection::open(store.database_path()).unwrap();
    drop(store);
    db.execute_batch("DROP TRIGGER events_no_delete;").unwrap();
    assert!(matches!(
        RunStore::open(root.path()),
        Err(RunStoreError::UnsupportedSchema { version: 1, .. })
    ));
}

#[test]
fn child_initializer() {
    let Some(root) = std::env::var_os("METEWAND_TEST_RUN_STORE_ROOT") else {
        return;
    };
    RunStore::open(std::path::Path::new(&root)).unwrap();
}

#[test]
fn concurrent_processes_initialize_one_store() {
    use std::process::{Command, Stdio};
    let root = TempDir::new().unwrap();
    let children: Vec<_> = (0..4)
        .map(|_| {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runtime_tests::run_store_initialization::child_initializer",
                ])
                .env("METEWAND_TEST_RUN_STORE_ROOT", root.path())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let store = RunStore::open(root.path()).unwrap();
    assert_eq!(
        Connection::open(store.database_path())
            .unwrap()
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn initialization_lock_contention_is_bounded_and_releases_cleanly() {
    let root = TempDir::new().unwrap();
    let lock = fs::File::create(root.path().join(".run-store-init.lock")).unwrap();
    lock.lock().unwrap();
    assert!(matches!(
        RunStore::open(root.path()),
        Err(RunStoreError::Busy)
    ));
    assert!(!root.path().join("run.sqlite3").exists());
    drop(lock);
    RunStore::open(root.path()).unwrap();
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_lock_files_and_sqlite_sidecars() {
    for name in [
        ".run-store-init.lock",
        "run.sqlite3-wal",
        "run.sqlite3-shm",
        "run.sqlite3-journal",
    ] {
        let root = TempDir::new().unwrap();
        let target = root.path().join("unrelated");
        fs::write(&target, b"do not change").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join(name)).unwrap();
        assert!(RunStore::open(root.path()).is_err(), "{name}");
        assert_eq!(fs::read(target).unwrap(), b"do not change");
        assert!(!root.path().join("run.sqlite3").exists());
    }
}
