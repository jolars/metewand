use metewand_core::{manifest::ObservationPolicyDefinition, profile::ProfilePolicy};
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
    use metewand_core::profile::{CheckSchedule, SelectedStreamingProfile};
    let policy = serde_json::from_value(
        json!({"max-observations":4,"max-checkpoint-bytes":4096,"max-active-time":"1 s"}),
    )
    .unwrap();
    let selected = SelectedStreamingProfile {
        policy,
        check_schedule: CheckSchedule { every: 1, start: 0 },
    };
    assert_eq!(
        (0..12)
            .map(|slot| selected.measurement_index(slot))
            .collect::<Vec<_>>(),
        vec![0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2]
    );
}
