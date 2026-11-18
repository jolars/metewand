//! Bounded local collection and validation for instrumented profiles.

use crate::core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    materialization::hex,
    profile::*,
    records::ContentDigest,
};
use crate::runtime::worker_sdk::{WorkerError, WorkerResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

mod validation;
pub(crate) use validation::validate_buffer;

const TRACE_PATH: &str = "trace.jsonl";
const PLACEHOLDER_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

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
    size: BufferSize,
    thresholds: Vec<f64>,
    all_threshold_bytes: u64,
    reached: usize,
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
    crate::protocol::WorkerFailure::new(
        crate::protocol::WorkerFailureCode::ResourceLimit,
        message,
        None,
    )
    .expect("nonempty resource failure")
    .into()
}
fn bytes(value: Value) -> WorkerResult<Vec<u8>> {
    Ok(CanonicalValue::try_from(value)?.to_canonical_bytes()?)
}

fn encoded_len(value: impl Serialize) -> WorkerResult<u64> {
    Ok(bytes(serde_json::to_value(value)?)?.len() as u64)
}

fn manifest_header(
    scope: &str,
    trace: PathBuf,
    total: u64,
    reason: StopReason,
) -> BufferedManifest {
    BufferedManifest {
        schema_version: 1,
        complete: true,
        clock: "monotonic_ns".into(),
        instrumented_scope: scope.into(),
        instrumented_total_time_ns: total,
        stop_reason: reason,
        trace,
        snapshots: vec![],
        files: vec![],
    }
}

/// Size of the payload plus its canonical manifest with the reserved header.
#[derive(Clone, Copy)]
struct BufferSize {
    manifest_base: u64,
    snapshot_entries: u64,
    other_files: u64,
}

impl BufferSize {
    fn new(scope: &str, trace: BufferFile) -> WorkerResult<Self> {
        let mut manifest = manifest_header(
            scope,
            trace.path.clone(),
            MAX_SAFE_INTEGER,
            StopReason::ImplementationStopped,
        );
        manifest.files.push(BufferFile {
            size_bytes: 0,
            ..trace
        });
        Ok(Self {
            // Replace the one digit for `size_bytes: 0` with the current trace size.
            manifest_base: encoded_len(&manifest)? - 1,
            snapshot_entries: 0,
            other_files: 0,
        })
    }

    fn snapshot(&mut self, entry_bytes: u64) {
        self.snapshot_entries += entry_bytes + u64::from(self.snapshot_entries != 0);
    }

    fn file_charge(entry: &BufferFile) -> WorkerResult<u64> {
        // The files array already contains the trace, so each other entry adds a comma.
        Ok(entry.size_bytes + encoded_len(entry)? + 1)
    }

    fn total(self, trace_bytes: u64) -> u64 {
        let digits = u64::from(trace_bytes.checked_ilog10().unwrap_or(0)) + 1;
        self.manifest_base + self.snapshot_entries + self.other_files + digits + trace_bytes
    }
}

fn file_entry(path: PathBuf, size_bytes: u64, sha256: &str) -> BufferFile {
    BufferFile {
        path,
        size_bytes,
        sha256: sha256.into(),
    }
}

