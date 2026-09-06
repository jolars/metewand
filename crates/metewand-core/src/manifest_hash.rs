//! Canonical hashing for complete, name-resolved benchmark manifests.

use std::{collections::BTreeMap, fmt, path::Path};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    canonical::{CanonicalJsonError, CanonicalValue},
    manifest::{
        CommandWorker, DatasetDefinition, Enforcement, EnvironmentDefinition, ExperimentCase,
        ExperimentDefinition, ImplementationCapability, ImplementationDefinition,
        InterpretedRunner, InterpretedWorker, Manifest, Name, NixOutputKind, ObservationKind,
        ParameterAxes, ParameterAxis, PrimaryTime, ProtocolTransport, RemoteSource, RepositoryPath,
        RepositoryRootPath, RunOrder, TimingScope, WorkerDefinition,
    },
};

/// The SHA-256 digest of a complete canonical manifest representation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ManifestHash([u8; 32]);

impl ManifestHash {
    /// Returns the raw SHA-256 bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ManifestHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Hashes a parsed manifest after applying defaults and resolving every name.
///
/// The digest is SHA-256 over [`canonical_manifest_bytes`]. It deliberately has
/// no edge into component identity records; a lockfile can therefore bind the
/// complete manifest without coupling an unchanged component to unrelated
/// definitions or experiments.
///
/// # Errors
///
/// Returns an error if a manifest-local reference is undefined or a public
/// field was programmatically changed to violate the canonical JSON domain.
pub fn hash_manifest(manifest: &Manifest) -> Result<ManifestHash, ManifestHashError> {
    let canonical = canonical_manifest_bytes(manifest)?;
    Ok(ManifestHash(Sha256::digest(canonical).into()))
}

/// Returns the exact canonical bytes hashed by [`hash_manifest`].
///
/// The representation is built from typed fields rather than through TOML
/// reserialization. Every optional collection and built-in execution-policy
/// default is explicit. Manifest maps are canonical JSON objects, while
/// experiments, cases, arguments, selections, source declarations, and grids
/// retain their semantic sequence order.
///
/// # Errors
///
/// Returns an error if a manifest-local reference is undefined or a public
/// field was programmatically changed to violate the canonical JSON domain.
pub fn canonical_manifest_bytes(manifest: &Manifest) -> Result<Vec<u8>, ManifestHashError> {
    validate_references(manifest)?;
    let value = CanonicalValue::try_from(manifest_value(manifest))?;
    Ok(value.to_canonical_bytes()?)
}

/// A failure to resolve or canonically encode a complete manifest.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ManifestHashError {
    /// One typed manifest reference has no definition in its target namespace.
    #[error("{owner} references undefined {target_kind} `{target}` through `{field}`")]
    UndefinedReference {
        /// Human-readable typed owner of the reference.
        owner: String,
        /// Manifest field containing the reference.
        field: &'static str,
        /// Kind of definition the field must select.
        target_kind: &'static str,
        /// Missing manifest-local name.
        target: String,
    },

