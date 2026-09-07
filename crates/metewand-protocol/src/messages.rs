//! Typed version-1 handshake messages and session invariants.

use std::{collections::BTreeSet, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

use crate::{WIRE_PROTOCOL_VERSION, framing::MAX_LINE_BYTES};

const RESOLVED_WORKER_PREFIX: &str = "mw1-resolved-worker-";
const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

/// A nonempty request identifier carried by every protocol message.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    /// Returns the request identifier as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RequestId {
    type Err = MetadataError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(MetadataError::EmptyRequestId);
        }
        Ok(Self(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(de::Error::custom)
    }
}

/// The resolved identity of the worker the orchestrator intends to launch.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct WorkerIdentity(String);

impl WorkerIdentity {
    /// Returns the typed worker identity as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WorkerIdentity {
    type Err = MetadataError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let digest = value.strip_prefix(RESOLVED_WORKER_PREFIX).ok_or_else(|| {
            MetadataError::InvalidWorkerIdentity {
                value: value.to_owned(),
            }
        })?;
        if digest.len() != 64
            || !digest
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(MetadataError::InvalidWorkerIdentity {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for WorkerIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(de::Error::custom)
    }
}

/// The worker role selected for a process before it starts work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    /// A worker that produces a dataset artifact.
    DatasetMaterializer,
    /// A worker that executes a benchmark implementation.
    Implementation,
    /// A problem-owned worker that evaluates canonical results.
    ProblemEvaluator,
}

/// A protocol capability reported by an implementation worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Produces one final result per execution.
    OneShot,
    /// Can report whether a configuration is applicable before execution.
    Applicability,
    /// Can run a fresh execution for each observation control.
    FreshSequence,
    /// Can publish bounded checkpoints during one execution.
    StreamingProfile,
}

/// Identifies the language SDK used by a worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SdkMetadata {
    name: String,
    version: String,
}

impl SdkMetadata {
    /// Creates SDK metadata from nonempty name and version strings.
    ///
    /// # Errors
    ///
    /// Returns an error when either value is empty.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Result<Self, MetadataError> {
        let name = name.into();
        let version = version.into();
        if name.is_empty() {
            return Err(MetadataError::EmptySdkName);
        }
        if version.is_empty() {
            return Err(MetadataError::EmptySdkVersion);
        }
        Ok(Self { name, version })
    }

    /// Returns the SDK package name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the SDK package version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSdkMetadata {
    name: String,
    version: String,
}

impl<'de> Deserialize<'de> for SdkMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawSdkMetadata::deserialize(deserializer)?;
        Self::new(raw.name, raw.version).map_err(de::Error::custom)
    }
}

/// The orchestrator's first request to a worker process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelloRequest {
    id: RequestId,
    method: HelloMethod,
    protocols: ProtocolOffers,
    role: WorkerRole,
    worker_id: WorkerIdentity,
}

impl private::Sealed for HelloRequest {}

impl WireMessage for HelloRequest {}

impl HelloRequest {
    /// Creates the version-1 hello request for a resolved worker.
    #[must_use]
    pub fn new(id: RequestId, role: WorkerRole, worker_id: WorkerIdentity) -> Self {
        Self {
            id,
            method: HelloMethod::Hello,
            protocols: ProtocolOffers(vec![WIRE_PROTOCOL_VERSION]),
            role,
            worker_id,
        }
    }

    /// Returns the request identifier.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the protocol versions offered by the orchestrator.
    #[must_use]
    pub fn protocols(&self) -> &[u32] {
        &self.protocols.0
    }

    /// Returns the role selected for this worker.
    #[must_use]
    pub const fn role(&self) -> WorkerRole {
        self.role
    }

