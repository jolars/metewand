use metewand_core::{
    canonical::CanonicalValue,
    measurements::{ExecutionMeasurements, Measurement, MissingReason, PartialReason},
};
use serde_json::json;

#[test]
fn measurement_states_have_distinct_canonical_representations() {
    for (measurement, expected) in [
        (
            Measurement::Complete { value: 0 },
            r#"{"status":"complete","value":0}"#,
        ),
        (
            Measurement::Partial {
                value: 12,
                reason: PartialReason::ProcessOnly,
            },
            r#"{"reason":"process_only","status":"partial","value":12}"#,
        ),
        (
            Measurement::Unavailable {
                reason: MissingReason::NotStarted,
            },
            r#"{"reason":"not_started","status":"unavailable"}"#,
        ),
    ] {
        let value = serde_json::to_value(measurement).unwrap();
        let canonical = CanonicalValue::try_from(value.clone()).unwrap();
        assert_eq!(canonical.to_canonical_bytes().unwrap(), expected.as_bytes());
        assert_eq!(
            serde_json::from_value::<Measurement>(value).unwrap(),
            measurement
        );
    }
    let value = serde_json::to_value(ExecutionMeasurements::default()).unwrap();
    assert_eq!(
        value["prepare_time_ns"],
        json!({"status":"unavailable", "reason":"not_started"})
    );
    assert_eq!(
        value["materialization_time_ns"]["reason"],
        "provided_dataset"
    );
}

#[test]
fn elapsed_values_preserve_zero_and_report_unrepresentable_intervals() {
    use std::time::Duration;
    assert_eq!(
        Measurement::elapsed(Duration::ZERO, true),
        Measurement::Complete { value: 0 }
    );
    assert_eq!(
        Measurement::elapsed(Duration::from_nanos(9_007_199_254_740_992), true),
        Measurement::unavailable(MissingReason::OutOfRange)
    );
    assert_eq!(
        Measurement::elapsed(Duration::from_nanos(17), false),
        Measurement::Partial {
            value: 17,
            reason: PartialReason::Interrupted
        }
    );
    assert_eq!(
        Measurement::Partial {
            value: 17,
            reason: PartialReason::ProcessOnly
        }
        .complete_value(),
        None
    );
}
