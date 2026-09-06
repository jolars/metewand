//! Typed parsing for version-1 benchmark manifests.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Deserializer, de};
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use thiserror::Error;

use crate::{MANIFEST_VERSION, canonical::CanonicalValue};

/// Parses a version-1 TOML manifest into core domain types.
///
/// `source_path` identifies the document in diagnostics and does not cause any
/// filesystem access. Parsing rejects unknown fields at every Metewand-owned
/// object boundary.
///
/// # Errors
///
/// Returns a path-aware, source-spanned diagnostic for malformed TOML or a
/// value that cannot be represented by the version-1 manifest types.
pub fn parse_manifest(source_path: &Path, source: &str) -> Result<Manifest, ManifestParseError> {
    toml::from_str(source).map_err(|error: toml::de::Error| {
        let byte_span = error.span().unwrap_or(source.len()..source.len());
        ManifestParseError {
            message: error.message().to_owned(),
            source_span: SourceSpan::from_byte_range(source_path, source, byte_span),
        }
    })
}

/// A parsed version-1 benchmark manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Manifest compatibility version.
    #[serde(deserialize_with = "deserialize_version_one")]
    pub version: u32,
    /// Benchmark name.
    pub name: Name,
    /// Dataset definitions keyed by manifest-local name.
    #[serde(default)]
    pub datasets: BTreeMap<Name, DatasetDefinition>,
    /// Problem definitions keyed by manifest-local name.
    #[serde(default)]
    pub problems: BTreeMap<Name, ProblemDefinition>,
    /// Implementation definitions keyed by manifest-local name.
    #[serde(default)]
    pub implementations: BTreeMap<Name, ImplementationDefinition>,
    /// Environment definitions keyed by manifest-local name.
    #[serde(default)]
    pub environments: BTreeMap<Name, EnvironmentDefinition>,
    /// Experiments in their declared order.
    #[serde(default)]
    pub experiments: Vec<ExperimentDefinition>,
    /// Execution policies keyed by manifest-local name.
    #[serde(default)]
    pub execution_policies: BTreeMap<Name, ExecutionPolicyDefinition>,
    /// Observation policies keyed by manifest-local name.
    #[serde(default)]
    pub observation_policies: BTreeMap<Name, ObservationPolicyDefinition>,
}

/// A dataset source or materializer definition.
#[derive(Debug)]
pub enum DatasetDefinition {
    /// A repository-owned, fixed source bundle.
    Fixed(FixedDatasetDefinition),
    /// A dataset produced by a worker, optionally from a pinned remote source.
    Generated(GeneratedDatasetDefinition),
}

/// A repository-owned fixed dataset.
#[derive(Debug)]
pub struct FixedDatasetDefinition {
    /// Schema describing the materialized dataset.
    pub output_schema: RepositoryPath,
    /// Files and patterns that constitute the dataset.
    pub sources: Vec<RepositoryPath>,
}

/// A dataset produced by a materializer worker.
#[derive(Debug)]
pub struct GeneratedDatasetDefinition {
    /// Optional schema for dataset parameters.
    pub parameter_schema: Option<RepositoryPath>,
    /// Literal defaults merged before parameter validation.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Schema describing the materialized dataset.
    pub output_schema: RepositoryPath,
    /// Optional pinned remote input.
    pub source: Option<RemoteSource>,
    /// Worker that materializes the dataset.
    pub worker: WorkerDefinition,
}

/// A problem contract and its independent evaluator.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProblemDefinition {
    /// Repository-relative problem-contract path.
    pub contract: RepositoryPath,
    /// Literal problem-parameter defaults.
    #[serde(default, deserialize_with = "deserialize_parameter_defaults")]
    pub parameter_defaults: Option<CanonicalValue>,
    /// Worker that evaluates implementation results.
    pub evaluator: WorkerDefinition,
}

/// An implementation adapter and its declared contract support.
#[derive(Debug)]
pub struct ImplementationDefinition {
    /// Worker that runs the implementation.
    pub worker: WorkerDefinition,
    /// Manifest problem names implemented by this adapter.
    pub problem_contracts: Vec<Name>,
    /// Optional schema for implementation parameters.
    pub parameter_schema: Option<RepositoryPath>,
    /// Literal implementation-parameter defaults.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Capabilities declared by the implementation.
    pub capabilities: Vec<ImplementationCapability>,
}

