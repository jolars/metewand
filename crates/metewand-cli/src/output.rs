use std::{collections::BTreeMap, fmt::Write as _, path::Path};

use metewand_core::{
    PUBLIC_SCHEMA_VERSION,
    manifest::{
        DatasetDefinition, Enforcement, EnvironmentDefinition, ImplementationCapability,
        InterpretedRunner, PrimaryTime, RunOrder, SourceSpan, WorkerDefinition,
    },
    planning::{
        ConfigurationSource, DatasetConfigurationDefinition, LogicalPlan, LogicalPlanningError,
    },
    public_schemas::{PUBLIC_SCHEMAS, PublicSchema},
};
use metewand_runtime::repository::{CheckedRepository, RepositoryCheckError};
use serde::{Serialize, Serializer};
use serde_json::Value;

pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_INVALID_REPOSITORY: u8 = 3;
pub const EXIT_INPUT_UNAVAILABLE: u8 = 4;
pub const EXIT_INTERNAL: u8 = 70;
pub const EXIT_IO: u8 = 74;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Human,
    Json,
    JsonLines,
}

impl OutputFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "json" => Some(Self::Json),
            "jsonl" => Some(Self::JsonLines),
            _ => None,
        }
    }

    pub const fn is_machine(self) -> bool {
        matches!(self, Self::Json | Self::JsonLines)
    }
}

#[derive(Debug)]
pub enum CommandResult {
    Help(HelpResult),
    Version(VersionResult),
    SchemaList(SchemaListResult),
    SchemaDocument(SchemaDocumentResult),
    Check(CheckResult),
    Plan(Box<PlanResult>),
}

impl CommandResult {
    pub fn help(text: &'static str) -> Self {
        Self::Help(HelpResult {
            command: "help",
            usage: text,
        })
    }

    pub fn version(version: &'static str) -> Self {
        Self::Version(VersionResult {
            command: "version",
            version,
        })
    }

    pub fn schema_list() -> Self {
        Self::SchemaList(SchemaListResult {
            command: "schema",
            compatibility_version: PUBLIC_SCHEMA_VERSION,
            schemas: PUBLIC_SCHEMAS
                .into_iter()
                .map(|schema| SchemaEntry {
                    name: schema.slug(),
                    id: schema.id(),
                })
                .collect(),
        })
    }

    pub fn schema_document(schema: PublicSchema) -> Self {
        let document = serde_json::from_str(schema.source())
            .expect("checked-in public schemas must always contain valid JSON");
        Self::SchemaDocument(SchemaDocumentResult {
            command: "schema",
            name: schema.slug(),
            id: schema.id(),
            document,
            source: schema.source(),
        })
    }

    pub fn check(repository: &CheckedRepository) -> Self {
        Self::Check(CheckResult {
            command: "check",
            benchmark: repository.manifest.name.to_string(),
            manifest_hash: repository.manifest_hash.to_string(),
            problem_contracts: repository.problem_contracts.len(),
            repository_schemas: repository.schema_count,
            source_bundles: repository.source_bundle_count,
            worker_launches: 0,
        })
    }

    pub fn plan(repository: &CheckedRepository, plan: &LogicalPlan) -> Self {
        Self::Plan(Box::new(PlanResult::new(repository, plan)))
    }

    pub fn render_human(&self) -> String {
        match self {
            Self::Help(result) => result.usage.to_owned(),
            Self::Version(result) => format!("metewand {}\n", result.version),
            Self::SchemaList(result) => result.render_human(),
            Self::SchemaDocument(result) => result.source.to_owned(),
            Self::Check(result) => result.render_human(),
            Self::Plan(result) => result.render_human(),
        }
    }
}

impl Serialize for CommandResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Help(result) => result.serialize(serializer),
            Self::Version(result) => result.serialize(serializer),
            Self::SchemaList(result) => result.serialize(serializer),
            Self::SchemaDocument(result) => result.serialize(serializer),
            Self::Check(result) => result.serialize(serializer),
            Self::Plan(result) => result.serialize(serializer),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HelpResult {
    command: &'static str,
    usage: &'static str,
}

