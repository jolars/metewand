//! Typed, versioned content identities for core domain records.
//!
//! Identity inputs are explicit canonical JSON representations rather than
//! ordinary Rust serialization. This keeps compatibility independent of field
//! order, Rust enum layouts, and provenance-only fields.

use std::{fmt, str::FromStr, time::Duration};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    IDENTITY_VERSION,
    canonical::{CanonicalJsonError, CanonicalValue},
    manifest::{
        Enforcement, ImplementationCapability, InterpretedRunner, NixOutputKind, ObservationKind,
        PrimaryTime, ProtocolTransport, RunOrder, TimingScope,
    },
    records::{
        ArtifactReference, AttemptSlotRole, CompletionStatus, ContentDigest, DatasetArtifact,
        DatasetConfigurationRecord, DatasetDefinitionKind, DatasetDefinitionRecord,
        DatasetInstanceRecord, EnvironmentDefinitionKind, EnvironmentDefinitionRecord,
        ExecutionPolicyRecord, ExecutorDefinitionRecord, IdentifiedRecord,
        ImplementationConfigurationRecord, ImplementationDefinitionRecord,
        LogicalAttemptSlotRecord, LogicalCandidateRecord, LogicalObservationSlotRecord,
        MetricsArtifact, ObservationPolicyRecord, ObservationTimingRecord,
        OneShotLogicalSpecificationRecord, ProblemConfigurationRecord, ProblemContractResource,
        ProblemDefinitionRecord, ProblemInstanceRecord, RecordId, RemoteSourceRecord,
        ResolvedAttemptSlotRecord, ResolvedEnvironmentRecord, ResolvedLaunchRecord,
        ResolvedObservationSlotRecord, ResolvedOneShotSpecificationRecord, ResolvedWorkerRecord,
        ResultArtifact, RunAttemptRecord, RunObservationRecord, SchemaResource,
        SourceBundleArtifact, SourceBundleResource, WorkerDefinitionRecord, WorkerLaunch,
        WorkerWorkingDirectory,
    },
};

/// A type with a stable identity kind tag.
pub trait IdentityKind {
    /// Lowercase kind inserted into the public `mw1-<kind>-<sha256>` spelling
    /// and the digest domain separator.
    const KIND: &'static str;
}

/// A canonical resource whose identity is selected by a typed marker.
pub trait CanonicalResource: IdentityKind {}

/// A core record with an explicit version-1 canonical identity representation.
pub trait IdentityRecord: IdentityKind {
    /// Produces the fields beneath the representation's top-level `version`.
    #[doc(hidden)]
    fn identity_fields(&self) -> Map<String, Value>;
}

/// Computes and attaches the typed identity for a core domain record.
///
/// # Errors
///
/// Returns an error if a programmatically constructed record contains a value
/// outside the version-1 canonical JSON domain or canonical serialization
/// fails.
pub fn identify_record<T: IdentityRecord>(record: T) -> Result<IdentifiedRecord<T>, IdentityError> {
    let id = record_id(&record)?;
    Ok(IdentifiedRecord { id, record })
}

/// Computes the typed identity for a borrowed core domain record.
///
/// # Errors
///
/// Returns an error if the record cannot be represented in canonical JSON.
pub fn record_id<T: IdentityRecord>(record: &T) -> Result<RecordId<T>, IdentityError> {
    Ok(RecordId::from_digest(hash_identity::<T>(
        &canonical_identity_bytes(record)?,
    )))
}

/// Returns a record's exact versioned canonical identity bytes.
///
/// # Errors
///
/// Returns an error if the record cannot be represented in canonical JSON.
pub fn canonical_identity_bytes<T: IdentityRecord>(record: &T) -> Result<Vec<u8>, IdentityError> {
    let mut fields = record.identity_fields();
    fields.insert("version".to_owned(), json!(IDENTITY_VERSION));
    canonical_bytes(Value::Object(fields))
}

