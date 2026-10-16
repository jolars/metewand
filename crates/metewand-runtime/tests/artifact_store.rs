#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    sync::{Arc, Barrier},
    thread,
};

use metewand_runtime::{
    artifact_store::{ArtifactStore, ArtifactStoreError},
    local_tree::hash_local_tree,
};

#[test]
fn imported_trees_survive_reopening_and_attempt_views_are_independent() {
    let source = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let attempts = tempfile::tempdir().unwrap();
    fs::create_dir(source.path().join("empty")).unwrap();
    fs::write(source.path().join("payload"), b"abc").unwrap();
    fs::set_permissions(
        source.path().join("payload"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let object = store.import_tree(source.path(), digest).unwrap();
    assert_eq!(object.digest(), digest);
    let first = object.private_view(attempts.path()).unwrap();
    let second = object.private_view(attempts.path()).unwrap();
    assert_eq!(store.active_leases().unwrap().len(), 3);
    assert_ne!(first.directory(), second.directory());
    assert_ne!(
        fs::metadata(first.directory().join("payload"))
            .unwrap()
            .ino(),
        fs::metadata(second.directory().join("payload"))
            .unwrap()
            .ino()
    );
    assert_ne!(
        fs::metadata(first.directory().join("payload"))
            .unwrap()
            .ino(),
        fs::metadata(object.directory().join("payload"))
            .unwrap()
            .ino()
    );
    assert_eq!(hash_local_tree(first.directory()).unwrap(), digest);
    fs::write(first.directory().join("payload"), b"changed").unwrap();
    assert!(first.verify().is_err());
    second.verify().unwrap();
    object.verify().unwrap();
    assert_eq!(fs::read(source.path().join("payload")).unwrap(), b"abc");
    drop((first, second, object, store));
    assert_eq!(fs::read_dir(attempts.path()).unwrap().count(), 0);
    let store = ArtifactStore::open(cache.path()).unwrap();
    store.get(digest).unwrap().unwrap().verify().unwrap();
    assert!(store.active_leases().unwrap().is_empty());
}

#[test]
fn concurrent_publishers_deduplicate_only_verified_complete_objects() {
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"abc").unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let store = ArtifactStore::open(cache.path()).unwrap();
            let source = source.path().to_owned();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store.import_tree(&source, digest).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let objects = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(objects[0].directory(), objects[1].directory());
    assert_eq!(
        fs::read_dir(cache.path().join("objects")).unwrap().count(),
        1
    );
    for object in objects {
        object.verify().unwrap();
    }
}

#[test]
fn incomplete_changed_and_unsafe_trees_cannot_be_consumed() {
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"abc").unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    fs::write(source.path().join("payload"), b"abd").unwrap();
    assert!(matches!(
        store.import_tree(source.path(), digest),
        Err(ArtifactStoreError::ChangedContent)
    ));
    fs::write(source.path().join("payload"), b"abc").unwrap();
    symlink("payload", source.path().join("link")).unwrap();
    assert!(
        store
            .import_tree(source.path(), hash_local_tree(source.path()).unwrap())
            .is_err()
    );
    fs::remove_file(source.path().join("link")).unwrap();
    let object = store.import_tree(source.path(), digest).unwrap();
    fs::remove_file(object.directory().parent().unwrap().join("manifest.json")).unwrap();
    assert!(store.get(digest).is_err());
    assert!(object.private_view(source.path()).is_err());
}

#[test]
fn empty_unit_trees_are_valid_cache_objects() {
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let object = store
        .import_tree(source.path(), hash_local_tree(source.path()).unwrap())
        .unwrap();
    object.verify().unwrap();
    assert_eq!(fs::read_dir(object.directory()).unwrap().count(), 0);
}

