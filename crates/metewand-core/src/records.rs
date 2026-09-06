//! Identity-ready records for the Gate 1 benchmark lifecycle.
//!
//! These records separate authored definitions, logical planning state,
//! resolution state, and observed execution state. They deliberately do not
//! implement identity hashing, expansion, persistence, or execution.

use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    time::{Duration, SystemTime},
};

use crate::{
    canonical::CanonicalValue,
    manifest::{
        Enforcement, ImplementationCapability, InterpretedRunner, Name, NixOutputKind,
        ObservationKind, OciImageReference, PrimaryTime, ProtocolTransport, RepositoryPath,
        RepositoryRootPath, RunOrder, TimingScope,
    },
    problem_contract::ScientificBudget,
};

/// A type-safe reference to an identity-bearing record or resource.
///
/// The digest is stored separately from the public `mw1-<kind>-<sha256>`
/// spelling. The marker type prevents substituting, for example, a dataset
/// definition identity where a problem definition identity is required.
///
/// ```compile_fail
/// use metewand_core::records::{
///     DatasetDefinitionRecord, ProblemDefinitionRecord, RecordId,
/// };
///
/// fn requires_problem(_: RecordId<ProblemDefinitionRecord>) {}
///
/// let dataset = RecordId::<DatasetDefinitionRecord>::from_digest([0; 32]);
/// requires_problem(dataset);
/// ```
pub struct RecordId<T> {
    digest: [u8; 32],
    marker: PhantomData<fn() -> T>,
}

impl<T> RecordId<T> {
    /// Creates a typed reference from a previously computed identity digest.
    ///
    /// This constructor does not compute or verify the digest. Prefer
    /// [`crate::identity::identify_record`] or
    /// [`crate::identity::identify_canonical`] when constructing a new ID.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self {
            digest,
            marker: PhantomData,
        }
    }

    /// Returns the identity digest bytes.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

impl<T> Clone for RecordId<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for RecordId<T> {}

impl<T> fmt::Debug for RecordId<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RecordId")
            .field(&self.digest)
            .finish()
    }
}

impl<T> PartialEq for RecordId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}

impl<T> Eq for RecordId<T> {}

impl<T> PartialOrd for RecordId<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for RecordId<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.digest.cmp(&other.digest)
    }
}

impl<T> Hash for RecordId<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.digest.hash(state);
    }
}

/// A domain record paired with its typed identity.
///
/// Keeping the identity outside `record` prevents a record from containing its
/// own hash input. Identity construction will return this wrapper after hashing
/// the record's versioned canonical representation.
#[derive(Clone, Debug, PartialEq)]
pub struct IdentifiedRecord<T> {
    /// Typed identity assigned to the record.
    pub id: RecordId<T>,
    /// Domain data addressed by `id`.
    pub record: T,
}

/// How Metewand knows that a worker or executor has a capability.
///
/// A declaration is suitable for side-effect-free planning, but it is not an
/// observation of runtime behavior. Runtime checks produce `VerifiedNow`,
/// while persisted resolution records expose their observations as
/// `PreviouslyVerified` when read back later.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CapabilityEvidence {
    /// The benchmark definition declares the capability.
    Declared,
    /// The capability was observed during the current operation.
    VerifiedNow,
    /// The capability was observed by an earlier recorded operation.
    PreviouslyVerified,
}

impl CapabilityEvidence {
    /// Returns the stable machine-readable label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::VerifiedNow => "verified_now",
            Self::PreviouslyVerified => "previously_verified",
        }
    }
}

impl fmt::Display for CapabilityEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A typed capability paired with the evidence available to an operation.
///
/// The generic capability keeps implementation, worker, and executor
/// namespaces distinct while giving their reports one evidence model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityReport<C> {
    /// Capability being reported.
    pub capability: C,
    /// Evidence supporting the report.
    pub evidence: CapabilityEvidence,
}

impl<C> CapabilityReport<C> {
    /// Reports a capability from definition data without claiming it was run.
    #[must_use]
    pub const fn declared(capability: C) -> Self {
        Self {
            capability,
            evidence: CapabilityEvidence::Declared,
        }
    }
}

/// A raw SHA-256 content digest.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    /// Creates a content digest from its 32 bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Marker for a versioned repository schema resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SchemaResource {}

/// Marker for a validated problem-contract resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProblemContractResource {}

