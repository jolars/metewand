use super::*;
use crate::protocol::{
    ExecuteStreamRequest, ExecuteStreamResponse, StreamAction, StreamCheckpoint, StreamDecision,
};

pub enum StreamMessage {
    Checkpoint(StreamCheckpoint),
    Finished(ExecuteStreamResponse),
}

#[derive(Debug)]
pub(super) struct StreamState {
    root: std::path::PathBuf,
    bound: u64,
    bytes: u64,
    count: u64,
    awaiting: bool,
    stopped: bool,
}

impl WorkerSession {
    pub fn start_stream(
        &mut self,
        result_root: String,
        profile: Value,
    ) -> Result<StreamMessage, WorkerSessionError> {
        self.require_role(WorkerPhase::Execute, WorkerRole::Implementation)?;
        if !self
            .negotiated()
            .selected_capabilities()
            .contains(&Capability::StreamingProfile)
        {
            return Err(WorkerSessionError::CapabilityNotSelected {
                phase: WorkerPhase::Execute,
                capability: Capability::StreamingProfile,
            });
        }
        let id = self.begin_request(WorkerPhase::Execute)?;
        self.stream = Some(StreamState {
            root: result_root.clone().into(),
            bound: profile["policy"]["max-observations"].as_u64().unwrap_or(0),
            bytes: profile["policy"]["max-checkpoint-bytes"]
                .as_u64()
                .unwrap_or(0),
            count: 0,
            awaiting: false,
            stopped: false,
        });
        let request = ExecuteStreamRequest::new(id, result_root, profile);
        let encoded = encode_message(&request).map_err(|source| WorkerSessionError::Encode {
            phase: WorkerPhase::Execute,
            source,
        })?;
        self.stream_frame(encoded, false)
    }

    pub fn decide_stream(
        &mut self,
        action: StreamAction,
        reason: Option<String>,
    ) -> Result<StreamMessage, WorkerSessionError> {
        let state = self
            .stream
            .as_mut()
            .ok_or_else(|| WorkerSessionError::StreamProtocol {
                message: "no stream is active".into(),
            })?;
        if !state.awaiting || state.stopped {
            return self.stream_error("no checkpoint is awaiting a decision");
        }
        let id = self
            .requests
            .pending_request()
            .expect("active stream request")
            .clone();
        let decision = StreamDecision::new(id, state.count - 1, action, reason);
        state.awaiting = false;
        state.stopped = action == StreamAction::Stop;
        let encoded = encode_message(&decision).map_err(|source| WorkerSessionError::Encode {
            phase: WorkerPhase::Execute,
            source,
        })?;
        self.stream_frame(encoded, action == StreamAction::Stop)
    }

    fn stream_error<T>(&mut self, message: impl Into<String>) -> Result<T, WorkerSessionError> {
        self.discard();
        Err(WorkerSessionError::StreamProtocol {
            message: self.redactor.text(&message.into()),
        })
    }

