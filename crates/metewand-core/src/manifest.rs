//! Typed parsing for version-1 benchmark manifests.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
};

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
};
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::{
    MANIFEST_VERSION,
    canonical::CanonicalValue,
    field_declaration::{ParameterBoundary, ParameterDeclarations},
    problem_contract::{ContractDeclaration, ProblemContract},
    schema::SchemaDeclaration,
};

/// Parses a version-1 YAML manifest into core domain types.
///
/// `source_path` is the logical repository document path used in diagnostics
/// and as the base for inline schema references. Parsing performs no filesystem
/// access and rejects unknown fields at every Metewand-owned object boundary.
///
/// # Errors
///
/// Returns a path-aware, source-spanned diagnostic for malformed YAML or a
/// value that cannot be represented by the version-1 manifest types.
pub fn parse_manifest(source_path: &Path, source: &str) -> Result<Manifest, ManifestParseError> {
    let (mut manifest, source_map): (Manifest, _) = crate::yaml::parse(source_path, source)
        .map_err(|error| ManifestParseError {
            message: error.message,
            source_span: error.source_span,
        })?;
    for dataset in manifest.datasets.values_mut() {
        match dataset {
            DatasetDefinition::Fixed(dataset) => dataset.output_schema.set_owner(source_path),
            DatasetDefinition::Generated(dataset) => {
                dataset.output_schema.set_owner(source_path);
                if let Some(schema) = &mut dataset.parameter_schema {
                    schema.set_owner(source_path);
                }
            }
        }
    }
    for implementation in manifest.implementations.values_mut() {
        if let Some(schema) = &mut implementation.parameter_schema {
            schema.set_owner(source_path);
        }
    }
    for policy in manifest.observation_policies.values_mut() {
        if let Some(profile) = &mut policy.profile {
            for binding in profile.implementations.values_mut() {
                if let Some(schema) = binding.schema_mut() {
                    schema.set_owner(source_path);
                }
            }
        }
    }
    for (name, problem) in &mut manifest.problems {
        if let ContractDeclaration::Inline(contract) = &mut problem.contract {
            contract.set_schema_owner(source_path);
            let mut pointer = format!("/problems/{}", crate::yaml::escape_pointer(name.as_str()));
            if source_map.contains(&format!("{pointer}/contract")) {
                pointer.push_str("/contract");
            }
            contract.source_map = source_map.subtree(&pointer);
        }
    }
    manifest.source_map = source_map;
    Ok(manifest)
}

/// A parsed version-1 benchmark manifest.
#[derive(Debug)]
pub struct Manifest {
    pub(crate) source_map: crate::yaml::SourceMap,
    /// Manifest compatibility version.
    pub version: u32,
    /// Benchmark name.
    pub name: Name,
    /// Dataset definitions keyed by manifest-local name.
    pub datasets: BTreeMap<Name, DatasetDefinition>,
    /// Problem definitions keyed by manifest-local name.
    pub problems: BTreeMap<Name, ProblemDefinition>,
    /// Implementation definitions keyed by manifest-local name.
    pub implementations: BTreeMap<Name, ImplementationDefinition>,
    /// Environment definitions keyed by manifest-local name.
    pub environments: BTreeMap<Name, EnvironmentDefinition>,
    /// Experiments in their declared order.
    pub experiments: Vec<ExperimentDefinition>,
    /// Execution policies keyed by manifest-local name.
    pub execution_policies: BTreeMap<Name, ExecutionPolicyDefinition>,
    /// Observation policies keyed by manifest-local name.
    pub observation_policies: BTreeMap<Name, ObservationPolicyDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawManifest {
    #[serde(deserialize_with = "deserialize_version_one")]
    version: u32,
    name: Name,
    #[serde(default)]
    datasets: BTreeMap<Name, DatasetDefinition>,
    #[serde(default, deserialize_with = "deserialize_problems")]
    problems: BTreeMap<Name, ProblemDefinition>,
    #[serde(default)]
    implementations: BTreeMap<Name, ImplementationDefinition>,
    #[serde(default)]
    environments: BTreeMap<Name, EnvironmentDefinition>,
    #[serde(default)]
    experiments: Vec<ExperimentDefinition>,
    #[serde(default)]
    execution_policies: BTreeMap<Name, ExecutionPolicyDefinition>,
    #[serde(default)]
    observation_policies: BTreeMap<Name, ObservationPolicyDefinition>,
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawManifest::deserialize(deserializer)?;
        let mut manifest = Self {
            source_map: crate::yaml::SourceMap::default(),
            version: raw.version,
            name: raw.name,
            datasets: raw.datasets,
            problems: raw.problems,
            implementations: raw.implementations,
            environments: raw.environments,
            experiments: raw.experiments,
            execution_policies: raw.execution_policies,
            observation_policies: raw.observation_policies,
        };
        manifest
            .resolve_local_environments()
            .map_err(de::Error::custom)?;
        manifest
            .resolve_authoring_defaults()
            .map_err(de::Error::custom)?;
        Ok(manifest)
    }
}