/// Marker for a declared source-bundle resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SourceBundleResource {}

/// Marker for a resolved source-bundle artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SourceBundleArtifact {}

/// Marker for a materialized dataset artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DatasetArtifact {}

/// Marker for a canonical implementation-result artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResultArtifact {}

/// Marker for an evaluator-metrics artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MetricsArtifact {}

/// One unresolved software-environment definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentDefinitionRecord {
    /// Manifest-local environment name.
    pub name: Name,
    /// Backend-specific unresolved environment definition.
    pub kind: EnvironmentDefinitionKind,
}

/// Backend-specific data in an unresolved environment definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentDefinitionKind {
    /// The existing host software context.
    Local,
    /// A Python environment described by a uv project and lockfile.
    Uv {
        /// Repository-relative project root.
        project: RepositoryRootPath,
        /// Repository-relative uv lockfile.
        lockfile: RepositoryPath,
    },
    /// An R environment described by an renv project and lockfile.
    Renv {
        /// Repository-relative project root.
        project: RepositoryRootPath,
        /// Repository-relative renv lockfile.
        lockfile: RepositoryPath,
    },
    /// A selected Nix flake output.
    Nix {
        /// Repository-relative flake root.
        flake: RepositoryRootPath,
        /// Exact flake installable.
        installable: String,
        /// Target Nix system.
        system: String,
        /// Selected output namespace.
        output_kind: NixOutputKind,
        /// Selected output name.
        output: String,
    },
    /// An OCI image pinned by digest.
    Oci {
        /// Immutable image reference.
        image: OciImageReference,
    },
}

/// An identity-ready worker definition shared by dataset, problem, and
/// implementation definitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerDefinitionRecord {
    /// Runner-specific launch definition and declared sources.
    pub launch: WorkerLaunch,
    /// Worker arguments after the runner-owned program or entrypoint.
    pub args: Vec<String>,
    /// Unresolved environment definition selected by this worker.
    pub environment: RecordId<EnvironmentDefinitionRecord>,
    /// Optional transport override.
    pub protocol_transport: Option<ProtocolTransport>,
}

/// Runner-specific data in a worker definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerLaunch {
    /// A worker launched by one of the language SDK runners.
    Interpreted {
        /// Language launch convention.
        runner: InterpretedRunner,
        /// Repository-relative worker entrypoint.
        entrypoint: RepositoryPath,
        /// Required declared source bundle.
        source_bundle: RecordId<SourceBundleResource>,
    },
    /// A worker launched as a raw command.
    Command {
        /// Program selected inside the unresolved environment.
        program: String,
        /// Optional repository-owned source bundle.
        source_bundle: Option<RecordId<SourceBundleResource>>,
    },
}

/// One dataset definition after source-bundle and schema resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct DatasetDefinitionRecord {
    /// Manifest-local dataset name.
    pub name: Name,
    /// Optional dataset-parameter schema.
    pub parameter_schema: Option<RecordId<SchemaResource>>,
    /// Literal defaults merged before parameter validation.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Schema of every materialized output.
    pub output_schema: RecordId<SchemaResource>,
    /// Dataset acquisition or materialization definition.
    pub kind: DatasetDefinitionKind,
}

/// Acquisition or materialization data in a dataset definition.
#[derive(Clone, Debug, PartialEq)]
pub enum DatasetDefinitionKind {
    /// Metewand's versioned unit dataset for dataset-free problems.
    Unit,
    /// A repository-owned fixed dataset tree.
    Fixed {
        /// Declared fixed-data source bundle.
        source_bundle: RecordId<SourceBundleResource>,
    },
    /// A dataset produced by a materializer worker.
    Generated {
        /// Optional pinned remote input.
        source: Option<RemoteSourceRecord>,
        /// Dataset materializer definition.
        materializer: WorkerDefinitionRecord,
    },
}

/// A pinned remote input to a dataset materializer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteSourceRecord {
    /// Remote resource URI.
    pub url: String,
    /// Expected bytes.
    pub sha256: ContentDigest,
}

/// One problem definition after contract and evaluator-source resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ProblemDefinitionRecord {
    /// Manifest-local problem name.
    pub name: Name,
    /// Validated problem and fairness contract.
    pub contract: RecordId<ProblemContractResource>,
    /// Literal defaults merged before problem-parameter validation.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Independent evaluator definition owned by this problem.
    pub evaluator: WorkerDefinitionRecord,
}

