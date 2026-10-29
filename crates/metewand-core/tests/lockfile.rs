use metewand_core::{canonical::CanonicalValue, lockfile::Lockfile};
use serde_json::json;

#[test]
fn version_one_lockfile_has_exact_canonical_bytes() {
    let bytes = include_bytes!("../../../schemas/v1/fixtures/lockfile-v1.json");
    let lock = Lockfile::parse(bytes).unwrap();
    assert_eq!(lock.to_bytes().unwrap(), bytes);
    assert_eq!(
        lock.environments["python"].nix().unwrap().system,
        "x86_64-linux"
    );
    let mut invalid = serde_json::to_value(&lock).unwrap();
    invalid["environments"]["python"]["closure"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["nar_hash"] = json!("invalid");
    assert!(Lockfile::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn lockfile_version_and_fields_are_strict() {
    let empty = json!({"schema_version": 1, "source": {"git": false, "files": {}},
        "environments": {}, "workers": {}});
    let lock = Lockfile::parse(&serde_json::to_vec(&empty).unwrap()).unwrap();
    assert_eq!(
        lock.to_bytes().unwrap(),
        CanonicalValue::try_from(empty.clone())
            .unwrap()
            .to_canonical_bytes()
            .unwrap()
    );
    for invalid in [
        json!({"schema_version": 3, "source": {"git": false, "files": {}}, "environments": {}, "workers": {}}),
        json!({"schema_version": 1, "source": {"git": false, "files": {}}, "environments": {}, "workers": {}, "extra": true}),
    ] {
        assert!(Lockfile::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    assert!(Lockfile::parse(br#"{"schema_version":1,"schema_version":1}"#).is_err());
}

#[test]
fn version_two_tags_nix_evidence_without_changing_its_fingerprint() {
    let old = Lockfile::parse(include_bytes!(
        "../../../schemas/v1/fixtures/lockfile-v1.json"
    ))
    .unwrap();
    let mut value = serde_json::to_value(&old).unwrap();
    value["schema_version"] = json!(2);
    let evidence = value["environments"]["python"].take();
    value["environments"]["python"] = json!({"backend":"nix", "evidence":evidence});
    let new = Lockfile::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        new.environments["python"].fingerprint(),
        old.environments["python"].fingerprint()
    );
    assert_eq!(
        new.to_bytes().unwrap(),
        CanonicalValue::try_from(value)
            .unwrap()
            .to_canonical_bytes()
            .unwrap()
    );
}

#[test]
fn uv_evidence_has_exact_version_two_bytes_and_strict_receipts() {
    let bytes = include_bytes!("../../../schemas/v2/fixtures/uv-lockfile-v2.json");
    let lock = Lockfile::parse(bytes).unwrap();
    assert_eq!(lock.to_bytes().unwrap(), bytes);
    for field in ["receipt_sha256", "preparation"] {
        let mut value = serde_json::to_value(&lock).unwrap();
        value["environments"]["python"]["evidence"][field] = json!("../invalid");
        assert!(Lockfile::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let old = Lockfile::parse(include_bytes!(
        "../../../schemas/v2/fixtures/lockfile-v2.json"
    ))
    .unwrap();
    assert_eq!(
        old.to_bytes().unwrap(),
        include_bytes!("../../../schemas/v2/fixtures/lockfile-v2.json")
    );
}