/// A software environment definition.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum EnvironmentDefinition {
    /// The existing host software context.
    Local {},
    /// A Python environment resolved from a uv project and lockfile.
    Uv {
        /// Project root.
        project: RepositoryRootPath,
        /// uv lockfile.
        lockfile: RepositoryPath,
    },
    /// An R environment resolved from an renv project and lockfile.
    Renv {
        /// Project root.
        project: RepositoryRootPath,
        /// renv lockfile.
        lockfile: RepositoryPath,
    },
    /// A Nix flake output.
    Nix {
        /// Flake root.
        flake: RepositoryRootPath,
        /// Nix installable selected from the flake.
        #[serde(deserialize_with = "deserialize_nonempty_string")]
        installable: String,
        /// Nix system identifier.
        #[serde(deserialize_with = "deserialize_nonempty_string")]
        system: String,
        /// Whether the output is a package or an app.
        output_kind: NixOutputKind,
        /// Selected Nix output name.
        #[serde(deserialize_with = "deserialize_nonempty_string")]
        output: String,
    },
    /// A content-addressed OCI image.
    Oci {
        /// Image reference ending in a SHA-256 digest.
        image: OciImageReference,
    },
}

/// The kind of Nix output exposed as an environment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NixOutputKind {
    /// A package output whose selected program is resolved separately.
    Package,
    /// An app output with flake app metadata.
    App,
}

/// One experiment before deterministic matrix expansion.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentDefinition {
    /// Experiment name.
    pub name: Name,
    /// Selected problem name.
    pub problem: Name,
    /// Selected dataset names; empty denotes a dataset-free problem.
    #[serde(default, deserialize_with = "deserialize_optional_unique_names")]
    pub datasets: Vec<Name>,
    /// Selected implementation names.
    #[serde(deserialize_with = "deserialize_nonempty_unique_names")]
    pub implementations: Vec<Name>,
    /// Selected execution-policy name.
    pub execution_policy: Name,
    /// Selected observation-policy name.
    pub observation_policy: Name,
    /// Number of independently seeded implementation replications.
    #[serde(deserialize_with = "deserialize_safe_positive_integer")]
    pub implementation_repetitions: u64,
    /// Number of measurements for each logical run.
    #[serde(deserialize_with = "deserialize_safe_positive_integer")]
    pub measurement_repetitions: u64,
    /// Root experiment seed.
    #[serde(deserialize_with = "deserialize_safe_nonnegative_integer")]
    pub seed: u64,
    /// Dataset-owned parameter axes, keyed first by dataset name.
    #[serde(default)]
    pub dataset_parameters: NamedParameterAxes,
    /// Problem-owned parameter axes.
    #[serde(default)]
    pub problem_parameters: ParameterAxes,
    /// Implementation-owned parameter axes, keyed first by implementation name.
    #[serde(default)]
    pub implementation_parameters: NamedParameterAxes,
    /// Named overlays on the experiment-level axes.
    #[serde(default, deserialize_with = "deserialize_optional_cases")]
    pub cases: Vec<ExperimentCase>,
}

/// A named parameter-axis overlay within an experiment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentCase {
    /// Case name.
    pub name: Name,
    /// Dataset-owned parameter axes.
    #[serde(default)]
    pub dataset_parameters: NamedParameterAxes,
    /// Problem-owned parameter axes.
    #[serde(default)]
    pub problem_parameters: ParameterAxes,
    /// Implementation-owned parameter axes.
    #[serde(default)]
    pub implementation_parameters: NamedParameterAxes,
}

/// Parameter axes keyed by parameter name.
pub type ParameterAxes = BTreeMap<String, ParameterAxis>;

/// Parameter axes keyed first by definition name and then by parameter name.
pub type NamedParameterAxes = BTreeMap<Name, ParameterAxes>;