impl Manifest {
    fn resolve_authoring_defaults(&mut self) -> Result<(), String> {
        for experiment in &mut self.experiments {
            if experiment.execution_policy.as_str().is_empty() {
                let name = Name("@local".to_owned());
                self.execution_policies
                    .entry(name.clone())
                    .or_insert_with(|| {
                        serde_json::from_value(serde_json::json!({}))
                            .expect("built-in execution policy")
                    });
                experiment.execution_policy = name;
            }
            if experiment.observation_policy.as_str().is_empty() {
                let name = Name("@final".to_owned());
                self.observation_policies.entry(name.clone()).or_insert(
                    ObservationPolicyDefinition {
                        version: 1,
                        kind: ObservationKind::OneShot,
                        profile: None,
                    },
                );
                experiment.observation_policy = name;
            }
        }
        for implementation in self.implementations.values_mut() {
            if implementation.problem_contracts.is_empty() {
                if self.problems.len() != 1 {
                    return Err("a function implementation requires `problem-contracts` when there is not exactly one problem".to_owned());
                }
                implementation
                    .problem_contracts
                    .push(self.problems.keys().next().expect("one problem").clone());
            }
        }
        Ok(())
    }

    fn resolve_local_environments(&mut self) -> Result<(), String> {
        let mut workers = self
            .datasets
            .values_mut()
            .filter_map(|dataset| match dataset {
                DatasetDefinition::Generated(dataset) => Some(&mut dataset.worker),
                DatasetDefinition::Fixed(_) => None,
            })
            .chain(
                self.problems
                    .values_mut()
                    .map(|problem| &mut problem.evaluator),
            )
            .chain(
                self.implementations
                    .values_mut()
                    .map(|implementation| &mut implementation.worker),
            );
        for worker in &mut workers {
            let environment = match worker {
                WorkerDefinition::Interpreted(worker) => &mut worker.environment,
                WorkerDefinition::Command(worker) => &mut worker.environment,
            };
            // The empty name is private to decoding and cannot be authored.
            if !environment.as_str().is_empty() {
                continue;
            }
            if self.environments.is_empty() {
                self.environments
                    .insert(Name("local".to_owned()), EnvironmentDefinition::Local {});
            }
            let local = self
                .environments
                .iter()
                .filter_map(|(name, definition)| {
                    matches!(definition, EnvironmentDefinition::Local {}).then_some(name)
                })
                .collect::<Vec<_>>();
            if local.len() != 1 {
                return Err("an omitted worker `environment` requires exactly one local environment; select `environment` explicitly".to_owned());
            }
            *environment = local[0].clone();
        }
        Ok(())
    }
}

/// A dataset source or materializer definition.
#[derive(Debug)]
pub enum DatasetDefinition {
    /// A repository-owned, fixed source bundle.
    Fixed(FixedDatasetDefinition),
    /// A dataset produced by a worker, optionally from a pinned remote source.
    Generated(Box<GeneratedDatasetDefinition>),
}

