use metewand_core::{
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
    let fixture = include_str!("../../../schemas/v1/fixtures/export-index-v1.jsonl");
    let record: ExportRecord = serde_json::from_str(fixture).unwrap();
    let mut bytes = CanonicalValue::try_from(serde_json::to_value(record).unwrap())
        .unwrap()
        .to_canonical_bytes()
        .unwrap();
    bytes.push(b'\n');
    assert_eq!(bytes, fixture.as_bytes());
}

#[test]
fn tidy_timing_keeps_partial_resource_values_distinct_from_complete_timing() {
    let mut record: ExportRecord = serde_json::from_str(include_str!(
        "../../../schemas/v1/fixtures/export-index-v1.jsonl"
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
