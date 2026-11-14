#![cfg(any(unix, windows))]

use std::{collections::BTreeMap, fs, path::Path};

use crate::core::{cache::CacheTargetState, materialization::hex};
use crate::runtime::{artifact_store::ArtifactStore, local_tree::hash_local_tree};

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
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if entry.file_type().unwrap().is_dir() {
            for (path, bytes) in files(&entry.path()) {
                result.insert(format!("{name}/{path}"), bytes);
            }
        } else {
            result.insert(name, file_state(&entry.path()));
        }
    }
    result
}

#[test]
fn exact_verification_and_collection_dry_runs_leave_every_byte_unchanged() {
    for shared in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("payload"), b"abc").unwrap();
        let digest = hash_local_tree(source.path()).unwrap();
        let id = format!("tree-v1-{}", hex(digest));
        let store = if shared {
            ArtifactStore::open_shared(cache.path())
        } else {
            ArtifactStore::open(cache.path())
        }
        .unwrap();
        drop(store.import_tree(source.path(), digest).unwrap());
        let before = files(cache.path());
        let existing = ArtifactStore::open_existing(cache.path()).unwrap();
        let report = existing.verify_targets(std::slice::from_ref(&id)).unwrap();
        assert_eq!(report.targets[0].state, CacheTargetState::Verified);
        assert_eq!(
            report,
            existing.verify_targets(&[id.clone(), id.clone()]).unwrap()
        );
        let report = existing.collect(&[], true).unwrap();
        assert_eq!(report.targets[0].state, CacheTargetState::WouldDelete);
        for target in [
            "",
            "tree-v1-",
            "../objects",
            "tree-v1-../outside",
            "tree-v1-*",
        ] {
            assert!(existing.verify_targets(&[target.into()]).is_err());
            assert!(existing.collect(&[target.into()], false).is_err());
        }
        assert_eq!(files(cache.path()), before);
        let report = store.collect(&[id], false).unwrap();
        assert_eq!(report.targets[0].state, CacheTargetState::Deleted);
        assert!(store.get(digest).unwrap().is_none());
    }
}

#[test]
fn roots_and_live_reader_and_view_leases_protect_the_same_exact_object() {
    for shared in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let views = tempfile::tempdir().unwrap();
        let digest = hash_local_tree(source.path()).unwrap();
        let id = format!("tree-v1-{}", hex(digest));
        let store = if shared {
            ArtifactStore::open_shared(cache.path())
        } else {
            ArtifactStore::open(cache.path())
        }
        .unwrap();
        let object = store.import_tree(source.path(), digest).unwrap();
        let view = object.private_view(views.path()).unwrap();
        drop(object);
        let report = store.collect(&[], false).unwrap();
        assert_eq!(report.targets[0].state, CacheTargetState::Retained);
        assert_eq!(report.targets[0].leases, ["view"]);
        drop(view);
        let before = files(cache.path());
        store
            .update_root(&id, "manual:fixture", true, true)
            .unwrap();
        assert_eq!(files(cache.path()), before);
        store
            .update_root(&id, "manual:fixture", true, false)
            .unwrap();
        let report = store.collect(&[], false).unwrap();
        assert_eq!(report.targets[0].roots, ["manual:fixture"]);
        assert_eq!(report.targets[0].state, CacheTargetState::Retained);
        let before = files(cache.path());
        store
            .update_root(&id, "manual:fixture", false, true)
            .unwrap();
        assert_eq!(files(cache.path()), before);
        store
            .update_root(&id, "manual:fixture", false, false)
            .unwrap();
        assert_eq!(
            store.collect(&[], false).unwrap().targets[0].state,
            CacheTargetState::Deleted
        );
    }
}

#[test]
fn stale_indexes_unregistered_paths_and_corruption_never_authorize_collection() {
    for shared in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("payload"), b"abc").unwrap();
        let digest = hash_local_tree(source.path()).unwrap();
        let id = format!("tree-v1-{}", hex(digest));
        let store = if shared {
            ArtifactStore::open_shared(cache.path())
        } else {
            ArtifactStore::open(cache.path())
        }
        .unwrap();
        let object = store.import_tree(source.path(), digest).unwrap();
        let path = object.directory().to_owned();
        drop(object);
        let before = files(cache.path());
        let unknown = format!("tree-v1-{}", "f".repeat(64));
        assert!(store.collect(&[id.clone(), unknown], false).is_err());
        assert_eq!(files(cache.path()), before);
        store.update_root(&id, "run:fixture", true, false).unwrap();
        fs::write(cache.path().join("index.jsonl"), "{\"unreferenced\":true}").unwrap();
        let untracked = cache
            .path()
            .join("objects")
            .join(format!("tree-v1-{}", "0".repeat(64)));
        fs::create_dir(&untracked).unwrap();
        fs::write(untracked.join("keep"), "unregistered").unwrap();
        assert_eq!(store.collect(&[], false).unwrap().targets.len(), 1);
        assert!(path.join("payload").exists());
        store.update_root(&id, "run:fixture", false, false).unwrap();
        fs::write(path.join("payload"), b"changed").unwrap();
        let before = files(cache.path());
        assert_eq!(
            store
                .verify_targets(std::slice::from_ref(&id))
                .unwrap()
                .targets[0]
                .state,
            CacheTargetState::Corrupt
        );
        assert!(store.collect(&[id], false).is_err());
        assert_eq!(files(cache.path()), before);
        assert_eq!(fs::read(untracked.join("keep")).unwrap(), b"unregistered");
    }
}