#[derive(Debug, Serialize)]
pub struct VersionResult {
    command: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct SchemaListResult {
    command: &'static str,
    compatibility_version: u32,
    schemas: Vec<SchemaEntry>,
}

impl SchemaListResult {
    fn render_human(&self) -> String {
        let mut output = format!(
            "Public schemas (compatibility version {}):\n",
            self.compatibility_version
        );
        for schema in &self.schemas {
            writeln!(output, "  {:<29} {}", schema.name, schema.id)
                .expect("writing to a string cannot fail");
        }
        output
    }
}

#[derive(Debug, Serialize)]
pub struct SchemaEntry {
    name: &'static str,
    id: &'static str,
}

#[derive(Debug, Serialize)]
pub struct SchemaDocumentResult {
    command: &'static str,
    name: &'static str,
    id: &'static str,
    document: Value,
    #[serde(skip)]
    source: &'static str,
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    command: &'static str,
    benchmark: String,
    manifest_hash: String,
    problem_contracts: usize,
    repository_schemas: usize,
    source_bundles: usize,
    worker_launches: usize,
}

impl CheckResult {
    fn render_human(&self) -> String {
        format!(
            "Benchmark `{}` is valid.\n  manifest hash: {}\n  problem contracts: {}\n  repository schemas: {}\n  source bundles: {}\n  worker launches: {}\n",
            self.benchmark,
            self.manifest_hash,
            self.problem_contracts,
            self.repository_schemas,
            self.source_bundles,
            self.worker_launches,
        )
    }
}

#[derive(Debug, Serialize)]
pub struct PlanResult {
    command: &'static str,
    benchmark: String,
    manifest_hash: String,
    definitions: PlanDefinitions,
    resources: PlanResources,
    requirements: PlanRequirements,
    experiments: Vec<PlanExperiment>,
    summary: PlanSummary,
    side_effects: SideEffectSummary,
}

impl PlanResult {
    fn new(repository: &CheckedRepository, plan: &LogicalPlan) -> Self {
        let definitions = PlanDefinitions {
            environments: repository
                .planning_catalog
                .environment_definitions
                .iter()
                .map(|(name, id)| NamedIdentity::new(name.as_str(), id.to_string()))
                .collect(),
            datasets: repository
                .planning_catalog
                .dataset_definitions
                .iter()
                .map(|(definition, id)| DatasetIdentity {
                    name: definition.to_string(),
                    built_in: matches!(definition, DatasetConfigurationDefinition::Unit),
                    id: id.to_string(),
                })
                .collect(),
            problems: repository
                .planning_catalog
                .problem_definitions
                .iter()
                .map(|(name, id)| NamedIdentity::new(name.as_str(), id.to_string()))
                .collect(),
            implementations: repository
                .planning_catalog
                .implementation_definitions
                .iter()
                .map(|(name, id)| NamedIdentity::new(name.as_str(), id.to_string()))
                .collect(),
        };
        let resources = PlanResources {
            schemas: repository
                .resources
                .schemas
                .iter()
                .map(|(path, id)| PathIdentity::new(path, id.to_string()))
                .collect(),
            problem_contracts: repository
                .resources
                .problem_contracts
                .iter()
                .map(|(name, id)| NamedIdentity::new(name.as_str(), id.to_string()))
                .collect(),
            source_bundles: repository
                .resources
                .source_bundles
                .iter()
                .map(|(paths, id)| SourceBundleIdentity {
                    paths: paths.iter().map(|path| path_string(path)).collect(),
                    id: id.to_string(),
                })
                .collect(),
        };
        let requirements = PlanRequirements::new(repository);

        let mut logical_candidates = 0;
        let mut logical_specifications = 0;
        let mut observation_slots = 0;
        let mut attempt_slots = 0;
        let experiments = plan
            .experiments
            .iter()
            .map(|experiment| {
                let candidates = experiment
                    .candidates
                    .iter()
                    .map(|candidate| {
                        logical_candidates += 1;
                        let specifications = candidate
                            .specifications
                            .iter()
                            .map(|specification| {
                                logical_specifications += 1;
                                observation_slots += specification.observation_slots.len();
                                attempt_slots += specification.attempt_slots.len();
                                PlanSpecification {
                                    id: specification.specification.id.to_string(),
                                    observation_slots: specification
                                        .observation_slots
                                        .iter()
                                        .map(|observation| observation.id.to_string())
                                        .collect(),
                                    attempt_slots: specification
                                        .attempt_slots
                                        .iter()
                                        .map(|attempt| attempt.id.to_string())
                                        .collect(),
                                }
                            })
                            .collect();
                        PlanCandidate {
                            id: candidate.candidate.id.to_string(),
                            source: PlanConfigurationSource::new(&candidate.source),
                            dataset_configuration: candidate.dataset_configuration.id.to_string(),
                            problem_configuration: candidate.problem_configuration.id.to_string(),
                            implementation_configuration: candidate
                                .implementation_configuration
                                .id
                                .to_string(),
                            capabilities: candidate
                                .implementation_capabilities
                                .iter()
                                .map(|capability| CapabilityOutput {
                                    capability: implementation_capability(capability.capability),
                                    evidence: capability.evidence.as_str(),
                                })
                                .collect(),
                            specifications,
                        }
                    })
                    .collect();
                PlanExperiment {
                    name: experiment.name.to_string(),
                    scheduling_seed: experiment.scheduling_seed.value,
                    execution_policy: experiment.execution_policy.id.to_string(),
                    observation_policy: experiment.observation_policy.id.to_string(),
                    candidates,
                }
            })
            .collect();

        Self {
            command: "plan",
            benchmark: repository.manifest.name.to_string(),
            manifest_hash: repository.manifest_hash.to_string(),
            definitions,
            resources,
            requirements,
            experiments,
            summary: PlanSummary {
                logical_candidates,
                logical_specifications,
                observation_slots,
                attempt_slots,
                applicable_runs: logical_specifications,
                execution_slots: attempt_slots,
                known_applicable_candidates: logical_candidates,
                known_applicable_attempt_slots: attempt_slots,
                unchecked_candidates: 0,
                known_exclusions: 0,
            },
            side_effects: SideEffectSummary {
                filesystem_writes: 0,
                downloads: 0,
                builds: 0,
                worker_launches: 0,
            },
        }
    }

