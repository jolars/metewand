use crate::core::{
    canonical::CanonicalValue,
    identity::{canonical_identity_bytes, record_id},
    materialization::{DatasetDerivationRecord, ProblemPreparationRecord, VerifiedSourceRecord},
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
fn shared_problem_inputs_have_a_distinct_canonical_identity() {
    let original = ProblemPreparationRecord {
        dataset: RecordId::from_digest([1; 32]),
        problem: RecordId::from_digest([2; 32]),
        preparer: record().materializer,
    };
    let bytes = canonical_identity_bytes(&original).unwrap();
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        include_str!("../../fixtures/identities/problem-preparation-v1.json").trim_end()
    );
    let id = record_id(&original).unwrap();
    assert!(id.to_string().starts_with("mw1-problem-preparation-"));
    for field in 0..5 {
        let mut changed = original.clone();
        match field {
            0 => changed.dataset = RecordId::from_digest([9; 32]),
            1 => changed.problem = RecordId::from_digest([9; 32]),
            2 => changed.preparer.source_bundle = None,
            3 => changed.preparer.environment = RecordId::from_digest([9; 32]),
            _ => changed.preparer.launch = RecordId::from_digest([9; 32]),
        }
        assert_ne!(record_id(&changed).unwrap(), id, "field {field}");
    }
}

#[test]
fn problem_preparation_has_one_parameter_owner_and_a_checked_output_schema() {
    use crate::core::{
        manifest::parse_manifest,
        public_schemas::{PublicSchema, public_schema_catalog},
    };
    use std::path::Path;
    let source = include_str!("../../examples/problem-preparation/metewand.yaml");
    let path = Path::new("metewand.yaml");
    let document: serde_json::Value = serde_saphyr::from_str(source).unwrap();
    let schemas = public_schema_catalog().unwrap();
    schemas
        .validate(PublicSchema::Manifest.repository_path(), &document)
        .unwrap();
    let manifest = parse_manifest(path, source).unwrap();
    let preparation = manifest.problems["path"].preparation.as_ref().unwrap();
    let crate::core::manifest::WorkerDefinition::Command(worker) = &preparation.worker else {
        panic!("function launch")
    };
    assert_eq!(worker.args[2], "problem_preparer");
    assert!(preparation.output_schema.inline_document().is_some());
    for (key, value) in [
        (
            "parameters",
            json!({"q": {"type": "number", "default": 0.1}}),
        ),
        ("output-schema", json!(null)),
        ("function", json!(null)),
    ] {
        let mut invalid = document.clone();
        invalid["problems"]["path"]["preparation"][key] = value;
        assert!(
            schemas
                .validate(PublicSchema::Manifest.repository_path(), &invalid)
                .is_err(),
            "{key}"
        );
        assert!(parse_manifest(path, &invalid.to_string()).is_err(), "{key}");
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
    let vector = include_str!("../../fixtures/identities/dataset-derivation-v1.json").trim_end();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), vector);
}
