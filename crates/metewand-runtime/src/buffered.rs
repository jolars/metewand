//! Bounded local collection and validation for instrumented profiles.

use crate::worker_sdk::{WorkerError, WorkerResult};
use metewand_core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    profile::*,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub check_index: u64,
    #[serde(default)]
    pub work: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Continue,
    Stop(StopReason),
}

/// The collector owns frozen scientific data, never a live solver reference.
pub struct BufferedTrace {
    pub profile: SelectedBufferedProfile,
    pub directory: PathBuf,
    points: Vec<TracePoint>,
    snapshots: Vec<(BufferedSnapshot, Vec<u8>)>,
    trace_bytes: u64,
    snapshot_bytes: u64,
    reached: Vec<f64>,
    stopped: Option<StopReason>,
    finished: bool,
    clock: Box<dyn Fn() -> u64>,
    total: u64,
    adopted: Option<String>,
}

fn invalid(message: impl Into<String>) -> WorkerError {
    WorkerError::operation_failed(message)
}
fn limit(message: impl Into<String>) -> WorkerError {
    metewand_protocol::WorkerFailure::new(
        metewand_protocol::WorkerFailureCode::ResourceLimit,
        message,
        None,
    )
    .expect("nonempty resource failure")
    .into()
}
fn bytes(value: Value) -> WorkerResult<Vec<u8>> {
    Ok(CanonicalValue::try_from(value)?.to_canonical_bytes()?)
}