/// A fixed parameter value or an explicit expansion grid.
#[derive(Debug)]
pub enum ParameterAxis {
    /// One literal value.
    Value(CanonicalValue),
    /// One or more values to include in matrix expansion.
    Grid(Vec<CanonicalValue>),
}

/// Resource, timing, and scheduling settings selected by an experiment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPolicyDefinition {
    /// Execution-policy compatibility version.
    #[serde(deserialize_with = "deserialize_version_one")]
    pub version: u32,
    /// Requested CPU count.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_safe_positive_integer"
    )]
    pub cpus: Option<u64>,
    /// Requested thread count.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_safe_positive_integer"
    )]
    pub threads: Option<u64>,
    /// Requested memory limit.
    #[serde(default, deserialize_with = "deserialize_optional_nonempty_string")]
    pub memory: Option<String>,
    /// Whether network access is requested.
    pub network: Option<bool>,
    /// Whether a worker may serve more than one operation.
    pub worker_reuse: Option<bool>,
    /// Number of untimed warm-up runs.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_safe_nonnegative_integer"
    )]
    pub warmup_runs: Option<u64>,
    /// Operational timeout expression.
    #[serde(default, deserialize_with = "deserialize_optional_nonempty_string")]
    pub timeout: Option<String>,
    /// Phases included in the primary timing measurement.
    pub timing_scope: Option<TimingScope>,
    /// Primary executor-observed time measure.
    pub primary_time: Option<PrimaryTime>,
    /// Deterministic run-order policy.
    pub run_order: Option<RunOrder>,
    /// Required control-enforcement level.
    pub enforcement: Option<Enforcement>,
}

impl ExecutionPolicyDefinition {
    /// Version-1 default for worker network access.
    pub const DEFAULT_NETWORK: bool = false;
    /// Version-1 default for process reuse.
    pub const DEFAULT_WORKER_REUSE: bool = false;
    /// Version-1 default number of process-local warm-ups.
    pub const DEFAULT_WARMUP_RUNS: u64 = 0;
    /// Version-1 default timing boundary.
    pub const DEFAULT_TIMING_SCOPE: TimingScope = TimingScope::PrepareAndExecute;
    /// Version-1 default authoritative time measure.
    pub const DEFAULT_PRIMARY_TIME: PrimaryTime = PrimaryTime::TimedWallTime;
    /// Version-1 default scheduling order.
    pub const DEFAULT_RUN_ORDER: RunOrder = RunOrder::Sequential;
    /// Version-1 default control-enforcement requirement.
    pub const DEFAULT_ENFORCEMENT: Enforcement = Enforcement::BestEffort;

    /// Returns network access after applying the version-1 default.
    #[must_use]
    pub fn resolved_network(&self) -> bool {
        self.network.unwrap_or(Self::DEFAULT_NETWORK)
    }

    /// Returns process reuse after applying the version-1 default.
    #[must_use]
    pub fn resolved_worker_reuse(&self) -> bool {
        self.worker_reuse.unwrap_or(Self::DEFAULT_WORKER_REUSE)
    }

    /// Returns the warm-up count after applying the version-1 default.
    #[must_use]
    pub fn resolved_warmup_runs(&self) -> u64 {
        self.warmup_runs.unwrap_or(Self::DEFAULT_WARMUP_RUNS)
    }

    /// Returns the timing scope after applying the version-1 default.
    #[must_use]
    pub fn resolved_timing_scope(&self) -> TimingScope {
        self.timing_scope.unwrap_or(Self::DEFAULT_TIMING_SCOPE)
    }

    /// Returns the primary time measure after applying the version-1 default.
    #[must_use]
    pub fn resolved_primary_time(&self) -> PrimaryTime {
        self.primary_time.unwrap_or(Self::DEFAULT_PRIMARY_TIME)
    }

    /// Returns the scheduling order after applying the version-1 default.
    #[must_use]
    pub fn resolved_run_order(&self) -> RunOrder {
        self.run_order.unwrap_or(Self::DEFAULT_RUN_ORDER)
    }

    /// Returns the enforcement requirement after applying the version-1 default.
    #[must_use]
    pub fn resolved_enforcement(&self) -> Enforcement {
        self.enforcement.unwrap_or(Self::DEFAULT_ENFORCEMENT)
    }
}