/// One implementation definition after contract and source resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ImplementationDefinitionRecord {
    /// Manifest-local implementation name.
    pub name: Name,
    /// Implementation worker definition.
    pub worker: WorkerDefinitionRecord,
    /// Exact problem definitions this implementation declares.
    pub problem_contracts: Vec<RecordId<ProblemDefinitionRecord>>,
    /// Optional implementation-parameter schema.
    pub parameter_schema: Option<RecordId<SchemaResource>>,
    /// Literal defaults merged before implementation-parameter validation.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Declared protocol capabilities.
    pub capabilities: Vec<ImplementationCapability>,
}

/// An execution policy after version-1 defaults have been applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionPolicyRecord {
    /// Manifest-local policy name.
    pub name: Name,
    /// Requested CPU count.
    pub cpus: Option<u64>,
    /// Requested thread count.
    pub threads: Option<u64>,
    /// Requested memory expression.
    pub memory: Option<String>,
    /// Whether workers may access the network.
    pub network: bool,
    /// Whether workers may serve more than one operation.
    pub worker_reuse: bool,
    /// Number of untimed process-local warm-ups.
    pub warmup_runs: u64,
    /// Scientific timeout expression.
    pub timeout: Option<String>,
    /// Phases included in the primary measurement.
    pub timing_scope: TimingScope,
    /// Authoritative executor-observed time measure.
    pub primary_time: PrimaryTime,
    /// Deterministic schedule policy.
    pub run_order: RunOrder,
    /// Required control-enforcement level.
    pub enforcement: Enforcement,
}

/// An observation policy after version-1 defaults have been applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationPolicyRecord {
    /// Manifest-local policy name.
    pub name: Name,
    /// Version-1 observation kind.
    pub kind: ObservationKind,
}

/// A seed value and its complete derivation digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DerivedSeedRecord {
    /// Nonnegative safe integer exposed to a worker.
    pub value: u64,
    /// Complete SHA-256 derivation digest retained for provenance.
    pub derivation_digest: ContentDigest,
}

/// A dataset definition with validated parameters and its deterministic seed.
#[derive(Clone, Debug, PartialEq)]
pub struct DatasetConfigurationRecord {
    /// Dataset definition being configured.
    pub definition: RecordId<DatasetDefinitionRecord>,
    /// Complete resolved dataset parameters.
    pub parameters: CanonicalValue,
    /// Dataset materialization seed.
    pub seed: DerivedSeedRecord,
}

/// A problem definition with validated parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ProblemConfigurationRecord {
    /// Problem definition being configured.
    pub definition: RecordId<ProblemDefinitionRecord>,
    /// Complete resolved problem parameters.
    pub parameters: CanonicalValue,
}

/// An implementation definition with validated parameters and selected
/// unresolved environment.
#[derive(Clone, Debug, PartialEq)]
pub struct ImplementationConfigurationRecord {
    /// Implementation definition being configured.
    pub definition: RecordId<ImplementationDefinitionRecord>,
    /// Complete resolved implementation parameters.
    pub parameters: CanonicalValue,
    /// Environment definition selected by the implementation.
    pub environment: RecordId<EnvironmentDefinitionRecord>,
}

/// A materialized, immutable dataset instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetInstanceRecord {
    /// Configuration that produced or selected this instance.
    pub configuration: RecordId<DatasetConfigurationRecord>,
    /// Immutable dataset artifact.
    pub artifact: RecordId<DatasetArtifact>,
    /// Schema validated for this instance.
    pub output_schema: RecordId<SchemaResource>,
}

/// Exactly one materialized dataset bound to one problem configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProblemInstanceRecord {
    /// Materialized dataset instance, including the built-in unit instance.
    pub dataset: RecordId<DatasetInstanceRecord>,
    /// Configured problem.
    pub configuration: RecordId<ProblemConfigurationRecord>,
}

/// A pre-applicability comparison candidate that requires no materialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalCandidateRecord {
    /// Dataset configuration selected for this candidate.
    pub dataset_configuration: RecordId<DatasetConfigurationRecord>,
    /// Problem configuration selected for this candidate.
    pub problem_configuration: RecordId<ProblemConfigurationRecord>,
    /// Implementation configuration selected for this candidate.
    pub implementation_configuration: RecordId<ImplementationConfigurationRecord>,
    /// Fully defaulted execution policy.
    pub execution_policy: RecordId<ExecutionPolicyRecord>,
    /// Fully defaulted observation policy.
    pub observation_policy: RecordId<ObservationPolicyRecord>,
}