    fn render_human(&self) -> String {
        let mut output = format!(
            "Plan for benchmark `{}`\nManifest hash: {}\n\nDefinition identities:\n",
            self.benchmark, self.manifest_hash
        );
        for definition in &self.definitions.environments {
            writeln!(
                output,
                "  environment `{}`: {}",
                definition.name, definition.id
            )
            .expect("writing to a string cannot fail");
        }
        for definition in &self.definitions.datasets {
            writeln!(output, "  dataset `{}`: {}", definition.name, definition.id)
                .expect("writing to a string cannot fail");
        }
        for definition in &self.definitions.problems {
            writeln!(output, "  problem `{}`: {}", definition.name, definition.id)
                .expect("writing to a string cannot fail");
        }
        for definition in &self.definitions.implementations {
            writeln!(
                output,
                "  implementation `{}`: {}",
                definition.name, definition.id
            )
            .expect("writing to a string cannot fail");
        }
        writeln!(output, "\nRequired resource identities:")
            .expect("writing to a string cannot fail");
        for resource in &self.resources.schemas {
            writeln!(output, "  schema `{}`: {}", resource.path, resource.id)
                .expect("writing to a string cannot fail");
        }
        for resource in &self.resources.problem_contracts {
            writeln!(
                output,
                "  problem contract `{}`: {}",
                resource.name, resource.id
            )
            .expect("writing to a string cannot fail");
        }
        for resource in &self.resources.source_bundles {
            writeln!(
                output,
                "  source bundle [{}]: {}",
                resource.paths.join(", "),
                resource.id
            )
            .expect("writing to a string cannot fail");
        }
        self.requirements.render_human(&mut output);

        for experiment in &self.experiments {
            writeln!(
                output,
                "\nExperiment `{}`\n  scheduling seed: {}\n  execution policy: {}\n  observation policy: {}",
                experiment.name,
                experiment.scheduling_seed,
                experiment.execution_policy,
                experiment.observation_policy,
            )
            .expect("writing to a string cannot fail");
            for candidate in &experiment.candidates {
                writeln!(
                    output,
                    "  logical candidate {} ({})\n    dataset configuration: {}\n    problem configuration: {}\n    implementation configuration: {}",
                    candidate.id,
                    candidate.source.human_label(),
                    candidate.dataset_configuration,
                    candidate.problem_configuration,
                    candidate.implementation_configuration,
                )
                .expect("writing to a string cannot fail");
                for capability in &candidate.capabilities {
                    writeln!(
                        output,
                        "    capability: {} ({})",
                        capability.capability, capability.evidence
                    )
                    .expect("writing to a string cannot fail");
                }
                for specification in &candidate.specifications {
                    writeln!(output, "    logical specification {}", specification.id)
                        .expect("writing to a string cannot fail");
                    for observation in &specification.observation_slots {
                        writeln!(output, "      observation slot {observation}")
                            .expect("writing to a string cannot fail");
                    }
                    for attempt in &specification.attempt_slots {
                        writeln!(output, "      attempt slot {attempt}")
                            .expect("writing to a string cannot fail");
                    }
                }
            }
        }
        writeln!(
            output,
            "\nSummary: {}, {}, {}, {}.\nApplicable runs: {}; execution slots: {}.\nApplicability: {} candidates and {} attempt slots known applicable, {} unchecked, and no known exclusions.\nCapabilities are declarations only; no workers were launched.\nThis command performed no downloads, builds, or filesystem writes.",
            count(self.summary.logical_candidates, "logical candidate"),
            count(self.summary.logical_specifications, "logical specification"),
            count(self.summary.observation_slots, "observation slot"),
            count(self.summary.attempt_slots, "attempt slot"),
            self.summary.applicable_runs,
            self.summary.execution_slots,
            self.summary.known_applicable_candidates,
            self.summary.known_applicable_attempt_slots,
            self.summary.unchecked_candidates,
        )
        .expect("writing to a string cannot fail");
        output
    }
}

#[derive(Debug, Serialize)]
struct PlanDefinitions {
    environments: Vec<NamedIdentity>,
    datasets: Vec<DatasetIdentity>,
    problems: Vec<NamedIdentity>,
    implementations: Vec<NamedIdentity>,
}

#[derive(Debug, Serialize)]
struct NamedIdentity {
    name: String,
    id: String,
}

impl NamedIdentity {
    fn new(name: &str, id: String) -> Self {
        Self {
            name: name.to_owned(),
            id,
        }
    }
}

#[derive(Debug, Serialize)]
struct DatasetIdentity {
    name: String,
    built_in: bool,
    id: String,
}

#[derive(Debug, Serialize)]
struct PlanResources {
    schemas: Vec<PathIdentity>,
    problem_contracts: Vec<NamedIdentity>,
    source_bundles: Vec<SourceBundleIdentity>,
}

#[derive(Debug, Serialize)]
struct PathIdentity {
    path: String,
    id: String,
}

impl PathIdentity {
    fn new(path: &Path, id: String) -> Self {
        Self {
            path: path_string(path),
            id,
        }
    }
}

#[derive(Debug, Serialize)]
struct SourceBundleIdentity {
    paths: Vec<String>,
    id: String,
}

#[derive(Debug, Serialize)]
struct PlanRequirements {
    artifacts: ArtifactRequirements,
    executor: &'static str,
    environments: Vec<NamedRequirement>,
    components: Vec<ComponentRequirement>,
    controls: Vec<ControlRequirement>,
}

impl PlanRequirements {
    fn new(repository: &CheckedRepository) -> Self {
        let environments = repository
            .manifest
            .environments
            .iter()
            .map(|(name, environment)| NamedRequirement {
                name: name.to_string(),
                requirement: environment_requirement(environment),
            })
            .collect();
        let mut components = Vec::new();
        for (name, dataset) in &repository.manifest.datasets {
            match dataset {
                DatasetDefinition::Fixed(_) => components.push(ComponentRequirement {
                    role: "dataset",
                    name: name.to_string(),
                    requirement: "read declared local artifact".to_owned(),
                }),
                DatasetDefinition::Generated(dataset) => components.push(ComponentRequirement {
                    role: "dataset_materializer",
                    name: name.to_string(),
                    requirement: format!(
                        "{}{}",
                        worker_description(&dataset.worker),
                        if dataset.source.is_some() {
                            "; pinned download required"
                        } else {
                            ""
                        }
                    ),
                }),
            }
        }
        for (name, problem) in &repository.manifest.problems {
            components.push(ComponentRequirement {
                role: "problem_evaluator",
                name: name.to_string(),
                requirement: worker_description(&problem.evaluator),
            });
        }
        for (name, implementation) in &repository.manifest.implementations {
            components.push(ComponentRequirement {
                role: "implementation_worker",
                name: name.to_string(),
                requirement: worker_description(&implementation.worker),
            });
        }
        let controls = repository
            .manifest
            .experiments
            .iter()
            .map(|experiment| {
                let policy = &repository.manifest.execution_policies[&experiment.execution_policy];
                ControlRequirement {
                    experiment: experiment.name.to_string(),
                    cpus: policy.cpus,
                    threads: policy.threads,
                    memory: policy.memory.clone(),
                    network: policy.resolved_network(),
                    worker_reuse: policy.resolved_worker_reuse(),
                    warmup_runs: policy.resolved_warmup_runs(),
                    timeout: policy.timeout.clone(),
                    timing_scope: policy.resolved_timing_scope().to_string(),
                    primary_time: primary_time(policy.resolved_primary_time()),
                    run_order: run_order(policy.resolved_run_order()),
                    enforcement: enforcement(policy.resolved_enforcement()),
                }
            })
            .collect();
        Self {
            artifacts: ArtifactRequirements {
                problem_contracts: repository.problem_contracts.len(),
                schemas: repository.schema_count,
                source_bundles: repository.source_bundle_count,
            },
            executor: "local_process_unresolved",
            environments,
            components,
            controls,
        }
    }