    /// The typed representation could not enter the canonical JSON domain.
    #[error("canonical manifest representation is invalid: {0}")]
    Canonical(#[from] CanonicalJsonError),
}

fn validate_references(manifest: &Manifest) -> Result<(), ManifestHashError> {
    for (name, dataset) in &manifest.datasets {
        if let DatasetDefinition::Generated(dataset) = dataset {
            validate_worker_environment(
                manifest,
                format!("dataset `{name}` materializer"),
                &dataset.worker,
            )?;
        }
    }

    for (name, problem) in &manifest.problems {
        validate_worker_environment(
            manifest,
            format!("problem `{name}` evaluator"),
            &problem.evaluator,
        )?;
    }

    for (name, implementation) in &manifest.implementations {
        let owner = format!("implementation `{name}`");
        validate_worker_environment(manifest, owner.clone(), &implementation.worker)?;
        for problem in &implementation.problem_contracts {
            require_name(
                manifest.problems.contains_key(problem),
                owner.clone(),
                "problem_contracts",
                "problem",
                problem,
            )?;
        }
    }

    for experiment in &manifest.experiments {
        let owner = format!("experiment `{}`", experiment.name);
        require_name(
            manifest.problems.contains_key(&experiment.problem),
            owner.clone(),
            "problem",
            "problem",
            &experiment.problem,
        )?;
        for dataset in &experiment.datasets {
            require_name(
                manifest.datasets.contains_key(dataset),
                owner.clone(),
                "datasets",
                "dataset",
                dataset,
            )?;
        }
        for implementation in &experiment.implementations {
            require_name(
                manifest.implementations.contains_key(implementation),
                owner.clone(),
                "implementations",
                "implementation",
                implementation,
            )?;
        }
        require_name(
            manifest
                .execution_policies
                .contains_key(&experiment.execution_policy),
            owner.clone(),
            "execution_policy",
            "execution policy",
            &experiment.execution_policy,
        )?;
        require_name(
            manifest
                .observation_policies
                .contains_key(&experiment.observation_policy),
            owner.clone(),
            "observation_policy",
            "observation policy",
            &experiment.observation_policy,
        )?;
        validate_parameter_namespace_names(
            manifest,
            owner.clone(),
            &experiment.dataset_parameters,
            &experiment.implementation_parameters,
        )?;
        for case in &experiment.cases {
            validate_parameter_namespace_names(
                manifest,
                format!("case `{}` in {owner}", case.name),
                &case.dataset_parameters,
                &case.implementation_parameters,
            )?;
        }
    }

    Ok(())
}

fn validate_worker_environment(
    manifest: &Manifest,
    owner: String,
    worker: &WorkerDefinition,
) -> Result<(), ManifestHashError> {
    let environment = match worker {
        WorkerDefinition::Interpreted(worker) => &worker.environment,
        WorkerDefinition::Command(worker) => &worker.environment,
    };
    require_name(
        manifest.environments.contains_key(environment),
        owner,
        "environment",
        "environment",
        environment,
    )
}

fn validate_parameter_namespace_names(
    manifest: &Manifest,
    owner: String,
    datasets: &BTreeMap<Name, ParameterAxes>,
    implementations: &BTreeMap<Name, ParameterAxes>,
) -> Result<(), ManifestHashError> {
    for dataset in datasets.keys() {
        require_name(
            manifest.datasets.contains_key(dataset),
            owner.clone(),
            "dataset_parameters",
            "dataset",
            dataset,
        )?;
    }
    for implementation in implementations.keys() {
        require_name(
            manifest.implementations.contains_key(implementation),
            owner.clone(),
            "implementation_parameters",
            "implementation",
            implementation,
        )?;
    }
    Ok(())
}

fn require_name(
    resolved: bool,
    owner: String,
    field: &'static str,
    target_kind: &'static str,
    target: &Name,
) -> Result<(), ManifestHashError> {
    if resolved {
        Ok(())
    } else {
        Err(ManifestHashError::UndefinedReference {
            owner,
            field,
            target_kind,
            target: target.to_string(),
        })
    }
}

fn manifest_value(manifest: &Manifest) -> Value {
    object([
        ("version", json!(manifest.version)),
        ("name", json!(manifest.name.as_str())),
        ("datasets", named_map(&manifest.datasets, dataset)),
        ("problems", named_map(&manifest.problems, problem)),
        (
            "implementations",
            named_map(&manifest.implementations, implementation),
        ),
        (
            "environments",
            named_map(&manifest.environments, environment),
        ),
        (
            "experiments",
            Value::Array(manifest.experiments.iter().map(experiment).collect()),
        ),
        (
            "execution_policies",
            named_map(&manifest.execution_policies, execution_policy),
        ),
        (
            "observation_policies",
            named_map(&manifest.observation_policies, |policy| {
                object([
                    ("version", json!(policy.version)),
                    ("kind", json!(observation_kind(policy.kind))),
                ])
            }),
        ),
    ])
}

fn dataset(dataset: &DatasetDefinition) -> Value {
    match dataset {
        DatasetDefinition::Fixed(dataset) => object([
            ("kind", json!("fixed")),
            ("output_schema", repository_path(&dataset.output_schema)),
            ("sources", repository_paths(&dataset.sources)),
        ]),
        DatasetDefinition::Generated(dataset) => object([
            ("kind", json!("generated")),
            (
                "parameter_schema",
                optional_repository_path(dataset.parameter_schema.as_ref()),
            ),
            (
                "parameter_defaults",
                optional_canonical(dataset.parameter_defaults.as_ref()),
            ),
            ("output_schema", repository_path(&dataset.output_schema)),
            (
                "source",
                dataset.source.as_ref().map_or(Value::Null, remote_source),
            ),
            ("worker", worker(&dataset.worker)),
        ]),
    }
}

fn problem(problem: &crate::manifest::ProblemDefinition) -> Value {
    object([
        ("contract", repository_path(&problem.contract)),
        (
            "parameter_defaults",
            optional_canonical(problem.parameter_defaults.as_ref()),
        ),
        ("evaluator", worker(&problem.evaluator)),
    ])
}

fn implementation(implementation: &ImplementationDefinition) -> Value {
    object([
        ("worker", worker(&implementation.worker)),
        (
            "problem_contracts",
            names(&implementation.problem_contracts),
        ),
        (
            "parameter_schema",
            optional_repository_path(implementation.parameter_schema.as_ref()),
        ),
        (
            "parameter_defaults",
            optional_canonical(implementation.parameter_defaults.as_ref()),
        ),
        (
            "capabilities",
            Value::Array(
                implementation
                    .capabilities
                    .iter()
                    .copied()
                    .map(|capability| json!(implementation_capability(capability)))
                    .collect(),
            ),
        ),
    ])
}

fn environment(environment: &EnvironmentDefinition) -> Value {
    match environment {
        EnvironmentDefinition::Local {} => object([("kind", json!("local"))]),
        EnvironmentDefinition::Uv { project, lockfile } => object([
            ("kind", json!("uv")),
            ("project", repository_root_path(project)),
            ("lockfile", repository_path(lockfile)),
        ]),
        EnvironmentDefinition::Renv { project, lockfile } => object([
            ("kind", json!("renv")),
            ("project", repository_root_path(project)),
            ("lockfile", repository_path(lockfile)),
        ]),
        EnvironmentDefinition::Nix {
            flake,
            installable,
            system,
            output_kind,
            output,
        } => object([
            ("kind", json!("nix")),
            ("flake", repository_root_path(flake)),
            ("installable", json!(installable)),
            ("system", json!(system)),
            ("output_kind", json!(nix_output_kind(*output_kind))),
            ("output", json!(output)),
        ]),
        EnvironmentDefinition::Oci { image } => {
            object([("kind", json!("oci")), ("image", json!(image.as_str()))])
        }
    }
}

fn worker(worker: &WorkerDefinition) -> Value {
    match worker {
        WorkerDefinition::Interpreted(worker) => interpreted_worker(worker),
        WorkerDefinition::Command(worker) => command_worker(worker),
    }
}

fn interpreted_worker(worker: &InterpretedWorker) -> Value {
    object([
        ("kind", json!("interpreted")),
        ("runner", json!(interpreted_runner(worker.runner))),
        ("entrypoint", repository_path(&worker.entrypoint)),
        ("args", json!(worker.args)),
        ("sources", repository_paths(&worker.sources)),
        ("environment", json!(worker.environment.as_str())),
        (
            "protocol_transport",
            optional_protocol_transport(worker.protocol_transport),
        ),
    ])
}

fn command_worker(worker: &CommandWorker) -> Value {
    object([
        ("kind", json!("command")),
        ("program", json!(worker.program)),
        ("args", json!(worker.args)),
        (
            "sources",
            worker
                .sources
                .as_ref()
                .map_or(Value::Null, |sources| repository_paths(sources)),
        ),
        ("environment", json!(worker.environment.as_str())),
        (
            "protocol_transport",
            optional_protocol_transport(worker.protocol_transport),
        ),
    ])
}

fn remote_source(source: &RemoteSource) -> Value {
    object([
        ("url", json!(source.url)),
        ("sha256", json!(source.sha256.as_str())),
    ])
}

fn experiment(experiment: &ExperimentDefinition) -> Value {
    object([
        ("name", json!(experiment.name.as_str())),
        ("problem", json!(experiment.problem.as_str())),
        ("datasets", names(&experiment.datasets)),
        ("implementations", names(&experiment.implementations)),
        (
            "execution_policy",
            json!(experiment.execution_policy.as_str()),
        ),
        (
            "observation_policy",
            json!(experiment.observation_policy.as_str()),
        ),
        (
            "implementation_repetitions",
            json!(experiment.implementation_repetitions),
        ),
        (
            "measurement_repetitions",
            json!(experiment.measurement_repetitions),
        ),
        ("seed", json!(experiment.seed)),
        (
            "dataset_parameters",
            named_parameter_axes(&experiment.dataset_parameters),
        ),
        (
            "problem_parameters",
            parameter_axes(&experiment.problem_parameters),
        ),
        (
            "implementation_parameters",
            named_parameter_axes(&experiment.implementation_parameters),
        ),
        (
            "cases",
            Value::Array(experiment.cases.iter().map(experiment_case).collect()),
        ),
    ])
}

fn experiment_case(case: &ExperimentCase) -> Value {
    object([
        ("name", json!(case.name.as_str())),
        (
            "dataset_parameters",
            named_parameter_axes(&case.dataset_parameters),
        ),
        (
            "problem_parameters",
            parameter_axes(&case.problem_parameters),
        ),
        (
            "implementation_parameters",
            named_parameter_axes(&case.implementation_parameters),
        ),
    ])
}

fn named_parameter_axes(axes: &BTreeMap<Name, ParameterAxes>) -> Value {
    named_map(axes, parameter_axes)
}

fn parameter_axes(axes: &ParameterAxes) -> Value {
    Value::Object(
        axes.iter()
            .map(|(name, axis)| (name.clone(), parameter_axis(axis)))
            .collect(),
    )
}

fn parameter_axis(axis: &ParameterAxis) -> Value {
    match axis {
        ParameterAxis::Value(value) => object([("value", value.as_json().clone())]),
        ParameterAxis::Grid(values) => object([(
            "grid",
            Value::Array(values.iter().map(|value| value.as_json().clone()).collect()),
        )]),
    }
}

fn execution_policy(policy: &crate::manifest::ExecutionPolicyDefinition) -> Value {
    object([
        ("version", json!(policy.version)),
        ("cpus", json!(policy.cpus)),
        ("threads", json!(policy.threads)),
        (
            "memory",
            policy.memory.clone().map_or(Value::Null, Value::String),
        ),
        ("network", json!(policy.resolved_network())),
        ("worker_reuse", json!(policy.resolved_worker_reuse())),
        ("warmup_runs", json!(policy.resolved_warmup_runs())),
        (
            "timeout",
            policy.timeout.clone().map_or(Value::Null, Value::String),
        ),
        (
            "timing_scope",
            json!(timing_scope(policy.resolved_timing_scope())),
        ),
        (
            "primary_time",
            json!(primary_time(policy.resolved_primary_time())),
        ),
        ("run_order", json!(run_order(policy.resolved_run_order()))),
        (
            "enforcement",
            json!(enforcement(policy.resolved_enforcement())),
        ),
    ])
}

fn named_map<T>(values: &BTreeMap<Name, T>, convert: impl Fn(&T) -> Value) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(name, value)| (name.to_string(), convert(value)))
            .collect(),
    )
}