/// Computes a typed identity for an already validated canonical resource.
///
/// Schemas, problem contracts, and content resources use this entry point;
/// their public kind marker supplies the domain separator. The resource is
/// wrapped in `{"version":1,"value":...}` before hashing.
///
/// # Errors
///
/// Returns an error if canonical serialization unexpectedly fails.
pub fn identify_canonical<T: CanonicalResource>(
    value: &CanonicalValue,
) -> Result<RecordId<T>, IdentityError> {
    let bytes = canonical_resource_identity_bytes(value)?;
    Ok(RecordId::from_digest(hash_identity::<T>(&bytes)))
}

/// Returns the exact versioned identity bytes for a canonical resource.
///
/// # Errors
///
/// Returns an error if canonical serialization unexpectedly fails.
pub fn canonical_resource_identity_bytes(value: &CanonicalValue) -> Result<Vec<u8>, IdentityError> {
    canonical_bytes(object([
        ("version", json!(IDENTITY_VERSION)),
        ("value", value.as_json().clone()),
    ]))
}

fn canonical_bytes(value: Value) -> Result<Vec<u8>, IdentityError> {
    Ok(CanonicalValue::try_from(value)?.to_canonical_bytes()?)
}

fn hash_identity<T: IdentityKind>(canonical: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(T::KIND.as_bytes());
    hasher.update([0]);
    hasher.update(canonical);
    hasher.finalize().into()
}

/// A failure to construct a typed content identity.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IdentityError {
    /// The explicit representation violated the canonical JSON contract.
    #[error("identity representation is not canonical-domain JSON: {0}")]
    Canonical(#[from] CanonicalJsonError),
}

/// A malformed textual identity or an identity with the wrong typed kind.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum RecordIdParseError {
    /// The version or kind prefix does not match the requested marker type.
    #[error("identity must begin with `{expected}`")]
    WrongKind {
        /// Required prefix for the requested identity type.
        expected: String,
    },
    /// The digest is not exactly 64 lowercase hexadecimal digits.
    #[error("identity digest must contain 64 lowercase hexadecimal digits")]
    InvalidDigest,
}

impl<T: IdentityKind> fmt::Display for RecordId<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "mw1-{}-{}", T::KIND, hex(self.digest()))
    }
}

impl<T: IdentityKind> FromStr for RecordId<T> {
    type Err = RecordIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let prefix = format!("mw1-{}-", T::KIND);
        let digest = value
            .strip_prefix(&prefix)
            .ok_or_else(|| RecordIdParseError::WrongKind {
                expected: prefix.clone(),
            })?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RecordIdParseError::InvalidDigest);
        }

        let mut bytes = [0; 32];
        let (pairs, remainder) = digest.as_bytes().as_chunks::<2>();
        debug_assert!(remainder.is_empty());
        for (output, pair) in bytes.iter_mut().zip(pairs) {
            *output = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
        }
        Ok(Self::from_digest(bytes))
    }
}

impl<T: IdentityKind> Serialize for RecordId<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de, T: IdentityKind> Deserialize<'de> for RecordId<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("identity digest was validated before decoding"),
    }
}

fn object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

fn id<T: IdentityKind>(value: &RecordId<T>) -> Value {
    Value::String(value.to_string())
}

fn optional_id<T: IdentityKind>(value: Option<RecordId<T>>) -> Value {
    value.map_or(Value::Null, |value| id(&value))
}

fn canonical(value: &Option<CanonicalValue>) -> Value {
    value
        .as_ref()
        .map_or(Value::Null, |value| value.as_json().clone())
}

fn optional_string(value: &Option<String>) -> Value {
    value.clone().map_or(Value::Null, Value::String)
}

fn path(value: &std::path::Path) -> Value {
    Value::String(
        value
            .to_str()
            .expect("repository paths are validated as UTF-8")
            .to_owned(),
    )
}

fn digest(value: &ContentDigest) -> Value {
    Value::String(hex(value.bytes()))
}

fn seed(value: &crate::records::DerivedSeedRecord) -> Value {
    object([
        ("value", json!(value.value)),
        ("derivation_digest", digest(&value.derivation_digest)),
    ])
}

