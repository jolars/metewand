use crate::core::{
    canonical::CanonicalValue,
    export::{ExportRecord, csv_cell, metric_values},
};
use serde_json::json;

#[test]
fn metric_coordinates_preserve_nulls_arrays_and_ambiguous_names() {
    assert_eq!(
        metric_values(&json!({"a/b": {"~x": 2}, "empty": {}, "missing": null, "trace": [1, 2]})),
        vec![
            ("/a~1b/~0x".into(), json!(2)),
            ("/empty".into(), json!({})),
            ("/missing".into(), json!(null)),
            ("/trace".into(), json!([1, 2])),
        ]
    );
}

#[test]
fn csv_cells_preserve_quotes_commas_and_line_breaks() {
    assert_eq!(csv_cell("plain"), "plain");
    assert_eq!(csv_cell("a,\"b\"\r\nc"), "\"a,\"\"b\"\"\r\nc\"");
}

#[test]
fn index_record_has_exact_version_one_canonical_bytes() {
    let fixture = include_str!("../../schemas/v1/fixtures/export-index-v1.jsonl");
    let record: ExportRecord = serde_json::from_str(fixture).unwrap();
    let mut bytes = CanonicalValue::try_from(serde_json::to_value(record).unwrap())
        .unwrap()
        .to_canonical_bytes()
        .unwrap();
    bytes.push(b'\n');
    assert_eq!(bytes, fixture.as_bytes());
}

#[test]
fn analysis_snapshot_fixture_has_versioned_canonical_bytes() {
    use crate::core::public_schemas::{PublicSchema, public_schema_catalog};
    let fixture = include_bytes!("../../schemas/v1/fixtures/analysis-snapshot-v1.json");
    let snapshot = CanonicalValue::from_slice(fixture).unwrap();
    let mut bytes = snapshot.to_canonical_bytes().unwrap();
    bytes.push(b'\n');
    assert_eq!(bytes, fixture);
    public_schema_catalog()
        .unwrap()
        .validate(
            PublicSchema::AnalysisSnapshot.repository_path(),
            snapshot.as_json(),
        )
        .unwrap();
}

#[test]
fn tidy_timing_keeps_partial_resource_values_distinct_from_complete_timing() {
    let mut record: ExportRecord = serde_json::from_str(include_str!(
        "../../schemas/v1/fixtures/export-index-v1.jsonl"
    ))
    .unwrap();
    record.timing = json!({"timed_wall_time_ns": 123, "cpu_time_ns": null});
    record.provenance = json!({"data":{"measurements":{"cpu_time_ns":{"status":"partial","value":100,"reason":"process_only"}}}});
    record.metrics = json!({"value":null});
    let row = record.tidy_rows().pop().unwrap();
    assert_eq!(row.timed_wall_time_ns, Some(123));
    assert_eq!(row.timed_wall_time_status, "complete");
    assert_eq!(row.cpu_time_ns, Some(100));
    assert_eq!(row.cpu_time_status, "partial");
    assert_eq!(row.peak_rss_bytes, None);
    assert_eq!(row.peak_rss_status, "unavailable");
    assert_eq!(row.metric_name.as_deref(), Some("/value"));
    assert_eq!(row.metric_value, json!(null));
}