impl BufferedTrace {
    pub fn new(profile: SelectedBufferedProfile, directory: PathBuf) -> WorkerResult<Self> {
        let mut trace = Self::with_clock(profile, directory, || 0)?;
        let epoch = Instant::now();
        trace.clock =
            Box::new(move || u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX));
        Ok(trace)
    }

    pub fn with_clock(
        profile: SelectedBufferedProfile,
        directory: PathBuf,
        clock: impl Fn() -> u64 + 'static,
    ) -> WorkerResult<Self> {
        profile.policy.validate().map_err(invalid)?;
        if profile.check_schedule.every == 0 {
            return Err(invalid("check interval must be positive"));
        }
        Ok(Self {
            profile,
            directory,
            points: vec![],
            snapshots: vec![],
            trace_bytes: 0,
            snapshot_bytes: 0,
            reached: vec![],
            stopped: None,
            finished: false,
            clock: Box::new(clock),
            total: 0,
            adopted: None,
        })
    }

    pub fn observe(
        &mut self,
        progress: Progress,
        measure: impl FnOnce() -> WorkerResult<Value>,
        snapshot: impl FnOnce() -> WorkerResult<Value>,
    ) -> WorkerResult<Decision> {
        self.check(progress, measure, snapshot, false)
    }

    pub fn record(
        &mut self,
        progress: Progress,
        metrics: Value,
        snapshot: impl FnOnce() -> WorkerResult<Value>,
    ) -> WorkerResult<Decision> {
        self.check(progress, || Ok(metrics), snapshot, false)
    }

    fn check(
        &mut self,
        progress: Progress,
        measure: impl FnOnce() -> WorkerResult<Value>,
        snapshot: impl FnOnce() -> WorkerResult<Value>,
        terminal: bool,
    ) -> WorkerResult<Decision> {
        if self.finished || self.stopped.is_some() || self.adopted.is_some() {
            return Err(invalid(
                "recording after stop or finish violates the buffered contract",
            ));
        }
        let expected = self
            .points
            .last()
            .map_or(Some(self.profile.check_schedule.start), |p| {
                p.check_index.checked_add(self.profile.check_schedule.every)
            });
        if progress.check_index > MAX_SAFE_INTEGER
            || progress.work.values().any(|v| *v > MAX_SAFE_INTEGER)
        {
            return Err(invalid("progress must contain safe integers"));
        }
        if self.points.last().is_some_and(|p| {
            progress.check_index <= p.check_index
                || p.work
                    .iter()
                    .any(|(k, v)| progress.work.get(k).is_none_or(|n| n < v))
        }) {
            return Err(invalid(
                "progress and work counts must increase monotonically",
            ));
        }
        if expected.is_some_and(|n| progress.check_index > n) {
            return Err(invalid("a scheduled check was skipped"));
        }
        let expired = (self.clock)() >= self.profile.policy.active_time_ns().map_err(invalid)?;
        if !terminal && !expired && expected != Some(progress.check_index) {
            return Ok(Decision::Continue);
        }
        let metrics = measure()?;
        if !metrics.is_object() {
            return Err(invalid("provisional metrics must be an object"));
        }
        CanonicalValue::try_from(metrics.clone())?;
        let elapsed = (self.clock)();
        if elapsed > MAX_SAFE_INTEGER
            || self
                .points
                .last()
                .is_some_and(|p| elapsed < p.instrumented_time_ns)
        {
            return Err(invalid(
                "instrumented clock must be monotonic safe nanoseconds",
            ));
        }
        let target_value = self
            .profile
            .policy
            .metric
            .as_ref()
            .map(|m| {
                metrics
                    .get(m)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| invalid("provisional target metric must be finite and numeric"))
            })
            .transpose()?;
        let crossed: Vec<_> = self
            .profile
            .policy
            .thresholds()
            .into_iter()
            .filter(|t| {
                !self.reached.contains(t)
                    && target_value.is_some_and(|v| self.profile.policy.reaches(v, *t))
            })
            .collect();
        let mut reason = if target_value
            .zip(self.profile.policy.threshold)
            .is_some_and(|(v, t)| self.profile.policy.reaches(v, t))
        {
            Some(StopReason::TargetReached)
        } else if terminal {
            Some(StopReason::ImplementationStopped)
        } else if elapsed >= self.profile.policy.active_time_ns().map_err(invalid)? {
            Some(StopReason::ActiveTimeLimit)
        } else if self.points.len() as u64 + 1 >= self.profile.policy.max_trace_points {
            Some(StopReason::TraceLimit)
        } else {
            None
        };
        let capture =
            self.profile.policy.metric.is_none() || !crossed.is_empty() || reason.is_some();
        if capture
            && self.snapshots.len() as u64 + 1 >= self.profile.policy.max_observations
            && reason.is_none()
        {
            reason = Some(if self.profile.policy.metric.is_none() {
                StopReason::FixedCount
            } else {
                StopReason::ObservationLimit
            });
        }
        let mut point = TracePoint {
            check_index: progress.check_index,
            work: progress.work,
            provisional_metrics: metrics,
            instrumented_time_ns: elapsed,
            terminal: reason.is_some(),
        };
        let mut encoded = bytes(serde_json::to_value(&point)?)?;
        // Account for the complete manifest, including its file inventory, before
        // allowing regular collection to consume the terminal allowance.
        let regular_limit =
            self.profile.policy.max_buffer_bytes - self.profile.policy.terminal_reserve_bytes;
        if reason.is_none() && self.estimated_bytes(encoded.len() as u64 + 1, None)? > regular_limit
        {
            reason = Some(StopReason::ByteLimit);
            point.terminal = true;
            encoded = bytes(serde_json::to_value(&point)?)?;
        }
        let mut captured = if capture || reason.is_some() {
            Some(bytes(
                json!({"schema_version":1,"data":snapshot()?,"files":[]}),
            )?)
        } else {
            None
        };
        if captured.as_ref().is_some_and(|data| {
            !CanonicalValue::from_slice(data)
                .expect("encoded result")
                .as_json()["data"]
                .is_object()
        }) {
            return Err(invalid("snapshot data must be an object"));
        }
        if reason.is_none()
            && self.estimated_bytes(encoded.len() as u64 + 1, captured.as_deref())? > regular_limit
        {
            reason = Some(StopReason::ByteLimit);
            point.terminal = true;
            encoded = bytes(serde_json::to_value(&point)?)?;
        }
        if self.estimated_bytes(encoded.len() as u64 + 1, captured.as_deref())?
            > self.profile.policy.max_buffer_bytes
        {
            return Err(limit("terminal check and snapshot exceed max-buffer-bytes"));
        }
        self.trace_bytes += encoded.len() as u64 + 1;
        self.points.push(point);
        if let Some(data) = captured.take() {
            if self.snapshots.len() as u64 >= self.profile.policy.max_observations {
                return Err(limit("terminal snapshot exceeds max-observations"));
            }
            let ordinal = self.snapshots.len() as u64;
            self.snapshot_bytes += data.len() as u64;
            self.snapshots.push((
                BufferedSnapshot {
                    capture_ordinal: ordinal,
                    trace_index: self.points.len() as u64 - 1,
                    thresholds: crossed.clone(),
                    manifest: format!("snapshots/{ordinal}/result.json").into(),
                },
                data,
            ));
        }
        self.reached.extend(crossed);
        self.stopped = reason.clone();
        Ok(reason.map_or(Decision::Continue, Decision::Stop))
    }

    fn manifest(&self, total: u64, reason: StopReason, files: Vec<BufferFile>) -> BufferedManifest {
        BufferedManifest {
            schema_version: 1,
            complete: true,
            clock: "monotonic_ns".into(),
            instrumented_scope: self.profile.instrumented_scope.clone(),
            instrumented_total_time_ns: total,
            stop_reason: reason,
            trace: "trace.jsonl".into(),
            snapshots: self.snapshots.iter().map(|(s, _)| s.clone()).collect(),
            files,
        }
    }

    fn estimated_bytes(
        &self,
        extra_trace: u64,
        extra_snapshot: Option<&[u8]>,
    ) -> WorkerResult<u64> {
        let mut files = vec![BufferFile {
            path: "trace.jsonl".into(),
            sha256: "0".repeat(64),
            size_bytes: self.trace_bytes + extra_trace,
        }];
        files.extend(self.snapshots.iter().map(|(s, b)| BufferFile {
            path: s.manifest.clone(),
            sha256: "0".repeat(64),
            size_bytes: b.len() as u64,
        }));
        let mut manifest =
            self.manifest(MAX_SAFE_INTEGER, StopReason::ImplementationStopped, files);
        if let Some(data) = extra_snapshot {
            let ordinal = self.snapshots.len() as u64;
            let path: PathBuf = format!("snapshots/{ordinal}/result.json").into();
            manifest.files.push(BufferFile {
                path: path.clone(),
                sha256: "0".repeat(64),
                size_bytes: data.len() as u64,
            });
            manifest.snapshots.push(BufferedSnapshot {
                capture_ordinal: ordinal,
                trace_index: self.points.len() as u64,
                thresholds: self.profile.policy.thresholds(),
                manifest: path,
            });
        }
        Ok(self.trace_bytes
            + extra_trace
            + self.snapshot_bytes
            + extra_snapshot.map_or(0, |b| b.len() as u64)
            + bytes(serde_json::to_value(manifest)?)?.len() as u64)
    }

    pub fn finish(
        &mut self,
        progress: Progress,
        measure: impl FnOnce() -> WorkerResult<Value>,
        snapshot: impl FnOnce() -> WorkerResult<Value>,
    ) -> WorkerResult<()> {
        if self.finished || self.adopted.is_some() {
            return Err(invalid("buffer was already finished"));
        }
        if self
            .points
            .last()
            .is_some_and(|p| p.check_index == progress.check_index)
        {
            if self.points.last().is_some_and(|p| p.work != progress.work) {
                return Err(invalid("terminal work changed without advancing progress"));
            }
            if self
                .snapshots
                .last()
                .is_none_or(|(s, _)| s.trace_index != self.points.len() as u64 - 1)
            {
                let data = bytes(json!({"schema_version":1,"data":snapshot()?,"files":[]}))?;
                if self.estimated_bytes(0, Some(&data))? > self.profile.policy.max_buffer_bytes {
                    return Err(limit("terminal snapshot does not fit"));
                }
                let ordinal = self.snapshots.len() as u64;
                if ordinal >= self.profile.policy.max_observations {
                    return Err(limit("terminal snapshot count exceeded"));
                }
                self.snapshot_bytes += data.len() as u64;
                self.snapshots.push((
                    BufferedSnapshot {
                        capture_ordinal: ordinal,
                        trace_index: self.points.len() as u64 - 1,
                        thresholds: vec![],
                        manifest: format!("snapshots/{ordinal}/result.json").into(),
                    },
                    data,
                ));
            }
            self.points.last_mut().expect("checked point").terminal = true;
        } else {
            self.check(progress, measure, snapshot, true)?;
        }
        self.stopped
            .get_or_insert(StopReason::ImplementationStopped);
        self.total = (self.clock)();
        if self.total > MAX_SAFE_INTEGER
            || self.total
                < self
                    .points
                    .last()
                    .expect("terminal check")
                    .instrumented_time_ns
        {
            return Err(invalid("invalid total instrumented duration"));
        }
        self.finished = true;
        Ok(())
    }

    /// Native code may produce the same contract without any host callbacks.
    pub fn adopt_buffer(&mut self, manifest: &str) -> WorkerResult<()> {
        if !self.points.is_empty() || self.finished {
            return Err(invalid(
                "cannot combine a native buffer with another trajectory",
            ));
        }
        validate_buffer(&self.directory, Path::new(manifest), &self.profile).map_err(invalid)?;
        self.adopted = Some(manifest.into());
        self.finished = true;
        Ok(())
    }

    pub fn write(&self) -> WorkerResult<String> {
        if !self.finished {
            return Err(invalid(
                "execute_buffered must finish the terminal check before returning",
            ));
        }
        if let Some(manifest) = &self.adopted {
            return Ok(manifest.clone());
        }
        fs::create_dir_all(&self.directory)?;
        let mut trace = Vec::new();
        for point in &self.points {
            trace.extend(bytes(serde_json::to_value(point)?)?);
            trace.push(b'\n');
        }
        fs::write(self.directory.join("trace.jsonl"), &trace)?;
        let mut files = vec![file("trace.jsonl".into(), &trace)];
        for (snapshot, data) in &self.snapshots {
            let path = self.directory.join(&snapshot.manifest);
            fs::create_dir_all(path.parent().expect("snapshot parent"))?;
            fs::write(path, data)?;
            files.push(file(snapshot.manifest.clone(), data));
        }
        let manifest = self.manifest(
            self.total,
            self.stopped.clone().expect("finished stop reason"),
            files,
        );
        let data = bytes(serde_json::to_value(manifest)?)?;
        if trace.len() as u64 + self.snapshot_bytes + data.len() as u64
            > self.profile.policy.max_buffer_bytes
        {
            return Err(limit("completed buffer exceeds byte bound"));
        }
        fs::write(self.directory.join("buffer.json"), data)?;
        Ok("buffer.json".into())
    }
}