/// A repository-owned fixed dataset.
#[derive(Debug)]
pub struct FixedDatasetDefinition {
    /// Schema describing the materialized dataset.
    pub output_schema: SchemaDeclaration,
    /// Files and patterns that constitute the dataset.
    pub sources: Vec<RepositoryPath>,
}

/// A dataset produced by a materializer worker.
#[derive(Debug)]
pub struct GeneratedDatasetDefinition {
    /// Optional schema for dataset parameters.
    pub parameter_schema: Option<SchemaDeclaration>,
    /// Literal defaults merged before parameter validation.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Default expansion axes, kept separate from the valid parameter domain.
    pub parameter_grids: ParameterAxes,
    /// Schema describing the materialized dataset.
    pub output_schema: SchemaDeclaration,
    /// Optional pinned remote input.
    pub source: Option<RemoteSource>,
    /// Worker that materializes the dataset.
    pub worker: WorkerDefinition,
}

/// A problem contract and its independent evaluator.
#[derive(Debug)]
pub struct ProblemDefinition {
    /// Repository-relative contract or normalized inline problem fields.
    pub contract: ContractDeclaration,
    /// Literal problem-parameter defaults.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Worker that evaluates implementation results.
    pub evaluator: WorkerDefinition,
}

impl ProblemDefinition {
    /// Returns literal defaults from the definition or its compact contract.
    ///
    /// Compatibility validation rejects competing declarations before planning.
    #[must_use]
    pub fn resolved_parameter_defaults<'a>(
        &'a self,
        contract: &'a ProblemContract,
    ) -> Option<&'a CanonicalValue> {
        self.parameter_defaults
            .as_ref()
            .or(contract.parameter_defaults.as_ref())
    }
}

/// An implementation adapter and its declared contract support.
#[derive(Debug)]
pub struct ImplementationDefinition {
    /// Worker that runs the implementation.
    pub worker: WorkerDefinition,
    /// Manifest problem names implemented by this adapter.
    pub problem_contracts: Vec<Name>,
    /// Optional schema for implementation parameters.
    pub parameter_schema: Option<SchemaDeclaration>,
    /// Literal implementation-parameter defaults.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Default expansion axes, kept separate from the valid parameter domain.
    pub parameter_grids: ParameterAxes,
    /// Capabilities declared by the implementation.
    pub capabilities: Vec<ImplementationCapability>,
}

/// A software environment definition.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "kebab-case",
    deny_unknown_fields
)]
pub enum EnvironmentDefinition {
    /// The existing host software context.
    Local {},
    /// A Python environment resolved from a uv project and lockfile.
    Uv {
        /// Project root.
        project: RepositoryRootPath,
        /// uv lockfile.
        lockfile: RepositoryPath,
        /// Dependency groups explicitly selected for installation.
        #[serde(default)]
        groups: BTreeSet<Name>,
        /// Optional dependencies explicitly selected for installation.
        #[serde(default)]
        extras: BTreeSet<Name>,
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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
    #[serde(default = "omitted_name")]
    pub execution_policy: Name,
    /// Selected observation-policy name.
    #[serde(default = "omitted_name")]
    pub observation_policy: Name,
    /// Number of independently seeded implementation replications.
    #[serde(
        default = "one_repetition",
        deserialize_with = "deserialize_safe_positive_integer"
    )]
    pub implementation_repetitions: u64,
    /// Number of measurements for each logical run.
    #[serde(
        default = "one_repetition",
        deserialize_with = "deserialize_safe_positive_integer"
    )]
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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
#[derive(Clone, Debug, PartialEq)]
pub enum ParameterAxis {
    /// One literal value.
    Value(CanonicalValue),
    /// One or more values to include in matrix expansion.
    Grid(Vec<CanonicalValue>),
}

