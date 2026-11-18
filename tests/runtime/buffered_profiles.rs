#![cfg(any(unix, windows))]
use crate::core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    materialization::hex,
    profile::{BufferFile, BufferedManifest, Direction, SelectedBufferedProfile, StopReason},
    records::ContentDigest,
};
use crate::runtime::buffered::{BufferedTrace, Decision, Progress, validate_buffer};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{cell::Cell, fs, path::Path, rc::Rc};

fn profile() -> SelectedBufferedProfile {
    serde_json::from_value(json!({"policy":{"metric":"gap","direction":"at_most","threshold":0.01,"record-thresholds":[1.0,0.1,0.01],"max-observations":3,"max-trace-points":20,"max-buffer-bytes":65536,"terminal-reserve-bytes":8192,"max-active-time":"1 s"},"instrumented_scope":"solver","scope":{"clock-origin":"before initialization","included-work":"solver and diagnostics","excluded-preparation":"dataset loading"},"instrumentation":{},"check_schedule":{"every":1,"start":0}})).unwrap()
}

#[test]
fn buffered_collectors_still_require_collection_limits() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = profile();
    spec.policy.max_trace_points = 0;
    spec.policy.max_buffer_bytes = 0;
    spec.policy.terminal_reserve_bytes = 0;
    assert!(BufferedTrace::new(spec, root.path().to_owned()).is_err());
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
    let validated = validate_buffer(root.path(), Path::new(&manifest), &profile()).unwrap();
    let manifest = validated.manifest;
    assert_eq!(
        validated
            .snapshot_points
            .iter()
            .map(|p| p.instrumented_time_ns)
            .collect::<Vec<_>>(),
        [17, 45, 73]
    );
    assert_eq!(manifest.instrumented_total_time_ns, 84);
    assert_eq!(manifest.snapshots.len(), 3);
    for (index, snapshot) in manifest.snapshots.iter().enumerate() {
        assert_eq!(
            snapshot.manifest.to_str().unwrap(),
            format!("snapshots/{index}/result.json")
        );
    }
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
        let manifest = validate_buffer(root.path(), Path::new(&path), &spec)
            .unwrap()
            .manifest;
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

fn canonical_bytes(value: impl Serialize) -> Vec<u8> {
    CanonicalValue::try_from(serde_json::to_value(value).unwrap())
        .unwrap()
        .to_canonical_bytes()
        .unwrap()
}

fn completed_size(manifest: &BufferedManifest) -> u64 {
    manifest.files.iter().map(|f| f.size_bytes).sum::<u64>()
        + canonical_bytes(manifest).len() as u64
}

fn reserved_size(mut prefix: BufferedManifest, trace_bytes: u64) -> u64 {
    prefix.instrumented_total_time_ns = MAX_SAFE_INTEGER;
    prefix.stop_reason = StopReason::ImplementationStopped;
    prefix
        .files
        .iter_mut()
        .find(|entry| entry.path == prefix.trace)
        .unwrap()
        .size_bytes = trace_bytes;
    completed_size(&prefix)
}