// The policy orders thresholds from least to most demanding. Once a threshold
// has been reached, only the uncrossed suffix can trigger another capture.
fn crossings<'a>(policy: &ProfilePolicy, remaining: &'a [f64], metric: Option<f64>) -> &'a [f64] {
    let count = remaining
        .iter()
        .take_while(|&&t| metric.is_some_and(|v| policy.reaches(v, t)))
        .count();
    &remaining[..count]
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
        profile.policy.validate_buffered().map_err(invalid)?;
        profile.check_schedule.validate().map_err(invalid)?;
        let size = BufferSize::new(
            &profile.instrumented_scope,
            file_entry(TRACE_PATH.into(), 0, PLACEHOLDER_DIGEST),
        )?;
        let thresholds = profile.policy.thresholds();
        let all_threshold_bytes = encoded_len(&thresholds)? - 2;
        Ok(Self {
            profile,
            directory,
            points: vec![],
            snapshots: vec![],
            trace_bytes: 0,
            size,
            thresholds,
            all_threshold_bytes,
            reached: 0,
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
            .profile
            .check_schedule
            .next_after(self.points.last().map(|p| p.check_index));
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
        let crossed = crossings(
            &self.profile.policy,
            &self.thresholds[self.reached..],
            target_value,
        );
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
        let mut trace_bytes = self.trace_bytes + encoded_len(&point)? + 1;
        // Account for the complete manifest, including its file inventory, before
        // allowing regular collection to consume the terminal allowance.
        let regular_limit =
            self.profile.policy.max_buffer_bytes - self.profile.policy.terminal_reserve_bytes;
        if reason.is_none() && self.size.total(trace_bytes) > regular_limit {
            reason = Some(StopReason::ByteLimit);
            point.terminal = true;
            trace_bytes -= 1; // `true` is one byte shorter than `false`.
        }
        let captured = (capture || reason.is_some())
            .then(|| snapshot().and_then(snapshot_bytes))
            .transpose()?;
        let estimate = match &captured {
            Some(data) => self.estimate_capture(data.len() as u64)?,
            None => self.size,
        };
        if reason.is_none() && estimate.total(trace_bytes) > regular_limit {
            reason = Some(StopReason::ByteLimit);
            point.terminal = true;
            trace_bytes -= 1;
        }
        if estimate.total(trace_bytes) > self.profile.policy.max_buffer_bytes {
            return Err(limit("terminal check and snapshot exceed max-buffer-bytes"));
        }
        let crossed_count = crossed.len();
        if let Some(data) = captured {
            self.capture(self.points.len() as u64, crossed.to_vec(), data)?;
        }
        self.trace_bytes = trace_bytes;
        self.points.push(point);
        self.reached += crossed_count;
        self.stopped = reason.clone();
        Ok(reason.map_or(Decision::Continue, Decision::Stop))
    }

    fn next_snapshot(&self, trace_index: u64, thresholds: Vec<f64>) -> BufferedSnapshot {
        let ordinal = self.snapshots.len() as u64;
        BufferedSnapshot {
            capture_ordinal: ordinal,
            trace_index,
            thresholds,
            manifest: format!("snapshots/{ordinal}/result.json").into(),
        }
    }

    fn estimate_capture(&self, payload_bytes: u64) -> WorkerResult<BufferSize> {
        let mut size = self.size;
        // Budget for every configured threshold on a prospective capture, as
        // before. The committed size then charges only the recorded thresholds.
        let snapshot = self.next_snapshot(self.points.len() as u64, vec![]);
        size.snapshot(encoded_len(&snapshot)? + self.all_threshold_bytes);
        size.other_files += BufferSize::file_charge(&file_entry(
            snapshot.manifest,
            payload_bytes,
            PLACEHOLDER_DIGEST,
        ))?;
        Ok(size)
    }

    fn capture(
        &mut self,
        trace_index: u64,
        thresholds: Vec<f64>,
        data: Vec<u8>,
    ) -> WorkerResult<()> {
        if self.snapshots.len() as u64 >= self.profile.policy.max_observations {
            return Err(limit("terminal snapshot exceeds max-observations"));
        }
        let snapshot = self.next_snapshot(trace_index, thresholds);
        let entry_bytes = encoded_len(&snapshot)?;
        self.size.other_files += BufferSize::file_charge(&file_entry(
            snapshot.manifest.clone(),
            data.len() as u64,
            PLACEHOLDER_DIGEST,
        ))?;
        self.size.snapshot(entry_bytes);
        self.snapshots.push((snapshot, data));
        Ok(())
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
                let data = snapshot_bytes(snapshot()?)?;
                if self
                    .estimate_capture(data.len() as u64)?
                    .total(self.trace_bytes)
                    > self.profile.policy.max_buffer_bytes
                {
                    return Err(limit("terminal snapshot does not fit"));
                }
                self.capture(self.points.len() as u64 - 1, vec![], data)?;
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
        fs::write(self.directory.join(TRACE_PATH), &trace)?;
        let mut files = vec![file(TRACE_PATH.into(), &trace)];
        for (snapshot, data) in &self.snapshots {
            let path = self.directory.join(&snapshot.manifest);
            fs::create_dir_all(path.parent().expect("snapshot parent"))?;
            fs::write(path, data)?;
            files.push(file(snapshot.manifest.clone(), data));
        }
        let payload_bytes: u64 = files.iter().map(|file| file.size_bytes).sum();
        let mut manifest = manifest_header(
            &self.profile.instrumented_scope,
            TRACE_PATH.into(),
            self.total,
            self.stopped.clone().expect("finished stop reason"),
        );
        manifest.snapshots = self.snapshots.iter().map(|(s, _)| s.clone()).collect();
        manifest.files = files;
        let data = bytes(serde_json::to_value(manifest)?)?;
        if payload_bytes + data.len() as u64 > self.profile.policy.max_buffer_bytes {
            return Err(limit("completed buffer exceeds byte bound"));
        }
        fs::write(self.directory.join("buffer.json"), data)?;
        Ok("buffer.json".into())
    }
}

fn file(path: PathBuf, data: &[u8]) -> BufferFile {
    BufferFile {
        path,
        sha256: hex(ContentDigest::new(Sha256::digest(data).into())),
        size_bytes: data.len() as u64,
    }
}

fn snapshot_bytes(data: Value) -> WorkerResult<Vec<u8>> {
    if !data.is_object() {
        return Err(invalid("snapshot data must be an object"));
    }
    bytes(json!({"schema_version":1,"data":data,"files":[]}))
}