fn object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect::<Map<_, _>>(),
    )
}

fn optional_canonical(value: Option<&CanonicalValue>) -> Value {
    value.map_or(Value::Null, |value| value.as_json().clone())
}

fn repository_path(path: &RepositoryPath) -> Value {
    path_value(path.as_path())
}

fn optional_repository_path(path: Option<&RepositoryPath>) -> Value {
    path.map_or(Value::Null, repository_path)
}

fn repository_root_path(path: &RepositoryRootPath) -> Value {
    path_value(path.as_path())
}

fn repository_paths(paths: &[RepositoryPath]) -> Value {
    Value::Array(paths.iter().map(repository_path).collect())
}

fn path_value(path: &Path) -> Value {
    Value::String(
        path.to_str()
            .expect("repository paths are validated as UTF-8")
            .to_owned(),
    )
}

fn names(names: &[Name]) -> Value {
    Value::Array(
        names
            .iter()
            .map(|name| Value::String(name.to_string()))
            .collect(),
    )
}

fn optional_protocol_transport(transport: Option<ProtocolTransport>) -> Value {
    transport.map_or(Value::Null, |transport| {
        json!(protocol_transport(transport))
    })
}

fn interpreted_runner(runner: InterpretedRunner) -> &'static str {
    match runner {
        InterpretedRunner::Python => "python",
        InterpretedRunner::R => "r",
        InterpretedRunner::Julia => "julia",
    }
}