fn worker(value: &WorkerDefinitionRecord) -> Value {
    let launch = match &value.launch {
        WorkerLaunch::Interpreted {
            runner,
            entrypoint,
            source_bundle,
        } => object([
            ("kind", json!("interpreted")),
            ("runner", json!(interpreted_runner(*runner))),
            ("entrypoint", path(entrypoint.as_path())),
            ("source_bundle", id(source_bundle)),
        ]),
        WorkerLaunch::Command {
            program,
            source_bundle,
        } => object([
            ("kind", json!("command")),
            ("program", json!(program)),
            ("source_bundle", optional_id(*source_bundle)),
        ]),
    };
    object([
        ("launch", launch),
        ("args", json!(value.args)),
        ("environment", id(&value.environment)),
        (
            "protocol_transport",
            value
                .protocol_transport
                .map_or(Value::Null, |value| json!(protocol_transport(value))),
        ),
    ])
}

fn resolved_worker(value: &ResolvedWorkerRecord) -> Value {
    object([
        ("source_bundle", optional_id(value.source_bundle)),
        ("environment", id(&value.environment)),
        ("launch", id(&value.launch)),
    ])
}

fn artifact<T: IdentityKind>(value: &ArtifactReference<T>) -> Value {
    object([
        ("id", id(&value.id)),
        ("path", path(value.path.as_path())),
        ("sha256", digest(&value.sha256)),
    ])
}

fn timing(value: &ObservationTimingRecord) -> Value {
    object([
        ("primary_time", json!(primary_time(value.primary_time))),
        ("timed_wall_time", duration(value.timed_wall_time)),
        (
            "implementation_time",
            value.implementation_time.map_or(Value::Null, duration),
        ),
        ("evaluation_time", duration(value.evaluation_time)),
        ("cpu_time", value.cpu_time.map_or(Value::Null, duration)),
        ("peak_rss_bytes", json!(value.peak_rss_bytes)),
    ])
}

fn duration(value: Duration) -> Value {
    object([
        ("seconds", json!(value.as_secs())),
        ("nanoseconds", json!(value.subsec_nanos())),
    ])
}

fn interpreted_runner(value: InterpretedRunner) -> &'static str {
    match value {
        InterpretedRunner::Python => "python",
        InterpretedRunner::R => "r",
        InterpretedRunner::Julia => "julia",
    }
}

fn protocol_transport(value: ProtocolTransport) -> &'static str {
    match value {
        ProtocolTransport::Pipes => "pipes",
        ProtocolTransport::Stdio => "stdio",
    }
}

fn timing_scope(value: TimingScope) -> &'static str {
    match value {
        TimingScope::ColdEndToEnd => "cold_end_to_end",
        TimingScope::PrepareAndExecute => "prepare_and_execute",
        TimingScope::ExecuteOnly => "execute_only",
    }
}

fn primary_time(value: PrimaryTime) -> &'static str {
    match value {
        PrimaryTime::TimedWallTime => "timed_wall_time",
        PrimaryTime::CpuTime => "cpu_time",
    }
}

fn run_order(value: RunOrder) -> &'static str {
    match value {
        RunOrder::Sequential => "sequential",
        RunOrder::Randomized => "randomized",
    }
}

fn enforcement(value: Enforcement) -> &'static str {
    match value {
        Enforcement::BestEffort => "best_effort",
        Enforcement::Required => "required",
    }
}

fn implementation_capability(value: ImplementationCapability) -> &'static str {
    match value {
        ImplementationCapability::OneShot => "one_shot",
    }
}

macro_rules! identity_kind {
    ($kind:literal => $($type:ty),+ $(,)?) => {
        $(impl IdentityKind for $type {
            const KIND: &'static str = $kind;
        })+
    };
}

