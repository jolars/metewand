use crate::core::adaptive::AdaptiveBudget;
use serde_json::json;

fn policy() -> AdaptiveBudget {
    serde_json::from_value(json!({
        "initial-pilot-time": "10 ms", "growth-factor": 2,
        "multiplier": 11, "minimum-time": "20 ms",
        "fallback-time": "500 ms", "watchdog-grace": "1 s"
    }))
    .unwrap()
}

#[test]
fn only_scientific_time_limits_justify_more_calibration_time() {
    use crate::core::adaptive::PilotEvidence;
    let fixture: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../schemas/v1/fixtures/adaptive-budget-v1.json"
    ))
    .unwrap();
    for (status, reason, expected) in [
        ("censored", Some("active_time_limit"), true),
        ("failed", Some("scientific_timeout"), true),
        ("censored", Some("implementation_stopped"), false),
        ("censored", Some("local_target_unconfirmed"), false),
        ("censored", Some("trace_limit"), false),
        ("censored", Some("observation_limit"), false),
        ("censored", Some("byte_limit"), false),
        ("failed", Some("phase_timeout"), false),
        ("failed", Some("evaluator_failure"), false),
        ("failed", Some("implementation_failure"), false),
        ("censored", None, false),
        ("accepted", Some("active_time_limit"), false),
    ] {
        let mut value = fixture["pilots"][0].clone();
        value["status"] = json!(status);
        if let Some(reason) = reason {
            value["reason"] = json!(reason);
        }
        let pilot: PilotEvidence = serde_json::from_value(value).unwrap();
        assert_eq!(pilot.was_time_limited(), expected, "{status}: {reason:?}");
    }
}

#[test]
fn empty_adaptive_configuration_has_useful_defaults_and_no_ceiling() {
    let policy: AdaptiveBudget = serde_json::from_value(json!({})).unwrap();
    policy.validate(None).unwrap();
    assert_eq!(policy.multiplier, 11);
    assert_eq!(policy.minimum_time, "1 ms");
    assert_eq!(
        policy.effective_limit(Some(100_000_000_000), None).unwrap(),
        1_100_000_000_000
    );
    assert!(policy.effective_limit(None, None).is_err());
    let mut limit = policy.initial_limit(None).unwrap();
    assert_eq!(limit, 1_000_000_000);
    for _ in 0..12 {
        limit = policy.next_limit(limit, None).unwrap().unwrap();
    }
    assert_eq!(limit, 4_096_000_000_000);
}

#[test]
fn bounded_defaults_fit_short_ceilings_and_fall_back_to_the_ceiling() {
    let policy: AdaptiveBudget = serde_json::from_value(json!({})).unwrap();
    assert_eq!(policy.initial_limit(Some(2_000_000)).unwrap(), 2_000_000);
    assert_eq!(
        policy.effective_limit(None, Some(2_000_000)).unwrap(),
        2_000_000
    );
}

#[test]
fn omitted_adaptive_settings_normalize_to_explicit_defaults() {
    let implicit: AdaptiveBudget = serde_json::from_value(json!({})).unwrap();
    let explicit: AdaptiveBudget = serde_json::from_value(json!({
        "initial-pilot-time":"1 s","growth-factor":2,"multiplier":11,
        "minimum-time":"1 ms","watchdog-grace":"10 s"
    }))
    .unwrap();
    assert_eq!(implicit, explicit);
    assert_eq!(
        serde_json::to_value(implicit).unwrap(),
        serde_json::to_value(explicit).unwrap()
    );
    assert!(serde_json::from_value::<AdaptiveBudget>(json!({"fallback-time":null})).is_err());
}

#[test]
fn unbounded_durations_report_representation_exhaustion_without_wrapping_or_capping() {
    let policy: AdaptiveBudget = serde_json::from_value(json!({})).unwrap();
    assert!(
        policy
            .next_limit(crate::core::canonical::MAX_SAFE_INTEGER, None)
            .is_err()
    );
    assert!(
        policy
            .effective_limit(Some(crate::core::canonical::MAX_SAFE_INTEGER), None)
            .is_err()
    );
}

