#![cfg(unix)]

use std::{fs, time::Duration};

use metewand_core::{
    canonical::CanonicalValue,
    export::{ExportFormat, ExportRecordType},
    identity::identify_record,
    manifest::PrimaryTime,
    records::{
        ArtifactReference, CompletionStatus, ContentDigest, ObservationTimingRecord,
        ProvenanceRecord, RecordId, ResolvedAttemptSlotRecord, ResolvedObservationSlotRecord,
        RunObservationRecord,
    },
    run_documents::observation_document,
};
use metewand_runtime::{
    export::ResultSet,
    run_store::{
        ArtifactKind, AttemptPhase, LeaseRequest, RetryAuthorization, RunStore, TerminalOutcome,
    },
};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn every_terminal_outcome_retains_earlier_observations_and_distinct_guarantees() {
    let root = tempfile::tempdir().unwrap();
    let mut store = RunStore::open(root.path()).unwrap();
    let slot = RecordId::<ResolvedAttemptSlotRecord>::from_digest([1; 32]);
    let observation_slot = RecordId::<ResolvedObservationSlotRecord>::from_digest([2; 32]);
    let db = Connection::open(store.database_path()).unwrap();
    db.execute_batch("INSERT INTO applicability_decisions VALUES ('decision', 'candidate', 1, '{}'); INSERT INTO applicability_decisions VALUES ('excluded', 'excluded-candidate', 0, '{\"reason\":\"unsupported shape\"}'); INSERT INTO resolved_runs VALUES ('run', 'logical', 'candidate', 'decision', 1, '{}');").unwrap();
    db.execute("INSERT INTO attempt_slots VALUES (?1, 'logical-slot', 'run', 'one_shot', 'measurement', '{}')", [slot.to_string()]).unwrap();
    db.execute(
        "INSERT INTO observation_slots VALUES (?1, 'logical-observation', 'run', '{}')",
        [observation_slot.to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO attempt_observation_slots VALUES (?1, ?2, 'run')",
        params![slot.to_string(), observation_slot.to_string()],
    )
    .unwrap();
    let outcomes = [
        TerminalOutcome::Superseded,
        TerminalOutcome::Censored {
            reason: "budget exhausted".into(),
        },
        TerminalOutcome::Failed {
            code: "invalid_result".into(),
            details: CanonicalValue::try_from(json!({"message": "bad answer"})).unwrap(),
        },
        TerminalOutcome::Failed {
            code: "implementation_crash".into(),
            details: CanonicalValue::try_from(json!({"message": "later failure"})).unwrap(),
        },
        TerminalOutcome::Accepted,
    ];
    for (index, outcome) in outcomes.into_iter().enumerate() {
        let attempt = store
            .reserve(
                slot,
                RetryAuthorization::Next { retry: true },
                LeaseRequest {
                    id: format!("reservation-{index}"),
                    owner_token: "owner".into(),
                    expires_at_ns: 1000,
                },
                10,
            )
            .unwrap();
        let accepted = matches!(outcome, TerminalOutcome::Accepted);
        let attempt = if accepted {
            store
                .begin_finalization(
                    &attempt,
                    LeaseRequest {
                        id: "claim".into(),
                        owner_token: "owner".into(),
                        expires_at_ns: 1000,
                    },
                    11,
                )
                .unwrap()
                .attempt
        } else {
            store
                .advance(&attempt, AttemptPhase::Finalizing, 11)
                .unwrap()
        };
        let mut references = Vec::new();
        if index != 2 {
            let metrics_bytes = br#"{"schema_version":1,"schema":"metrics.json","data":{"error":null,"nested":{"gap":0},"text":"a,\"b\"\r\nc"}}"#;
            let result_bytes =
                br#"{"schema_version":1,"schema":"result.json","data":{},"files":[]}"#;
            let scope = if index == 0 {
                "execute_only"
            } else {
                "prepare_and_execute"
            };
            let environment = if index == 1 {
                "closure_locked"
            } else {
                "development"
            };
            let controls = if index == 4 {
                "enforced"
            } else {
                "best_effort"
            };
            let observation = identify_record(RunObservationRecord {
        profile: None,
                slot: observation_slot,
                producing_attempt: attempt.id(),
                result: ArtifactReference {
                    id: RecordId::from_digest([4; 32]),
                    path: serde_json::from_value(json!("result")).unwrap(),
                    sha256: ContentDigest::new([0; 32]),
                },
                metrics: ArtifactReference {
                    id: RecordId::from_digest([5; 32]),
                    path: serde_json::from_value(json!("metrics.json")).unwrap(),
                    sha256: ContentDigest::new(Sha256::digest(metrics_bytes).into()),
                },
                timing: ObservationTimingRecord {
                    primary_time: PrimaryTime::TimedWallTime,
                    timed_wall_time: Duration::from_nanos(12),
                    implementation_time: None,
                    evaluation_time: Duration::ZERO,
                    cpu_time: None,
                    peak_rss_bytes: None,
                },
                completion: CompletionStatus::NotRequired,
                provenance: ProvenanceRecord {
                    tool_version: "test".into(),
                    wire_protocol_version: 1,
                    execution_semantics_version: 2,
                    data: CanonicalValue::try_from(json!({
                        "context": {
                            "timing_scope": scope,
                            "execution_policy": {"version":1,"primary_time":"timed_wall_time"},
                        },
                        "execution": {
                            "environment_class": environment,
                            "timing_scope": scope,
                            "controls": [{"name":"network","status":controls}],
                        },
                        "measurements": {
                            "version":2,
                            "cpu_time_scope":"prepare_and_execute",
                            "peak_rss_scope":"worker_lifetime_through_execute",
                            "cpu_time_ns":{"status":"partial","value":10,"reason":"process_only"},
                            "peak_rss_bytes":{"status":"partial","value":100,"reason":"process_only"},
                        },
                        "result_manifest":"result/result.json",
                    })).unwrap(),
                },
            }).unwrap();
            let document = observation_document(&observation.record)
                .unwrap()
                .to_canonical_bytes()
                .unwrap();
            let artifact = store
                .publish_artifact(
                    &attempt,
                    ArtifactKind::Observation,
                    |path| {
                        fs::create_dir(path.join("result"))?;
                        fs::write(path.join("result/result.json"), result_bytes)?;
                        fs::write(path.join("metrics.json"), metrics_bytes)?;
                        fs::write(path.join("observation.json"), &document)
                    },
                    || 12,
                )
                .unwrap();
            store
                .register_observation(&attempt, &observation, &artifact.artifact, accepted, 13)
                .unwrap();
            references.push(("observation".into(), artifact.artifact.id));
        }
        store
            .publish_finalization(&attempt, outcome, &references, |_| Ok(()), || 14)
            .unwrap();
    }
    drop(db);
    drop(store);
    let results = ResultSet::read(root.path()).unwrap();
    let attempts = results
        .records
        .iter()
        .filter(|r| r.record_type == ExportRecordType::Attempt)
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 5);
    for outcome in ["accepted", "invalid", "censored", "superseded", "failed"] {
        assert!(
            attempts
                .iter()
                .any(|r| r.outcome.as_deref() == Some(outcome)),
            "{outcome}"
        );
    }
    let observations = results
        .records
        .iter()
        .filter(|r| r.record_type == ExportRecordType::Observation)
        .collect::<Vec<_>>();
    assert_eq!(observations.len(), 4);
    assert_eq!(results.select(&["decision".into()]).unwrap().len(), 10);
    assert!(observations.iter().all(|r| r.valid == Some(true)));
    assert_eq!(
        observations
            .iter()
            .filter(|r| r.selected == Some(true))
            .count(),
        1
    );
    assert!(
        observations
            .iter()
            .any(|r| r.attempt_state.as_deref() == Some("failed"))
    );
    assert!(
        results
            .records
            .iter()
            .any(|r| r.outcome.as_deref() == Some("not_applicable"))
    );
    assert!(
        observations
            .iter()
            .all(|r| r.compatibility.unclassified_record.is_none())
    );
    // Each comparison changes only one of the classification dimensions.
    assert_ne!(
        observations[0].compatibility_id,
        observations[2].compatibility_id
    );
    assert_ne!(
        observations[1].compatibility_id,
        observations[2].compatibility_id
    );
    assert_ne!(
        observations[2].compatibility_id,
        observations[3].compatibility_id
    );
    let index = results.index_bytes().unwrap();
    assert_eq!(
        results.rebuild_index().unwrap(),
        root.path().join("index.jsonl")
    );
    fs::write(root.path().join("index.jsonl"), "stale").unwrap();
    assert_eq!(
        ResultSet::read(root.path()).unwrap().index_bytes().unwrap(),
        index
    );
    let selected = results.select(&[observations[0].id.clone()]).unwrap();
    assert_eq!(
        selected
            .iter()
            .filter(|r| r.record_type == ExportRecordType::Attempt)
            .count(),
        1
    );
    assert_eq!(
        selected
            .iter()
            .filter(|r| r.record_type == ExportRecordType::Observation)
            .count(),
        1
    );
    let target = root.path().join("table");
    let report = results
        .export(Some(&target), ExportFormat::Csv, &[], false)
        .unwrap();
    assert_eq!(report.records, 11);
    assert_eq!(report.rows, 19);
    let csv = fs::read_to_string(target.join("results.csv")).unwrap();
    assert!(csv.contains("superseded"));
    assert!(csv.contains("censored"));
    assert!(csv.contains("not_applicable"));
    let parsed = std::process::Command::new("python3").args(["-c", "import csv,json,sys; print(json.dumps(list(csv.DictReader(open(sys.argv[1], newline='')))))"]).arg(target.join("results.csv")).output().unwrap();
    assert!(parsed.status.success(), "{parsed:?}");
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&parsed.stdout).unwrap();
    let text = parsed.iter().find(|r| r["metric_name"] == "/text").unwrap();
    assert_eq!(
        serde_json::from_str::<String>(text["metric_value"].as_str().unwrap()).unwrap(),
        "a,\"b\"\r\nc"
    );
    assert_eq!(text["timed_wall_time_ns"], "12");
    assert_eq!(text["cpu_time_status"], "partial");
    assert!(
        results
            .export(Some(&target), ExportFormat::Jsonl, &[], false)
            .is_err()
    );
    let unsafe_index = root.path().join("index.jsonl");
    fs::remove_file(&unsafe_index).unwrap();
    std::os::unix::fs::symlink("table/results.csv", &unsafe_index).unwrap();
    assert!(results.rebuild_index().is_err());
    assert_eq!(fs::read_to_string(target.join("results.csv")).unwrap(), csv);
    // Export re-verifies artifacts even when an index from the same run exists.
    let artifact = observations[0].evidence["source"]["artifact_id"]
        .as_str()
        .unwrap();
    let artifact = artifact.strip_prefix("run-store-v1:").unwrap();
    fs::write(
        root.path()
            .join("artifacts")
            .join(artifact)
            .join("metrics.json"),
        "corrupt",
    )
    .unwrap();
    assert!(ResultSet::read(root.path()).is_err());
}