fn protocol_transport(transport: ProtocolTransport) -> &'static str {
    match transport {
        ProtocolTransport::Pipes => "pipes",
        ProtocolTransport::Stdio => "stdio",
    }
}

fn nix_output_kind(kind: NixOutputKind) -> &'static str {
    match kind {
        NixOutputKind::Package => "package",
        NixOutputKind::App => "app",
    }
}

fn implementation_capability(capability: ImplementationCapability) -> &'static str {
    match capability {
        ImplementationCapability::OneShot => "one_shot",
    }
}

fn observation_kind(kind: ObservationKind) -> &'static str {
    match kind {
        ObservationKind::OneShot => "one_shot",
    }
}

fn timing_scope(scope: TimingScope) -> &'static str {
    match scope {
        TimingScope::ColdEndToEnd => "cold_end_to_end",
        TimingScope::PrepareAndExecute => "prepare_and_execute",
        TimingScope::ExecuteOnly => "execute_only",
    }
}

fn primary_time(primary_time: PrimaryTime) -> &'static str {
    match primary_time {
        PrimaryTime::TimedWallTime => "timed_wall_time",
        PrimaryTime::CpuTime => "cpu_time",
    }
}

fn run_order(run_order: RunOrder) -> &'static str {
    match run_order {
        RunOrder::Sequential => "sequential",
        RunOrder::Randomized => "randomized",
    }
}

fn enforcement(enforcement: Enforcement) -> &'static str {
    match enforcement {
        Enforcement::BestEffort => "best_effort",
        Enforcement::Required => "required",
    }
}
