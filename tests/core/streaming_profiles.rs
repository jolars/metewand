use crate::core::{manifest::ObservationPolicyDefinition, profile::ProfilePolicy};
use serde_json::{Value, json};

fn policy() -> Value {
    json!({"kind":"profile","max-observations":4,"max-active-time":"1 s",
        "max-checkpoint-bytes":65536,"implementations":{"solver":{
            "delivery":"streaming_profile","measurement-basis":"executor_observed",
            "check-schedule":{"every":2,"start":1}}}})
}

#[test]
fn streaming_requires_bounded_checkpoints_and_executor_timing() {
    let parsed: ObservationPolicyDefinition = serde_json::from_value(policy()).unwrap();
    let profile = parsed.profile.unwrap();
    assert_eq!(profile.policy.max_checkpoint_bytes, 65536);
    assert_eq!(
        profile.declaration_value()["implementations"]["solver"]["delivery"],
        "streaming_profile"
    );
    for (field, value) in [
        ("max-checkpoint-bytes", json!(0)),
        ("max-checkpoint-bytes", Value::Null),
    ] {
        let mut invalid = policy();
        invalid[field] = value;
        assert!(serde_json::from_value::<ObservationPolicyDefinition>(invalid).is_err());
    }
    let mut invalid = policy();
    invalid
        .as_object_mut()
        .unwrap()
        .remove("max-checkpoint-bytes");
    assert!(serde_json::from_value::<ObservationPolicyDefinition>(invalid).is_err());
    let mut invalid = policy();
    invalid["implementations"]["solver"]["measurement-basis"] = json!("instrumented");
    assert!(serde_json::from_value::<ObservationPolicyDefinition>(invalid).is_err());
}

#[test]
fn existing_policy_bytes_do_not_gain_streaming_fields() {
    let value = json!({"max-observations":2,"max-active-time":"1 s"});
    let policy: ProfilePolicy = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(policy).unwrap(), value);
}

#[test]
fn checkpoint_slots_preserve_measurement_repetitions_across_deliveries() {
    use crate::core::profile::{CheckSchedule, SelectedStreamingProfile};
    let policy = serde_json::from_value(
        json!({"max-observations":4,"max-checkpoint-bytes":4096,"max-active-time":"1 s"}),
    )
    .unwrap();
    let selected = SelectedStreamingProfile {
        policy,
        check_schedule: CheckSchedule::Periodic { every: 1, start: 0 },
    };
    assert_eq!(
        (0..12)
            .map(|slot| selected.measurement_index(slot))
            .collect::<Vec<_>>(),
        vec![0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2]
    );
}

#[test]
fn explicit_checkpoint_indices_are_strict_and_preserve_periodic_bytes() {
    use crate::core::profile::CheckSchedule;
    let value = json!({"indices": [0, 1, 2, 4, 8]});
    let schedule: CheckSchedule = serde_json::from_value(value.clone()).unwrap();
    schedule.validate().unwrap();
    assert_eq!(serde_json::to_value(&schedule).unwrap(), value);
    for index in 0..10 {
        assert_eq!(schedule.includes(index), [0, 1, 2, 4, 8].contains(&index));
    }
    assert_eq!(schedule.next_after(None), Some(0));
    assert_eq!(schedule.next_after(Some(2)), Some(4));
    assert_eq!(schedule.next_after(Some(3)), Some(4));
    assert_eq!(schedule.next_after(Some(8)), None);
    for value in [
        json!({"indices": []}),
        json!({"indices": [1, 1]}),
        json!({"indices": [2, 1]}),
        json!({"indices": [-1]}),
        json!({"indices": [9007199254740992u64]}),
        json!({"indices": [0], "every": 1}),
    ] {
        assert!(match serde_json::from_value::<CheckSchedule>(value) {
            Err(_) => true,
            Ok(schedule) => schedule.validate().is_err(),
        });
    }
    let periodic = json!({"every": 3, "start": 1});
    let schedule: CheckSchedule = serde_json::from_value(periodic.clone()).unwrap();
    assert_eq!(serde_json::to_value(schedule).unwrap(), periodic);
}