    fn render_human(&self, output: &mut String) {
        writeln!(
            output,
            "\nDeclared execution requirements:\n  artifacts: {} contracts, {} schemas, {} source bundles\n  executor: local process (unresolved)",
            self.artifacts.problem_contracts,
            self.artifacts.schemas,
            self.artifacts.source_bundles,
        )
        .expect("writing to a string cannot fail");
        for environment in &self.environments {
            writeln!(
                output,
                "  environment `{}`: {}",
                environment.name, environment.requirement
            )
            .expect("writing to a string cannot fail");
        }
        for component in &self.components {
            let role = match component.role {
                "dataset" => format!("dataset `{}`", component.name),
                "dataset_materializer" => {
                    format!("dataset `{}` materializer", component.name)
                }
                "problem_evaluator" => format!("problem `{}` evaluator", component.name),
                "implementation_worker" => {
                    format!("implementation `{}` worker", component.name)
                }
                _ => unreachable!("component roles are constructed internally"),
            };
            writeln!(output, "  {role}: {}", component.requirement)
                .expect("writing to a string cannot fail");
        }
        for control in &self.controls {
            writeln!(
                output,
                "  controls for experiment `{}`: cpus={}, threads={}, memory={}, network={}, worker_reuse={}, warmups={}, timeout={}, timing_scope={}, primary_time={}, run_order={}, enforcement={}",
                control.experiment,
                optional_value(control.cpus),
                optional_value(control.threads),
                control.memory.as_deref().unwrap_or("unspecified"),
                control.network,
                control.worker_reuse,
                control.warmup_runs,
                control.timeout.as_deref().unwrap_or("unspecified"),
                control.timing_scope,
                control.primary_time,
                control.run_order,
                control.enforcement,
            )
            .expect("writing to a string cannot fail");
        }
        writeln!(
            output,
            "  writable paths for this plan command: none\n  downloads, builds, and worker launches for this plan command: none"
        )
        .expect("writing to a string cannot fail");
    }
}

#[derive(Debug, Serialize)]
struct ArtifactRequirements {
    problem_contracts: usize,
    schemas: usize,
    source_bundles: usize,
}

#[derive(Debug, Serialize)]
struct NamedRequirement {
    name: String,
    requirement: &'static str,
}

#[derive(Debug, Serialize)]
struct ComponentRequirement {
    role: &'static str,
    name: String,
    requirement: String,
}

#[derive(Debug, Serialize)]
struct ControlRequirement {
    experiment: String,
    cpus: Option<u64>,
    threads: Option<u64>,
    memory: Option<String>,
    network: bool,
    worker_reuse: bool,
    warmup_runs: u64,
    timeout: Option<String>,
    timing_scope: String,
    primary_time: &'static str,
    run_order: &'static str,
    enforcement: &'static str,
}

#[derive(Debug, Serialize)]
struct PlanExperiment {
    name: String,
    scheduling_seed: u64,
    execution_policy: String,
    observation_policy: String,
    candidates: Vec<PlanCandidate>,
}

#[derive(Debug, Serialize)]
struct PlanCandidate {
    id: String,
    source: PlanConfigurationSource,
    dataset_configuration: String,
    problem_configuration: String,
    implementation_configuration: String,
    capabilities: Vec<CapabilityOutput>,
    specifications: Vec<PlanSpecification>,
}

#[derive(Debug, Serialize)]
struct PlanConfigurationSource {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    case: Option<String>,
}

impl PlanConfigurationSource {
    fn new(source: &ConfigurationSource) -> Self {
        match source {
            ConfigurationSource::Experiment => Self {
                kind: "experiment",
                case: None,
            },
            ConfigurationSource::Case(name) => Self {
                kind: "case",
                case: Some(name.to_string()),
            },
        }
    }