#[test]
fn completion_marker_has_exact_versioned_bytes() {
    assert_eq!(metewand_runtime::artifact_store::ARTIFACT_STORE_VERSION, 1);
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"abc").unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let object = store.import_tree(source.path(), digest).unwrap();
    let marker = fs::read(object.directory().parent().unwrap().join("manifest.json")).unwrap();
    let digest_text = metewand_core::materialization::hex(digest);
    assert_eq!(marker, format!(r#"{{"complete":true,"entries":[{{"executable":false,"kind":"file","path":"payload","sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","size_bytes":3}}],"schema_version":1,"tree_hash_version":1,"tree_sha256":"{digest_text}"}}"#).as_bytes());
}

#[test]
fn changed_canonical_objects_are_quarantined_even_with_live_readers_and_views() {
    for boundary in ["get", "verify", "view", "existing_view", "publication"] {
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let views = tempfile::tempdir().unwrap();
        fs::write(source.path().join("payload"), b"abc").unwrap();
        let digest = hash_local_tree(source.path()).unwrap();
        let store = ArtifactStore::open(cache.path()).unwrap();
        let object = store.import_tree(source.path(), digest).unwrap();
        let reader = store.get(digest).unwrap().unwrap();
        let view = reader.private_view(views.path()).unwrap();
        let canonical = object.directory().parent().unwrap().to_owned();
        fs::write(object.directory().join("payload"), b"changed").unwrap();

        let failed = match boundary {
            "get" => store.get(digest).is_err(),
            "verify" => object.verify().is_err(),
            "view" => reader.private_view(views.path()).is_err(),
            "existing_view" => view.verify().is_err(),
            "publication" => store.import_tree(source.path(), digest).is_err(),
            _ => unreachable!(),
        };
        assert!(failed, "{boundary}");
        assert!(!canonical.exists(), "{boundary}");
        assert!(store.get(digest).unwrap().is_none());
        assert!(reader.private_view(views.path()).is_err());
        let quarantined = fs::read_dir(cache.path().join("quarantine"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(quarantined.len(), 1, "{boundary}");
        assert_eq!(
            fs::read(quarantined[0].join("object/data/payload")).unwrap(),
            b"changed"
        );
        assert!(quarantined[0].join("object/manifest.json").is_file());
        assert_eq!(fs::read(view.directory().join("payload")).unwrap(), b"abc");
        // Reuse requires independently verified publication of the original bytes.
        let repaired = store.import_tree(source.path(), digest).unwrap();
        repaired.verify().unwrap();
        assert_eq!(
            fs::read(repaired.directory().join("payload")).unwrap(),
            b"abc"
        );
        assert_eq!(
            fs::read_dir(cache.path().join("quarantine"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[test]
fn concurrent_readers_quarantine_a_corrupted_address_once() {
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"abc").unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let object = store.import_tree(source.path(), digest).unwrap();
    fs::write(object.directory().join("payload"), b"changed").unwrap();
    let barrier = Arc::new(Barrier::new(4));
    let handles = (0..4)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                match store.get(digest) {
                    Ok(Some(_)) => panic!("a corrupted address must never be reused"),
                    Ok(None) => false,
                    Err(_) => true,
                }
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|failed| *failed)
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(cache.path().join("objects")).unwrap().count(),
        0
    );
    assert_eq!(
        fs::read_dir(cache.path().join("quarantine"))
            .unwrap()
            .count(),
        1
    );
    let reopened = ArtifactStore::open(cache.path()).unwrap();
    assert!(reopened.get(digest).unwrap().is_none());
    assert!(object.verify().is_err());
}

#[test]
fn quarantine_survives_the_detecting_process_exiting_abruptly() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), b"abc").unwrap();
    let digest = hash_local_tree(source.path()).unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let object = store.import_tree(source.path(), digest).unwrap();
    fs::write(object.directory().join("payload"), b"changed").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "quarantine_process_helper", "--nocapture"])
        .env("METEWAND_TEST_QUARANTINE_ROOT", cache.path())
        .env(
            "METEWAND_TEST_QUARANTINE_DIGEST",
            serde_json::to_string(digest.bytes()).unwrap(),
        )
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        if line.trim() == "QUARANTINED" {
            break;
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let reopened = ArtifactStore::open(cache.path()).unwrap();
    assert!(reopened.get(digest).unwrap().is_none());
    assert!(object.verify().is_err());
    let quarantine = fs::read_dir(cache.path().join("quarantine"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read(quarantine.join("object/data/payload")).unwrap(),
        b"changed"
    );
    assert_eq!(reopened.quarantine_abandoned_leases().unwrap(), 0);
}

#[test]
fn quarantine_process_helper() {
    use std::io::Write;
    let Some(cache) = std::env::var_os("METEWAND_TEST_QUARANTINE_ROOT") else {
        return;
    };
    let digest = metewand_core::records::ContentDigest::new(
        serde_json::from_str(&std::env::var("METEWAND_TEST_QUARANTINE_DIGEST").unwrap()).unwrap(),
    );
    let store = ArtifactStore::open(std::path::Path::new(&cache)).unwrap();
    assert!(matches!(
        store.get(digest),
        Err(ArtifactStoreError::Quarantined { .. })
    ));
    println!("QUARANTINED");
    std::io::stdout().flush().unwrap();
    loop {
        thread::park();
    }
}

#[test]
fn corrupted_objects_are_quarantined_without_worker_copies() {
    for corruption in [
        "bytes",
        "mode",
        "missing",
        "extra",
        "directory",
        "symlink",
        "version",
        "incomplete",
        "duplicate",
    ] {
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let views = tempfile::tempdir().unwrap();
        fs::write(source.path().join("payload"), b"abc").unwrap();
        let digest = hash_local_tree(source.path()).unwrap();
        let store = ArtifactStore::open(cache.path()).unwrap();
        let object = store.import_tree(source.path(), digest).unwrap();
        let data = object.directory();
        match corruption {
            "bytes" => fs::write(data.join("payload"), b"abd").unwrap(),
            "mode" => fs::set_permissions(data.join("payload"), fs::Permissions::from_mode(0o700))
                .unwrap(),
            "missing" => fs::remove_file(data.join("payload")).unwrap(),
            "extra" => fs::write(data.join("extra"), b"extra").unwrap(),
            "directory" => fs::create_dir(data.join("empty")).unwrap(),
            "symlink" => {
                fs::remove_file(data.join("payload")).unwrap();
                symlink(source.path().join("payload"), data.join("payload")).unwrap();
            }
            "version" | "incomplete" | "duplicate" => {
                let marker = data.parent().unwrap().join("manifest.json");
                let mut document: serde_json::Value =
                    serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
                match corruption {
                    "version" => document["schema_version"] = serde_json::json!(2),
                    "incomplete" => document["complete"] = serde_json::json!(false),
                    "duplicate" => {
                        let duplicate = document["entries"][0].clone();
                        document["entries"].as_array_mut().unwrap().push(duplicate);
                    }
                    _ => unreachable!(),
                }
                fs::write(
                    marker,
                    metewand_core::canonical::CanonicalValue::try_from(document)
                        .unwrap()
                        .to_canonical_bytes()
                        .unwrap(),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(store.get(digest).is_err(), "{corruption}");
        assert!(object.private_view(views.path()).is_err(), "{corruption}");
        assert_eq!(fs::read_dir(views.path()).unwrap().count(), 0);
        assert_eq!(
            fs::read_dir(cache.path().join("objects")).unwrap().count(),
            0
        );
        assert_eq!(
            fs::read_dir(cache.path().join("quarantine"))
                .unwrap()
                .count(),
            1
        );
        assert!(store.get(digest).unwrap().is_none());
    }
}

#[test]
fn active_owners_keep_leases_and_crashed_owners_leave_reclaimable_staging() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    let cache = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(cache.path()).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_process_helper", "--nocapture"])
        .env("METEWAND_TEST_LEASE_ROOT", cache.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        if line.trim() == "LEASE_READY" {
            break;
        }
    }
    assert_eq!(store.active_leases().unwrap().len(), 1);
    assert_eq!(store.quarantine_abandoned_leases().unwrap(), 0);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(store.active_leases().unwrap().is_empty());
    assert_eq!(store.quarantine_abandoned_leases().unwrap(), 1);
    assert_eq!(
        fs::read_dir(cache.path().join("objects")).unwrap().count(),
        1
    );
    assert_eq!(store.quarantine_abandoned_leases().unwrap(), 0);
    let quarantined = fs::read_dir(cache.path().join("quarantine"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read(quarantined.join("object/partial")).unwrap(),
        b"unfinished staging"
    );
}

#[test]
fn lease_process_helper() {
    use std::io::Write;
    let Some(cache) = std::env::var_os("METEWAND_TEST_LEASE_ROOT") else {
        return;
    };
    let store = ArtifactStore::open(std::path::Path::new(&cache)).unwrap();
    let source = store.root().join("helper-source");
    fs::create_dir(&source).unwrap();
    let _object = store
        .import_tree(&source, hash_local_tree(&source).unwrap())
        .unwrap();
    let lease = fs::read_dir(store.root().join("leases"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::create_dir(lease.join("object")).unwrap();
    fs::write(lease.join("object/partial"), b"unfinished staging").unwrap();
    println!("LEASE_READY");
    std::io::stdout().flush().unwrap();
    loop {
        thread::park();
    }
}

#[test]
fn provider_exports_enter_the_store_through_independent_verified_copies() {
    use metewand_core::{canonical::CanonicalValue, records::ContentDigest};
    use metewand_runtime::source_resolution::{
        AcquiredSource, SourceAcquirer, SourceFile, SourceRequest, SourceResolutionError,
        resolve_source,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    struct Provider(std::path::PathBuf);
    impl SourceAcquirer for Provider {
        fn acquire(
            &self,
            _: &SourceRequest<'_>,
            output: &std::path::Path,
        ) -> Result<AcquiredSource, SourceResolutionError> {
            fs::copy(self.0.join("payload"), output.join("payload"))?;
            Ok(AcquiredSource {
                files: vec![SourceFile {
                    path: serde_json::from_value(json!("payload")).unwrap(),
                    size_bytes: 3,
                    sha256: ContentDigest::new(Sha256::digest(b"abc").into()),
                }],
                provenance: CanonicalValue::try_from(json!({"receipt":"provider-owned"})).unwrap(),
            })
        }
    }
    let provider = tempfile::tempdir().unwrap();
    fs::write(provider.path().join("payload"), b"abc").unwrap();
    fs::set_permissions(
        provider.path().join("payload"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let private = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let source = resolve_source(
        &SourceRequest::External {
            provider: &serde_json::from_value(json!("provider")).unwrap(),
            selector: &CanonicalValue::try_from(json!({"version":"pinned"})).unwrap(),
        },
        Some(&Provider(provider.path().to_owned())),
        private.path(),
    )
    .unwrap();
    let object = ArtifactStore::open(cache.path())
        .unwrap()
        .import_source(&source)
        .unwrap();
    drop(source);
    assert_eq!(fs::read_dir(private.path()).unwrap().count(), 0);
    object.verify().unwrap();
    fs::write(provider.path().join("payload"), b"provider changed").unwrap();
    object.verify().unwrap();
    assert_eq!(
        fs::read(object.directory().join("payload")).unwrap(),
        b"abc"
    );
    fs::write(object.directory().join("payload"), b"changed").unwrap();
    assert_eq!(
        fs::read(provider.path().join("payload")).unwrap(),
        b"provider changed"
    );
    assert_eq!(
        fs::metadata(provider.path().join("payload"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(fs::read_dir(provider.path()).unwrap().count(), 1);
}