    /// Returns the resolved worker identity expected by the orchestrator.
    #[must_use]
    pub const fn worker_id(&self) -> &WorkerIdentity {
        &self.worker_id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum HelloMethod {
    Hello,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
struct ProtocolOffers(Vec<u32>);

impl<'de> Deserialize<'de> for ProtocolOffers {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let versions = Vec::<u32>::deserialize(deserializer)?;
        if versions.is_empty() {
            return Err(de::Error::custom(
                "at least one protocol version is required",
            ));
        }
        let unique = versions.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != versions.len() {
            return Err(de::Error::custom("protocol versions must be unique"));
        }
        Ok(Self(versions))
    }
}

/// A successful version-1 response to [`HelloRequest`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelloResponse {
    id: RequestId,
    ok: Success,
    protocol: VersionOne,
    worker_id: WorkerIdentity,
    #[serde(deserialize_with = "deserialize_required_option")]
    sdk: Option<SdkMetadata>,
    capabilities: Capabilities,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn deserialize_safe_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > MAX_SAFE_INTEGER {
        return Err(de::Error::custom(
            "integer exceeds the version-1 safe-integer maximum",
        ));
    }
    Ok(value)
}

fn deserialize_required_safe_u64_option<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<u64>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        if value > MAX_SAFE_INTEGER {
            Err(de::Error::custom(
                "integer exceeds the version-1 safe-integer maximum",
            ))
        } else {
            Ok(Some(value))
        }
    })
}

fn deserialize_nonempty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(de::Error::custom("string must not be empty"));
    }
    Ok(value)
}

fn deserialize_optional_object<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    if value.as_ref().is_some_and(|details| !details.is_object()) {
        return Err(de::Error::custom(
            "worker failure details must be an object",
        ));
    }
    Ok(value)
}

fn deserialize_object<'de, D>(deserializer: D) -> Result<Value, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    if !value.is_object() {
        return Err(de::Error::custom("value must be an object"));
    }
    Ok(value)
}

impl private::Sealed for HelloResponse {}

impl WireMessage for HelloResponse {}

impl WireResponse for HelloResponse {
    fn id(&self) -> &RequestId {
        &self.id
    }
}

impl HelloResponse {
    /// Creates a successful version-1 hello response.
    ///
    /// Capability values are deduplicated and placed in stable protocol order.
    #[must_use]
    pub fn new(
        id: RequestId,
        worker_id: WorkerIdentity,
        sdk: Option<SdkMetadata>,
        capabilities: impl IntoIterator<Item = Capability>,
    ) -> Self {
        Self {
            id,
            ok: Success,
            protocol: VersionOne,
            worker_id,
            sdk,
            capabilities: Capabilities(
                capabilities
                    .into_iter()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            ),
        }
    }

    /// Returns the request identifier this response answers.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the negotiated wire-protocol version.
    #[must_use]
    pub const fn protocol(&self) -> u32 {
        WIRE_PROTOCOL_VERSION
    }

    /// Returns the worker identity reported by the process.
    #[must_use]
    pub const fn worker_id(&self) -> &WorkerIdentity {
        &self.worker_id
    }

    /// Returns SDK metadata, or `None` for a direct protocol worker.
    #[must_use]
    pub const fn sdk(&self) -> Option<&SdkMetadata> {
        self.sdk.as_ref()
    }

    /// Returns every capability reported by the worker.
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities.0
    }
}

/// A request for a dataset materializer to produce one immutable dataset.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaterializeRequest {
    id: RequestId,
    method: MaterializeMethod,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    source_dir: String,
    #[serde(deserialize_with = "deserialize_object")]
    dataset_parameters: Value,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    dataset_seed: u64,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    output_dir: String,
}

impl private::Sealed for MaterializeRequest {}

impl WireMessage for MaterializeRequest {}

impl MaterializeRequest {
    /// Creates one dataset materialization request.
    #[must_use]
    pub fn new(
        id: RequestId,
        source_dir: impl Into<String>,
        dataset_parameters: Value,
        dataset_seed: u64,
        output_dir: impl Into<String>,
    ) -> Self {
        Self {
            id,
            method: MaterializeMethod::Materialize,
            source_dir: source_dir.into(),
            dataset_parameters,
            dataset_seed,
            output_dir: output_dir.into(),
        }
    }

    /// Returns the request identifier.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the worker's assigned source-bundle directory.
    #[must_use]
    pub fn source_dir(&self) -> &str {
        &self.source_dir
    }

    /// Returns the canonical dataset parameters.
    #[must_use]
    pub const fn dataset_parameters(&self) -> &Value {
        &self.dataset_parameters
    }

    /// Returns the deterministic dataset seed.
    #[must_use]
    pub const fn dataset_seed(&self) -> u64 {
        self.dataset_seed
    }