#[test]
fn pilots_grow_to_the_ceiling_without_overflow() {
    let policy = policy();
    policy.validate(Some(1_000_000_000)).unwrap();
    let ceiling = Some(1_000_000_000);
    let mut limit = policy.initial_limit(ceiling).unwrap();
    let mut limits = vec![limit];
    while let Some(next) = policy.next_limit(limit, ceiling).unwrap() {
        limits.push(next);
        limit = next;
    }
    assert_eq!(
        limits,
        [10, 20, 40, 80, 160, 320, 640, 1000].map(|ms| ms * 1_000_000)
    );
}

#[test]
fn common_limit_rounds_up_and_obeys_floor_ceiling_and_fallback() {
    let policy = policy();
    let ceiling = Some(1_000_000_000);
    assert_eq!(
        policy.effective_limit(Some(1), ceiling).unwrap(),
        20_000_000
    );
    assert_eq!(
        policy.effective_limit(Some(10_000_001), ceiling).unwrap(),
        111_000_000
    );
    assert_eq!(
        policy.effective_limit(Some(100_000_000), ceiling).unwrap(),
        1_000_000_000
    );
    assert_eq!(policy.effective_limit(None, ceiling).unwrap(), 500_000_000);
}

#[test]
fn invalid_adaptive_bounds_are_rejected() {
    for (key, value) in [
        ("initial-pilot-time", json!("0 s")),
        ("growth-factor", json!(1)),
        ("multiplier", json!(0)),
        ("minimum-time", json!("2 s")),
        ("fallback-time", json!("2 s")),
        ("watchdog-grace", json!("0 s")),
    ] {
        let mut source = serde_json::to_value(policy()).unwrap();
        source[key] = value;
        let result = serde_json::from_value::<AdaptiveBudget>(source);
        assert!(
            result.is_err() || result.unwrap().validate(Some(1_000_000_000)).is_err(),
            "{key}"
        );
    }
}

#[test]
fn frozen_budget_fixture_preserves_its_canonical_schema_and_bytes() {
    use crate::core::{
        adaptive::AdaptiveBudgetDecision, canonical::CanonicalValue,
        public_schemas::PUBLIC_SCHEMAS, schema::SchemaCatalog,
    };
    let mut documents = PUBLIC_SCHEMAS
        .into_iter()
        .map(|schema| {
            (
                schema.repository_path().to_path_buf(),
                serde_json::from_str(schema.source()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    documents.push((
        "schemas/v1/common.schema.json".into(),
        serde_json::from_str(include_str!("../../schemas/v1/common.schema.json")).unwrap(),
    ));
    documents.push(("decision.schema.json".into(),json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"metewand://schemas/v1/profile-observation-policy.schema.json#/$defs/adaptive_decision"})));
    let catalog = SchemaCatalog::try_new(documents).unwrap();
    let schema = std::path::Path::new("decision.schema.json");
    let fixtures: [&[u8]; 2] = [
        include_bytes!("../../schemas/v1/fixtures/adaptive-budget-v1.json"),
        include_bytes!("../../schemas/v1/fixtures/adaptive-budget-v2.json"),
    ];
    for bytes in fixtures {
        let parsed: AdaptiveBudgetDecision = serde_json::from_slice(bytes).unwrap();
        parsed.validate_format().unwrap();
        let value = serde_json::to_value(parsed).unwrap();
        catalog.validate(schema, &value).unwrap();
        let mut mismatched_clock = value.clone();
        mismatched_clock["schema_version"] =
            json!(if value["schema_version"] == 1 { 2 } else { 1 });
        assert!(catalog.validate(schema, &mismatched_clock).is_err());
        assert!(
            serde_json::from_value::<AdaptiveBudgetDecision>(mismatched_clock)
                .unwrap()
                .validate_format()
                .is_err()
        );
        let mut future = value.clone();
        future["schema_version"] = json!(3);
        assert!(serde_json::from_value::<AdaptiveBudgetDecision>(future).is_err());
        assert_eq!(
            CanonicalValue::try_from(value)
                .unwrap()
                .to_canonical_bytes()
                .unwrap(),
            bytes
        );
    }
}
