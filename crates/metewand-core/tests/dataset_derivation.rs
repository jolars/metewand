use metewand_core::{
    canonical::CanonicalValue,
    identity::{canonical_identity_bytes, record_id},
    materialization::{DatasetDerivationRecord, VerifiedSourceRecord},
    records::{ContentDigest, DerivedSeedRecord, RecordId, ResolvedWorkerRecord},
};
use serde_json::json;
fn record() -> DatasetDerivationRecord {
    DatasetDerivationRecord {
        definition: RecordId::from_digest([1;32]),
        sources: vec![VerifiedSourceRecord { tree_hash_version: 1, selection: CanonicalValue::try_from(json!({"provider":"registry", "selector":{"version":"v1"}})).unwrap(), tree_digest: ContentDigest::new([2;32]), decoding: Some(CanonicalValue::try_from(json!({"resolved_dataset_version":"v1", "decoding_recipe":{"format":"binary"},"decoded_values":"provider_verified"})).unwrap()) }],
        materializer: ResolvedWorkerRecord { source_bundle: Some(RecordId::from_digest([3;32])), environment: RecordId::from_digest([4;32]), launch: RecordId::from_digest([5;32]) },
        parameters: CanonicalValue::try_from(json!({"n":10})).unwrap(), seed: DerivedSeedRecord { value: 7, derivation_digest: ContentDigest::new([6;32]) },
    }
}
#[test]
fn every_construction_input_affects_the_derivation() {
    let original = record();
    let id = record_id(&original).unwrap();
    for field in 0..11 {
        let mut changed = original.clone();
        match field {
            0 => changed.definition = RecordId::from_digest([9; 32]),
            1 => {
                changed.sources[0].selection =
                    CanonicalValue::try_from(json!({"provider":"other"})).unwrap()
            }
            2 => changed.sources[0].tree_digest = ContentDigest::new([9; 32]),
            3 => changed.sources[0].decoding = None,
            4 => changed.materializer.source_bundle = Some(RecordId::from_digest([9; 32])),
            5 => changed.materializer.environment = RecordId::from_digest([9; 32]),
            6 => changed.materializer.launch = RecordId::from_digest([9; 32]),
            7 => changed.parameters = CanonicalValue::try_from(json!({"n":11})).unwrap(),
            8 => changed.seed.value += 1,
            9 => changed.seed.derivation_digest = ContentDigest::new([9; 32]),
            _ => changed.sources[0].tree_hash_version += 1,
        }
        assert_ne!(record_id(&changed).unwrap(), id, "field {field}");
    }
    assert_eq!(
        id.to_string(),
        "mw1-dataset-derivation-875a677f1d0ce2b6b77bc5a03f213774188e0ab45f4b8fc18cbf754de70847c4"
    );
    let bytes = canonical_identity_bytes(&original).unwrap();
    let vector = include_str!("../../../fixtures/identities/dataset-derivation-v1.json").trim_end();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), vector);
}
