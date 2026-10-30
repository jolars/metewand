#![cfg(any(unix, windows))]

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use crate::core::canonical::CanonicalValue;
use crate::runtime::{
    archive::{ArchiveError, ArchiveLimits, create_archive, import_archive, verify_archive},
    run_store::RunStore,
};
use flate2::{Compression, GzBuilder};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/v1/fixtures/archive-executable-v1")
}

fn copy(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            for (name, value) in bytes(&entry.path()) {
                files.insert(PathBuf::from(entry.file_name()).join(name), value);
            }
        } else {
            files.insert(entry.file_name().into(), fs::read(entry.path()).unwrap());
        }
    }
    files
}

fn canonical(value: Value) -> Vec<u8> {
    CanonicalValue::try_from(value)
        .unwrap()
        .to_canonical_bytes()
        .unwrap()
}

fn rewrite_evidence(root: &Path, update: impl FnOnce(&mut Value)) {
    let mut evidence: Value =
        serde_json::from_slice(&fs::read(root.join("evidence.json")).unwrap()).unwrap();
    update(&mut evidence);
    let evidence = canonical(evidence);
    fs::write(root.join("evidence.json"), &evidence).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(root.join("archive.json")).unwrap()).unwrap();
    let file = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["path"] == "evidence.json")
        .unwrap();
    file["sha256"] = json!(
        Sha256::digest(&evidence)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    file["size_bytes"] = json!(evidence.len());
    fs::write(root.join("archive.json"), canonical(manifest)).unwrap();
}

#[test]
fn portable_executable_metadata_survives_import_without_host_permission_bits() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    copy(&fixture(), &source);
    let before = bytes(&source);
    let report = verify_archive(&source, ArchiveLimits::default()).unwrap();
    assert_eq!(report.records, 1);
    let archive = root.path().join("result.tar.gz");
    let created = create_archive(&source, &archive).unwrap();
    assert_eq!(before, bytes(&source));
    let target = root.path().join("imported");
    import_archive(&archive, &target, ArchiveLimits::default()).unwrap();
    let imported = verify_archive(&target, ArchiveLimits::default()).unwrap();
    assert_eq!(created.manifest_sha256, imported.manifest_sha256);
    assert!(RunStore::open(&target).is_err());
    assert!(!target.join("run.sqlite3").exists());
}

#[test]
fn empty_run_archives_are_deterministic_and_ignore_unregistered_files() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("run");
    fs::create_dir(&run).unwrap();
    drop(RunStore::open(&run).unwrap());
    fs::write(run.join("index.jsonl"), "not evidence").unwrap();
    fs::create_dir(run.join("staging")).unwrap();
    fs::write(run.join("staging/unpublished"), "not committed").unwrap();
    let before = bytes(&run);
    let first = root.path().join("first.tar.gz");
    let second = root.path().join("second.tar.gz");
    create_archive(&run, &first).unwrap();
    create_archive(&run, &second).unwrap();
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    assert_eq!(before, bytes(&run));
    let target = root.path().join("imported");
    import_archive(&first, &target, ArchiveLimits::default()).unwrap();
    assert_eq!(bytes(&target).len(), 2);
}

#[test]
fn unfinished_attempts_require_explicit_recovery_and_leave_the_source_unchanged() {
    use crate::core::records::{RecordId, ResolvedAttemptSlotRecord};
    use crate::runtime::run_store::{LeaseRequest, RetryAuthorization};
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("run");
    fs::create_dir(&run).unwrap();
    let mut store = RunStore::open(&run).unwrap();
    let slot = RecordId::<ResolvedAttemptSlotRecord>::from_digest([1; 32]);
    let connection = rusqlite::Connection::open(store.database_path()).unwrap();
    connection.execute_batch("INSERT INTO applicability_decisions VALUES ('decision','candidate',1,'{}'); INSERT INTO resolved_runs VALUES ('run','logical','candidate','decision',1,'{}');").unwrap();
    connection.execute("INSERT INTO attempt_slots VALUES (?1,'logical-slot','run','one_shot','measurement','{}')", [slot.to_string()]).unwrap();
    drop(connection);
    store
        .reserve(
            slot,
            RetryAuthorization::FirstAttempt,
            LeaseRequest {
                id: "lease".into(),
                owner_token: "owner".into(),
                expires_at_ns: 100,
            },
            10,
        )
        .unwrap();
    drop(store);
    let before = bytes(&run);
    let target = root.path().join("result.tar.gz");
    assert!(matches!(
        create_archive(&run, &target),
        Err(ArchiveError::Unsettled)
    ));
    assert!(!target.exists());
    assert_eq!(before, bytes(&run));
}