/// Resource, timing, and scheduling settings selected by an experiment.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExecutionPolicyDefinition {
    /// Execution-policy compatibility version.
    #[serde(default = "version_one", deserialize_with = "deserialize_version_one")]
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
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    pub network: Option<bool>,
    /// Whether a worker may serve more than one operation.
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
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
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    pub timing_scope: Option<TimingScope>,
    /// Primary executor-observed time measure.
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    pub primary_time: Option<PrimaryTime>,
    /// Deterministic run-order policy.
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    pub run_order: Option<RunOrder>,
    /// Maximum number of attempts that may execute concurrently.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_safe_positive_integer"
    )]
    pub max_concurrency: Option<u64>,
    /// Required control-enforcement level.
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
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
    /// Default attempt limit keeps performance measurements serial.
    pub const DEFAULT_MAX_CONCURRENCY: u64 = 1;
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

    /// Returns the concurrency bound after applying the serial default.
    #[must_use]
    pub fn resolved_max_concurrency(&self) -> u64 {
        self.max_concurrency
            .unwrap_or(Self::DEFAULT_MAX_CONCURRENCY)
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

/// A one-shot or bounded profile observation policy.
#[derive(Debug)]
pub struct ObservationPolicyDefinition {
    pub version: u32,
    pub kind: ObservationKind,
    pub profile: Option<crate::profile::ProfileDefinition>,
}

impl<'de> Deserialize<'de> for ObservationPolicyDefinition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| de::Error::custom("observation policy must be an object"))?;
        let version = object.remove("version").unwrap_or(serde_json::json!(1));
        if version != serde_json::json!(1) {
            return Err(de::Error::custom("expected observation policy version 1"));
        }
        let kind = object
            .remove("kind")
            .ok_or_else(|| de::Error::missing_field("kind"))?;
        match kind.as_str() {
            Some("one_shot") if object.is_empty() => Ok(Self {
                version: 1,
                kind: ObservationKind::OneShot,
                profile: None,
            }),
            Some("profile") => {
                let bindings = object
                    .remove("implementations")
                    .ok_or_else(|| de::Error::missing_field("implementations"))?;
                let implementations: BTreeMap<Name, crate::profile::ProfileBinding> =
                    serde_json::from_value(bindings).map_err(de::Error::custom)?;
                let policy: crate::profile::ProfilePolicy =
                    serde_json::from_value(value).map_err(de::Error::custom)?;
                policy.validate().map_err(de::Error::custom)?;
                if implementations.is_empty() {
                    return Err(de::Error::custom("profile implementations cannot be empty"));
                }
                for binding in implementations.values() {
                    binding.validate(&policy).map_err(de::Error::custom)?;
                }
                Ok(Self {
                    version: 1,
                    kind: ObservationKind::Profile,
                    profile: Some(crate::profile::ProfileDefinition {
                        policy,
                        implementations,
                    }),
                })
            }
            _ => Err(de::Error::custom(
                "expected one_shot without profile fields, or profile with delivery bindings",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    OneShot,
    Profile,
}
impl fmt::Display for ObservationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OneShot => "one_shot",
            Self::Profile => "profile",
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
    pub(crate) fn is_function(&self) -> bool {
        matches!(self, Self::Command(worker) if
            worker.args.starts_with(&["-m".to_owned(), "metewand".to_owned()]) ||
            worker.args.get(2).is_some_and(|arg| arg == "metewand::serve_function()"))
    }

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
    /// Python package runner.
    Python,
    /// R package runner.
    R,
    /// Julia package runner.
    Julia,
    /// Raw command runner.
    Command,
}

/// An interpreted-language launch convention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterpretedRunner {
    /// Python package runner.
    Python,
    /// R package runner.
    R,
    /// Julia package runner.
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
    /// Authenticated loopback sockets for CLI-owned language adapters.
    Socket,
    /// Standard input and output.
    Stdio,
}

/// An implementation capability admitted by manifest version 1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationCapability {
    /// Runs independent executions for a finite observation-control schedule.
    FreshSequence,
    /// One request produces one final result.
    OneShot,
    /// Checks configuration-dependent support before measurement.
    Applicability,
    /// Produces a bounded local trace and selected snapshots.
    BufferedProfile,
    /// Exchanges independently evaluated checkpoints during one trajectory.
    StreamingProfile,
}

/// A pinned remote input to a dataset materializer.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
pub struct RepositoryPath(String);

