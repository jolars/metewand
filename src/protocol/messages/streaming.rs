use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StreamMethod {
    ExecuteStream,
}

/// One execution remains pending until its terminal response.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExecuteStreamRequest {
    pub id: RequestId,
    method: StreamMethod,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    pub result_root: String,
    #[serde(deserialize_with = "deserialize_object")]
    pub profile: Value,
}
impl ExecuteStreamRequest {
    pub fn new(id: RequestId, result_root: String, profile: Value) -> Self {
        Self {
            id,
            method: StreamMethod::ExecuteStream,
            result_root,
            profile,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CheckpointEvent {
    Checkpoint,
}

/// An immutable scientific snapshot, followed by a blocking decision barrier.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, try_from = "UncheckedCheckpoint")]
pub struct StreamCheckpoint {
    pub id: RequestId,
    event: CheckpointEvent,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    pub sequence: u64,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    pub check_index: u64,
    pub work: BTreeMap<String, u64>,
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedCheckpoint {
    id: RequestId,
    event: CheckpointEvent,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    sequence: u64,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    check_index: u64,
    work: BTreeMap<String, u64>,
    terminal: bool,
    #[serde(default, deserialize_with = "optional_nonempty")]
    manifest: Option<String>,
    #[serde(default, deserialize_with = "optional_nonempty")]
    draft: Option<String>,
}

fn optional_nonempty<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    deserialize_nonempty_string(deserializer).map(Some)
}

impl TryFrom<UncheckedCheckpoint> for StreamCheckpoint {
    type Error = &'static str;
    fn try_from(raw: UncheckedCheckpoint) -> Result<Self, Self::Error> {
        if raw.manifest.is_some() == raw.draft.is_some()
            || raw.work.values().any(|v| *v > MAX_SAFE_INTEGER)
        {
            return Err(
                "checkpoint requires exactly one snapshot reference and safe work counters",
            );
        }
        Ok(Self {
            id: raw.id,
            event: raw.event,
            sequence: raw.sequence,
            check_index: raw.check_index,
            work: raw.work,
            terminal: raw.terminal,
            manifest: raw.manifest,
            draft: raw.draft,
        })
    }
}
impl StreamCheckpoint {
    pub fn new(
        id: RequestId,
        sequence: u64,
        check_index: u64,
        work: BTreeMap<String, u64>,
        terminal: bool,
        manifest: String,
    ) -> Self {
        Self {
            id,
            event: CheckpointEvent::Checkpoint,
            sequence,
            check_index,
            work,
            terminal,
            manifest: Some(manifest),
            draft: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamAction {
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DecisionMethod {
    StreamDecision,
}

/// A decision is sent only after its checkpoint has been durably committed.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StreamDecision {
    pub id: RequestId,
    method: DecisionMethod,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    pub sequence: u64,
    pub action: StreamAction,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_nonempty"
    )]
    pub reason: Option<String>,
}
impl StreamDecision {
    pub fn new(id: RequestId, sequence: u64, action: StreamAction, reason: Option<String>) -> Self {
        Self {
            id,
            method: DecisionMethod::StreamDecision,
            sequence,
            action,
            reason,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExecuteStreamResponse {
    pub id: RequestId,
    ok: Success,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    pub checkpoint_count: u64,
}
impl ExecuteStreamResponse {
    pub fn new(id: RequestId, checkpoint_count: u64) -> Self {
        Self {
            id,
            ok: Success,
            checkpoint_count,
        }
    }
}
impl WireResponse for ExecuteStreamResponse {
    fn id(&self) -> &RequestId {
        &self.id
    }
}

macro_rules! message {
    ($($ty:ty),*) => { $(impl private::Sealed for $ty {} impl WireMessage for $ty {})* };
}
message!(
    ExecuteStreamRequest,
    StreamCheckpoint,
    StreamDecision,
    ExecuteStreamResponse
);
