use super::{BufferSize, crossings, encoded_len};
use crate::core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    materialization::hex,
    profile::{
        BufferFile, BufferedManifest, Direction, SelectedBufferedProfile, StopReason, TracePoint,
    },
    records::ContentDigest,
};
use crate::runtime::{local_tree::hash_local_tree, platform_fs};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Take},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct ValidatedBuffer {
    pub manifest: BufferedManifest,
    pub snapshot_points: Vec<TracePoint>,
    pub tree_digest: ContentDigest,
}

/// Validate the entire buffer before using any of its snapshots. Keep decoded
/// points in capture order for independent evaluation.
pub(crate) fn validate_buffer(
    root: &Path,
    path: &Path,
    profile: &SelectedBufferedProfile,
) -> Result<ValidatedBuffer, String> {
    profile.policy.validate()?;
    profile.policy.validate_buffered()?;
    let mut actual = BTreeMap::new();
    inventory_files(root, root, &mut actual)?;
    let total = actual
        .values()
        .try_fold(0u64, |sum, value| sum.checked_add(*value))
        .ok_or("buffer size overflow")?;
    if total > profile.policy.max_buffer_bytes {
        return Err("buffer exceeds max-buffer-bytes".into());
    }
    let tree_digest = hash_local_tree(root).map_err(|error| error.to_string())?;
    safe_path(path)?;
    let size = actual.remove(path).ok_or("missing buffer manifest")?;
    let manifest = read_manifest(root, path, size)?;
    if manifest.schema_version != 1
        || !manifest.complete
        || manifest.clock != "monotonic_ns"
        || manifest.instrumented_scope != profile.instrumented_scope
        || manifest.instrumented_total_time_ns > MAX_SAFE_INTEGER
    {
        return Err("invalid buffer version, completion, clock, scope, or total duration".into());
    }
    safe_path(&manifest.trace)?;
    let mut declared = BTreeSet::new();
    let mut trace = None;
    for entry in &manifest.files {
        safe_path(&entry.path)?;
        if !declared.insert(entry.path.as_path())
            || actual.get(&entry.path) != Some(&entry.size_bytes)
        {
            return Err("duplicate, missing, or incorrectly sized buffer file".into());
        }
        if entry.path == manifest.trace {
            trace = Some(entry);
        } else {
            verify_file(root, entry)?;
        }
    }
    if declared.len() != actual.len() {
        return Err("buffer contains undeclared files".into());
    }
    let trace = trace.ok_or("trace is not declared")?;
    if manifest.snapshots.is_empty()
        || manifest.snapshots.len() as u64 > profile.policy.max_observations
    {
        return Err("snapshot count is empty or exceeds bound".into());
    }
    let file_charges = snapshot_file_charges(&manifest, &declared)?;
    let snapshot_points = validate_trace(root, trace, &manifest, profile, &file_charges)?;
    if hash_local_tree(root).map_err(|error| error.to_string())? != tree_digest {
        return Err("buffer changed during validation".into());
    }
    Ok(ValidatedBuffer {
        manifest,
        snapshot_points,
        tree_digest,
    })
}

fn open_buffer_file(root: &Path, path: &Path, size: u64) -> Result<Take<File>, String> {
    let file = platform_fs::open_regular(&root.join(path), OpenOptions::new().read(true))
        .map_err(|error| error.to_string())?;
    // Read at most one byte past the checked inventory size to detect growth.
    Ok(file.take(size + 1))
}

fn read_manifest(root: &Path, path: &Path, size: u64) -> Result<BufferedManifest, String> {
    let mut data = Vec::new();
    let len = open_buffer_file(root, path, size)?
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if len as u64 != size {
        return Err("buffer file changed size during validation".into());
    }
    let canonical = CanonicalValue::from_slice(&data).map_err(|error| error.to_string())?;
    BufferedManifest::deserialize(canonical.as_json()).map_err(|error| error.to_string())
}

fn verify_file(root: &Path, entry: &BufferFile) -> Result<(), String> {
    let mut reader = BufReader::new(open_buffer_file(root, &entry.path, entry.size_bytes)?);
    let mut hash = Sha256::new();
    let mut size = 0;
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        };
        if chunk.is_empty() {
            break;
        }
        hash.update(chunk);
        let len = chunk.len();
        reader.consume(len);
        size += len as u64;
    }
    verify_file_digest(entry, size, hash)
}

fn verify_file_digest(entry: &BufferFile, size: u64, hash: Sha256) -> Result<(), String> {
    if size != entry.size_bytes {
        return Err("buffer file changed size during validation".into());
    }
    if hex(ContentDigest::new(hash.finalize().into())) != entry.sha256 {
        return Err("buffer file hash mismatch".into());
    }
    Ok(())
}

