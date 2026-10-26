use metewand_core::{
    applicability::{ApplicabilityDecisionRecord, ApplicabilityOutcome},
    identity::{canonical_identity_bytes, record_id},
    records::RecordId,
};
use serde_json::json;

#[test]
fn applicability_has_exact_canonical_bytes_and_binds_checked_inputs() {
    let mut decision = ApplicabilityDecisionRecord {
        candidate_id: RecordId::from_digest([0; 32]),
        outcome: ApplicabilityOutcome::Unchecked,
        code: "check_pending".into(),
        reason: "Check pending.".into(),
        checked_inputs: None,
        evidence: None,
        details: None,
    };
    assert_eq!(canonical_identity_bytes(&decision).unwrap(),
        br#"{"candidate_id":"mw1-logical-candidate-0000000000000000000000000000000000000000000000000000000000000000","checked_inputs":null,"code":"check_pending","details":null,"evidence":null,"outcome":"unchecked","reason":"Check pending.","version":1}"#);
    decision.outcome = ApplicabilityOutcome::Applicable;
    decision.code = "supported".into();
    decision.checked_inputs =
        Some(json!({"problem_parameters":{"offset":2}, "implementation_parameters":{}}));
    let supported = record_id(&decision).unwrap();
    decision.checked_inputs.as_mut().unwrap()["problem_parameters"]["offset"] = json!(3);
    assert_ne!(record_id(&decision).unwrap(), supported);
}

#[test]
fn every_checked_input_and_part_of_the_evidence_changes_the_decision_identity() {
    let decision = json!({
        "candidate_id": RecordId::<metewand_core::records::LogicalCandidateRecord>::from_digest([0; 32]),
        "outcome": "applicable",
        "code": "supported",
        "reason": "Supported configuration.",
        "checked_inputs": {
            "dataset_instance": "dataset-1",
            "dataset_manifest_path": "dataset-manifest.json",
            "dataset_manifest": {"complete": true, "files": []},
            "problem_contract_id": "contract-1",
            "problem_parameters": {"offset": 2},
            "implementation_parameters": {"method": "dense"},
        },
        "evidence": {
            "worker": {"source_bundle": "source-1", "environment": "environment-1", "launch": "launch-1"},
            "wire_protocol_version": 1,
            "environment": {"umask": "0077"},
            "controls": {"isolation": "unsupported"},
        },
        "details": {"representation": "dense"},
    });
    let identity = record_id(
        &serde_json::from_value::<ApplicabilityDecisionRecord>(decision.clone()).unwrap(),
    )
    .unwrap();
    for (pointer, replacement) in [
        (
            "/candidate_id",
            json!(RecordId::<metewand_core::records::LogicalCandidateRecord>::from_digest([1; 32])),
        ),
        ("/outcome", json!("not_applicable")),
        ("/code", json!("unsupported_representation")),
        ("/reason", json!("Sparse data is unsupported.")),
        ("/checked_inputs/dataset_instance", json!("dataset-2")),
        (
            "/checked_inputs/dataset_manifest_path",
            json!("manifest.json"),
        ),
        (
            "/checked_inputs/dataset_manifest/files",
            json!([{"path": "data.json"}]),
        ),
        ("/checked_inputs/problem_contract_id", json!("contract-2")),
        ("/checked_inputs/problem_parameters/offset", json!(3)),
        (
            "/checked_inputs/implementation_parameters/method",
            json!("sparse"),
        ),
        ("/evidence/worker/source_bundle", json!("source-2")),
        ("/evidence/worker/environment", json!("environment-2")),
        ("/evidence/worker/launch", json!("launch-2")),
        ("/evidence/wire_protocol_version", json!(2)),
        ("/evidence/environment/umask", json!("0022")),
        ("/evidence/controls/isolation", json!("enforced")),
        ("/details/representation", json!("sparse")),
    ] {
        let mut changed = decision.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let changed: ApplicabilityDecisionRecord = serde_json::from_value(changed).unwrap();
        assert_ne!(record_id(&changed).unwrap(), identity, "{pointer}");
    }
}