#[test]
fn corruption_unsupported_versions_and_inconsistent_evidence_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    copy(&fixture(), &source);
    fs::write(source.join("data/artifacts/opaque/payload.bin"), "tampered").unwrap();
    assert!(verify_archive(&source, ArchiveLimits::default()).is_err());
    fs::remove_dir_all(&source).unwrap();
    copy(&fixture(), &source);
    fs::remove_file(source.join("data/artifacts/opaque/payload.bin")).unwrap();
    assert!(verify_archive(&source, ArchiveLimits::default()).is_err());
    fs::remove_dir_all(&source).unwrap();
    copy(&fixture(), &source);
    fs::write(source.join("extra"), "undeclared").unwrap();
    assert!(verify_archive(&source, ArchiveLimits::default()).is_err());
    fs::remove_dir_all(&source).unwrap();
    copy(&fixture(), &source);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(source.join("archive.json")).unwrap()).unwrap();
    manifest["schema_version"] = json!(2);
    fs::write(source.join("archive.json"), canonical(manifest)).unwrap();
    assert!(matches!(
        verify_archive(&source, ArchiveLimits::default()),
        Err(ArchiveError::UnsupportedVersion)
    ));
    fs::remove_dir_all(&source).unwrap();
    copy(&fixture(), &source);
    rewrite_evidence(&source, |e| {
        e["records"].as_array_mut().unwrap().push(json!({"kind":"observation-selection","record":{"slot_id":"missing","observation_id":"missing","selected_at_ns":"1"}}))
    });
    assert!(
        verify_archive(&source, ArchiveLimits::default())
            .unwrap_err()
            .to_string()
            .contains("referenced evidence")
    );
}

#[test]
fn verification_and_import_enforce_limits_without_publishing_a_destination() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("result.tar.gz");
    create_archive(&fixture(), &archive).unwrap();
    for limits in [
        ArchiveLimits {
            max_entries: 1,
            max_expanded_bytes: u64::MAX,
        },
        ArchiveLimits {
            max_entries: u64::MAX,
            max_expanded_bytes: 10,
        },
    ] {
        assert!(matches!(
            verify_archive(&archive, limits),
            Err(ArchiveError::Limit)
        ));
        assert!(matches!(
            verify_archive(&fixture(), limits),
            Err(ArchiveError::Limit)
        ));
        let target = root.path().join("imported");
        assert!(matches!(
            import_archive(&archive, &target, limits),
            Err(ArchiveError::Limit)
        ));
        assert!(!target.exists());
    }
    verify_archive(&archive, ArchiveLimits::default()).unwrap();
    let before = fs::read(&archive).unwrap();
    assert!(create_archive(&fixture(), &archive).is_err());
    assert_eq!(before, fs::read(&archive).unwrap());
}

fn malicious_tar(path: &Path, name: &str, kind: tar::EntryType, duplicate: bool) {
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(fs::File::create(path).unwrap(), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for _ in 0..if duplicate { 2 } else { 1 } {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o600);
        header.set_entry_type(kind);
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        archive.append(&header, [].as_slice()).unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap();
}

#[test]
fn unsafe_tar_members_and_truncated_streams_never_escape_or_publish() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("bad.tar.gz");
    for (name, kind, duplicate) in [
        ("../escape", tar::EntryType::Regular, false),
        ("/escape", tar::EntryType::Regular, false),
        ("data\\escape", tar::EntryType::Regular, false),
        ("C:escape", tar::EntryType::Regular, false),
        ("data/link", tar::EntryType::Symlink, false),
        ("data/link", tar::EntryType::Link, false),
        ("archive.json", tar::EntryType::Regular, true),
    ] {
        malicious_tar(&archive, name, kind, duplicate);
        let target = root.path().join("imported");
        assert!(import_archive(&archive, &target, ArchiveLimits::default()).is_err());
        assert!(!target.exists());
        assert!(!root.path().join("escape").exists());
    }
    fs::remove_file(&archive).unwrap();
    create_archive(&fixture(), &archive).unwrap();
    let valid = fs::read(&archive).unwrap();
    fs::write(&archive, [&valid[..], &valid[..]].concat()).unwrap();
    assert!(verify_archive(&archive, ArchiveLimits::default()).is_err());
    fs::write(&archive, &valid[..valid.len() - 8]).unwrap();
    assert!(verify_archive(&archive, ArchiveLimits::default()).is_err());
    let mut file = fs::OpenOptions::new().append(true).open(&archive).unwrap();
    file.write_all(b"corrupt").unwrap();
    assert!(verify_archive(&archive, ArchiveLimits::default()).is_err());
}

#[test]
fn gnu_filename_metadata_is_bounded_before_decoding_and_cannot_hide_traversal() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("bad.tar.gz");
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(fs::File::create(&path).unwrap(), Compression::default());
    let mut header = tar::Header::new_gnu();
    header.set_path("././@LongLink").unwrap();
    header.set_entry_type(tar::EntryType::GNULongName);
    header.set_size(1_000_000_000);
    header.set_cksum();
    encoder.write_all(header.as_bytes()).unwrap();
    encoder.finish().unwrap();
    assert!(
        verify_archive(&path, ArchiveLimits::default())
            .unwrap_err()
            .to_string()
            .contains("oversized")
    );
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(fs::File::create(&path).unwrap(), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    let name = b"../escape\0";
    header.set_size(name.len() as u64);
    header.set_cksum();
    archive.append(&header, &name[..]).unwrap();
    let mut payload = tar::Header::new_gnu();
    payload.set_path("placeholder").unwrap();
    payload.set_size(0);
    payload.set_cksum();
    archive.append(&payload, [].as_slice()).unwrap();
    archive.into_inner().unwrap().finish().unwrap();
    let destination = root.path().join("imported");
    assert!(import_archive(&path, &destination, ArchiveLimits::default()).is_err());
    assert!(!destination.exists());
    assert!(!root.path().join("escape").exists());
}