    /// Returns the private directory assigned for dataset output.
    #[must_use]
    pub fn output_dir(&self) -> &str {
        &self.output_dir
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MaterializeMethod {
    Materialize,
}

/// A request to establish clean implementation state for one attempt.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRequest {
    id: RequestId,
    method: PrepareMethod,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    dataset_dir: String,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    problem_contract_id: String,
    #[serde(deserialize_with = "deserialize_object")]
    problem_parameters: Value,
    #[serde(deserialize_with = "deserialize_object")]
    implementation_parameters: Value,
    #[serde(deserialize_with = "deserialize_safe_u64")]
    implementation_seed: u64,
}

impl private::Sealed for PrepareRequest {}

impl WireMessage for PrepareRequest {}

impl PrepareRequest {
    /// Creates one implementation preparation request.
    #[must_use]
    pub fn new(
        id: RequestId,
        dataset_dir: impl Into<String>,
        problem_contract_id: impl Into<String>,
        problem_parameters: Value,
        implementation_parameters: Value,
        implementation_seed: u64,
    ) -> Self {
        Self {
            id,
            method: PrepareMethod::Prepare,
            dataset_dir: dataset_dir.into(),
            problem_contract_id: problem_contract_id.into(),
            problem_parameters,
            implementation_parameters,
            implementation_seed,
        }
    }

    /// Returns the request identifier.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the private dataset-view directory.
    #[must_use]
    pub fn dataset_dir(&self) -> &str {
        &self.dataset_dir
    }

    /// Returns the selected problem contract identity.
    #[must_use]
    pub fn problem_contract_id(&self) -> &str {
        &self.problem_contract_id
    }

    /// Returns the canonical problem parameters.
    #[must_use]
    pub const fn problem_parameters(&self) -> &Value {
        &self.problem_parameters
    }

    /// Returns the canonical implementation parameters.
    #[must_use]
    pub const fn implementation_parameters(&self) -> &Value {
        &self.implementation_parameters
    }

    /// Returns the deterministic implementation seed.
    #[must_use]
    pub const fn implementation_seed(&self) -> u64 {
        self.implementation_seed
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PrepareMethod {
    Prepare,
}

/// A request to run one prepared implementation to its final result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteRequest {
    id: RequestId,
    method: ExecuteMethod,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    result_dir: String,
}

impl private::Sealed for ExecuteRequest {}

impl WireMessage for ExecuteRequest {}

impl ExecuteRequest {
    /// Creates one capability-limited, one-shot execution request.
    #[must_use]
    pub fn new(id: RequestId, result_dir: impl Into<String>) -> Self {
        Self {
            id,
            method: ExecuteMethod::Execute,
            result_dir: result_dir.into(),
        }
    }

    /// Returns the request identifier.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the private directory assigned for canonical result output.
    #[must_use]
    pub fn result_dir(&self) -> &str {
        &self.result_dir
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ExecuteMethod {
    Execute,
}

/// A request to clear slot-scoped implementation state before reuse.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResetRequest {
    id: RequestId,
    method: ResetMethod,
}

impl private::Sealed for ResetRequest {}

impl WireMessage for ResetRequest {}

impl ResetRequest {
    /// Creates one implementation reset request.
    #[must_use]
    pub const fn new(id: RequestId) -> Self {
        Self {
            id,
            method: ResetMethod::Reset,
        }
    }

    /// Returns the request identifier.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResetMethod {
    Reset,
}

/// A request for independent evaluation of one canonical result.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluateRequest {
    id: RequestId,
    method: EvaluateMethod,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    dataset_dir: String,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    problem_contract_id: String,
    #[serde(deserialize_with = "deserialize_object")]
    problem_parameters: Value,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    result_dir: String,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    metrics_path: String,
}

impl private::Sealed for EvaluateRequest {}

impl WireMessage for EvaluateRequest {}

impl EvaluateRequest {
    /// Creates one independent evaluator request.
    #[must_use]
    pub fn new(
        id: RequestId,
        dataset_dir: impl Into<String>,
        problem_contract_id: impl Into<String>,
        problem_parameters: Value,
        result_dir: impl Into<String>,
        metrics_path: impl Into<String>,
    ) -> Self {
        Self {
            id,
            method: EvaluateMethod::Evaluate,
            dataset_dir: dataset_dir.into(),
            problem_contract_id: problem_contract_id.into(),
            problem_parameters,
            result_dir: result_dir.into(),
            metrics_path: metrics_path.into(),
        }
    }