fn snapshot_file_charges(
    manifest: &BufferedManifest,
    declared: &BTreeSet<&Path>,
) -> Result<Vec<u64>, String> {
    let mut parents = BTreeMap::new();
    let mut previous = None;
    for (ordinal, snapshot) in manifest.snapshots.iter().enumerate() {
        safe_path(&snapshot.manifest)?;
        if snapshot.capture_ordinal != ordinal as u64
            || previous.is_some_and(|index| index >= snapshot.trace_index)
            || !declared.contains(snapshot.manifest.as_path())
        {
            return Err("invalid snapshot ordinal, order, or trace reference".into());
        }
        previous = Some(snapshot.trace_index);
        let parent = snapshot
            .manifest
            .parent()
            .expect("declared relative file has a parent");
        parents.entry(parent).or_insert(ordinal);
    }

    let mut charges = vec![0; manifest.snapshots.len()];
    for entry in manifest
        .files
        .iter()
        .filter(|entry| entry.path != manifest.trace)
    {
        // A file enters the reserve calculation at the first capture whose
        // directory contains it, including files under nested snapshot paths.
        let first_capture = entry
            .path
            .ancestors()
            .filter_map(|path| parents.get(path))
            .min();
        if let Some(&ordinal) = first_capture {
            charges[ordinal] +=
                BufferSize::file_charge(entry).map_err(|error| error.to_string())?;
        }
    }
    Ok(charges)
}

fn validate_trace(
    root: &Path,
    trace: &BufferFile,
    manifest: &BufferedManifest,
    profile: &SelectedBufferedProfile,
    file_charges: &[u64],
) -> Result<Vec<TracePoint>, String> {
    let mut reader = BufReader::new(open_buffer_file(root, &trace.path, trace.size_bytes)?);
    let mut hash = Sha256::new();
    let mut file_size = 0;
    let mut line = Vec::new();
    let mut line_number = 0u64;
    let mut count = 0;
    let mut saved: Vec<TracePoint> = Vec::with_capacity(manifest.snapshots.len());
    let mut previous_unsaved: Option<TracePoint> = None;
    let mut target_value = None;
    let thresholds = profile.policy.thresholds();
    let mut remaining = thresholds.as_slice();
    let active_time = profile.policy.active_time_ns();
    let regular_limit = profile.policy.max_buffer_bytes - profile.policy.terminal_reserve_bytes;
    let mut prefix = BufferSize::new(&manifest.instrumented_scope, trace.clone())
        .map_err(|error| error.to_string())?;
    let mut prefix_trace_bytes = 0;

    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            break;
        }
        line_number += 1;
        file_size += line.len() as u64;
        hash.update(&line);
        let row = line
            .strip_suffix(b"\n")
            .ok_or("trace must end with a complete JSONL row")?;
        if row.is_empty() {
            continue;
        }
        if count >= profile.policy.max_trace_points {
            return Err("trace point bound exceeded".into());
        }
        let previous = previous_unsaved.as_ref().or_else(|| saved.last());
        if previous.is_some_and(|p| p.terminal) {
            return Err("buffer requires exactly one terminal check at its end".into());
        }
        let point = parse_point(row).map_err(|error| {
            format!("trace {} line {line_number}: {error}", trace.path.display())
        })?;
        target_value = validate_point(
            &point,
            previous,
            profile,
            manifest.instrumented_total_time_ns,
        )?;
        if !point.terminal
            && (point.instrumented_time_ns >= *active_time.as_ref()?
                || target_value
                    .zip(profile.policy.threshold)
                    .is_some_and(|(v, t)| profile.policy.reaches(v, t)))
        {
            return Err("execution continued after a required local stop".into());
        }

        let snapshot = manifest
            .snapshots
            .get(saved.len())
            .filter(|s| s.trace_index == count);
        let crossed = crossings(&profile.policy, remaining, target_value);
        let claims = snapshot.map_or(&[][..], |s| s.thresholds.as_slice());
        validate_crossing_claims(crossed, claims, profile.policy.direction)?;
        remaining = &remaining[crossed.len()..];
        if profile.policy.metric.is_none() && snapshot.is_none() {
            return Err("fixed-count profiles must save every checked iterate".into());
        }

        if !point.terminal {
            // Empty lines count towards the total file size, while the prefix
            // reserve is defined over the nonempty JSONL rows.
            prefix_trace_bytes += line.len() as u64;
            if let Some(snapshot) = snapshot {
                prefix.snapshot(encoded_len(snapshot).map_err(|error| error.to_string())?);
                prefix.other_files += file_charges[saved.len()];
            }
            if prefix.total(prefix_trace_bytes) > regular_limit {
                return Err("regular collection consumed the terminal byte reserve".into());
            }
        }
        if snapshot.is_some() {
            saved.push(point);
            previous_unsaved = None;
        } else {
            previous_unsaved = Some(point);
        }
        count += 1;
    }

    verify_file_digest(trace, file_size, hash)?;
    if file_size == 0 {
        return Err("trace must end with a complete JSONL row".into());
    }
    let last = previous_unsaved
        .as_ref()
        .or_else(|| saved.last())
        .filter(|p| p.terminal)
        .ok_or("buffer requires exactly one terminal check at its end")?;
    if saved.len() != manifest.snapshots.len() {
        return Err("invalid snapshot ordinal, order, or trace reference".into());
    }
    if previous_unsaved.is_some() {
        return Err("terminal iterate requires a snapshot".into());
    }
    match manifest.stop_reason {
        StopReason::TargetReached
            if target_value
                .zip(profile.policy.threshold)
                .is_none_or(|(v, t)| !profile.policy.reaches(v, t)) =>
        {
            return Err("unsubstantiated local target stop".into());
        }
        StopReason::FixedCount
            if profile.policy.metric.is_some()
                || saved.len() as u64 != profile.policy.max_observations =>
        {
            return Err("unsubstantiated fixed-count stop".into());
        }
        StopReason::TraceLimit if count != profile.policy.max_trace_points => {
            return Err("unsubstantiated trace-limit stop".into());
        }
        StopReason::ObservationLimit if saved.len() as u64 != profile.policy.max_observations => {
            return Err("unsubstantiated observation-limit stop".into());
        }
        StopReason::ActiveTimeLimit if last.instrumented_time_ns < *active_time.as_ref()? => {
            return Err("unsubstantiated active-time stop".into());
        }
        _ => {}
    }
    Ok(saved)
}