fn analysis_fixture() -> crate::core::export::analysis::AnalysisSnapshot {
    use crate::core::{comparison::ComparisonRow, export::analysis::*};
    let observation = AnalysisObservation {
        comparison: ComparisonRow {
            profile: Some(json!({"delivery":"fresh_sequence","schedule_index":0})),
            candidate_id: "candidate".into(),
            attempt_id: "accepted".into(),
            observation_id: Some("point".into()),
            experiment: None,
            case: None,
            dataset: None,
            problem: None,
            implementation: None,
            parameters: json!({}),
            implementation_repetition: Some(0),
            measurement_index: Some(0),
            retry_index: 1,
            status: "accepted".into(),
            valid: Some(true),
            completion: Some("reached".into()),
            selected: Some(true),
            metrics: json!({"gap":0.1}),
            timed_wall_time_ns: Some(100),
            timed_wall_time_status: "complete".into(),
            timing_scope: Some("execute_only".into()),
            instrumented_time_ns: None,
            instrumented_scope: None,
            diagnostic: None,
        },
        warmup: Some(false),
        implementation_time_ns: Some(50),
        evaluation_time_ns: Some(10),
        active_time_ns: None,
        elapsed_wall_time_ns: None,
        statistics: json!({}),
        compatibility_id: "class".into(),
        provenance_id: "metadata".into(),
    };
    AnalysisSnapshot {
        schema_version: 1,
        candidates: vec![AnalysisCandidate {
            id: "candidate".into(),
            experiment: None,
            implementation: None,
            dataset: None,
            problem: None,
            case: None,
            parameters: json!({}),
            implementation_repetitions: Some(1),
            measurement_repetitions: Some(1),
            execution_policy: json!({}),
            observation_policy: json!({"kind":"profile"}),
        }],
        observations: vec![observation],
        attempts: vec![],
        applicability: vec![],
        slots: vec![AnalysisSlot {
            id: "slot".into(),
            candidate_id: "candidate".into(),
            resolved_specification_id: "run".into(),
            state: "satisfied".into(),
            selected_observation_ids: vec!["point".into()],
        }],
        profiles: vec![AnalysisProfile {
            candidate_id: "candidate".into(),
            resolved_specification_id: "run".into(),
            measurement_index: Some(0),
            delivery: "fresh_sequence".into(),
            status: "accepted".into(),
            reason: Some("target_reached".into()),
            progress: json!({}),
        }],
        provenance: Default::default(),
        compatibility_classes: Default::default(),
        completeness: Default::default(),
    }
}

#[test]
fn analysis_completeness_retains_intermediate_points_and_target_based_skips() {
    let mut analysis = analysis_fixture();
    let mut intermediate = analysis.observations[0].clone();
    intermediate.comparison.observation_id = Some("early".into());
    intermediate.comparison.completion = Some("not_reached".into());
    analysis.observations.insert(0, intermediate);
    let mut early = analysis.slots[0].clone();
    early.id = "early-slot".into();
    early.selected_observation_ids = vec!["early".into()];
    analysis.slots.push(early);
    let mut skipped = analysis.slots[0].clone();
    skipped.id = "skipped".into();
    skipped.state = "skipped_target_reached".into();
    skipped.selected_observation_ids.clear();
    analysis.slots.push(skipped);
    analysis.check_completeness();
    assert!(analysis.completeness.complete);
    assert_eq!(
        analysis.observations[0].comparison.completion.as_deref(),
        Some("not_reached")
    );
}

#[test]
fn analysis_completeness_rejects_exhausted_profiles_missing_repetitions_and_selections() {
    let mut analysis = analysis_fixture();
    analysis.profiles[0].status = "censored".into();
    analysis.profiles[0].reason = Some("schedule_exhausted".into());
    analysis.candidates[0].measurement_repetitions = Some(2);
    analysis.observations[0].comparison.selected = Some(false);
    analysis.check_completeness();
    assert!(!analysis.completeness.complete);
    let codes = analysis
        .completeness
        .issues
        .iter()
        .map(|i| i.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"unfinished_profile"));
    assert!(codes.contains(&"missing_repetitions"));
    assert!(codes.contains(&"missing_observation"));
}

#[test]
fn analysis_completeness_uses_selected_results_after_successful_retries() {
    use crate::core::export::analysis::AnalysisAttempt;
    let mut analysis = analysis_fixture();
    analysis.attempts.push(AnalysisAttempt {
        id: "earlier".into(),
        candidate_id: "candidate".into(),
        slot_id: Some("slot".into()),
        retry_index: Some(0),
        state: Some("failed".into()),
        failure_code: Some("invalid_result".into()),
        warmup: Some(false),
        started_at_ns: None,
        finished_at_ns: None,
    });
    analysis.check_completeness();
    assert!(analysis.completeness.complete);
    analysis.observations[0].comparison.valid = Some(false);
    analysis.check_completeness();
    assert!(!analysis.completeness.complete);
}