/// One applicable one-shot logical run with a fixed implementation seed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OneShotLogicalSpecificationRecord {
    /// Candidate admitted as applicable.
    pub candidate: RecordId<LogicalCandidateRecord>,
    /// Shared problem-defined scientific budget.
    pub scientific_budget: ScientificBudget,
    /// Zero-based implementation-replication index.
    pub implementation_repetition: u64,
    /// Seed passed to the implementation worker.
    pub implementation_seed: DerivedSeedRecord,
}

/// One independently evaluated one-shot measurement position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalObservationSlotRecord {
    /// Logical run being observed.
    pub specification: RecordId<OneShotLogicalSpecificationRecord>,
    /// Zero-based measurement-repetition index.
    pub measurement_index: u64,
}

/// Whether a one-shot execution slot warms or measures a worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptSlotRole {
    /// An untimed process-local warm-up.
    Warmup {
        /// Zero-based warm-up index.
        warmup_index: u64,
    },
    /// A measured execution that produces one observation slot.
    Measured {
        /// Observation position produced by the attempt.
        observation_slot: RecordId<LogicalObservationSlotRecord>,
    },
}

/// One planned one-shot worker execution before runtime resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalAttemptSlotRecord {
    /// Logical run executed by this slot.
    pub specification: RecordId<OneShotLogicalSpecificationRecord>,
    /// Warm-up or measured role.
    pub role: AttemptSlotRole,
    /// Deterministically derived ordering priority.
    pub scheduling_priority: ContentDigest,
}

/// One environment after backend-specific resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedEnvironmentRecord {
    /// Unresolved environment definition.
    pub definition: RecordId<EnvironmentDefinitionRecord>,
    /// Complete backend-owned, path-independent fingerprint.
    pub fingerprint: CanonicalValue,
}

/// Stable working-directory convention for a resolved worker launch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerWorkingDirectory {
    /// Private directory allocated for one run attempt.
    PrivateRunDirectory,
}

/// Exact worker launch selected from a resolved environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedLaunchRecord {
    /// Resolved environment providing the program.
    pub environment: RecordId<ResolvedEnvironmentRecord>,
    /// Exact executable selected by resolution.
    pub program: String,
    /// Exact worker arguments.
    pub args: Vec<String>,
    /// Allowlisted environment variables.
    pub environment_variables: BTreeMap<String, String>,
    /// Path-independent working-directory convention.
    pub working_directory: WorkerWorkingDirectory,
}

/// Executor kind and canonical configuration selected for a run.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutorDefinitionRecord {
    /// Stable executor kind.
    pub kind: Name,
    /// Complete canonical executor configuration.
    pub configuration: CanonicalValue,
}

/// Resolved artifacts needed to launch one worker role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedWorkerRecord {
    /// Materialized source-bundle artifact, when the worker has one.
    pub source_bundle: Option<RecordId<SourceBundleArtifact>>,
    /// Resolved environment fingerprint.
    pub environment: RecordId<ResolvedEnvironmentRecord>,
    /// Exact launch program and arguments.
    pub launch: RecordId<ResolvedLaunchRecord>,
}

/// A one-shot logical run bound to all immutable execution dependencies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedOneShotSpecificationRecord {
    /// Resolution-free logical run.
    pub logical_specification: RecordId<OneShotLogicalSpecificationRecord>,
    /// Materialized dataset bound to the configured problem.
    pub problem_instance: RecordId<ProblemInstanceRecord>,
    /// Resolved implementation worker dependencies.
    pub implementation: ResolvedWorkerRecord,
    /// Resolved independent evaluator dependencies.
    pub evaluator: ResolvedWorkerRecord,
    /// Executor definition.
    pub executor: RecordId<ExecutorDefinitionRecord>,
    /// Selected worker wire-protocol version.
    pub wire_protocol_version: u32,
    /// Selected Metewand execution-semantics version.
    pub execution_semantics_version: u32,
}

/// A logical observation position bound to a resolved run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedObservationSlotRecord {
    /// Resolution-free observation slot.
    pub logical_slot: RecordId<LogicalObservationSlotRecord>,
    /// Resolved run used to produce the observation.
    pub specification: RecordId<ResolvedOneShotSpecificationRecord>,
}

