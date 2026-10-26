#![cfg(unix)]
use metewand_core::{
    canonical::CanonicalValue,
    profile::{SelectedBufferedProfile, StopReason},
};
use metewand_runtime::buffered::{BufferedTrace, Decision, Progress, validate_buffer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{cell::Cell, fs, path::Path, rc::Rc};

fn profile() -> SelectedBufferedProfile {
    serde_json::from_value(json!({"policy":{"metric":"gap","direction":"at_most","threshold":0.01,"record-thresholds":[1.0,0.1,0.01],"max-observations":3,"max-trace-points":20,"max-buffer-bytes":65536,"terminal-reserve-bytes":8192,"max-active-time":"1 s"},"instrumented_scope":"solver","scope":{"clock-origin":"before initialization","included-work":"solver and diagnostics","excluded-preparation":"dataset loading"},"instrumentation":{},"check_schedule":{"every":1,"start":0}})).unwrap()
}
fn progress(index: u64) -> Progress {
    Progress {
        check_index: index,
        work: Default::default(),
    }
}

#[test]
fn measurement_and_snapshot_work_remain_in_cumulative_time() {
    let root = tempfile::tempdir().unwrap();
    let clock = Rc::new(Cell::new(0));
    let read = clock.clone();
    let mut trace =
        BufferedTrace::with_clock(profile(), root.path().to_owned(), move || read.get()).unwrap();
    for (index, gap) in [(0, 0.5), (1, 0.05), (2, 0.005)] {
        clock.set(clock.get() + 10);
        let measured = clock.clone();
        let copied = clock.clone();
        let decision = trace
            .observe(
                progress(index),
                move || {
                    measured.set(measured.get() + 7);
                    Ok(json!({"gap":gap}))
                },
                move || {
                    copied.set(copied.get() + 11);
                    Ok(json!({"x":gap}))
                },
            )
            .unwrap();
        assert_eq!(matches!(decision, Decision::Stop(_)), index == 2);
    }
    trace
        .finish(
            progress(2),
            || panic!("no duplicate metric"),
            || panic!("no duplicate snapshot"),
        )
        .unwrap();
    let manifest = trace.write().unwrap();
    let (manifest, points) =
        validate_buffer(root.path(), Path::new(&manifest), &profile()).unwrap();
    assert_eq!(
        points
            .iter()
            .map(|p| p.instrumented_time_ns)
            .collect::<Vec<_>>(),
        [17, 45, 73]
    );
    assert_eq!(manifest.instrumented_total_time_ns, 84);
    assert_eq!(manifest.snapshots.len(), 3);
}

#[test]
fn bounds_stop_explicitly_and_reserve_the_terminal_result() {
    for bound in ["trace", "time", "bytes", "observations"] {
        let root = tempfile::tempdir().unwrap();
        let mut spec = profile();
        match bound {
            "trace" => spec.policy.max_trace_points = 1,
            "time" => spec.policy.max_active_time = "1 ms".into(),
            "bytes" => spec.policy.terminal_reserve_bytes = 65535,
            "observations" => {
                spec.policy.record_thresholds.clear();
                spec.policy.max_observations = 1;
            }
            _ => unreachable!(),
        };
        let gap = if bound == "observations" { 0.005 } else { 2.0 };
        let mut trace =
            BufferedTrace::with_clock(spec.clone(), root.path().to_owned(), move || {
                if bound == "time" { 2_000_000 } else { 100 }
            })
            .unwrap();
        let decision = trace
            .record(progress(0), json!({"gap":gap}), || Ok(json!({"x":1})))
            .unwrap();
        assert!(matches!(decision, Decision::Stop(_)), "{bound}");
        trace.finish(progress(0), || panic!(), || panic!()).unwrap();
        let path = trace.write().unwrap();
        let (manifest, _) = validate_buffer(root.path(), Path::new(&path), &spec).unwrap();
        assert_eq!(manifest.snapshots.len(), 1);
        if bound == "trace" {
            assert_eq!(manifest.stop_reason, StopReason::TraceLimit);
        }
        assert!(
            trace
                .record(progress(1), json!({"gap":0}), || Ok(json!({"x":0})))
                .is_err()
        );
    }
}

fn rewrite_trace(root: &Path, change: impl FnOnce(&mut Vec<Value>)) {
    let path = root.join("trace.jsonl");
    let mut rows: Vec<Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    change(&mut rows);
    let data = rows
        .into_iter()
        .flat_map(|row| {
            let mut bytes = CanonicalValue::try_from(row)
                .unwrap()
                .to_canonical_bytes()
                .unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect::<Vec<_>>();
    fs::write(&path, &data).unwrap();
    let manifest = root.join("buffer.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    for entry in value["files"].as_array_mut().unwrap() {
        if entry["path"] == "trace.jsonl" {
            entry["size_bytes"] = json!(data.len());
            entry["sha256"] = json!(
                Sha256::digest(&data)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
        }
    }
    fs::write(
        manifest,
        CanonicalValue::try_from(value)
            .unwrap()
            .to_canonical_bytes()
            .unwrap(),
    )
    .unwrap();
}

#[test]
fn returned_buffers_reject_schedule_clock_terminal_and_stopping_forgery() {
    for mutation in ["schedule", "clock", "terminal", "stop"] {
        let root = tempfile::tempdir().unwrap();
        let mut trace =
            BufferedTrace::with_clock(profile(), root.path().to_owned(), || 100).unwrap();
        trace
            .record(progress(0), json!({"gap":2}), || Ok(json!({"x":1})))
            .unwrap();
        trace
            .finish(
                progress(1),
                || Ok(json!({"gap":1.5})),
                || Ok(json!({"x":1})),
            )
            .unwrap();
        trace.write().unwrap();
        rewrite_trace(root.path(), |rows| match mutation {
            "schedule" => rows[0]["check_index"] = json!(2),
            "clock" => rows[1]["instrumented_time_ns"] = json!(99),
            "terminal" => rows[1]["terminal"] = json!(false),
            "stop" => rows[0]["provisional_metrics"]["gap"] = json!(0.001),
            _ => unreachable!(),
        });
        assert!(
            validate_buffer(root.path(), Path::new("buffer.json"), &profile()).is_err(),
            "{mutation}"
        );
    }
}