    /// Returns the request identifier.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the evaluator's private dataset-view directory.
    #[must_use]
    pub fn dataset_dir(&self) -> &str {
        &self.dataset_dir
    }

    /// Returns the selected problem contract identity.
    #[must_use]
    pub fn problem_contract_id(&self) -> &str {
        &self.problem_contract_id
    }

    /// Returns the canonical problem parameters.
    #[must_use]
    pub const fn problem_parameters(&self) -> &Value {
        &self.problem_parameters
    }

    /// Returns the private directory containing the canonical result.
    #[must_use]
    pub fn result_dir(&self) -> &str {
        &self.result_dir
    }

    /// Returns the exact path assigned for the completed metrics document.
    #[must_use]
    pub fn metrics_path(&self) -> &str {
        &self.metrics_path
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EvaluateMethod {
    Evaluate,
}

/// A request for a worker to end its session cleanly.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownRequest {
    id: RequestId,
    method: ShutdownMethod,
}

impl private::Sealed for ShutdownRequest {}

impl WireMessage for ShutdownRequest {}

impl ShutdownRequest {
    /// Creates one worker shutdown request.
    #[must_use]
    pub const fn new(id: RequestId) -> Self {
        Self {
            id,
            method: ShutdownMethod::Shutdown,
        }
    }

    /// Returns the request identifier.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ShutdownMethod {
    Shutdown,
}

/// A manifest path returned after a worker has completed an artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestReference {
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    manifest: String,
}

impl ManifestReference {
    fn new(manifest: impl Into<String>) -> Self {
        Self {
            manifest: manifest.into(),
        }
    }
}

/// A successful dataset materialization response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaterializeResponse {
    id: RequestId,
    ok: Success,
    dataset: ManifestReference,
}

impl private::Sealed for MaterializeResponse {}

impl WireMessage for MaterializeResponse {}

impl WireResponse for MaterializeResponse {
    fn id(&self) -> &RequestId {
        &self.id
    }
}

impl MaterializeResponse {
    /// Creates a successful dataset materialization response.
    #[must_use]
    pub fn new(id: RequestId, manifest: impl Into<String>) -> Self {
        Self {
            id,
            ok: Success,
            dataset: ManifestReference::new(manifest),
        }
    }

    /// Returns the completed dataset manifest path beneath the assigned output.
    #[must_use]
    pub fn manifest(&self) -> &str {
        &self.dataset.manifest
    }

    /// Returns the request identifier this response answers.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }
}

/// A successful response from a phase with no response payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    id: RequestId,
    ok: Success,
}

impl private::Sealed for Acknowledgement {}

impl WireMessage for Acknowledgement {}

impl WireResponse for Acknowledgement {
    fn id(&self) -> &RequestId {
        &self.id
    }
}

impl Acknowledgement {
    /// Creates a successful response without a phase-specific payload.
    #[must_use]
    pub const fn new(id: RequestId) -> Self {
        Self { id, ok: Success }
    }

    /// Returns the request identifier this response answers.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }
}

/// A successful one-shot implementation response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteResponse {
    id: RequestId,
    ok: Success,
    #[serde(deserialize_with = "deserialize_required_safe_u64_option")]
    implementation_time_ns: Option<u64>,
    result: ManifestReference,
    #[serde(deserialize_with = "deserialize_object")]
    statistics: Value,
}

impl private::Sealed for ExecuteResponse {}

impl WireMessage for ExecuteResponse {}

impl WireResponse for ExecuteResponse {
    fn id(&self) -> &RequestId {
        &self.id
    }
}

impl ExecuteResponse {
    /// Creates one completed implementation response.
    #[must_use]
    pub fn new(
        id: RequestId,
        implementation_time_ns: Option<u64>,
        manifest: impl Into<String>,
        statistics: serde_json::Map<String, Value>,
    ) -> Self {
        Self {
            id,
            ok: Success,
            implementation_time_ns,
            result: ManifestReference::new(manifest),
            statistics: Value::Object(statistics),
        }
    }