fn parse_point(row: &[u8]) -> Result<TracePoint, String> {
    let value = CanonicalValue::from_slice(row).map_err(|error| error.to_string())?;
    TracePoint::deserialize(value.as_json()).map_err(|error| error.to_string())
}

fn validate_point(
    point: &TracePoint,
    previous: Option<&TracePoint>,
    profile: &SelectedBufferedProfile,
    total_time: u64,
) -> Result<Option<f64>, String> {
    let expected = profile
        .check_schedule
        .next_after(previous.map(|p| p.check_index));
    if point.check_index > MAX_SAFE_INTEGER
        || point.instrumented_time_ns > total_time
        || !point.provisional_metrics.is_object()
        || (!point.terminal && Some(point.check_index) != expected)
        || (point.terminal && expected.is_some_and(|n| point.check_index > n))
        || point.work.values().any(|v| *v > MAX_SAFE_INTEGER)
    {
        return Err("invalid trace clock, progress, metrics, or check schedule".into());
    }
    if let Some(previous) = previous
        && (point.check_index <= previous.check_index
            || point.instrumented_time_ns < previous.instrumented_time_ns
            || previous
                .work
                .iter()
                .any(|(k, v)| point.work.get(k).is_none_or(|n| n < v)))
    {
        return Err("trace progress, times, and work counts must be monotonic".into());
    }
    profile
        .policy
        .metric
        .as_ref()
        .map(|metric| {
            point
                .provisional_metrics
                .get(metric)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .ok_or_else(|| "trace target metric must be finite and numeric".into())
        })
        .transpose()
}

fn validate_crossing_claims(
    crossed: &[f64],
    claims: &[f64],
    direction: Option<Direction>,
) -> Result<(), String> {
    if claims == crossed {
        return Ok(());
    }
    // Native producers may list simultaneous crossings in any order.
    let mut claims = claims.to_vec();
    claims.sort_unstable_by(|a, b| {
        let order = a.partial_cmp(b).expect("finite trace thresholds");
        if direction == Some(Direction::AtMost) {
            order.reverse()
        } else {
            order
        }
    });
    let mut needed = crossed.iter();
    for claim in &claims {
        if !needed.by_ref().any(|t| t == claim) {
            return Err("invalid first-observed threshold claim".into());
        }
    }
    if claims.len() != crossed.len() {
        return Err("first threshold crossing was not saved".into());
    }
    Ok(())
}

fn safe_path(path: &Path) -> Result<(), String> {
    let text = path.to_str().ok_or("buffer path must be UTF-8")?;
    if text.is_empty()
        || text.starts_with('/')
        || text.contains(['\\', '\0'])
        || text
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.contains(':'))
    {
        return Err("buffer path must be normalized and relative".into());
    }
    Ok(())
}
fn inventory_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<PathBuf, u64>,
) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        let path = entry.path();
        let relative = crate::runtime::platform_fs::relative_path(
            path.strip_prefix(root).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        safe_path(Path::new(&relative))?;
        if platform_fs::is_real_directory(&metadata) {
            inventory_files(root, &path, files)?;
        } else if platform_fs::is_regular_file(&metadata) {
            files.insert(relative.into(), metadata.len());
        } else {
            return Err("buffer entries must be regular files or directories".into());
        }
    }
    Ok(())
}