    fn human_label(&self) -> String {
        self.case.as_ref().map_or_else(
            || "experiment parameters".to_owned(),
            |name| format!("case `{name}`"),
        )
    }
}

#[derive(Debug, Serialize)]
struct CapabilityOutput {
    capability: &'static str,
    evidence: &'static str,
}

#[derive(Debug, Serialize)]
struct PlanSpecification {
    id: String,
    observation_slots: Vec<String>,
    attempt_slots: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PlanSummary {
    logical_candidates: usize,
    logical_specifications: usize,
    observation_slots: usize,
    attempt_slots: usize,
    applicable_runs: usize,
    execution_slots: usize,
    known_applicable_candidates: usize,
    known_applicable_attempt_slots: usize,
    unchecked_candidates: usize,
    known_exclusions: usize,
}

#[derive(Debug, Serialize)]
struct SideEffectSummary {
    filesystem_writes: usize,
    downloads: usize,
    builds: usize,
    worker_launches: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    details: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_span: Option<DiagnosticSourceSpan>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    affected_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    causes: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation: Option<String>,
}

impl Diagnostic {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: BTreeMap::new(),
            source_span: None,
            affected_ids: Vec::new(),
            causes: Vec::new(),
            remediation: None,
        }
    }

    fn with_detail(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }

    fn with_source_span(mut self, source_span: &SourceSpan) -> Self {
        self.source_span = Some(DiagnosticSourceSpan::new(source_span));
        self
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read-only Gate 1 failures occur before affected identities exist"
        )
    )]
    pub fn with_affected_ids(mut self, identities: impl IntoIterator<Item = String>) -> Self {
        self.affected_ids.extend(identities);
        self.affected_ids.sort();
        self.affected_ids.dedup();
        self
    }

    fn caused_by(mut self, cause: Diagnostic) -> Self {
        self.causes.push(cause);
        self
    }

    fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }

    pub fn render_human(&self) -> String {
        let mut output = String::new();
        self.render_human_at(0, &mut output);
        output
    }

    fn render_human_at(&self, depth: usize, output: &mut String) {
        let indentation = "  ".repeat(depth);
        if depth == 0 {
            writeln!(output, "error[{}]: {}", self.code, self.message)
                .expect("writing to a string cannot fail");
        } else {
            writeln!(
                output,
                "{indentation}caused by error[{}]: {}",
                self.code, self.message
            )
            .expect("writing to a string cannot fail");
        }
        if let Some(source_span) = &self.source_span {
            writeln!(
                output,
                "{indentation}  --> {}:{}:{}-{}:{}",
                source_span.path,
                source_span.line,
                source_span.column,
                source_span.end_line,
                source_span.end_column,
            )
            .expect("writing to a string cannot fail");
        }
        for identity in &self.affected_ids {
            writeln!(output, "{indentation}  affected: {identity}")
                .expect("writing to a string cannot fail");
        }
        if let Some(remediation) = &self.remediation {
            writeln!(output, "{indentation}  help: {remediation}")
                .expect("writing to a string cannot fail");
        }
        for cause in &self.causes {
            cause.render_human_at(depth + 1, output);
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct DiagnosticSourceSpan {
    path: String,
    line: usize,
    column: usize,
    end_line: usize,
    end_column: usize,
}

impl DiagnosticSourceSpan {
    fn new(source_span: &SourceSpan) -> Self {
        Self {
            path: path_string(&source_span.path),
            line: source_span.line,
            column: source_span.column,
            end_line: source_span.end_line,
            end_column: source_span.end_column,
        }
    }
}

#[derive(Debug)]
pub struct CommandError {
    pub exit_code: u8,
    pub diagnostic: Box<Diagnostic>,
}

impl CommandError {
    pub fn usage(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_USAGE,
            diagnostic: Box::new(
                Diagnostic::new(code, message)
                    .with_remediation("Run `metewand --help` for command usage."),
            ),
        }
    }

    pub fn unsupported(message: impl Into<String>, remediation: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_USAGE,
            diagnostic: Box::new(
                Diagnostic::new("unsupported_operation", message).with_remediation(remediation),
            ),
        }
    }

    pub fn repository(command: &'static str, error: &RepositoryCheckError) -> Self {
        let exit_code = match error {
            RepositoryCheckError::ManifestAccess { .. }
            | RepositoryCheckError::ManifestNotFile { .. }
            | RepositoryCheckError::RepositoryFileAccess { .. } => EXIT_INPUT_UNAVAILABLE,
            _ => EXIT_INVALID_REPOSITORY,
        };
        Self {
            exit_code,
            diagnostic: Box::new(
                Diagnostic::new(
                    "repository_check_failed",
                    format!("Repository validation for `{command}` failed."),
                )
                .with_detail("command", command)
                .caused_by(repository_diagnostic(error)),
            ),
        }
    }

    pub fn planning(error: &LogicalPlanningError) -> Self {
        let cause = match error {
            LogicalPlanningError::Configuration(_) => {
                Diagnostic::new("invalid_configuration", error.to_string()).with_remediation(
                    "Correct the experiment configuration and run the plan again.",
                )
            }
            LogicalPlanningError::MissingDefinitionIdentity { .. } => {
                Diagnostic::new("missing_definition_identity", error.to_string())
            }
            LogicalPlanningError::Seed(_) => {
                Diagnostic::new("seed_derivation_failed", error.to_string())
            }
            LogicalPlanningError::Identity(_) => {
                Diagnostic::new("identity_construction_failed", error.to_string())
            }
            _ => Diagnostic::new("logical_planning_failed", error.to_string()),
        };
        Self {
            exit_code: EXIT_INVALID_REPOSITORY,
            diagnostic: Box::new(
                Diagnostic::new("logical_planning_failed", "Logical planning failed.")
                    .with_detail("command", "plan")
                    .caused_by(cause),
            ),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_INTERNAL,
            diagnostic: Box::new(Diagnostic::new("internal_error", message)),
        }
    }

    pub fn output_write(message: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_IO,
            diagnostic: Box::new(Diagnostic::new("output_write_failed", message)),
        }
    }
}