identity_kind!("schema" => SchemaResource);
identity_kind!("problem-contract" => ProblemContractResource);
identity_kind!("source-bundle" => SourceBundleResource);
identity_kind!("artifact" => SourceBundleArtifact, DatasetArtifact, ResultArtifact, MetricsArtifact);
identity_kind!("environment-definition" => EnvironmentDefinitionRecord);
identity_kind!("worker-definition" => WorkerDefinitionRecord);
identity_kind!("dataset-definition" => DatasetDefinitionRecord);
identity_kind!("problem-definition" => ProblemDefinitionRecord);
identity_kind!("implementation-definition" => ImplementationDefinitionRecord);
identity_kind!("execution-policy" => ExecutionPolicyRecord);
identity_kind!("observation-policy" => ObservationPolicyRecord);
identity_kind!("dataset-configuration" => DatasetConfigurationRecord);
identity_kind!("problem-configuration" => ProblemConfigurationRecord);
identity_kind!("implementation-configuration" => ImplementationConfigurationRecord);
identity_kind!("dataset-instance" => DatasetInstanceRecord);
identity_kind!("problem-instance" => ProblemInstanceRecord);
identity_kind!("logical-candidate" => LogicalCandidateRecord);
identity_kind!("logical-specification" => OneShotLogicalSpecificationRecord);
identity_kind!("logical-observation-slot" => LogicalObservationSlotRecord);
identity_kind!("logical-attempt-slot" => LogicalAttemptSlotRecord);
identity_kind!("resolved-environment" => ResolvedEnvironmentRecord);
identity_kind!("resolved-launch" => ResolvedLaunchRecord);
identity_kind!("executor-definition" => ExecutorDefinitionRecord);
identity_kind!("resolved-worker" => ResolvedWorkerRecord);
identity_kind!("resolved-specification" => ResolvedOneShotSpecificationRecord);
identity_kind!("resolved-observation-slot" => ResolvedObservationSlotRecord);
identity_kind!("resolved-attempt-slot" => ResolvedAttemptSlotRecord);
identity_kind!("observation" => RunObservationRecord);
identity_kind!("attempt" => RunAttemptRecord);

impl CanonicalResource for SchemaResource {}
impl CanonicalResource for ProblemContractResource {}
impl CanonicalResource for SourceBundleResource {}
impl CanonicalResource for SourceBundleArtifact {}
impl CanonicalResource for DatasetArtifact {}
impl CanonicalResource for ResultArtifact {}
impl CanonicalResource for MetricsArtifact {}

impl IdentityRecord for EnvironmentDefinitionRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        let definition = match &self.kind {
            EnvironmentDefinitionKind::Local => object([("kind", json!("local"))]),
            EnvironmentDefinitionKind::Uv { project, lockfile } => object([
                ("kind", json!("uv")),
                ("project", path(project.as_path())),
                ("lockfile", path(lockfile.as_path())),
            ]),
            EnvironmentDefinitionKind::Renv { project, lockfile } => object([
                ("kind", json!("renv")),
                ("project", path(project.as_path())),
                ("lockfile", path(lockfile.as_path())),
            ]),
            EnvironmentDefinitionKind::Nix {
                flake,
                installable,
                system,
                output_kind,
                output,
            } => object([
                ("kind", json!("nix")),
                ("flake", path(flake.as_path())),
                ("installable", json!(installable)),
                ("system", json!(system)),
                (
                    "output_kind",
                    json!(match output_kind {
                        NixOutputKind::Package => "package",
                        NixOutputKind::App => "app",
                    }),
                ),
                ("output", json!(output)),
            ]),
            EnvironmentDefinitionKind::Oci { image } => {
                object([("kind", json!("oci")), ("image", json!(image.as_str()))])
            }
        };
        fields([
            ("name", json!(self.name.as_str())),
            ("definition", definition),
        ])
    }
}

impl IdentityRecord for WorkerDefinitionRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        as_fields(worker(self))
    }
}

