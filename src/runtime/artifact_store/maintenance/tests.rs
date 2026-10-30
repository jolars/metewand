use std::{
    fs::{self, File},
    path::Path,
    process::{Command, Output},
};

use crate::core::cache::CacheTargetState;

use super::*;
use crate::runtime::{fault_injection::arm, local_tree::hash_local_tree};

struct Fixture {
    directory: tempfile::TempDir,
    store: ArtifactStore,
    rooted: ContentDigest,
    leased: StoredArtifact,
    target: ContentDigest,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(&directory.path().join("cache")).unwrap();
    let source = tempfile::tempdir().unwrap();
    let mut digests = Vec::new();
    let mut leased = None;
    for name in ["rooted", "leased", "target"] {
        fs::write(source.path().join("payload"), name).unwrap();
        let digest = hash_local_tree(source.path()).unwrap();
        let object = store.import_tree(source.path(), digest).unwrap();
        if name == "rooted" {
            object.add_root("run:retained").unwrap();
        }
        if name == "leased" {
            leased = Some(object);
        }
        digests.push(digest);
    }
    Fixture {
        directory,
        store,
        rooted: digests[0],
        leased: leased.unwrap(),
        target: digests[2],
    }
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("METEWAND_CACHE_CRASH_ROOT") else {
        return;
    };
    let store = ArtifactStore::open_existing(Path::new(&root)).unwrap();
    let trace = File::create(std::env::var_os("METEWAND_CACHE_CRASH_TRACE").unwrap()).unwrap();
    let target = std::env::var("METEWAND_CACHE_CRASH_TARGET")
        .unwrap()
        .parse()
        .unwrap();
    arm(trace, target);
    store.collect(&[], false).unwrap();
}

fn child(fixture: &Fixture, target: usize) -> (Output, Vec<String>) {
    let trace = fixture.directory.path().join("trace");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::artifact_store::maintenance::tests::crash_child",
            "--nocapture",
        ])
        .env("METEWAND_CACHE_CRASH_ROOT", fixture.store.root())
        .env("METEWAND_CACHE_CRASH_TRACE", &trace)
        .env("METEWAND_CACHE_CRASH_TARGET", target.to_string())
        .output()
        .unwrap();
    (
        output,
        fs::read_to_string(trace)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect(),
    )
}

#[test]
#[ignore = "exhaustive cache crash campaign; run explicitly after collection changes"]
fn collection_recovers_after_each_database_sync_rename_and_cleanup_boundary() {
    let baseline = fixture();
    let (output, boundaries) = child(&baseline, usize::MAX);
    assert!(output.status.success(), "{output:?}");
    assert!(boundaries.iter().any(|label| label == "rename.after"));
    assert!(
        boundaries
            .iter()
            .any(|label| label == "cache.cleanup.before")
    );
    for (index, boundary) in boundaries.iter().enumerate() {
        let fixture = fixture();
        let (output, _) = child(&fixture, index + 1);
        assert_eq!(
            output.status.code(),
            Some(86),
            "{index}: {boundary}: {output:?}"
        );
        fixture.leased.verify().unwrap();
        fixture
            .store
            .get(fixture.rooted)
            .unwrap()
            .unwrap()
            .verify()
            .unwrap();
        fixture.store.collect(&[], false).unwrap();
        assert!(
            fixture.store.get(fixture.target).unwrap().is_none(),
            "{index}: {boundary}"
        );
        let retained = fixture.store.collect(&[], false).unwrap();
        assert_eq!(retained.targets.len(), 2, "{index}: {boundary}");
        assert!(
            retained
                .targets
                .iter()
                .all(|target| target.state == CacheTargetState::Retained)
        );
        assert_eq!(
            fs::read_dir(fixture.store.root().join("garbage"))
                .unwrap()
                .count(),
            0
        );
        let snapshot = Catalog::read(&fixture.store).unwrap();
        assert!(snapshot.pending.is_empty(), "{index}: {boundary}");
    }
}