fn file(path: PathBuf, data: &[u8]) -> BufferFile {
    BufferFile {
        path,
        sha256: Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        size_bytes: data.len() as u64,
    }
}

/// Validates a completed buffer before any snapshot can become an observation.
pub fn validate_buffer(
    root: &Path,
    path: &Path,
    profile: &SelectedBufferedProfile,
) -> Result<(BufferedManifest, Vec<TracePoint>), String> {
    let fail = |e: std::io::Error| e.to_string();
    profile.policy.validate()?;
    let mut actual = BTreeMap::new();
    inventory_files(root, root, &mut actual)?;
    let total = actual
        .values()
        .try_fold(0u64, |sum, value| sum.checked_add(*value))
        .ok_or("buffer size overflow")?;
    if total > profile.policy.max_buffer_bytes {
        return Err("buffer exceeds max-buffer-bytes".into());
    }
    let inventory = crate::local_tree::hash_local_tree(root).map_err(|e| e.to_string())?;
    safe_path(path)?;
    let data = fs::read(root.join(path)).map_err(fail)?;
    let canonical = CanonicalValue::from_slice(&data).map_err(|e| e.to_string())?;
    let manifest: BufferedManifest =
        serde_json::from_value(canonical.as_json().clone()).map_err(|e| e.to_string())?;
    if manifest.schema_version != 1
        || !manifest.complete
        || manifest.clock != "monotonic_ns"
        || manifest.instrumented_scope != profile.instrumented_scope
        || manifest.instrumented_total_time_ns > MAX_SAFE_INTEGER
    {
        return Err("invalid buffer version, completion, clock, scope, or total duration".into());
    }
    actual.remove(path);
    let mut declared = BTreeSet::new();
    for entry in &manifest.files {
        safe_path(&entry.path)?;
        if !declared.insert(entry.path.clone())
            || actual.get(&entry.path) != Some(&entry.size_bytes)
        {
            return Err("duplicate, missing, or incorrectly sized buffer file".into());
        }
        let data = fs::read(root.join(&entry.path)).map_err(fail)?;
        if file(entry.path.clone(), &data).sha256 != entry.sha256 {
            return Err("buffer file hash mismatch".into());
        }
    }
    if declared != actual.keys().cloned().collect() {
        return Err("buffer contains undeclared files".into());
    }
    if !declared.contains(&manifest.trace) {
        return Err("trace is not declared".into());
    }
    let trace = fs::read(root.join(&manifest.trace)).map_err(fail)?;
    if !trace.ends_with(b"\n") {
        return Err("trace must end with a complete JSONL row".into());
    }
    let mut points: Vec<TracePoint> = vec![];
    for line in trace.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let value = CanonicalValue::from_slice(line).map_err(|e| e.to_string())?;
        points.push(serde_json::from_value(value.as_json().clone()).map_err(|e| e.to_string())?);
        if points.len() as u64 > profile.policy.max_trace_points {
            return Err("trace point bound exceeded".into());
        }
    }
    if points.is_empty()
        || !points.last().expect("nonempty").terminal
        || points[..points.len() - 1].iter().any(|p| p.terminal)
    {
        return Err("buffer requires exactly one terminal check at its end".into());
    }
    let mut expected = profile.check_schedule.start;
    for (i, p) in points.iter().enumerate() {
        if p.check_index > MAX_SAFE_INTEGER
            || p.instrumented_time_ns > manifest.instrumented_total_time_ns
            || !p.provisional_metrics.is_object()
            || (!p.terminal && p.check_index != expected)
            || (p.terminal && p.check_index > expected)
            || p.work.values().any(|v| *v > MAX_SAFE_INTEGER)
        {
            return Err("invalid trace clock, progress, metrics, or check schedule".into());
        }
        if let Some(previous) = i.checked_sub(1).map(|j| &points[j])
            && (p.check_index <= previous.check_index
                || p.instrumented_time_ns < previous.instrumented_time_ns
                || previous
                    .work
                    .iter()
                    .any(|(k, v)| p.work.get(k).is_none_or(|n| n < v)))
        {
            return Err("trace progress, times, and work counts must be monotonic".into());
        }
        if let Some(metric) = &profile.policy.metric
            && p.provisional_metrics
                .get(metric)
                .and_then(Value::as_f64)
                .is_none_or(|v| !v.is_finite())
        {
            return Err("trace target metric must be finite and numeric".into());
        }
        if !p.terminal
            && (p.instrumented_time_ns >= profile.policy.active_time_ns()?
                || profile
                    .policy
                    .metric
                    .as_ref()
                    .zip(profile.policy.threshold)
                    .is_some_and(|(m, t)| {
                        p.provisional_metrics[m]
                            .as_f64()
                            .is_some_and(|v| profile.policy.reaches(v, t))
                    }))
        {
            return Err("execution continued after a required local stop".into());
        }
        expected = p
            .check_index
            .checked_add(profile.check_schedule.every)
            .ok_or("check schedule overflow")?;
    }
    if manifest.snapshots.is_empty()
        || manifest.snapshots.len() as u64 > profile.policy.max_observations
    {
        return Err("snapshot count is empty or exceeds bound".into());
    }
    if profile.policy.metric.is_none() && manifest.snapshots.len() != points.len() {
        return Err("fixed-count profiles must save every checked iterate".into());
    }
    let mut seen = BTreeSet::new();
    let mut thresholds = vec![];
    for (i, snapshot) in manifest.snapshots.iter().enumerate() {
        let index = usize::try_from(snapshot.trace_index).map_err(|_| "invalid trace reference")?;
        if snapshot.capture_ordinal != i as u64
            || index >= points.len()
            || !seen.insert(index)
            || !declared.contains(&snapshot.manifest)
            || (i > 0 && snapshot.trace_index <= manifest.snapshots[i - 1].trace_index)
        {
            return Err("invalid snapshot ordinal, order, or trace reference".into());
        }
        for threshold in &snapshot.thresholds {
            let metric = profile
                .policy
                .metric
                .as_ref()
                .and_then(|m| points[index].provisional_metrics[m].as_f64());
            if !profile.policy.thresholds().contains(threshold)
                || thresholds.contains(threshold)
                || metric.is_none_or(|v| !profile.policy.reaches(v, *threshold))
                || points[..index].iter().any(|p| {
                    profile
                        .policy
                        .metric
                        .as_ref()
                        .and_then(|m| p.provisional_metrics[m].as_f64())
                        .is_some_and(|v| profile.policy.reaches(v, *threshold))
                })
            {
                return Err("invalid first-observed threshold claim".into());
            }
            thresholds.push(*threshold);
        }
    }
    if manifest.snapshots.last().expect("nonempty").trace_index != points.len() as u64 - 1 {
        return Err("terminal iterate requires a snapshot".into());
    }
    for threshold in profile.policy.thresholds() {
        if points.iter().any(|p| {
            profile
                .policy
                .metric
                .as_ref()
                .and_then(|m| p.provisional_metrics[m].as_f64())
                .is_some_and(|v| profile.policy.reaches(v, threshold))
        }) && !thresholds.contains(&threshold)
        {
            return Err("first threshold crossing was not saved".into());
        }
    }
    let last = points.last().expect("terminal check");
    match manifest.stop_reason {
        StopReason::TargetReached
            if profile
                .policy
                .threshold
                .zip(
                    profile
                        .policy
                        .metric
                        .as_ref()
                        .and_then(|m| last.provisional_metrics[m].as_f64()),
                )
                .is_none_or(|(t, v)| !profile.policy.reaches(v, t)) =>
        {
            return Err("unsubstantiated local target stop".into());
        }
        StopReason::FixedCount
            if profile.policy.metric.is_some()
                || manifest.snapshots.len() as u64 != profile.policy.max_observations =>
        {
            return Err("unsubstantiated fixed-count stop".into());
        }
        StopReason::TraceLimit if points.len() as u64 != profile.policy.max_trace_points => {
            return Err("unsubstantiated trace-limit stop".into());
        }
        StopReason::ObservationLimit
            if manifest.snapshots.len() as u64 != profile.policy.max_observations =>
        {
            return Err("unsubstantiated observation-limit stop".into());
        }
        StopReason::ActiveTimeLimit
            if last.instrumented_time_ns < profile.policy.active_time_ns()? =>
        {
            return Err("unsubstantiated active-time stop".into());
        }
        _ => {}
    }
    let mut prefix_trace_bytes = 0u64;
    for (index, line) in trace
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .enumerate()
        .take(points.len() - 1)
    {
        prefix_trace_bytes += line.len() as u64 + 1;
        let mut prefix = manifest.clone();
        prefix.snapshots.retain(|s| s.trace_index <= index as u64);
        let parents = prefix
            .snapshots
            .iter()
            .map(|s| s.manifest.parent().expect("snapshot parent").to_owned())
            .collect::<Vec<_>>();
        prefix
            .files
            .retain(|f| f.path == manifest.trace || parents.iter().any(|p| f.path.starts_with(p)));
        for file in &mut prefix.files {
            if file.path == manifest.trace {
                file.size_bytes = prefix_trace_bytes;
            }
        }
        prefix.instrumented_total_time_ns = MAX_SAFE_INTEGER;
        prefix.stop_reason = StopReason::ImplementationStopped;
        let size = prefix.files.iter().map(|f| f.size_bytes).sum::<u64>()
            + CanonicalValue::try_from(serde_json::to_value(prefix).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
                .to_canonical_bytes()
                .map_err(|e| e.to_string())?
                .len() as u64;
        if size > profile.policy.max_buffer_bytes - profile.policy.terminal_reserve_bytes {
            return Err("regular collection consumed the terminal byte reserve".into());
        }
    }
    if crate::local_tree::hash_local_tree(root).map_err(|e| e.to_string())? != inventory {
        return Err("buffer changed during validation".into());
    }
    Ok((manifest, points))
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
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
        safe_path(relative)?;
        if metadata.is_dir() {
            inventory_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.insert(relative.to_owned(), metadata.len());
        } else {
            return Err("buffer entries must be regular files or directories".into());
        }
    }
    Ok(())
}