impl IdentityRecord for DatasetDefinitionRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        let definition = match &self.kind {
            DatasetDefinitionKind::Unit => object([("kind", json!("unit"))]),
            DatasetDefinitionKind::Fixed { source_bundle } => object([
                ("kind", json!("fixed")),
                ("source_bundle", id(source_bundle)),
            ]),
            DatasetDefinitionKind::Generated {
                source,
                materializer,
            } => object([
                ("kind", json!("generated")),
                ("source", source.as_ref().map_or(Value::Null, remote_source)),
                ("materializer", worker(materializer)),
            ]),
        };
        fields([
            ("name", json!(self.name.as_str())),
            ("parameter_schema", optional_id(self.parameter_schema)),
            ("parameter_defaults", canonical(&self.parameter_defaults)),
            ("output_schema", id(&self.output_schema)),
            ("definition", definition),
        ])
    }
}

fn remote_source(value: &RemoteSourceRecord) -> Value {
    object([("url", json!(value.url)), ("sha256", digest(&value.sha256))])
}

impl IdentityRecord for ProblemDefinitionRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("name", json!(self.name.as_str())),
            ("contract", id(&self.contract)),
            ("parameter_defaults", canonical(&self.parameter_defaults)),
            ("evaluator", worker(&self.evaluator)),
        ])
    }
}

impl IdentityRecord for ImplementationDefinitionRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        let mut contracts = self
            .problem_contracts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        contracts.sort();
        let mut capabilities = self
            .capabilities
            .iter()
            .copied()
            .map(implementation_capability)
            .collect::<Vec<_>>();
        capabilities.sort_unstable();
        fields([
            ("name", json!(self.name.as_str())),
            ("worker", worker(&self.worker)),
            ("problem_contracts", json!(contracts)),
            ("parameter_schema", optional_id(self.parameter_schema)),
            ("parameter_defaults", canonical(&self.parameter_defaults)),
            ("capabilities", json!(capabilities)),
        ])
    }
}

impl IdentityRecord for ExecutionPolicyRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("name", json!(self.name.as_str())),
            ("cpus", json!(self.cpus)),
            ("threads", json!(self.threads)),
            ("memory", optional_string(&self.memory)),
            ("network", json!(self.network)),
            ("worker_reuse", json!(self.worker_reuse)),
            ("warmup_runs", json!(self.warmup_runs)),
            ("timeout", optional_string(&self.timeout)),
            ("timing_scope", json!(timing_scope(self.timing_scope))),
            ("primary_time", json!(primary_time(self.primary_time))),
            ("run_order", json!(run_order(self.run_order))),
            ("enforcement", json!(enforcement(self.enforcement))),
        ])
    }
}

impl IdentityRecord for ObservationPolicyRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("name", json!(self.name.as_str())),
            (
                "kind",
                json!(match self.kind {
                    ObservationKind::OneShot => "one_shot",
                }),
            ),
        ])
    }
}

impl IdentityRecord for DatasetConfigurationRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("definition", id(&self.definition)),
            ("parameters", self.parameters.as_json().clone()),
            ("seed", seed(&self.seed)),
        ])
    }
}

impl IdentityRecord for ProblemConfigurationRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("definition", id(&self.definition)),
            ("parameters", self.parameters.as_json().clone()),
        ])
    }
}

impl IdentityRecord for ImplementationConfigurationRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("definition", id(&self.definition)),
            ("parameters", self.parameters.as_json().clone()),
            ("environment", id(&self.environment)),
        ])
    }
}

impl IdentityRecord for DatasetInstanceRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("configuration", id(&self.configuration)),
            ("artifact", id(&self.artifact)),
            ("output_schema", id(&self.output_schema)),
        ])
    }
}

impl IdentityRecord for ProblemInstanceRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("dataset", id(&self.dataset)),
            ("configuration", id(&self.configuration)),
        ])
    }
}

impl IdentityRecord for LogicalCandidateRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("dataset_configuration", id(&self.dataset_configuration)),
            ("problem_configuration", id(&self.problem_configuration)),
            (
                "implementation_configuration",
                id(&self.implementation_configuration),
            ),
            ("execution_policy", id(&self.execution_policy)),
            ("observation_policy", id(&self.observation_policy)),
        ])
    }
}