/// Phases included in an execution timing measurement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum TimingScope {
    /// Worker startup, preparation, execution, and result serialization.
    ColdEndToEnd,
    /// Preparation, execution, and result serialization on a started worker.
    PrepareAndExecute,
    /// Only the implementation's execute operation and result serialization.
    ExecuteOnly,
}

impl fmt::Display for TimingScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ColdEndToEnd => "cold_end_to_end",
            Self::PrepareAndExecute => "prepare_and_execute",
            Self::ExecuteOnly => "execute_only",
        })
    }
}

/// An execution policy's primary time measure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PrimaryTime {
    /// Executor-observed elapsed wall time.
    TimedWallTime,
    /// Executor-observed process-tree CPU time.
    CpuTime,
}

/// Ordering of logical run blocks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunOrder {
    /// Declared deterministic order.
    Sequential,
    /// Deterministically randomized order.
    Randomized,
}

/// Required enforcement strength for execution controls.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    /// Record unavailable controls without failing preflight.
    BestEffort,
    /// Fail preflight when a requested control cannot be enforced.
    Required,
}

/// The version-1 one-shot observation policy.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationPolicyDefinition {
    /// Observation-policy compatibility version.
    #[serde(deserialize_with = "deserialize_version_one")]
    pub version: u32,
    /// Version-1 observation kind.
    #[serde(deserialize_with = "deserialize_one_shot_kind")]
    pub kind: ObservationKind,
}

/// A supported observation-policy kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    /// Produce one final result without a scientific budget.
    OneShot,
}

impl fmt::Display for ObservationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OneShot => "one_shot",
        })
    }
}

/// A worker process definition.
#[derive(Debug)]
pub enum WorkerDefinition {
    /// A worker launched through a language runner.
    Interpreted(InterpretedWorker),
    /// A worker launched as a raw command.
    Command(CommandWorker),
}

impl WorkerDefinition {
    /// Returns the worker's launch convention.
    #[must_use]
    pub fn runner(&self) -> Runner {
        match self {
            Self::Interpreted(worker) => worker.runner.into(),
            Self::Command(_) => Runner::Command,
        }
    }
}

/// An interpreted-language worker.
#[derive(Debug)]
pub struct InterpretedWorker {
    /// Language launch convention.
    pub runner: InterpretedRunner,
    /// Repository-relative worker entrypoint.
    pub entrypoint: RepositoryPath,
    /// Additional worker arguments.
    pub args: Vec<String>,
    /// Declared source bundle.
    pub sources: Vec<RepositoryPath>,
    /// Selected environment name.
    pub environment: Name,
    /// Optional protocol transport override.
    pub protocol_transport: Option<ProtocolTransport>,
}

/// A raw command worker.
#[derive(Debug)]
pub struct CommandWorker {
    /// Program resolved from the selected environment.
    pub program: String,
    /// Additional worker arguments.
    pub args: Vec<String>,
    /// Optional repository-owned source bundle.
    pub sources: Option<Vec<RepositoryPath>>,
    /// Selected environment name.
    pub environment: Name,
    /// Optional protocol transport override.
    pub protocol_transport: Option<ProtocolTransport>,
}

/// A worker launch convention.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    /// Python SDK runner.
    Python,
    /// R SDK runner.
    R,
    /// Julia SDK runner.
    Julia,
    /// Raw command runner.
    Command,
}

/// An interpreted-language launch convention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterpretedRunner {
    /// Python SDK runner.
    Python,
    /// R SDK runner.
    R,
    /// Julia SDK runner.
    Julia,
}

impl From<InterpretedRunner> for Runner {
    fn from(runner: InterpretedRunner) -> Self {
        match runner {
            InterpretedRunner::Python => Self::Python,
            InterpretedRunner::R => Self::R,
            InterpretedRunner::Julia => Self::Julia,
        }
    }
}

/// Transport used for protocol messages.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolTransport {
    /// Dedicated protocol pipes.
    Pipes,
    /// Standard input and output.
    Stdio,
}