#[test]
fn catalog_snapshots_see_commits_and_exclude_uncommitted_roots_without_writes() {
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    drop(store.import_tree(source.path(), digest).unwrap());
    let db = rusqlite::Connection::open(cache.path().join("cache.sqlite3")).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    db.execute(
        "INSERT INTO roots VALUES ('uncommitted', ?1)",
        [hex(digest)],
    )
    .unwrap();
    let before = files(cache.path());
    assert_eq!(
        store.collect(&[], true).unwrap().targets[0].state,
        CacheTargetState::WouldDelete
    );
    assert_eq!(files(cache.path()), before);
    db.execute_batch("COMMIT").unwrap();
    let before = files(cache.path());
    let report = store.collect(&[], true).unwrap();
    assert_eq!(report.targets[0].state, CacheTargetState::Retained);
    assert_eq!(report.targets[0].roots, ["uncommitted"]);
    assert_eq!(files(cache.path()), before);
}

#[test]
fn unsafe_paths_and_incompatible_catalogs_fail_before_any_object_is_detached() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let object = store.import_tree(source.path(), digest).unwrap();
    let object_path = object.directory().parent().unwrap().to_owned();
    drop(object);
    #[cfg(unix)]
    {
        let saved = cache.path().join("saved");
        fs::rename(&object_path, &saved).unwrap();
        symlink(&saved, &object_path).unwrap();
        assert!(store.collect(&[], false).is_err());
        assert!(saved.join("data").exists());
        fs::remove_file(&object_path).unwrap();
        fs::rename(&saved, &object_path).unwrap();
    }
    let db = rusqlite::Connection::open(cache.path().join("cache.sqlite3")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let before = files(cache.path());
    assert!(ArtifactStore::open_existing(cache.path()).is_err());
    assert!(store.collect(&[], false).is_err());
    assert_eq!(files(cache.path()), before);
    assert!(object_path.join("data").exists());
}

#[test]
fn simultaneous_readers_publishers_roots_and_collectors_preserve_live_handles() {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"abc").unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    drop(store.import_tree(source.path(), digest).unwrap());
    let barrier = Arc::new(Barrier::new(4));
    thread::scope(|scope| {
        for lane in 0..3 {
            let store = &store;
            let source = source.path();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                for index in 0..10 {
                    let object = store.import_tree(source, digest).unwrap();
                    if lane == 0 {
                        object.add_root("manual:concurrent").unwrap();
                    }
                    object.verify().unwrap();
                    let view = object.private_view(source.parent().unwrap()).unwrap();
                    drop(object);
                    if index % 2 == 0 {
                        view.verify().unwrap();
                    }
                }
            });
        }
        barrier.wait();
        for _ in 0..10 {
            store.collect(&[], false).unwrap();
        }
    });
    store.get(digest).unwrap().unwrap().verify().unwrap();
    assert_eq!(
        store.collect(&[], false).unwrap().targets[0].roots,
        ["manual:concurrent"]
    );
}

#[test]
fn a_missing_authoritative_catalog_is_never_recreated_by_reads_or_collection() {
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    drop(store.import_tree(source.path(), digest).unwrap());
    fs::remove_file(cache.path().join("cache.sqlite3")).unwrap();
    let before = files(cache.path());
    assert!(store.get(digest).is_err());
    assert!(store.collect(&[], false).is_err());
    assert_eq!(files(cache.path()), before);
}