    /// Returns the worker's optional diagnostic implementation-call time.
    #[must_use]
    pub const fn implementation_time_ns(&self) -> Option<u64> {
        self.implementation_time_ns
    }

    /// Returns the completed result manifest path beneath the assigned output.
    #[must_use]
    pub fn manifest(&self) -> &str {
        &self.result.manifest
    }

    /// Returns implementation-owned scalar diagnostic statistics.
    #[must_use]
    pub const fn statistics(&self) -> &Value {
        &self.statistics
    }

    /// Returns the request identifier this response answers.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }
}

/// A stable version-1 worker-reported failure category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerFailureCode {
    /// The request was semantically invalid for the worker.
    InvalidRequest,
    /// The worker does not implement the requested operation.
    UnsupportedOperation,
    /// The requested worker operation failed.
    OperationFailed,
    /// An unexpected internal worker failure occurred.
    InternalError,
}

/// Structured failure details returned by a worker operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFailure {
    code: WorkerFailureCode,
    #[serde(deserialize_with = "deserialize_nonempty_string")]
    message: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_object",
        skip_serializing_if = "Option::is_none"
    )]
    details: Option<Value>,
}

impl WorkerFailure {
    /// Creates a worker failure with optional object-valued details.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty message or non-object details.
    pub fn new(
        code: WorkerFailureCode,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Result<Self, MetadataError> {
        let message = message.into();
        if message.is_empty() {
            return Err(MetadataError::EmptyWorkerFailureMessage);
        }
        if details.as_ref().is_some_and(|value| !value.is_object()) {
            return Err(MetadataError::InvalidWorkerFailureDetails);
        }
        Ok(Self {
            code,
            message,
            details,
        })
    }

    /// Returns the stable worker failure category.
    #[must_use]
    pub const fn code(&self) -> WorkerFailureCode {
        self.code
    }

    /// Returns the worker's human-readable failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns optional structured worker details.
    #[must_use]
    pub const fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }
}

/// A failed response to any version-1 worker operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFailureResponse {
    id: RequestId,
    ok: Failure,
    error: WorkerFailure,
}

impl private::Sealed for WorkerFailureResponse {}

impl WireMessage for WorkerFailureResponse {}

impl WorkerFailureResponse {
    /// Creates a failed worker response.
    #[must_use]
    pub const fn new(id: RequestId, error: WorkerFailure) -> Self {
        Self {
            id,
            ok: Failure,
            error,
        }
    }

