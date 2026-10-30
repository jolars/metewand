use crate::core::{
    archive::{ArchiveEvidence, ArchiveManifest, RESULT_ARCHIVE_VERSION},
    canonical::CanonicalValue,
    public_schemas::{PublicSchema, public_schema_catalog},
};

#[test]
fn archive_fixtures_preserve_their_exact_canonical_bytes() {
    let evidence = include_bytes!("../../schemas/v1/fixtures/archive-evidence-v1.json");
    let manifest = include_bytes!("../../schemas/v1/fixtures/archive-manifest-v1.json");
    let parsed: ArchiveEvidence = serde_json::from_slice(evidence).unwrap();
    let inventory: ArchiveManifest = serde_json::from_slice(manifest).unwrap();
    assert_eq!(parsed.schema_version, RESULT_ARCHIVE_VERSION);
    assert_eq!(inventory.schema_version, RESULT_ARCHIVE_VERSION);
    for (schema, bytes, value) in [
        (
            PublicSchema::ArchiveEvidence,
            evidence.as_slice(),
            serde_json::to_value(parsed).unwrap(),
        ),
        (
            PublicSchema::ArchiveManifest,
            manifest.as_slice(),
            serde_json::to_value(inventory).unwrap(),
        ),
    ] {
        public_schema_catalog()
            .unwrap()
            .validate(schema.repository_path(), &value)
            .unwrap();
        assert_eq!(
            CanonicalValue::try_from(value)
                .unwrap()
                .to_canonical_bytes()
                .unwrap(),
            bytes
        );
    }
}

#[test]
fn archive_records_reject_unknown_types_and_fields() {
    for bytes in [
        br#"{"schema_version":1,"records":[{"kind":"future-record","record":{}}]}"#.as_slice(),
        br#"{"schema_version":1,"records":[],"unknown":true}"#,
    ] {
        assert!(serde_json::from_slice::<ArchiveEvidence>(bytes).is_err());
    }
}
