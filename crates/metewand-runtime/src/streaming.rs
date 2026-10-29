//! Cooperative snapshots and their blocking, independently evaluated decisions.

pub use crate::buffered::{Decision, Progress};

use crate::worker_sdk::{WorkerError, WorkerResult};
use metewand_core::{
    canonical::MAX_SAFE_INTEGER,
    profile::{SelectedStreamingProfile, StopReason},
};
use metewand_protocol::{RequestId, StreamAction, StreamCheckpoint, StreamDecision};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub struct StreamingContext<'a> {
    pub profile: SelectedStreamingProfile,
    pub directory: PathBuf,
    id: RequestId,
    count: u64,
    last: Option<u64>,
    stopped: Option<StopReason>,
    exchange: &'a mut dyn FnMut(&StreamCheckpoint) -> WorkerResult<StreamDecision>,
}

impl<'a> StreamingContext<'a> {
    pub(crate) fn new(
        profile: SelectedStreamingProfile,
        directory: PathBuf,
        id: RequestId,
        exchange: &'a mut dyn FnMut(&StreamCheckpoint) -> WorkerResult<StreamDecision>,
    ) -> WorkerResult<Self> {
        profile
            .policy
            .validate()
            .map_err(WorkerError::operation_failed)?;
        profile
            .check_schedule
            .validate()
            .map_err(WorkerError::operation_failed)?;
        if profile.policy.max_checkpoint_bytes == 0 {
            return Err(WorkerError::operation_failed(
                "streaming requires max-checkpoint-bytes",
            ));
        }
        Ok(Self {
            profile,
            directory,
            id,
            count: 0,
            last: None,
            stopped: None,
            exchange,
        })
    }

    pub fn checkpoint(
        &mut self,
        progress: Progress,
        snapshot: impl FnOnce() -> WorkerResult<Value>,
    ) -> WorkerResult<Decision> {
        self.capture(progress, false, |directory| {
            crate::worker_sdk::write_result(directory, snapshot()?)
        })
    }

    /// A custom writer must close every file before returning its relative manifest.
    pub fn checkpoint_with(
        &mut self,
        progress: Progress,
        writer: impl FnOnce(&Path) -> WorkerResult<String>,
    ) -> WorkerResult<Decision> {
        self.capture(progress, false, writer)
    }

    pub fn finish(
        &mut self,
        progress: Progress,
        snapshot: impl FnOnce() -> WorkerResult<Value>,
    ) -> WorkerResult<()> {
        if self.stopped.is_some() || self.last == Some(progress.check_index) {
            return Ok(());
        }
        self.capture(progress, true, |directory| {
            crate::worker_sdk::write_result(directory, snapshot()?)
        })?;
        Ok(())
    }

    fn capture(
        &mut self,
        progress: Progress,
        terminal: bool,
        writer: impl FnOnce(&Path) -> WorkerResult<String>,
    ) -> WorkerResult<Decision> {
        if self.stopped.is_some() {
            return Err(WorkerError::operation_failed("checkpoint after stop"));
        }
        if progress.check_index > MAX_SAFE_INTEGER
            || progress.work.values().any(|n| *n > MAX_SAFE_INTEGER)
            || self.last.is_some_and(|last| progress.check_index <= last)
        {
            return Err(WorkerError::operation_failed(
                "checkpoint progress must increase and use safe integers",
            ));
        }
        if !terminal && !self.profile.check_schedule.includes(progress.check_index) {
            return Ok(Decision::Continue);
        }
        if self.count >= self.profile.policy.max_observations {
            return Err(WorkerError::operation_failed(
                "checkpoint count exceeds policy",
            ));
        }
        let relative = PathBuf::from("checkpoints").join(self.count.to_string());
        let directory = self.directory.join(&relative);
        fs::create_dir_all(&directory)?;
        let manifest = writer(&directory)?;
        let manifest = crate::worker_output::validate_relative_path(&manifest)
            .map_err(|e| WorkerError::operation_failed(e.to_string()))?;
        let reference = crate::platform_fs::relative_path(&relative.join(manifest))?;
        snapshot_directory(
            &self.directory,
            &reference,
            self.profile.policy.max_checkpoint_bytes,
        )?;
        let checkpoint = StreamCheckpoint::new(
            self.id.clone(),
            self.count,
            progress.check_index,
            progress.work,
            terminal,
            reference,
        );
        let decision = (self.exchange)(&checkpoint)?;
        if decision.id != self.id
            || decision.sequence != self.count
            || (terminal && decision.action != StreamAction::Stop)
        {
            return Err(WorkerError::operation_failed(
                "decision does not match the pending checkpoint",
            ));
        }
        self.count += 1;
        self.last = Some(progress.check_index);
        match decision.action {
            StreamAction::Continue if decision.reason.is_none() => Ok(Decision::Continue),
            StreamAction::Stop => {
                let reason: StopReason =
                    serde_json::from_value(serde_json::json!(decision.reason))?;
                self.stopped = Some(reason.clone());
                Ok(Decision::Stop(reason))
            }
            _ => Err(WorkerError::operation_failed(
                "continue must omit a stopping reason",
            )),
        }
    }

    pub(crate) fn count(&self) -> u64 {
        self.count
    }
}

/// Check every ancestor and file before reading or canonicalizing an untrusted snapshot.
pub(crate) fn snapshot_directory(
    root: &Path,
    reference: &str,
    limit: u64,
) -> io::Result<(PathBuf, String)> {
    let path = crate::worker_output::validate_relative_path(reference).map_err(io::Error::other)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("checkpoint requires its own directory"))?;
    let mut directory = root.to_owned();
    if !crate::platform_fs::is_real_directory(&fs::symlink_metadata(&directory)?) {
        return Err(io::Error::other("unsafe checkpoint root"));
    }
    for component in parent.components() {
        directory.push(component);
        if !crate::platform_fs::is_real_directory(&fs::symlink_metadata(&directory)?) {
            return Err(io::Error::other("unsafe checkpoint ancestor"));
        }
    }
    fn visit(path: &Path, total: &mut u64, limit: u64) -> io::Result<()> {
        crate::platform_fs::validate_directory_names(path)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if crate::platform_fs::is_real_directory(&meta) {
                visit(&entry.path(), total, limit)?;
            } else if crate::platform_fs::is_regular_file(&meta) {
                *total = total
                    .checked_add(meta.len())
                    .filter(|n| *n <= limit)
                    .ok_or_else(|| io::Error::other("checkpoint exceeds max-checkpoint-bytes"))?;
            } else {
                return Err(io::Error::other("unsafe checkpoint entry"));
            }
        }
        Ok(())
    }
    visit(&directory, &mut 0, limit)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::other("invalid checkpoint filename"))?
        .to_owned();
    Ok((directory, name))
}
