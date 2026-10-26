use metewand_core::{
    canonical::CanonicalValue,
    profile::{ControlSchedule, ProfilePolicy},
};
use serde_json::json;

#[test]
fn finite_schedules_expand_to_canonical_controls() {
    for (schedule, expected) in [
        (
            json!({"kind":"explicit","controls":[{"iterations":1},{"iterations":4}]}),
            json!([{"iterations":1},{"iterations":4}]),
        ),
        (
            json!({"kind":"linear","field":"iterations","start":1,"step":3,"count":2}),
            json!([{"iterations":1},{"iterations":4}]),
        ),
        (
            json!({"kind":"geometric","field":"tolerance","start":0.5,"ratio":0.5,"count":3}),
            json!([{"tolerance":0.5},{"tolerance":0.25},{"tolerance":0.125}]),
        ),
    ] {
        let schedule: ControlSchedule = serde_json::from_value(schedule).unwrap();
        let controls = schedule.expand(3).unwrap();
        assert_eq!(
            controls
                .iter()
                .map(CanonicalValue::as_json)
                .collect::<Vec<_>>(),
            expected.as_array().unwrap().iter().collect::<Vec<_>>()
        );
    }
}

#[test]
fn schedules_reject_empty_unbounded_and_unsafe_controls() {
    for schedule in [
        json!({"kind":"explicit","controls":[]}),
        json!({"kind":"explicit","controls":[null]}),
        json!({"kind":"linear","field":"iterations","start":1,"step":1,"count":0}),
        json!({"kind":"linear","field":"iterations","start":1,"step":1,"count":4}),
        json!({"kind":"linear","field":"iterations","start":9007199254740991u64,"step":1,"count":2}),
        json!({"kind":"geometric","field":"tolerance","start":1e308,"ratio":2,"count":3}),
    ] {
        let schedule: ControlSchedule = serde_json::from_value(schedule).unwrap();
        assert!(schedule.expand(3).is_err(), "{schedule:?}");
    }
}

#[test]
fn fresh_policies_do_not_require_buffer_collection_limits() {
    let policy: ProfilePolicy =
        serde_json::from_value(json!({"max-observations":3,"max-active-time":"1 s"})).unwrap();
    policy.validate().unwrap();
    assert!(policy.validate_buffered().is_err());
    assert_eq!(
        serde_json::to_value(policy).unwrap(),
        json!({"max-observations":3,"max-active-time":"1 s"})
    );
}

#[test]
fn fresh_observation_identity_vectors_preserve_both_coordinates() {
    use metewand_core::{
        identity::{canonical_identity_bytes, record_id},
        profile::FreshControl,
        records::{LogicalObservationSlotRecord, RecordId},
    };
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/identities/fresh-sequence-v1.json"
    ))
    .unwrap();
    for vector in fixture["observations"].as_array().unwrap() {
        let record = LogicalObservationSlotRecord {
            specification: RecordId::from_digest([17; 32]),
            measurement_index: vector["measurement_index"].as_u64().unwrap(),
            fresh: Some(FreshControl {
                schedule_index: vector["schedule_index"].as_u64().unwrap(),
                observation_control: json!({"iterations":1}),
            }),
        };
        assert_eq!(
            String::from_utf8(canonical_identity_bytes(&record).unwrap()).unwrap(),
            vector["canonical"].as_str().unwrap()
        );
        assert_eq!(
            record_id(&record).unwrap().to_string(),
            vector["identity"].as_str().unwrap()
        );
    }
}