impl RepositoryPath {
    /// Returns the path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// Returns the logical path without native separator conversion.
    #[must_use]
    pub fn as_str(&self) -> &str {
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
            Ok(Self(value))
        } else {
            Err(de::Error::custom(format_args!(
                "path {value:?} must be a normalized, relative repository path"
            )))
        }
    }
}

/// A repository path that may name the repository root as `.`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryRootPath(String);

impl RepositoryRootPath {
    /// Returns the path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// Returns the logical path without native separator conversion.
    #[must_use]
    pub fn as_str(&self) -> &str {
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
            Ok(Self(value))
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

/// A YAML syntax or typed-manifest decoding failure.
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

    /// Returns the location attributed by the YAML decoder.
    #[must_use]
    pub fn source_span(&self) -> Option<&SourceSpan> {
        Some(&self.source_span)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawDatasetDefinition {
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    parameters: Option<ParameterDeclarations>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    parameter_schema: Option<SchemaDeclaration>,
    #[serde(default, deserialize_with = "deserialize_parameter_defaults")]
    parameter_defaults: Option<CanonicalValue>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    output_schema: Option<SchemaDeclaration>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    function: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    source: Option<RemoteSource>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    runner: Option<Runner>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    entrypoint: Option<RepositoryPath>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    program: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    sources: Option<Vec<RepositoryPath>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    environment: Option<Name>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    protocol_transport: Option<ProtocolTransport>,
}

impl<'de> Deserialize<'de> for DatasetDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawDatasetDefinition::deserialize(deserializer)?;
        let function = raw.function;
        match (raw.runner, function) {
            (None, None) => {
                if raw.parameter_schema.is_some()
                    || raw.parameters.is_some()
                    || raw.parameter_defaults.is_some()
                    || raw.source.is_some()
                    || raw.entrypoint.is_some()
                    || raw.program.is_some()
                    || raw.args.is_some()
                    || raw.environment.is_some()
                    || raw.protocol_transport.is_some()
                {
                    return Err(de::Error::custom(
                        "a fixed dataset accepts only `output-schema` and `sources`",
                    ));
                }
                let sources = required_source_bundle(raw.sources, "fixed dataset")
                    .map_err(de::Error::custom)?;
                Ok(Self::Fixed(FixedDatasetDefinition {
                    output_schema: raw
                        .output_schema
                        .ok_or_else(|| de::Error::missing_field("output-schema"))?,
                    sources,
                }))
            }
            (runner, function) => {
                let parameters = ParameterBoundary::lower(
                    raw.parameters,
                    raw.parameter_schema,
                    raw.parameter_defaults,
                )
                .map_err(de::Error::custom)?;
                let is_function = function.is_some();
                let worker = if let Some(function) = function {
                    if runner.is_some() || raw.entrypoint.is_some() || raw.program.is_some() || raw.args.is_some() {
                        return Err(de::Error::custom("`function` cannot be combined with runner, entrypoint, program, or args"));
                    }
                    build_function_worker(&function, "dataset_materializer", raw.sources, raw.environment, raw.protocol_transport)
                } else {
                    build_worker(runner.expect("generated runner"), raw.entrypoint, raw.program,
                        raw.args.unwrap_or_default(), raw.sources, raw.environment, raw.protocol_transport)
                }.map_err(de::Error::custom)?;
                Ok(Self::Generated(Box::new(GeneratedDatasetDefinition {
                    parameter_schema: parameters.schema,
                    parameter_defaults: parameters.defaults,
                    parameter_grids: parameters.grids,
                    output_schema: match raw.output_schema {
                        Some(schema) => schema,
                        None if is_function => json_dataset_schema(),
                        None => return Err(de::Error::missing_field("output-schema")),
                    },
                    source: raw.source,
                    worker,
                })))
            }
        }
    }
}

impl<'de> Deserialize<'de> for ProblemDefinition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ProblemDefinitionSeed { name: None }.deserialize(deserializer)
    }
}