#[test]
fn collector_accounts_for_prospective_snapshot_manifest_at_the_byte_limit() {
    let probe = tempfile::tempdir().unwrap();
    let mut spec = profile();
    spec.instrumented_scope = "solver \"é\"".into();
    let record = |trace: &mut BufferedTrace| {
        trace.record(progress(0), json!({"gap": 0.5}), || Ok(json!({"x": 1})))
    };
    let finish = |trace: &mut BufferedTrace| {
        trace
            .finish(
                progress(0),
                || panic!("already measured"),
                || panic!("already saved"),
            )
            .unwrap();
        trace.write().unwrap();
    };
    let mut trace =
        BufferedTrace::with_clock(spec.clone(), probe.path().to_owned(), || 100).unwrap();
    assert_eq!(record(&mut trace).unwrap(), Decision::Continue);
    finish(&mut trace);
    let path = Path::new("buffer.json");
    let saved_bytes = fs::read(probe.path().join(path)).unwrap();
    let mut manifest: BufferedManifest = serde_json::from_slice(&saved_bytes).unwrap();
    let regular_trace_bytes = manifest.files[0].size_bytes + 1; // Encode terminal as false.
    manifest.snapshots[0].thresholds = spec.policy.thresholds();
    let regular = reserved_size(manifest, regular_trace_bytes);

    for (limit, expected) in [
        (regular, Decision::Continue),
        (regular - 1, Decision::Stop(StopReason::ByteLimit)),
    ] {
        let root = tempfile::tempdir().unwrap();
        spec.policy.terminal_reserve_bytes = 256;
        spec.policy.max_buffer_bytes = limit + spec.policy.terminal_reserve_bytes;
        let mut trace =
            BufferedTrace::with_clock(spec.clone(), root.path().to_owned(), || 100).unwrap();
        assert_eq!(record(&mut trace).unwrap(), expected);
        finish(&mut trace);
        let validated = validate_buffer(root.path(), path, &spec).unwrap();
        assert_eq!(validated.manifest.snapshots[0].thresholds, [1.0]);
        if expected == Decision::Continue {
            assert_eq!(fs::read(root.path().join(path)).unwrap(), saved_bytes);
        }
    }

    spec.policy.max_buffer_bytes = regular - 2;
    let root = tempfile::tempdir().unwrap();
    let mut trace = BufferedTrace::with_clock(spec, root.path().to_owned(), || 100).unwrap();
    assert!(
        record(&mut trace)
            .unwrap_err()
            .to_string()
            .contains("max-buffer-bytes")
    );
    assert!(trace.write().is_err());
}

#[test]
fn returned_buffers_reject_native_separators_in_logical_paths() {
    let root = tempfile::tempdir().unwrap();
    let mut trace = BufferedTrace::with_clock(profile(), root.path().to_owned(), || 100).unwrap();
    trace
        .record(progress(0), json!({"gap":0.005}), || Ok(json!({"x":1})))
        .unwrap();
    trace.finish(progress(0), || panic!(), || panic!()).unwrap();
    let path = trace.write().unwrap();
    let mut manifest = validate_buffer(root.path(), Path::new(&path), &profile())
        .unwrap()
        .manifest;

    assert_eq!(
        validate_buffer(
            root.path(),
            Path::new(r"snapshots\0\result.json"),
            &profile()
        )
        .unwrap_err(),
        "buffer path must be normalized and relative"
    );

    manifest.files[1].path = r"snapshots\0\result.json".into();
    fs::write(root.path().join(&path), canonical_bytes(manifest)).unwrap();
    assert_eq!(
        validate_buffer(root.path(), Path::new(&path), &profile()).unwrap_err(),
        "buffer path must be normalized and relative"
    );
}