/// An implementation capability admitted by manifest version 1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationCapability {
    /// One request produces one final result.
    OneShot,
}

/// A pinned remote input to a dataset materializer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSource {
    /// Remote resource URI.
    #[serde(deserialize_with = "deserialize_uri_string")]
    pub url: String,
    /// Expected content digest.
    pub sha256: Sha256Digest,
}

/// A manifest-local component or parameter name.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Name(String);

impl Name {
    /// Returns the name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn builtin_unit() -> Self {
        Self("unit".to_owned())
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if valid_name(&value) {
            Ok(Self(value))
        } else {
            Err(de::Error::custom(format_args!(
                "name {value:?} must match ^[A-Za-z0-9][A-Za-z0-9._-]*$"
            )))
        }
    }
}

/// A normalized-looking repository-relative path awaiting repository checks.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RepositoryPath(PathBuf);

impl RepositoryPath {
    /// Returns the path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for RepositoryPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl<'de> Deserialize<'de> for RepositoryPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if valid_repository_path(&value) {
            Ok(Self(PathBuf::from(value)))
        } else {
            Err(de::Error::custom(format_args!(
                "path {value:?} must be a normalized, relative repository path"
            )))
        }
    }
}

/// A repository path that may name the repository root as `.`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryRootPath(PathBuf);

impl RepositoryRootPath {
    /// Returns the path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for RepositoryRootPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl<'de> Deserialize<'de> for RepositoryRootPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == "." || valid_repository_path(&value) {
            Ok(Self(PathBuf::from(value)))
        } else {
            Err(de::Error::custom(format_args!(
                "path {value:?} must be `.` or a normalized, relative repository path"
            )))
        }
    }
}

/// A lowercase SHA-256 digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Returns the 64 hexadecimal digits.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if valid_sha256(&value) {
            Ok(Self(value))
        } else {
            Err(de::Error::custom(
                "SHA-256 digest must contain 64 lowercase hexadecimal digits",
            ))
        }
    }
}

/// An OCI image reference pinned by a SHA-256 digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OciImageReference(String);

impl OciImageReference {
    /// Returns the pinned image reference.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OciImageReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let valid = value
            .rsplit_once("@sha256:")
            .is_some_and(|(_, digest)| valid_sha256(digest));
        if valid {
            Ok(Self(value))
        } else {
            Err(de::Error::custom(
                "OCI image must end in `@sha256:` and 64 lowercase hexadecimal digits",
            ))
        }
    }
}

/// A one-indexed source range attached to a diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpan {
    /// Logical repository path supplied by the parser's caller.
    pub path: PathBuf,
    /// One-indexed start line.
    pub line: usize,
    /// One-indexed start column, counted in Unicode scalar values.
    pub column: usize,
    /// One-indexed end line.
    pub end_line: usize,
    /// One-indexed, exclusive end column, counted in Unicode scalar values.
    pub end_column: usize,
}

impl SourceSpan {
    pub(crate) fn from_byte_range(
        path: &Path,
        source: &str,
        range: std::ops::Range<usize>,
    ) -> Self {
        let start = source_position(source, range.start);
        let end = source_position(source, range.end);
        Self {
            path: path.to_path_buf(),
            line: start.0,
            column: start.1,
            end_line: end.0,
            end_column: end.1,
        }
    }
}

/// A TOML syntax or typed-manifest decoding failure.
#[derive(Debug, Error)]
#[error("failed to parse manifest at `{}`: {message}", source_span.path.display())]
pub struct ManifestParseError {
    message: String,
    source_span: SourceSpan,
}