fn deserialize_problems<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<Name, ProblemDefinition>, D::Error> {
    struct ProblemsVisitor;

    impl<'de> Visitor<'de> for ProblemsVisitor {
        type Value = BTreeMap<Name, ProblemDefinition>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a mapping of problem names to definitions")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut problems = BTreeMap::new();
            while let Some(name) = map.next_key::<Name>()? {
                let problem = map.next_value_seed(ProblemDefinitionSeed { name: Some(&name) })?;
                if problems.insert(name, problem).is_some() {
                    return Err(de::Error::custom("duplicate problem name"));
                }
            }
            Ok(problems)
        }
    }

    deserializer.deserialize_map(ProblemsVisitor)
}

struct ProblemDefinitionSeed<'a> {
    name: Option<&'a Name>,
}

struct ProblemField(String);

#[derive(Deserialize)]
#[serde(transparent)]
struct ProblemParameterDefaults(
    #[serde(deserialize_with = "deserialize_parameter_defaults")] Option<CanonicalValue>,
);

impl<'de> Deserialize<'de> for ProblemField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = String::deserialize(deserializer)?;
        if matches!(
            key.as_str(),
            "contract" | "evaluator" | "parameter-defaults"
        ) || crate::problem_contract::CONTRACT_FIELDS.contains(&key.as_str())
        {
            Ok(Self(key))
        } else {
            Err(de::Error::unknown_field(
                &key,
                crate::problem_contract::CONTRACT_FIELDS,
            ))
        }
    }
}