#[derive(Serialize)]
struct MachineEnvelope<'a, T> {
    schema_version: u32,
    kind: &'static str,
    data: &'a T,
}

pub fn render_result(
    result: &CommandResult,
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    if format == OutputFormat::Human {
        return Ok(result.render_human());
    }
    render_machine("result", result, format)
}

pub fn render_diagnostic(
    diagnostic: &Diagnostic,
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    render_machine("diagnostic", diagnostic, format)
}

fn render_machine<T: Serialize>(
    kind: &'static str,
    data: &T,
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    let envelope = MachineEnvelope {
        schema_version: PUBLIC_SCHEMA_VERSION,
        kind,
        data,
    };
    let mut output = match format {
        OutputFormat::Human => unreachable!("human output does not use machine envelopes"),
        OutputFormat::Json => serde_json::to_string_pretty(&envelope)?,
        OutputFormat::JsonLines => serde_json::to_string(&envelope)?,
    };
    output.push('\n');
    Ok(output)
}

fn repository_diagnostic(error: &RepositoryCheckError) -> Diagnostic {
    match error {
        RepositoryCheckError::ManifestAccess { path, .. } => {
            Diagnostic::new("manifest_access_failed", error.to_string())
                .with_detail("path", path_string(path))
                .with_remediation("Verify that the manifest path exists and is readable.")
        }
        RepositoryCheckError::ManifestNotFile { path } => {
            Diagnostic::new("manifest_not_file", error.to_string())
                .with_detail("path", path_string(path))
                .with_remediation("Select a regular manifest file with `--manifest PATH`.")
        }
        RepositoryCheckError::RepositoryFileAccess { path, .. } => {
            Diagnostic::new("repository_file_access_failed", error.to_string())
                .with_detail("path", path_string(path))
                .with_remediation(
                    "Verify that the declared repository file exists and is readable.",
                )
        }
        RepositoryCheckError::RepositoryPathEscape { path, target } => {
            Diagnostic::new("repository_path_escape", error.to_string())
                .with_detail("path", path_string(path))
                .with_detail("target", path_string(target))
                .with_remediation(
                    "Keep every declared repository path beneath the manifest directory.",
                )
        }
        RepositoryCheckError::NonUtf8File { path, .. } => {
            Diagnostic::new("invalid_utf8_file", error.to_string())
                .with_detail("path", path_string(path))
                .with_remediation("Encode the declared text file as UTF-8.")
        }
        RepositoryCheckError::InvalidSchemaJson { path, .. } => {
            Diagnostic::new("invalid_schema_json", error.to_string())
                .with_detail("path", path_string(path))
                .with_remediation("Correct the JSON document at the declared schema path.")
        }
        RepositoryCheckError::InvalidSchemaReference {
            document,
            reference,
        } => Diagnostic::new("invalid_schema_reference", error.to_string())
            .with_detail("document", path_string(document))
            .with_detail("reference", reference.clone())
            .with_remediation("Use a normalized repository-relative schema reference."),
        RepositoryCheckError::MissingSchemaIdentity { path } => {
            Diagnostic::new("missing_schema_identity", error.to_string())
                .with_detail("path", path_string(path))
        }
        RepositoryCheckError::InvalidTypedDigest => {
            Diagnostic::new("invalid_typed_digest", error.to_string())
        }
        RepositoryCheckError::Manifest(source) => {
            let mut diagnostic = Diagnostic::new("invalid_manifest", source.to_string())
                .with_remediation("Correct the manifest value at the reported source span.");
            if let Some(source_span) = source.source_span() {
                diagnostic = diagnostic.with_source_span(source_span);
            }
            diagnostic
        }
        RepositoryCheckError::ManifestHash(_) => {
            Diagnostic::new("invalid_manifest_reference", error.to_string())
                .with_remediation("Correct the undefined manifest reference.")
        }
        RepositoryCheckError::ProblemContract(source) => {
            let mut diagnostic = Diagnostic::new("invalid_problem_contract", source.to_string())
                .with_remediation("Correct the declared problem contract.");
            if let Some(source_span) = source.source_span() {
                diagnostic = diagnostic.with_source_span(source_span);
            }
            diagnostic
        }
        RepositoryCheckError::SchemaCatalog(_) => {
            Diagnostic::new("invalid_schema_catalog", error.to_string())
                .with_remediation("Correct the reported repository schema.")
        }
        RepositoryCheckError::Configuration(_) => {
            Diagnostic::new("invalid_configuration", error.to_string())
                .with_remediation("Correct the incompatible experiment configuration.")
        }
        RepositoryCheckError::SourceBundle(_) => {
            Diagnostic::new("invalid_source_bundle", error.to_string())
                .with_remediation("Correct the reported source declaration or repository entry.")
        }
        RepositoryCheckError::Canonical(_) => {
            Diagnostic::new("canonicalization_failed", error.to_string())
        }
        RepositoryCheckError::Identity(_) => {
            Diagnostic::new("identity_construction_failed", error.to_string())
        }
        _ => Diagnostic::new("repository_check_failed", error.to_string()),
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn environment_requirement(environment: &EnvironmentDefinition) -> &'static str {
    match environment {
        EnvironmentDefinition::Local {} => "use the existing host context; no build",
        EnvironmentDefinition::Uv { .. } => "resolve and provision the declared uv project",
        EnvironmentDefinition::Renv { .. } => "resolve and provision the declared renv project",
        EnvironmentDefinition::Nix { .. } => "resolve and realize the declared Nix output",
        EnvironmentDefinition::Oci { .. } => "retrieve the pinned OCI image",
    }
}

fn worker_description(worker: &WorkerDefinition) -> String {
    match worker {
        WorkerDefinition::Interpreted(worker) => format!(
            "{} entrypoint `{}` in environment `{}` with args {:?}",
            interpreted_runner(worker.runner),
            worker.entrypoint.as_path().display(),
            worker.environment,
            worker.args,
        ),
        WorkerDefinition::Command(worker) => format!(
            "command `{}` in environment `{}` with args {:?}",
            worker.program, worker.environment, worker.args,
        ),
    }
}

fn interpreted_runner(runner: InterpretedRunner) -> &'static str {
    match runner {
        InterpretedRunner::Python => "python",
        InterpretedRunner::R => "r",
        InterpretedRunner::Julia => "julia",
    }
}

fn optional_value(value: Option<u64>) -> String {
    value.map_or_else(|| "unspecified".to_owned(), |value| value.to_string())
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

fn implementation_capability(capability: ImplementationCapability) -> &'static str {
    match capability {
        ImplementationCapability::OneShot => "one_shot",
    }
}

fn count(value: usize, noun: &str) -> String {
    if value == 1 {
        format!("{value} {noun}")
    } else {
        format!("{value} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, OutputFormat, render_diagnostic};
    use serde_json::Value;

    #[test]
    fn diagnostics_canonicalize_affected_identity_sets() {
        let first = format!("mw1-logical-candidate-{}", "a".repeat(64));
        let second = format!("mw1-logical-attempt-slot-{}", "b".repeat(64));
        let diagnostic = Diagnostic::new("attempt_failed", "The attempt failed.")
            .with_affected_ids([second.clone(), first.clone(), second]);

        let output = render_diagnostic(&diagnostic, OutputFormat::JsonLines).unwrap();
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            value["data"]["affected_ids"],
            serde_json::json!([
                format!("mw1-logical-attempt-slot-{}", "b".repeat(64)),
                first
            ])
        );
    }
}