#[test]
fn shared_cache_roots_survive_relocation_without_a_sqlite_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"portable").unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let id = format!("tree-v1-{}", hex(digest));
    let original = directory.path().join("original");
    let store = ArtifactStore::open_shared(&original).unwrap();
    let initial = fs::read_dir(original.join("shared-catalog"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(initial).unwrap(), br#"{"previous_sha256":null,"schema_version":1,"sequence":0,"snapshot":{"objects":{},"pending":{},"roots":{}}}"#);
    let object = store.import_tree(source.path(), digest).unwrap();
    object.add_root("run-v1:portable-owner").unwrap();
    drop(object);
    let before = files(&original);
    let reader = ArtifactStore::open_existing(&original).unwrap();
    reader.verify_targets(std::slice::from_ref(&id)).unwrap();
    reader.roots().unwrap();
    reader
        .update_root(&id, "manual:dry-run", true, true)
        .unwrap();
    reader
        .update_root(&id, "run-v1:portable-owner", false, true)
        .unwrap();
    assert_eq!(
        reader.collect(&[], true).unwrap().targets[0].state,
        CacheTargetState::Retained
    );
    assert_eq!(files(&original), before);
    assert!(!original.join("cache.sqlite3").exists());
    let relocated = directory.path().join("relocated");
    fs::rename(&original, &relocated).unwrap();
    let store = ArtifactStore::open(&relocated).unwrap();
    assert_eq!(
        store.roots().unwrap().targets[0].roots,
        ["run-v1:portable-owner"]
    );
    store.get(digest).unwrap().unwrap().verify().unwrap();
    store
        .update_root(&id, "run-v1:portable-owner", false, false)
        .unwrap();
    assert_eq!(
        store.collect(&[id], false).unwrap().targets[0].state,
        CacheTargetState::Deleted
    );
    assert!(!relocated.join("cache.sqlite3").exists());
}

#[test]
fn shared_cache_serializes_publishers_roots_views_and_collectors() {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let views = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"shared").unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let store = ArtifactStore::open_shared(cache.path()).unwrap();
    let barrier = Arc::new(Barrier::new(4));
    thread::scope(|scope| {
        for lane in 0..3 {
            let barrier = barrier.clone();
            let store = &store;
            let source = source.path();
            let views = views.path();
            scope.spawn(move || {
                // Open independent handles, as separate hosts would.
                let other = ArtifactStore::open(store.root()).unwrap();
                barrier.wait();
                for _ in 0..5 {
                    let object = other.import_tree(source, digest).unwrap();
                    object.add_root(&format!("manual:lane-{lane}")).unwrap();
                    let view = object.private_view(views).unwrap();
                    drop(object);
                    view.verify().unwrap();
                }
            });
        }
        barrier.wait();
        for _ in 0..5 {
            store.collect(&[], false).unwrap();
        }
    });
    let report = store.collect(&[], false).unwrap();
    assert_eq!(report.targets[0].state, CacheTargetState::Retained);
    assert_eq!(
        report.targets[0].roots,
        ["manual:lane-0", "manual:lane-1", "manual:lane-2"]
    );
    store.get(digest).unwrap().unwrap().verify().unwrap();
}

#[test]
fn shared_catalog_damage_and_mixed_backends_fail_before_collection() {
    for damage in ["missing", "gap", "bytes", "sqlite", "symlink", "directory"] {
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open_shared(cache.path()).unwrap();
        let digest = hash_local_tree(source.path()).unwrap();
        let object = store.import_tree(source.path(), digest).unwrap();
        object.add_root("manual:keep").unwrap();
        let data = object.directory().to_owned();
        drop(object);
        let catalog = cache.path().join("shared-catalog");
        let mut records: Vec<_> = fs::read_dir(&catalog)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        records.sort();
        match damage {
            "directory" => {
                fs::remove_dir_all(&catalog).unwrap();
            }
            "missing" => {
                fs::remove_file(&records[0]).unwrap();
            }
            "gap" => {
                fs::remove_file(&records[1]).unwrap();
            }
            "bytes" => {
                fs::write(&records[1], b"{}").unwrap();
            }
            "sqlite" => {
                fs::write(cache.path().join("cache.sqlite3"), b"untrusted").unwrap();
            }
            "symlink" => {
                #[cfg(unix)]
                {
                    fs::remove_file(&records[1]).unwrap();
                    std::os::unix::fs::symlink(&records[0], &records[1]).unwrap();
                }
                #[cfg(not(unix))]
                {
                    continue;
                }
            }
            _ => unreachable!(),
        }
        let before = files(cache.path());
        assert!(
            ArtifactStore::open_existing(cache.path()).is_err(),
            "{damage}"
        );
        assert!(store.collect(&[], false).is_err(), "{damage}");
        assert!(ArtifactStore::open(cache.path()).is_err(), "{damage}");
        assert!(
            ArtifactStore::open_shared(cache.path()).is_err(),
            "{damage}"
        );
        assert_eq!(files(cache.path()), before, "{damage}");
        assert!(data.exists(), "{damage}");
    }
    let cache = tempfile::tempdir().unwrap();
    let local = ArtifactStore::open(cache.path()).unwrap();
    let before = files(cache.path());
    assert!(ArtifactStore::open_shared(cache.path()).is_err());
    assert_eq!(files(cache.path()), before);
    assert!(ArtifactStore::open_existing(local.root()).is_ok());
}
