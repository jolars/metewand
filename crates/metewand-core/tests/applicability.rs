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