impl IdentityRecord for OneShotLogicalSpecificationRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("kind", json!("one_shot")),
            ("candidate", id(&self.candidate)),
            (
                "scientific_budget",
                json!(self.scientific_budget.to_string()),
            ),
            (
                "implementation_repetition",
                json!(self.implementation_repetition),
            ),
            ("implementation_seed", seed(&self.implementation_seed)),
        ])
    }
}

impl IdentityRecord for LogicalObservationSlotRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("specification", id(&self.specification)),
            ("measurement_index", json!(self.measurement_index)),
        ])
    }
}

impl IdentityRecord for LogicalAttemptSlotRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        let role = match self.role {
            AttemptSlotRole::Warmup { warmup_index } => object([
                ("kind", json!("warmup")),
                ("warmup_index", json!(warmup_index)),
            ]),
            AttemptSlotRole::Measured { observation_slot } => object([
                ("kind", json!("measured")),
                ("observation_slot", id(&observation_slot)),
            ]),
        };
        fields([
            ("specification", id(&self.specification)),
            ("role", role),
            ("scheduling_priority", digest(&self.scheduling_priority)),
        ])
    }
}

impl IdentityRecord for ResolvedEnvironmentRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("definition", id(&self.definition)),
            ("fingerprint", self.fingerprint.as_json().clone()),
        ])
    }
}

impl IdentityRecord for ResolvedLaunchRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        let environment_variables = self
            .environment_variables
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        fields([
            ("environment", id(&self.environment)),
            ("program", json!(self.program)),
            ("args", json!(self.args)),
            (
                "environment_variables",
                Value::Object(environment_variables),
            ),
            (
                "working_directory",
                json!(match self.working_directory {
                    WorkerWorkingDirectory::PrivateRunDirectory => "private_run_directory",
                }),
            ),
        ])
    }
}

impl IdentityRecord for ExecutorDefinitionRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("kind", json!(self.kind.as_str())),
            ("configuration", self.configuration.as_json().clone()),
        ])
    }
}

impl IdentityRecord for ResolvedWorkerRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        as_fields(resolved_worker(self))
    }
}

impl IdentityRecord for ResolvedOneShotSpecificationRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("kind", json!("one_shot")),
            ("logical_specification", id(&self.logical_specification)),
            ("problem_instance", id(&self.problem_instance)),
            ("implementation", resolved_worker(&self.implementation)),
            ("evaluator", resolved_worker(&self.evaluator)),
            ("executor", id(&self.executor)),
            ("wire_protocol_version", json!(self.wire_protocol_version)),
            (
                "execution_semantics_version",
                json!(self.execution_semantics_version),
            ),
        ])
    }
}

impl IdentityRecord for ResolvedObservationSlotRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("logical_slot", id(&self.logical_slot)),
            ("specification", id(&self.specification)),
        ])
    }
}

impl IdentityRecord for ResolvedAttemptSlotRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("logical_slot", id(&self.logical_slot)),
            ("specification", id(&self.specification)),
        ])
    }
}

impl IdentityRecord for RunObservationRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        fields([
            ("slot", id(&self.slot)),
            ("producing_attempt", id(&self.producing_attempt)),
            ("result", artifact(&self.result)),
            ("metrics", artifact(&self.metrics)),
            ("timing", timing(&self.timing)),
            (
                "completion",
                json!(match self.completion {
                    CompletionStatus::NotRequired => "not_required",
                    CompletionStatus::Reached => "reached",
                    CompletionStatus::NotReached => "not_reached",
                }),
            ),
        ])
    }
}

impl IdentityRecord for RunAttemptRecord {
    fn identity_fields(&self) -> Map<String, Value> {
        // Attempts must be addressable before execution, and including the
        // outcome would form a cycle with the observation's producing-attempt
        // identity. Terminal state remains data associated with this retry ID.
        fields([
            ("slot", id(&self.slot)),
            ("retry_index", json!(self.retry_index)),
        ])
    }
}

fn fields<const N: usize>(values: [(&str, Value); N]) -> Map<String, Value> {
    match object(values) {
        Value::Object(fields) => fields,
        _ => unreachable!(),
    }
}

fn as_fields(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(fields) => fields,
        _ => unreachable!(),
    }
}