    fn stream_frame(
        &mut self,
        encoded: Vec<u8>,
        stopping: bool,
    ) -> Result<StreamMessage, WorkerSessionError> {
        self.ensure_usable()?;
        self.verify_controls()?;
        let phase = WorkerPhase::Execute;
        let timeout = if stopping {
            self.timeouts.shutdown
        } else {
            self.timeouts.execute
        };
        let started = Instant::now();
        let deadline = if stopping {
            None
        } else {
            self.scientific_deadline
        };
        let remaining = deadline.map(|d| d.saturating_duration_since(started));
        let scientific = remaining.is_some_and(|d| d <= timeout);
        let limit = remaining.map_or(timeout, |d| d.min(timeout));
        let timeout_error = || {
            if scientific {
                WorkerSessionError::ScientificTimeout { phase }
            } else {
                WorkerSessionError::Timeout {
                    phase,
                    limit: timeout,
                }
            }
        };
        let (sender, receiver) = mpsc::channel();
        if limit.is_zero() {
            self.discard_after_timeout();
            return Err(timeout_error());
        }
        if self
            .protocol_sender
            .as_ref()
            .expect("usable session")
            .send(ProtocolCommand {
                encoded,
                reply: sender,
            })
            .is_err()
        {
            self.discard();
            return Err(WorkerSessionError::ProtocolThreadStopped { phase });
        }
        let mut value = match receiver.recv_timeout(limit) {
            Ok(Ok(Some(v))) => v,
            Ok(Ok(None)) => {
                self.discard();
                return Err(WorkerSessionError::ProtocolClosed { phase });
            }
            Ok(Err(ProtocolIoError::Write(source))) => {
                self.discard();
                return Err(WorkerSessionError::ProtocolWrite { phase, source });
            }
            Ok(Err(ProtocolIoError::Read(source))) => {
                self.discard();
                return Err(if self.redactor.is_empty() {
                    WorkerSessionError::ProtocolRead { phase, source }
                } else {
                    WorkerSessionError::RedactedProtocolRead { phase }
                });
            }
            Err(RecvTimeoutError::Timeout) => {
                self.discard_after_timeout();
                return Err(timeout_error());
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.discard();
                return Err(WorkerSessionError::ProtocolThreadStopped { phase });
            }
        };
        self.redactor.value(&mut value);
        if value.get("event").is_none() {
            let response =
                decode_operation_response::<ExecuteStreamResponse>(value, &mut self.requests);
            let count = self.stream.as_ref().expect("active stream").count;
            return match response {
                Ok(OperationResponse::Success(response)) if response.checkpoint_count == count => {
                    self.verify_controls()?;
                    self.stream = None;
                    Ok(StreamMessage::Finished(response))
                }
                Ok(OperationResponse::Success(_)) => self
                    .stream_error("terminal checkpoint count does not match received checkpoints"),
                Ok(OperationResponse::Failure(failure)) => {
                    self.discard();
                    Err(WorkerSessionError::WorkerFailure { phase, failure })
                }
                Err(source) => {
                    self.discard();
                    Err(WorkerSessionError::Response { phase, source })
                }
            };
        }
        let checkpoint: StreamCheckpoint = match crate::protocol::decode_message(value.clone()) {
            Ok(checkpoint) => checkpoint,
            Err(error) => return self.stream_error(error.to_string()),
        };
        let state = self.stream.as_ref().expect("active stream");
        if state.stopped
            || checkpoint.id != *self.requests.pending_request().expect("pending stream")
            || checkpoint.sequence != state.count
            || state.count >= state.bound
            || checkpoint
                .work
                .values()
                .any(|v| *v > crate::core::canonical::MAX_SAFE_INTEGER)
            || checkpoint.manifest.is_some() == checkpoint.draft.is_some()
        {
            return self.stream_error("checkpoint ID, sequence, count, or state is invalid");
        }
        let root = state.root.clone();
        let bytes = state.bytes;
        if let Some(draft) = &checkpoint.draft {
            if !self
                .negotiated()
                .selected_capabilities()
                .contains(&Capability::ManagedOutputs)
            {
                return self.stream_error("checkpoint draft requires managed_outputs");
            }
            // Finalization removes the draft's kind field before adding artifact metadata.
            let draft_overhead = br#""kind":"result","#.len() as u64;
            let (directory, name) = match crate::runtime::streaming::snapshot_directory(
                &root,
                draft,
                bytes.saturating_add(draft_overhead),
            ) {
                Ok(snapshot) => snapshot,
                Err(error) => return self.stream_error(error.to_string()),
            };
            let mut managed = serde_json::json!({"id": checkpoint.id, "ok":true, "draft": name});
            let request =
                serde_json::json!({"id":checkpoint.id,"method":"execute","result_dir":directory});
            if let Err(error) =
                crate::runtime::managed_output::finalize(&request, &mut managed, started.checked_add(limit))
            {
                if started.elapsed() >= limit {
                    self.discard_after_timeout();
                    return Err(timeout_error());
                }
                return self.stream_error(error.to_string());
            }
            let parent = std::path::Path::new(draft)
                .parent()
                .expect("validated snapshot");
            value.as_object_mut().expect("checkpoint").remove("draft");
            value["manifest"] = Value::String(
                crate::runtime::platform_fs::relative_path(&parent.join("result.json")).map_err(|e| {
                    WorkerSessionError::StreamProtocol {
                        message: e.to_string(),
                    }
                })?,
            );
        }
        let checkpoint: StreamCheckpoint =
            serde_json::from_value(value).expect("finalized checkpoint");
        // Filesystem validation belongs to the runner's paused checkpoint phase.
        if let Err(error) = crate::runtime::worker_output::validate_relative_path(
            checkpoint.manifest.as_deref().expect("manifest"),
        ) {
            return self.stream_error(error.to_string());
        }
        if started.elapsed() >= limit {
            self.discard_after_timeout();
            return Err(timeout_error());
        }
        let state = self.stream.as_mut().expect("active stream");
        state.count += 1;
        state.awaiting = true;
        Ok(StreamMessage::Checkpoint(checkpoint))
    }
}