impl<'de> DeserializeSeed<'de> for ProblemDefinitionSeed<'_> {
    type Value = ProblemDefinition;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ProblemDefinitionSeed<'_> {
    type Value = ProblemDefinition;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a problem definition with an independent evaluator")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut contract = None;
        let mut parameter_defaults = None;
        let mut evaluator = None;
        let mut fields = serde_json::Map::new();
        while let Some(ProblemField(key)) = map.next_key()? {
            match key.as_str() {
                "contract" => contract = Some(map.next_value::<ContractDeclaration>()?),
                "evaluator" => evaluator = Some(map.next_value::<WorkerDefinition>()?),
                "parameter-defaults" => {
                    parameter_defaults = map.next_value::<ProblemParameterDefaults>()?.0;
                }
                key => {
                    fields.insert(key.to_owned(), map.next_value::<JsonValue>()?);
                }
            }
        }

        let contract = match contract {
            Some(contract) => {
                if let Some(key) = fields.keys().next() {
                    return Err(de::Error::custom(format_args!(
                        "`contract` cannot be combined with problem `{key}`"
                    )));
                }
                contract
            }
            None => {
                // Defaults normalize before hashing, just like explicit declarations.
                fields.entry("version").or_insert(JsonValue::from(1));
                if let Some(name) = self.name {
                    fields
                        .entry("name")
                        .or_insert_with(|| JsonValue::from(name.as_str()));
                }
                if let Some(name) = self.name {
                    fields
                        .entry("family")
                        .or_insert_with(|| JsonValue::from(name.as_str()));
                }
                if evaluator
                    .as_ref()
                    .is_some_and(WorkerDefinition::is_function)
                {
                    fields.entry("dataset-schemas").or_insert_with(|| {
                        serde_json::json!([json_dataset_schema().declaration_value()])
                    });
                }
                let contract = ProblemContract::deserialize(JsonValue::Object(fields))
                    .map_err(de::Error::custom)?;
                ContractDeclaration::Inline(Box::new(contract))
            }
        };
        if parameter_defaults.is_some()
            && matches!(&contract, ContractDeclaration::Inline(contract) if contract.compact_parameters)
        {
            return Err(de::Error::custom(
                "contract `parameters` cannot be combined with problem `parameter-defaults`",
            ));
        }
        Ok(ProblemDefinition {
            contract,
            parameter_defaults,
            evaluator: evaluator.ok_or_else(|| de::Error::missing_field("evaluator"))?,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawImplementationDefinition {
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    runner: Option<Runner>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    function: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    entrypoint: Option<RepositoryPath>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    program: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    sources: Option<Vec<RepositoryPath>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    environment: Option<Name>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    protocol_transport: Option<ProtocolTransport>,
    #[serde(default)]
    problem_contracts: Vec<Name>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    parameters: Option<ParameterDeclarations>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    parameter_schema: Option<SchemaDeclaration>,
    #[serde(default, deserialize_with = "deserialize_parameter_defaults")]
    parameter_defaults: Option<CanonicalValue>,
    #[serde(default)]
    capabilities: Vec<ImplementationCapability>,
}

impl<'de> Deserialize<'de> for ImplementationDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut raw = RawImplementationDefinition::deserialize(deserializer)?;
        let parameters =
            ParameterBoundary::lower(raw.parameters, raw.parameter_schema, raw.parameter_defaults)
                .map_err(de::Error::custom)?;
        if raw.function.is_some() {
            crate::problem_contract::ensure_unique(&raw.problem_contracts, "`problem-contracts`")
                .map_err(de::Error::custom)?;
            if raw.capabilities.is_empty() {
                raw.capabilities.push(ImplementationCapability::OneShot);
            }
            if raw.capabilities != [ImplementationCapability::OneShot] {
                return Err(de::Error::custom(
                    "function implementations support only `one_shot`; use lifecycle workers for profiles or applicability",
                ));
            }
        } else {
            ensure_nonempty_unique(&raw.problem_contracts, "`problem-contracts`")
                .map_err(de::Error::custom)?;
        }
        if !(raw
            .capabilities
            .contains(&ImplementationCapability::OneShot)
            || raw
                .capabilities
                .contains(&ImplementationCapability::BufferedProfile)
            || raw
                .capabilities
                .contains(&ImplementationCapability::StreamingProfile)
            || raw
                .capabilities
                .contains(&ImplementationCapability::FreshSequence))
            || raw.capabilities.len() > 5
            || raw
                .capabilities
                .iter()
                .enumerate()
                .any(|(i, c)| raw.capabilities[..i].contains(c))
        {
            return Err(de::Error::custom(
                "requires an execution capability, with optional `applicability`, without duplicates",
            ));
        }
        let worker = if let Some(function) = raw.function {
            if raw.runner.is_some()
                || raw.entrypoint.is_some()
                || raw.program.is_some()
                || raw.args.is_some()
            {
                return Err(de::Error::custom(
                    "`function` cannot be combined with runner, entrypoint, program, or args",
                ));
            }
            build_function_worker(
                &function,
                "implementation",
                raw.sources,
                raw.environment,
                raw.protocol_transport,
            )
        } else {
            build_worker(
                raw.runner
                    .ok_or_else(|| de::Error::missing_field("runner"))?,
                raw.entrypoint,
                raw.program,
                raw.args.unwrap_or_default(),
                raw.sources,
                raw.environment,
                raw.protocol_transport,
            )
        }
        .map_err(de::Error::custom)?;
        Ok(Self {
            worker,
            problem_contracts: raw.problem_contracts,
            parameter_schema: parameters.schema,
            parameter_defaults: parameters.defaults,
            parameter_grids: parameters.grids,
            capabilities: raw.capabilities,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawWorkerDefinition {
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    runner: Option<Runner>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    function: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    entrypoint: Option<RepositoryPath>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    program: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    sources: Option<Vec<RepositoryPath>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    environment: Option<Name>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    protocol_transport: Option<ProtocolTransport>,
}

impl<'de> Deserialize<'de> for WorkerDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawWorkerDefinition::deserialize(deserializer)?;
        if let Some(function) = raw.function {
            if raw.runner.is_some()
                || raw.entrypoint.is_some()
                || raw.program.is_some()
                || raw.args.is_some()
            {
                return Err(de::Error::custom(
                    "`function` cannot be combined with runner, entrypoint, program, or args",
                ));
            }
            build_function_worker(
                &function,
                "problem_evaluator",
                raw.sources,
                raw.environment,
                raw.protocol_transport,
            )
        } else {
            build_worker(
                raw.runner
                    .ok_or_else(|| de::Error::missing_field("runner"))?,
                raw.entrypoint,
                raw.program,
                raw.args.unwrap_or_default(),
                raw.sources,
                raw.environment,
                raw.protocol_transport,
            )
        }
        .map_err(de::Error::custom)
    }
}

fn omitted_name() -> Name {
    Name(String::new())
}

/// The JSON convenience interface owns its artifact envelope, not the author.
pub(crate) fn json_dataset_schema() -> SchemaDeclaration {
    serde_json::from_value(serde_json::json!({
        "type": "object", "properties": {
            "schema_version": {"const": 1}, "complete": {"const": true},
            "files": {"type": "array", "minItems": 1, "maxItems": 1,
                "items": {"type": "object", "properties": {"path": {"const": "data.json"}}, "required": ["path"]}}
        }, "required": ["schema_version", "complete", "files"]
    })).expect("standard JSON dataset schema")
}

fn build_function_worker(
    function: &str,
    role: &str,
    sources: Option<Vec<RepositoryPath>>,
    environment: Option<Name>,
    protocol_transport: Option<ProtocolTransport>,
) -> Result<WorkerDefinition, String> {
    let (file, name) = function
        .rsplit_once(':')
        .ok_or("`function` requires FILE:NAME")?;
    if name.is_empty()
        || !name.chars().enumerate().all(|(i, c)| {
            c.is_ascii_alphabetic() || c == '_' || c == '.' || (i > 0 && c.is_ascii_digit())
        })
    {
        return Err("`function` requires a simple function name, not an expression".to_owned());
    }
    let path: RepositoryPath = serde_json::from_value(serde_json::json!(file))
        .map_err(|e| format!("invalid function source: {e}"))?;
    let (program, mut args) = match path.as_path().extension().and_then(|s| s.to_str()) {
        Some("py") => ("python3", vec!["-m".to_owned(), "metewand".to_owned()]),
        Some("R" | "r") => (
            "Rscript",
            vec![
                "--vanilla".to_owned(),
                "-e".to_owned(),
                "metewand::serve_function()".to_owned(),
            ],
        ),
        _ => return Err("function sources require a .py or .R extension".to_owned()),
    };
    args.extend([
        role.to_owned(),
        format!("{{sources}}/{file}"),
        name.to_owned(),
    ]);
    let mut sources = sources.unwrap_or_default();
    if !sources.contains(&path) {
        sources.insert(0, path);
    }
    build_worker(
        Runner::Command,
        None,
        Some(program.to_owned()),
        args,
        Some(sources),
        environment,
        protocol_transport,
    )
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
    let environment = environment.unwrap_or_else(|| Name(String::new()));
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

impl<'de> Deserialize<'de> for ParameterAxis {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = deserialize_manifest_value(deserializer)?;
        match value.as_json() {
            JsonValue::Object(mapping) if mapping.len() == 1 && mapping.contains_key("value") => {
                CanonicalValue::try_from(mapping["value"].clone())
                    .map(Self::Value)
                    .map_err(de::Error::custom)
            }
            JsonValue::Object(mapping) if mapping.len() == 1 && mapping.contains_key("grid") => {
                let grid = mapping["grid"]
                    .as_array()
                    .filter(|values| !values.is_empty())
                    .ok_or_else(|| {
                        de::Error::custom("a parameter `grid` must be a nonempty array")
                    })?;
                grid.iter()
                    .map(|value| CanonicalValue::try_from(value.clone()).map_err(de::Error::custom))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Self::Grid)
            }
            JsonValue::Object(_) | JsonValue::Array(_) => Err(de::Error::custom(
                "expected a scalar parameter, `{value: ...}` for a literal array or object, or `{grid: [...]}` for a sweep",
            )),
            _ => Ok(Self::Value(value)),
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
        Err(de::Error::custom("`parameter-defaults` must be an object"))
    }
}

pub(crate) fn deserialize_manifest_value<'de, D>(
    deserializer: D,
) -> Result<CanonicalValue, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    CanonicalValue::try_from(value).map_err(de::Error::custom)
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

pub(crate) fn deserialize_optional_non_null<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    // Optional typed fields may be omitted, but explicit nulls violate the schema.
    T::deserialize(deserializer).map(Some)
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

fn version_one() -> u32 {
    1
}

fn one_repetition() -> u64 {
    1
}