    /// Returns the request identifier this response answers.
    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the structured worker failure.
    #[must_use]
    pub const fn error(&self) -> &WorkerFailure {
        &self.error
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Success;

impl Serialize for Success {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for Success {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(de::Error::custom(
                "a successful worker response requires `ok: true`",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Failure;

impl Serialize for Failure {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for Failure {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Err(de::Error::custom(
                "a failed worker response requires `ok: false`",
            ))
        } else {
            Ok(Self)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VersionOne;

impl Serialize for VersionOne {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(WIRE_PROTOCOL_VERSION)
    }
}

impl<'de> Deserialize<'de> for VersionOne {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u32::deserialize(deserializer)?;
        if version == WIRE_PROTOCOL_VERSION {
            Ok(Self)
        } else {
            Err(de::Error::invalid_value(
                de::Unexpected::Unsigned(u64::from(version)),
                &"wire-protocol version 1",
            ))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
struct Capabilities(Vec<Capability>);

impl<'de> Deserialize<'de> for Capabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<Capability>::deserialize(deserializer)?;
        let unique = values.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != values.len() {
            return Err(de::Error::custom("worker capabilities must be unique"));
        }
        Ok(Self(unique.into_iter().collect()))
    }
}

mod private {
    pub trait Sealed {}
}

/// A typed message admitted by the version-1 worker wire protocol.
pub trait WireMessage: private::Sealed + DeserializeOwned + Serialize {}

/// A successful response admitted by a version-1 worker exchange.
pub trait WireResponse: WireMessage {
    /// Returns the request identifier answered by the response.
    fn id(&self) -> &RequestId;
}

/// Serializes a typed message as one canonical, newline-terminated frame.
///
/// # Errors
///
/// Returns an error if serialization fails or the complete frame exceeds the
/// version-1 one-mebibyte line limit.
pub fn encode_message<M: WireMessage>(message: &M) -> Result<Vec<u8>, MessageEncodeError> {
    let mut encoded =
        serde_json_canonicalizer::to_vec(message).map_err(MessageEncodeError::Serialization)?;
    if encoded.len() >= MAX_LINE_BYTES {
        return Err(MessageEncodeError::LineTooLong {
            limit: MAX_LINE_BYTES,
        });
    }
    encoded.push(b'\n');
    Ok(encoded)
}

/// Deserializes a validated frame as the message type expected by session state.
///
/// # Errors
///
/// Returns an error when the JSON value does not have that message's strict
/// version-1 shape.
pub fn decode_message<M: WireMessage>(value: Value) -> Result<M, MessageDecodeError> {
    serde_json::from_value(value).map_err(MessageDecodeError::InvalidMessage)
}

/// The worker's terminal response to one operation request.
#[derive(Clone, Debug, PartialEq)]
pub enum OperationResponse<R> {
    /// The operation completed with its method-specific response.
    Success(R),
    /// The worker rejected or failed the requested operation.
    Failure(WorkerFailure),
}

/// Correlates and decodes one success or structured failure response.
///
/// # Errors
///
/// Returns a typed error when the envelope is malformed, its identifier does
/// not match the pending request, or its selected response shape is invalid.
pub fn decode_operation_response<R: WireResponse>(
    value: Value,
    requests: &mut RequestTracker,
) -> Result<OperationResponse<R>, OperationResponseDecodeError> {
    #[derive(Deserialize)]
    struct ResponseEnvelope {
        id: RequestId,
        ok: bool,
    }

    let envelope = serde_json::from_value::<ResponseEnvelope>(value.clone())
        .map_err(MessageDecodeError::InvalidMessage)?;
    requests.complete_response(&envelope.id)?;

    if envelope.ok {
        let response = decode_message::<R>(value)?;
        debug_assert_eq!(response.id(), &envelope.id);
        Ok(OperationResponse::Success(response))
    } else {
        let response = decode_message::<WorkerFailureResponse>(value)?;
        Ok(OperationResponse::Failure(response.error))
    }
}

/// The accepted state established by a valid hello response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NegotiatedSession {
    role: WorkerRole,
    protocol: u32,
    worker_id: WorkerIdentity,
    sdk: Option<SdkMetadata>,
    selected_capabilities: Vec<Capability>,
    reported_capabilities: Vec<Capability>,
}

impl NegotiatedSession {
    /// Returns the worker role fixed by the hello request.
    #[must_use]
    pub const fn role(&self) -> WorkerRole {
        self.role
    }

    /// Returns the selected wire-protocol version.
    #[must_use]
    pub const fn protocol(&self) -> u32 {
        self.protocol
    }

    /// Returns the verified resolved worker identity.
    #[must_use]
    pub const fn worker_id(&self) -> &WorkerIdentity {
        &self.worker_id
    }

    /// Returns worker SDK metadata when an SDK was used.
    #[must_use]
    pub const fn sdk(&self) -> Option<&SdkMetadata> {
        self.sdk.as_ref()
    }

    /// Returns the capabilities selected by the current plan.
    #[must_use]
    pub fn selected_capabilities(&self) -> &[Capability] {
        &self.selected_capabilities
    }

    /// Returns every capability reported by the worker.
    #[must_use]
    pub fn reported_capabilities(&self) -> &[Capability] {
        &self.reported_capabilities
    }
}

/// Selects version 1 when the orchestrator offered it.
///
/// # Errors
///
/// Returns an error when the offer has no version supported by this crate.
pub fn negotiate_protocol(offered: &[u32]) -> Result<u32, HandshakeError> {
    offered
        .contains(&WIRE_PROTOCOL_VERSION)
        .then_some(WIRE_PROTOCOL_VERSION)
        .ok_or_else(|| HandshakeError::NoCommonProtocol {
            offered: offered.to_vec(),
            supported: vec![WIRE_PROTOCOL_VERSION],
        })
}

/// Validates a worker hello response against its request and selected plan.
///
/// Reported capabilities that were not selected remain inert but are retained
/// in the negotiated metadata.
///
/// # Errors
///
/// Returns a typed mismatch when request correlation, protocol negotiation,
/// worker identity, or a selected capability is invalid.
pub fn validate_hello_response(
    request: &HelloRequest,
    response: &HelloResponse,
    selected_capabilities: &[Capability],
) -> Result<NegotiatedSession, HandshakeError> {
    if response.id != request.id {
        return Err(HandshakeError::MismatchedRequestId {
            expected: request.id.clone(),
            actual: response.id.clone(),
        });
    }
    let protocol = negotiate_protocol(request.protocols())?;
    if response.protocol() != protocol {
        return Err(HandshakeError::ProtocolMismatch {
            selected: protocol,
            reported: response.protocol(),
        });
    }
    if response.worker_id != request.worker_id {
        return Err(HandshakeError::WorkerIdentityMismatch {
            expected: request.worker_id.clone(),
            actual: response.worker_id.clone(),
        });
    }
    if request.role != WorkerRole::Implementation
        && let Some(capability) = selected_capabilities.first()
    {
        return Err(HandshakeError::CapabilityRoleMismatch {
            role: request.role,
            capability: *capability,
        });
    }
    if let Some(capability) = selected_capabilities
        .iter()
        .find(|capability| **capability != Capability::OneShot)
    {
        return Err(HandshakeError::ReservedCapability {
            capability: *capability,
        });
    }
    let selected_capabilities = selected_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for capability in &selected_capabilities {
        if !response.capabilities.0.contains(capability) {
            return Err(HandshakeError::MissingCapability {
                capability: *capability,
            });
        }
    }

    Ok(NegotiatedSession {
        role: request.role,
        protocol,
        worker_id: response.worker_id.clone(),
        sdk: response.sdk.clone(),
        selected_capabilities: selected_capabilities.into_iter().collect(),
        reported_capabilities: response.capabilities.0.clone(),
    })
}

/// Allocates request IDs and enforces the version-1 serial request limit.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct RequestTracker {
    next_id: u64,
    pending: Option<RequestId>,
}

impl RequestTracker {
    /// Creates a tracker whose first request ID is `"0"`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_id: 0,
            pending: None,
        }
    }

    /// Begins the next orchestrator request.
    ///
    /// # Errors
    ///
    /// Returns an error when another request is pending or the identifier
    /// sequence has been exhausted.
    pub fn begin_request(&mut self) -> Result<RequestId, RequestTrackerError> {
        if let Some(id) = &self.pending {
            return Err(RequestTrackerError::RequestAlreadyPending { id: id.clone() });
        }
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or(RequestTrackerError::RequestIdExhausted)?;
        let id = RequestId(self.next_id.to_string());
        self.next_id = next_id;
        self.pending = Some(id.clone());
        Ok(id)
    }

    /// Completes the pending request with a worker response identifier.
    ///
    /// A mismatched identifier does not consume the pending request.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched or extra response.
    pub fn complete_response(
        &mut self,
        response_id: &RequestId,
    ) -> Result<(), RequestTrackerError> {
        let Some(expected) = &self.pending else {
            return Err(RequestTrackerError::ExtraResponse {
                id: response_id.clone(),
            });
        };
        if expected != response_id {
            return Err(RequestTrackerError::MismatchedRequestId {
                expected: expected.clone(),
                actual: response_id.clone(),
            });
        }
        self.pending = None;
        Ok(())
    }

    /// Returns the identifier of the request awaiting a response.
    #[must_use]
    pub const fn pending_request(&self) -> Option<&RequestId> {
        self.pending.as_ref()
    }
}

/// Invalid request, worker, or SDK metadata.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum MetadataError {
    /// A request ID was empty.
    #[error("worker protocol request IDs must not be empty")]
    EmptyRequestId,
    /// A resolved worker identity had the wrong kind or digest spelling.
    #[error("`{value}` is not a version-1 resolved-worker identity")]
    InvalidWorkerIdentity {
        /// Invalid identity text.
        value: String,
    },
    /// An SDK name was empty.
    #[error("worker SDK names must not be empty")]
    EmptySdkName,
    /// An SDK version was empty.
    #[error("worker SDK versions must not be empty")]
    EmptySdkVersion,
    /// A worker failure omitted its human-readable explanation.
    #[error("worker failure messages must not be empty")]
    EmptyWorkerFailureMessage,
    /// Worker failure details were not a structured object.
    #[error("worker failure details must be an object")]
    InvalidWorkerFailureDetails,
}

/// A failure to establish a version-1 worker session.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum HandshakeError {
    /// No offered protocol is supported by this implementation.
    #[error("no common worker protocol version; offered {offered:?}, supported {supported:?}")]
    NoCommonProtocol {
        /// Versions offered by the orchestrator.
        offered: Vec<u32>,
        /// Versions supported by the worker implementation.
        supported: Vec<u32>,
    },
    /// The response answered another request.
    #[error("worker response ID `{actual}` does not match hello request `{expected}`")]
    MismatchedRequestId {
        /// Hello request ID.
        expected: RequestId,
        /// Response ID reported by the worker.
        actual: RequestId,
    },
    /// The worker reported a protocol other than the selected version.
    #[error("worker reported protocol {reported}, but protocol {selected} was selected")]
    ProtocolMismatch {
        /// Version selected from the offer.
        selected: u32,
        /// Version reported by the worker.
        reported: u32,
    },
    /// The worker did not reproduce its expected resolved identity.
    #[error("worker identity `{actual}` does not match expected identity `{expected}`")]
    WorkerIdentityMismatch {
        /// Resolved worker identity supplied by the orchestrator.
        expected: WorkerIdentity,
        /// Resolved worker identity returned by the worker.
        actual: WorkerIdentity,
    },
    /// A capability selected by the plan was absent from the response.
    #[error("worker did not report selected capability `{capability:?}`")]
    MissingCapability {
        /// Required capability absent from the worker response.
        capability: Capability,
    },
    /// A later version-1 capability was selected before its subprotocol exists.
    #[error("worker capability `{capability:?}` is reserved for a later subprotocol")]
    ReservedCapability {
        /// Reported capability whose request exchange is not implemented yet.
        capability: Capability,
    },
    /// An implementation capability was selected for another worker role.
    #[error("worker capability `{capability:?}` cannot be selected for role `{role:?}`")]
    CapabilityRoleMismatch {
        /// Role fixed by the hello request.
        role: WorkerRole,
        /// Implementation capability selected for that role.
        capability: Capability,
    },
}

/// A violation of request correlation or serial request execution.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum RequestTrackerError {
    /// A caller tried to begin an overlapping request.
    #[error("request `{id}` is still pending; version 1 permits only one request at a time")]
    RequestAlreadyPending {
        /// Currently pending request ID.
        id: RequestId,
    },
    /// The request identifier counter cannot allocate another identifier.
    #[error("worker request ID sequence is exhausted")]
    RequestIdExhausted,
    /// The response answered another request.
    #[error("worker response ID `{actual}` does not match pending request `{expected}`")]
    MismatchedRequestId {
        /// Pending request ID.
        expected: RequestId,
        /// Response ID reported by the worker.
        actual: RequestId,
    },
    /// A response arrived without any pending request.
    #[error("worker sent extra response `{id}` with no pending request")]
    ExtraResponse {
        /// Unexpected response ID.
        id: RequestId,
    },
}

/// A failure to encode one complete worker-protocol line.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MessageEncodeError {
    /// A typed message could not be serialized as canonical JSON.
    #[error("failed to serialize worker protocol message: {0}")]
    Serialization(#[source] serde_json::Error),
    /// The message and final newline exceed the framing limit.
    #[error("worker protocol line exceeds the {limit}-byte limit")]
    LineTooLong {
        /// Inclusive line limit, including the final newline.
        limit: usize,
    },
}

/// A frame did not match the strict message shape expected by session state.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MessageDecodeError {
    /// The decoded JSON value was not the expected message.
    #[error("invalid worker protocol message: {0}")]
    InvalidMessage(#[source] serde_json::Error),
}

/// A failure to correlate or decode one worker operation response.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OperationResponseDecodeError {
    /// The response did not match its success or failure message shape.
    #[error(transparent)]
    Message(#[from] MessageDecodeError),
    /// The response did not answer the one pending request.
    #[error(transparent)]
    Request(#[from] RequestTrackerError),
}