/// A logical worker-execution position bound to a resolved run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAttemptSlotRecord {
    /// Resolution-free attempt slot.
    pub logical_slot: RecordId<LogicalAttemptSlotRecord>,
    /// Resolved run executed by this slot.
    pub specification: RecordId<ResolvedOneShotSpecificationRecord>,
}

/// A finalized artifact referenced by an execution record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactReference<T> {
    /// Typed artifact identity.
    pub id: RecordId<T>,
    /// Normalized path relative to the containing finalized record.
    pub path: RepositoryPath,
    /// Hash of the referenced artifact contents.
    pub sha256: ContentDigest,
}

/// Timings and resource measurements attached to one observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationTimingRecord {
    /// Authoritative executor-observed comparison time.
    pub primary_time: PrimaryTime,
    /// Wall time over the execution policy's selected timing scope.
    pub timed_wall_time: Duration,
    /// Optional in-worker implementation-call timing.
    pub implementation_time: Option<Duration>,
    /// Independent evaluator duration.
    pub evaluation_time: Duration,
    /// Optional process-tree CPU time over the selected timing scope.
    pub cpu_time: Option<Duration>,
    /// Optional peak resident-set size for the process tree.
    pub peak_rss_bytes: Option<u64>,
}

/// One-shot completion decision made by the independent evaluator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionStatus {
    /// The problem contract has no additional one-shot completion target.
    NotRequired,
    /// The result reached the contract's completion target.
    Reached,
    /// The result was valid but did not reach the completion target.
    NotReached,
}

/// Attempt- and observation-scoped execution provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct ProvenanceRecord {
    /// Exact Metewand build version.
    pub tool_version: String,
    /// Worker wire-protocol version.
    pub wire_protocol_version: u32,
    /// Metewand execution-semantics version.
    pub execution_semantics_version: u32,
    /// Complete typed provenance payload for the current execution gate.
    pub data: CanonicalValue,
}

/// An independently finalized, evaluator-accepted one-shot result.
#[derive(Clone, Debug, PartialEq)]
pub struct RunObservationRecord {
    /// Resolved observation slot satisfied by this result.
    pub slot: RecordId<ResolvedObservationSlotRecord>,
    /// Attempt that produced this observation.
    pub producing_attempt: RecordId<RunAttemptRecord>,
    /// Canonical implementation result.
    pub result: ArtifactReference<ResultArtifact>,
    /// Independent evaluator metrics.
    pub metrics: ArtifactReference<MetricsArtifact>,
    /// Observation timing and resource measurements.
    pub timing: ObservationTimingRecord,
    /// One-shot completion status for this valid observation.
    pub completion: CompletionStatus,
    /// Observed execution provenance.
    pub provenance: ProvenanceRecord,
}

/// Structured evidence for a failed run attempt.
#[derive(Clone, Debug, PartialEq)]
pub struct AttemptDiagnostic {
    /// Stable machine-readable failure code.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Optional canonical structured details.
    pub details: Option<CanonicalValue>,
}

/// Terminal outcome of one one-shot run attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum RunAttemptOutcome {
    /// The attempt durably published its one required observation.
    Accepted {
        /// Exact observation produced by this one-shot attempt.
        observation: RecordId<RunObservationRecord>,
    },
    /// The attempt terminated without an accepted observation.
    Failed {
        /// Structured evidence retained for diagnosis.
        diagnostic: AttemptDiagnostic,
        /// Valid observation retained when failure occurred after publication.
        observation: Option<RecordId<RunObservationRecord>>,
    },
}

/// One immutable execution attempt for a resolved slot and retry index.
#[derive(Clone, Debug, PartialEq)]
pub struct RunAttemptRecord {
    /// Resolved attempt slot executed by this attempt.
    pub slot: RecordId<ResolvedAttemptSlotRecord>,
    /// Zero-based retry index; every retry remains a distinct record.
    pub retry_index: u64,
    /// Terminal accepted or failed outcome.
    pub outcome: RunAttemptOutcome,
    /// Wall-clock start time retained as provenance, never identity input.
    pub started_at: SystemTime,
    /// Wall-clock terminal time retained as provenance, never identity input.
    pub finished_at: SystemTime,
    /// Observed execution provenance.
    pub provenance: ProvenanceRecord,
}