fn rewrite_trace(root: &Path, change: impl FnOnce(&mut Vec<Value>)) {
    let mut rows: Vec<Value> = fs::read_to_string(root.join("trace.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    change(&mut rows);
    let data = rows
        .into_iter()
        .flat_map(|row| {
            let mut bytes = canonical_bytes(row);
            bytes.push(b'\n');
            bytes
        })
        .collect::<Vec<_>>();
    write_trace(root, &data);
}

fn file_entry(path: &str, data: &[u8]) -> BufferFile {
    BufferFile {
        path: path.into(),
        sha256: hex(ContentDigest::new(Sha256::digest(data).into())),
        size_bytes: data.len() as u64,
    }
}

fn write_trace(root: &Path, data: &[u8]) {
    let manifest_path = root.join("buffer.json");
    let mut manifest: BufferedManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    fs::write(root.join(&manifest.trace), data).unwrap();
    *manifest
        .files
        .iter_mut()
        .find(|entry| entry.path == manifest.trace)
        .unwrap() = file_entry(manifest.trace.to_str().unwrap(), data);
    fs::write(manifest_path, canonical_bytes(manifest)).unwrap();
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

#[test]
fn snapshot_prefix_charges_nested_files_at_the_first_covering_capture() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let mut spec = profile();
    let mut trace = BufferedTrace::with_clock(spec.clone(), root.to_owned(), || 100).unwrap();
    for (index, gap) in [(0, 0.5), (1, 0.05), (2, 0.005)] {
        trace
            .record(progress(index), json!({"gap": gap}), || {
                Ok(json!({"x": gap}))
            })
            .unwrap();
    }
    trace.finish(progress(2), || panic!(), || panic!()).unwrap();
    let path = Path::new("buffer.json");
    trace.write().unwrap();
    let mut manifest = validate_buffer(root, path, &spec).unwrap().manifest;
    let shared = "snapshots/shared";
    for (ordinal, new_path) in [
        (0, "snapshots/shared/result.json"),
        (1, "snapshots/shared/middle/result.json"),
        (2, "snapshots/shared/terminal/result.json"),
    ] {
        let target = root.join(new_path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::rename(root.join(&manifest.snapshots[ordinal].manifest), target).unwrap();
        manifest.snapshots[ordinal].manifest = new_path.into();
        manifest.files[ordinal + 1].path = new_path.into();
    }
    for (path, aux) in [
        ("snapshots/shared/terminal/coefficients.bin", &[0; 53][..]),
        ("snapshots/unrelated.bin", &[0; 71][..]),
    ] {
        fs::write(root.join(path), aux).unwrap();
        manifest.files.push(file_entry(path, aux));
    }
    fs::write(root.join(path), canonical_bytes(&manifest)).unwrap();
    let data = fs::read(root.join(&manifest.trace)).unwrap();
    let mut prefix = manifest.clone();
    prefix.snapshots.pop();
    prefix
        .files
        .retain(|f| f.path == manifest.trace || f.path.starts_with(shared));
    let prior_bytes = data
        .split_inclusive(|&byte| byte == b'\n')
        .take(2)
        .map(<[u8]>::len)
        .sum::<usize>();
    let regular = reserved_size(prefix, prior_bytes as u64);
    spec.policy.max_buffer_bytes = completed_size(&manifest);
    spec.policy.terminal_reserve_bytes = spec.policy.max_buffer_bytes - regular;
    assert!(validate_buffer(root, path, &spec).is_ok());
    spec.policy.terminal_reserve_bytes += 1;
    assert_eq!(
        validate_buffer(root, path, &spec).unwrap_err(),
        "regular collection consumed the terminal byte reserve"
    );
}

#[test]
fn long_returned_trace_keeps_snapshot_points_and_checks_every_prefix() {
    const SPACING: u64 = 50;
    const THRESHOLDS: u64 = 256;
    const LAST: u64 = SPACING * THRESHOLDS;
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let mut spec = profile();
    spec.policy.direction = Some(Direction::AtLeast);
    spec.policy.threshold = Some(THRESHOLDS as f64);
    spec.policy.record_thresholds = (1..=THRESHOLDS).map(|n| n as f64).collect();
    spec.policy.max_observations = THRESHOLDS;
    spec.policy.max_trace_points = LAST + 1;
    spec.policy.max_buffer_bytes = 4_000_000;
    let index_clock = Rc::new(Cell::new(0));
    let clock = index_clock.clone();
    let mut trace =
        BufferedTrace::with_clock(spec.clone(), root.to_owned(), move || clock.get()).unwrap();
    for index in 0..=LAST {
        index_clock.set(index + 1);
        let value = if index >= 2 * SPACING && index % SPACING < 2 {
            index / SPACING
        } else {
            0
        };
        let decision = trace
            .record(progress(index), json!({"gap": value}), || {
                Ok(json!({"x": index}))
            })
            .unwrap();
        assert_eq!(
            decision,
            if index == LAST {
                Decision::Stop(StopReason::TargetReached)
            } else {
                Decision::Continue
            }
        );
    }
    trace
        .finish(progress(LAST), || panic!(), || panic!())
        .unwrap();
    let path = Path::new("buffer.json");
    trace.write().unwrap();

    let validated = validate_buffer(root, path, &spec).unwrap();
    let manifest = validated.manifest;
    assert_eq!(validated.snapshot_points.len() as u64, THRESHOLDS - 1);
    assert_eq!(manifest.snapshots[0].thresholds, [1.0, 2.0]);
    for (multiple, (snapshot, point)) in
        (2..=THRESHOLDS).zip(manifest.snapshots.iter().zip(validated.snapshot_points))
    {
        assert_eq!(snapshot.trace_index, multiple * SPACING);
        assert_eq!(point.check_index, snapshot.trace_index);
        assert_eq!(point.instrumented_time_ns, point.check_index + 1);
        assert_eq!(point.provisional_metrics, json!({"gap": multiple}));
    }

    let recorded = fs::read(root.join(&manifest.trace)).unwrap();
    let last_offset = recorded[..recorded.len() - 1]
        .iter()
        .rposition(|&b| b == b'\n')
        .unwrap()
        + 1;
    let mut prefix = manifest.clone();
    prefix.snapshots.pop();
    prefix.files.pop();
    let regular = reserved_size(prefix, last_offset as u64);
    let mut tight = spec.clone();
    tight.policy.max_buffer_bytes = completed_size(&manifest);
    tight.policy.terminal_reserve_bytes = tight.policy.max_buffer_bytes - regular;
    assert!(validate_buffer(root, path, &tight).is_ok());
    tight.policy.terminal_reserve_bytes += 1;
    assert_eq!(
        validate_buffer(root, path, &tight).unwrap_err(),
        "regular collection consumed the terminal byte reserve"
    );

    let mut reordered = manifest.clone();
    reordered.snapshots[0].thresholds.reverse();
    fs::write(root.join(path), canonical_bytes(&reordered)).unwrap();
    assert!(validate_buffer(root, path, &spec).is_ok());
    let penultimate = reordered.snapshots.len() - 2;
    reordered.snapshots[penultimate].trace_index += 1; // The metric still reaches the threshold.
    fs::write(root.join(path), canonical_bytes(&reordered)).unwrap();
    assert_eq!(
        validate_buffer(root, path, &spec).unwrap_err(),
        "first threshold crossing was not saved"
    );

    fs::write(root.join(path), canonical_bytes(&manifest)).unwrap();
    let mut last: Value = serde_json::from_slice(&recorded[last_offset..]).unwrap();
    last["provisional_metrics"]["gap"] = json!(THRESHOLDS + 1);
    let mut changed = recorded[..last_offset].to_vec();
    changed.extend(canonical_bytes(&last));
    changed.push(b'\n');
    assert_eq!(changed.len(), recorded.len());
    fs::write(root.join(&manifest.trace), changed).unwrap();
    assert_eq!(
        validate_buffer(root, path, &spec).unwrap_err(),
        "buffer file hash mismatch"
    );

    last["check_index"] = json!("invalid");
    let mut malformed = recorded[..last_offset].to_vec();
    malformed.extend(canonical_bytes(last));
    malformed.push(b'\n');
    write_trace(root, &malformed);
    let error = validate_buffer(root, path, &spec).unwrap_err();
    assert!(
        error.contains(&format!("trace.jsonl line {}", LAST + 1)),
        "{error}"
    );
    assert!(error.contains("invalid type"), "{error}");

    write_trace(root, &recorded[..recorded.len() - 1]);
    assert_eq!(
        validate_buffer(root, path, &spec).unwrap_err(),
        "trace must end with a complete JSONL row"
    );
}