impl ManifestParseError {
    /// Returns the decoder's human-readable explanation.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the location attributed by the TOML decoder.
    #[must_use]
    pub fn source_span(&self) -> Option<&SourceSpan> {
        Some(&self.source_span)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDatasetDefinition {
    parameter_schema: Option<RepositoryPath>,
    #[serde(default, deserialize_with = "deserialize_parameter_defaults")]
    parameter_defaults: Option<CanonicalValue>,
    output_schema: RepositoryPath,
    source: Option<RemoteSource>,
    runner: Option<Runner>,
    entrypoint: Option<RepositoryPath>,
    program: Option<String>,
    args: Option<Vec<String>>,
    sources: Option<Vec<RepositoryPath>>,
    environment: Option<Name>,
    protocol_transport: Option<ProtocolTransport>,
}

impl<'de> Deserialize<'de> for DatasetDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawDatasetDefinition::deserialize(deserializer)?;
        match raw.runner {
            None => {
                if raw.parameter_schema.is_some()
                    || raw.parameter_defaults.is_some()
                    || raw.source.is_some()
                    || raw.entrypoint.is_some()
                    || raw.program.is_some()
                    || raw.args.is_some()
                    || raw.environment.is_some()
                    || raw.protocol_transport.is_some()
                {
                    return Err(de::Error::custom(
                        "a fixed dataset accepts only `output_schema` and `sources`",
                    ));
                }
                let sources = required_source_bundle(raw.sources, "fixed dataset")
                    .map_err(de::Error::custom)?;
                Ok(Self::Fixed(FixedDatasetDefinition {
                    output_schema: raw.output_schema,
                    sources,
                }))
            }
            Some(runner) => {
                let worker = build_worker(
                    runner,
                    raw.entrypoint,
                    raw.program,
                    raw.args.unwrap_or_default(),
                    raw.sources,
                    raw.environment,
                    raw.protocol_transport,
                )
                .map_err(de::Error::custom)?;
                Ok(Self::Generated(GeneratedDatasetDefinition {
                    parameter_schema: raw.parameter_schema,
                    parameter_defaults: raw.parameter_defaults,
                    output_schema: raw.output_schema,
                    source: raw.source,
                    worker,
                }))
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImplementationDefinition {
    runner: Runner,
    entrypoint: Option<RepositoryPath>,
    program: Option<String>,
    args: Option<Vec<String>>,
    sources: Option<Vec<RepositoryPath>>,
    environment: Option<Name>,
    protocol_transport: Option<ProtocolTransport>,
    problem_contracts: Vec<Name>,
    parameter_schema: Option<RepositoryPath>,
    #[serde(default, deserialize_with = "deserialize_parameter_defaults")]
    parameter_defaults: Option<CanonicalValue>,
    capabilities: Vec<ImplementationCapability>,
}

impl<'de> Deserialize<'de> for ImplementationDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawImplementationDefinition::deserialize(deserializer)?;
        ensure_nonempty_unique(&raw.problem_contracts, "`problem_contracts`")
            .map_err(de::Error::custom)?;
        if raw.capabilities != [ImplementationCapability::OneShot] {
            return Err(de::Error::custom(
                "version 1 requires `capabilities = [\"one_shot\"]`",
            ));
        }
        let worker = build_worker(
            raw.runner,
            raw.entrypoint,
            raw.program,
            raw.args.unwrap_or_default(),
            raw.sources,
            raw.environment,
            raw.protocol_transport,
        )
        .map_err(de::Error::custom)?;
        Ok(Self {
            worker,
            problem_contracts: raw.problem_contracts,
            parameter_schema: raw.parameter_schema,
            parameter_defaults: raw.parameter_defaults,
            capabilities: raw.capabilities,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkerDefinition {
    runner: Runner,
    entrypoint: Option<RepositoryPath>,
    program: Option<String>,
    args: Option<Vec<String>>,
    sources: Option<Vec<RepositoryPath>>,
    environment: Option<Name>,
    protocol_transport: Option<ProtocolTransport>,
}

impl<'de> Deserialize<'de> for WorkerDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawWorkerDefinition::deserialize(deserializer)?;
        build_worker(
            raw.runner,
            raw.entrypoint,
            raw.program,
            raw.args.unwrap_or_default(),
            raw.sources,
            raw.environment,
            raw.protocol_transport,
        )
        .map_err(de::Error::custom)
    }
}

fn build_worker(
    runner: Runner,
    entrypoint: Option<RepositoryPath>,
    program: Option<String>,
    args: Vec<String>,
    sources: Option<Vec<RepositoryPath>>,
    environment: Option<Name>,
    protocol_transport: Option<ProtocolTransport>,
) -> Result<WorkerDefinition, String> {
    let environment = environment.ok_or_else(|| "worker is missing `environment`".to_owned())?;
    match runner {
        Runner::Python | Runner::R | Runner::Julia => {
            if program.is_some() {
                return Err("an interpreted worker cannot declare `program`".to_owned());
            }
            let entrypoint = entrypoint
                .ok_or_else(|| "an interpreted worker is missing `entrypoint`".to_owned())?;
            let sources = required_source_bundle(sources, "interpreted worker")?;
            let runner = match runner {
                Runner::Python => InterpretedRunner::Python,
                Runner::R => InterpretedRunner::R,
                Runner::Julia => InterpretedRunner::Julia,
                Runner::Command => unreachable!(),
            };
            Ok(WorkerDefinition::Interpreted(InterpretedWorker {
                runner,
                entrypoint,
                args,
                sources,
                environment,
                protocol_transport,
            }))
        }
        Runner::Command => {
            if entrypoint.is_some() {
                return Err("a command worker cannot declare `entrypoint`".to_owned());
            }
            let program =
                program.ok_or_else(|| "a command worker is missing `program`".to_owned())?;
            if program.is_empty() {
                return Err("a command worker's `program` cannot be empty".to_owned());
            }
            if let Some(paths) = &sources {
                validate_source_bundle(paths, "command worker")?;
            }
            Ok(WorkerDefinition::Command(CommandWorker {
                program,
                args,
                sources,
                environment,
                protocol_transport,
            }))
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawParameterAxis {
    Value(RawValueAxis),
    Grid(RawGridAxis),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawValueAxis {
    #[serde(deserialize_with = "deserialize_manifest_value")]
    value: CanonicalValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGridAxis {
    #[serde(deserialize_with = "deserialize_manifest_values")]
    grid: Vec<CanonicalValue>,
}

impl<'de> Deserialize<'de> for ParameterAxis {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RawParameterAxis::deserialize(deserializer)? {
            RawParameterAxis::Value(axis) => Ok(Self::Value(axis.value)),
            RawParameterAxis::Grid(axis) if axis.grid.is_empty() => {
                Err(de::Error::custom("a parameter `grid` cannot be empty"))
            }
            RawParameterAxis::Grid(axis) => Ok(Self::Grid(axis.grid)),
        }
    }
}

pub(crate) fn deserialize_version_one<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == MANIFEST_VERSION {
        Ok(version)
    } else {
        Err(de::Error::custom(format_args!(
            "expected version 1, found version {version}"
        )))
    }
}

fn deserialize_one_shot_kind<'de, D>(deserializer: D) -> Result<ObservationKind, D::Error>
where
    D: Deserializer<'de>,
{
    let kind = String::deserialize(deserializer)?;
    if kind == "one_shot" {
        Ok(ObservationKind::OneShot)
    } else {
        Err(de::Error::custom(format_args!(
            "version 1 supports only the `one_shot` observation kind, found {kind:?}"
        )))
    }
}

fn deserialize_parameter_defaults<'de, D>(
    deserializer: D,
) -> Result<Option<CanonicalValue>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = deserialize_manifest_value(deserializer)?;
    if value.as_json().is_object() {
        Ok(Some(value))
    } else {
        Err(de::Error::custom("`parameter_defaults` must be an object"))
    }
}

pub(crate) fn deserialize_manifest_value<'de, D>(
    deserializer: D,
) -> Result<CanonicalValue, D::Error>
where
    D: Deserializer<'de>,
{
    let value = toml::Value::deserialize(deserializer)?;
    let value = toml_to_json(value).map_err(de::Error::custom)?;
    CanonicalValue::try_from(value).map_err(de::Error::custom)
}

fn deserialize_manifest_values<'de, D>(deserializer: D) -> Result<Vec<CanonicalValue>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<toml::Value>::deserialize(deserializer)?;
    values
        .into_iter()
        .map(|value| {
            toml_to_json(value)
                .and_then(|value| {
                    CanonicalValue::try_from(value).map_err(|error| error.to_string())
                })
                .map_err(de::Error::custom)
        })
        .collect()
}

fn toml_to_json(value: toml::Value) -> Result<JsonValue, String> {
    match value {
        toml::Value::String(value) => Ok(JsonValue::String(value)),
        toml::Value::Integer(value) => Ok(JsonValue::Number(value.into())),
        toml::Value::Float(value) => JsonNumber::from_f64(value)
            .map(JsonValue::Number)
            .ok_or_else(|| {
                format!("TOML number {value} is outside the version-1 Metewand JSON domain")
            }),
        toml::Value::Boolean(value) => Ok(JsonValue::Bool(value)),
        toml::Value::Datetime(_) => Err(
            "TOML date and time values are outside the version-1 Metewand JSON domain".to_owned(),
        ),
        toml::Value::Array(values) => values
            .into_iter()
            .map(toml_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        toml::Value::Table(values) => values
            .into_iter()
            .map(|(key, value)| toml_to_json(value).map(|value| (key, value)))
            .collect::<Result<JsonMap<_, _>, _>>()
            .map(JsonValue::Object),
    }
}

fn deserialize_safe_positive_integer<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = deserialize_safe_nonnegative_integer(deserializer)?;
    if value == 0 {
        Err(de::Error::custom("expected a positive safe integer"))
    } else {
        Ok(value)
    }
}

fn deserialize_safe_nonnegative_integer<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value <= crate::canonical::MAX_SAFE_INTEGER {
        Ok(value)
    } else {
        Err(de::Error::custom(format_args!(
            "integer {value} exceeds the version-1 safe-integer maximum"
        )))
    }
}

fn deserialize_optional_safe_positive_integer<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_safe_positive_integer(deserializer).map(Some)
}

fn deserialize_optional_safe_nonnegative_integer<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_safe_nonnegative_integer(deserializer).map(Some)
}

