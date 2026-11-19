use super::*;

fn write_record(directory: &Path, sequence: u64, bytes: &[u8]) -> String {
    let hash = digest(bytes);
    fs::write(directory.join(filename(sequence, &hash)), bytes).unwrap();
    hash
}

#[test]
fn version_one_history_accepts_version_two_updates_and_verifies_checkpoints() {
    let temp = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open_shared(temp.path()).unwrap();
    let _guard = store.lock().unwrap();
    let directory = store.root.join(DIRECTORY);
    let initial = br#"{"checkpoint":{"objects":{},"pending":{},"roots":{}},"previous_sha256":null,"schema_version":2,"sequence":0}"#;
    let initial_path = directory.join(filename(0, &digest(initial)));
    assert_eq!(fs::read(&initial_path).unwrap(), initial);
    fs::remove_file(initial_path).unwrap();

    let object = "1".repeat(64);
    let marker = "2".repeat(64);
    let initial_hash = write_record(
        &directory,
        0,
        br#"{"previous_sha256":null,"schema_version":1,"sequence":0,"snapshot":{"objects":{},"pending":{},"roots":{}}}"#,
    );
    let legacy_registration = format!(
        r#"{{"previous_sha256":"{initial_hash}","schema_version":1,"sequence":1,"snapshot":{{"objects":{{"{object}":"{marker}"}},"pending":{{}},"roots":{{}}}}}}"#
    );
    let legacy_hash = write_record(&directory, 1, legacy_registration.as_bytes());

    let catalog = SharedCatalog::open(&store, true).unwrap();
    let add_root = |name: &str| {
        catalog
            .update(Delta::Root {
                digest: object.clone(),
                name: name.into(),
                add: true,
            })
            .unwrap();
    };
    add_root("manual:keep");
    let checkpoint = format!(
        r#"{{"checkpoint":{{"objects":{{"{object}":"{marker}"}},"pending":{{}},"roots":{{"{object}":["manual:keep"]}}}},"delta":{{"add":true,"digest":"{object}","kind":"root","name":"manual:keep"}},"previous_sha256":"{legacy_hash}","schema_version":2,"sequence":2}}"#
    );
    let checkpoint_hash = digest(checkpoint.as_bytes());
    let checkpoint_path = directory.join(filename(2, &checkpoint_hash));
    assert_eq!(fs::read(&checkpoint_path).unwrap(), checkpoint.as_bytes());

    add_root("manual:keep");
    add_root("manual:second");
    let delta = format!(
        r#"{{"delta":{{"add":true,"digest":"{object}","kind":"root","name":"manual:second"}},"previous_sha256":"{checkpoint_hash}","schema_version":2,"sequence":3}}"#
    );
    let delta_path = directory.join(filename(3, &digest(delta.as_bytes())));
    assert_eq!(fs::read(&delta_path).unwrap(), delta.as_bytes());
    fs::write(directory.join(".pending-interrupted"), b"{").unwrap();
    let reopened = SharedCatalog::open(&store, false).unwrap();
    assert_eq!(reopened.snapshot().unwrap(), catalog.snapshot().unwrap());
    assert_eq!(
        reopened.snapshot().unwrap().roots[&object],
        ["manual:keep", "manual:second"],
    );

    fs::remove_file(delta_path).unwrap();
    fs::remove_file(checkpoint_path).unwrap();
    let bad_checkpoint = checkpoint.replacen("manual:keep", "manual:wrong", 1);
    write_record(&directory, 2, bad_checkpoint.as_bytes());
    assert!(matches!(
        SharedCatalog::open(&store, false),
        Err(ArtifactStoreError::Catalog(
            "shared cache checkpoint disagrees with replay"
        ))
    ));
}
