use metewand_core::profile::ProfilePolicy;
use serde_json::{Value, json};

fn policy() -> Value {
    json!({"metric":"gap", "direction":"at_most", "threshold":0.01,
        "record-thresholds":[1.0,0.1,0.01], "max-observations":3,
        "max-trace-points":100, "max-buffer-bytes":65536,
        "terminal-reserve-bytes":8192, "max-active-time":"1 s"})
}

#[test]
fn thresholds_reserve_a_terminal_snapshot_without_an_extra_solve() {
    let p: ProfilePolicy = serde_json::from_value(policy()).unwrap();
    p.validate().unwrap();
    assert!(p.reaches(0.001, 0.01));
    assert!(!p.reaches(0.1, 0.01));
    for (key, value) in [
        ("record-thresholds", json!([0.1, 1.0, 0.01])),
        ("max-observations", json!(2)),
        ("terminal-reserve-bytes", json!(65537)),
        ("max-active-time", json!("0 s")),
    ] {
        let mut invalid = policy();
        invalid[key] = value;
        assert!(
            serde_json::from_value::<ProfilePolicy>(invalid)
                .unwrap()
                .validate()
                .is_err(),
            "{key}"
        );
    }
}

#[test]
fn fixed_count_requires_its_declared_observations() {
    let mut value = policy();
    for key in ["metric", "direction", "threshold", "record-thresholds"] {
        value.as_object_mut().unwrap().remove(key);
    }
    let p: ProfilePolicy = serde_json::from_value(value).unwrap();
    p.validate().unwrap();
    assert!(p.metric.is_none());
}

#[test]
fn optional_target_fields_are_omitted_instead_of_null_or_empty() {
    for (key, invalid) in [
        ("metric", Value::Null),
        ("direction", Value::Null),
        ("threshold", Value::Null),
        ("record-thresholds", json!([])),
    ] {
        let mut value = policy();
        value[key] = invalid;
        assert!(
            serde_json::from_value::<ProfilePolicy>(value).is_err(),
            "{key}"
        );
    }
    for expression in ["+1 s", "01 s", "1.5 s"] {
        let mut value = policy();
        value["max-active-time"] = json!(expression);
        assert!(
            serde_json::from_value::<ProfilePolicy>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