fn deserialize_nonempty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Err(de::Error::custom("string cannot be empty"))
    } else {
        Ok(value)
    }
}

fn deserialize_optional_nonempty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_nonempty_string(deserializer).map(Some)
}

fn deserialize_uri_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    fluent_uri::Uri::parse(value.as_str())
        .map_err(|error| de::Error::custom(format_args!("invalid URI: {error}")))?;
    Ok(value)
}

fn deserialize_nonempty_unique_names<'de, D>(deserializer: D) -> Result<Vec<Name>, D::Error>
where
    D: Deserializer<'de>,
{
    let names = Vec::<Name>::deserialize(deserializer)?;
    ensure_nonempty_unique(&names, "name list").map_err(de::Error::custom)?;
    Ok(names)
}

fn deserialize_optional_unique_names<'de, D>(deserializer: D) -> Result<Vec<Name>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_nonempty_unique_names(deserializer)
}

fn deserialize_optional_cases<'de, D>(deserializer: D) -> Result<Vec<ExperimentCase>, D::Error>
where
    D: Deserializer<'de>,
{
    let cases = Vec::<ExperimentCase>::deserialize(deserializer)?;
    if cases.is_empty() {
        Err(de::Error::custom("`cases` cannot be empty when present"))
    } else {
        Ok(cases)
    }
}

pub(crate) fn ensure_nonempty_unique<T>(values: &[T], description: &str) -> Result<(), String>
where
    T: Ord,
{
    if values.is_empty() {
        return Err(format!("{description} cannot be empty"));
    }
    if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(format!("{description} cannot contain duplicates"));
    }
    Ok(())
}

fn required_source_bundle(
    sources: Option<Vec<RepositoryPath>>,
    description: &str,
) -> Result<Vec<RepositoryPath>, String> {
    let sources = sources.ok_or_else(|| format!("{description} is missing `sources`"))?;
    validate_source_bundle(&sources, description)?;
    Ok(sources)
}

fn validate_source_bundle(paths: &[RepositoryPath], description: &str) -> Result<(), String> {
    ensure_nonempty_unique(paths, &format!("{description} source bundle"))
}

fn valid_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_repository_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with(['/', '\\'])
        && !value.ends_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn source_position(source: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.chars().count() + 1);
    (line, column)
}