#[test]
fn collection_recovers_from_representative_crashes() {
    let baseline = fixture();
    let (output, boundaries) = child(&baseline, usize::MAX);
    assert!(output.status.success(), "{output:?}");
    for label in [
        "rename.after",
        "cache.cleanup.before",
        "cache.cleanup.after",
        "database.after COMMIT",
    ] {
        let index = boundaries
            .iter()
            .rposition(|boundary| boundary == label)
            .unwrap();
        let fixture = fixture();
        let (output, _) = child(&fixture, index + 1);
        assert_eq!(output.status.code(), Some(86), "{label}: {output:?}");
        fixture.store.collect(&[], false).unwrap();
        fixture.leased.verify().unwrap();
        fixture
            .store
            .get(fixture.rooted)
            .unwrap()
            .unwrap()
            .verify()
            .unwrap();
        assert!(fixture.store.get(fixture.target).unwrap().is_none());
        assert!(Catalog::read(&fixture.store).unwrap().pending.is_empty());
    }
}

#[test]
fn partial_detached_cleanup_never_deletes_a_reimported_rooted_or_leased_object() {
    let fixture = fixture();
    let id = format!("tree-v1-{}", hex(fixture.target));
    let garbage = fixture.store.root.join("garbage").join(&id);
    {
        let _guard = fixture.store.lock().unwrap();
        let catalog = Catalog::open(&fixture.store).unwrap();
        catalog.pending(&hex(fixture.target), false).unwrap();
        rename_no_replace(&fixture.store.object_path(fixture.target), &garbage).unwrap();
        crate::runtime::platform_fs::complete_namespace_change(&fixture.store.root.join("objects")).unwrap();
        crate::runtime::platform_fs::complete_namespace_change(&fixture.store.root.join("garbage")).unwrap();
        catalog.pending(&hex(fixture.target), true).unwrap();
    }
    fs::remove_file(garbage.join("manifest.json")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("keep"), "outside collection").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), garbage.join("outside")).unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("payload"), "target").unwrap();
    let replacement = fixture
        .store
        .import_tree(source.path(), fixture.target)
        .unwrap();
    let retained = fixture
        .store
        .collect(std::slice::from_ref(&id), false)
        .unwrap();
    assert_eq!(retained.targets[0].state, CacheTargetState::Retained);
    replacement.add_root("manual:replacement").unwrap();
    drop(replacement);
    assert_eq!(
        fixture
            .store
            .collect(std::slice::from_ref(&id), false)
            .unwrap()
            .targets[0]
            .state,
        CacheTargetState::Retained
    );
    fixture
        .store
        .update_root(&id, "manual:replacement", false, false)
        .unwrap();
    assert_eq!(
        fixture.store.collect(&[id], false).unwrap().targets[0].state,
        CacheTargetState::Deleted
    );
    assert!(!garbage.exists());
    assert_eq!(
        fs::read(outside.path().join("keep")).unwrap(),
        b"outside collection"
    );
    assert!(fixture.store.get(fixture.target).unwrap().is_none());
    fixture.leased.verify().unwrap();
}

#[test]
fn completed_orphans_are_preserved_until_a_verified_operation_registers_them() {
    let fixture = fixture();
    {
        let _guard = fixture.store.lock().unwrap();
        Catalog::open(&fixture.store)
            .unwrap()
            .forget(&hex(fixture.target))
            .unwrap();
    }
    let id = format!("tree-v1-{}", hex(fixture.target));
    assert_eq!(fixture.store.collect(&[], false).unwrap().targets.len(), 2);
    assert!(
        fixture
            .store
            .collect(std::slice::from_ref(&id), false)
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .verify_targets(std::slice::from_ref(&id))
            .unwrap()
            .targets[0]
            .state,
        CacheTargetState::Verified
    );
    assert_eq!(fixture.store.collect(&[], false).unwrap().targets.len(), 2);
    drop(fixture.store.get(fixture.target).unwrap().unwrap());
    assert_eq!(
        fixture.store.collect(&[id], false).unwrap().targets[0].state,
        CacheTargetState::Deleted
    );
}
