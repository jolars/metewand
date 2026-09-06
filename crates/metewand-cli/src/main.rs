use std::{env, ffi::OsString, fmt, io, io::Write as _, path::PathBuf, process::ExitCode};

use metewand_core::{
    manifest::{
        DatasetDefinition, Enforcement, EnvironmentDefinition, ImplementationCapability,
        InterpretedRunner, PrimaryTime, RunOrder, WorkerDefinition,
    },
    planning::{DatasetConfigurationDefinition, LogicalPlan},
    public_schemas::{PUBLIC_SCHEMAS, PublicSchema},
};
use metewand_runtime::repository::{CheckedRepository, check_repository};

const HELP: &str = "\
Metewand reproducible benchmark runner

Usage:
  metewand schema [SCHEMA]
  metewand check [--manifest PATH]
  metewand plan [--manifest PATH]

Commands:
  schema  List public schemas, or print one exact versioned document.
  check   Validate a benchmark repository without launching workers.
  plan    Print the expanded logical plan without side effects.

Options:
  --manifest PATH  Use PATH instead of ./metewand.toml.
  -h, --help       Print help.
  -V, --version    Print the package version.
";

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(output) => match io::stdout().write_all(output.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: failed to write command output: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<String, CliError> {
    let Some(command) = arguments.first().and_then(|argument| argument.to_str()) else {
        if arguments.is_empty() {
            return Ok(HELP.to_owned());
        }
        return Err(CliError::new("command name is not valid UTF-8"));
    };

    match command {
        "-h" | "--help" | "help" => no_arguments(&arguments[1..]).map(|()| HELP.to_owned()),
        "-V" | "--version" => no_arguments(&arguments[1..])
            .map(|()| format!("metewand {}\n", env!("CARGO_PKG_VERSION"))),
        "schema" => schema_command(&arguments[1..]),
        "check" => {
            if arguments[1..] == ["-h"] || arguments[1..] == ["--help"] {
                return Ok(String::from(
                    "Usage: metewand check [--manifest PATH]\n\nValidate the manifest, contracts, schemas, source bundles, and logical compatibility without launching workers or modifying the repository.\n",
                ));
            }
            if arguments[1..] == ["--workers"] {
                return Err(CliError::new(
                    "`check --workers` is not available until the worker protocol is implemented; no workers were launched",
                ));
            }
            let manifest = manifest_argument(&arguments[1..])?;
            let repository = check_repository(&manifest).map_err(CliError::source)?;
            Ok(render_check(&repository))
        }
        "plan" => {
            if arguments[1..] == ["-h"] || arguments[1..] == ["--help"] {
                return Ok(String::from(
                    "Usage: metewand plan [--manifest PATH]\n\nPrint the deterministic unresolved logical plan without downloads, builds, worker launches, or filesystem writes.\n",
                ));
            }
            let manifest = manifest_argument(&arguments[1..])?;
            let repository = check_repository(&manifest).map_err(CliError::source)?;
            let plan = repository.logical_plan().map_err(CliError::source)?;
            Ok(render_plan(&repository, &plan))
        }
        _ => Err(CliError::new(format!(
            "unknown command `{command}`; run `metewand --help` for usage"
        ))),
    }
}

fn schema_command(arguments: &[OsString]) -> Result<String, CliError> {
    match arguments {
        [] => {
            let mut output = String::from("Public schemas (compatibility version 1):\n");
            for schema in PUBLIC_SCHEMAS {
                use fmt::Write as _;
                writeln!(output, "  {:<29} {}", schema.slug(), schema.id())
                    .expect("writing to a string cannot fail");
            }
            Ok(output)
        }
        [argument] if argument == "-h" || argument == "--help" => Ok(String::from(
            "Usage: metewand schema [SCHEMA]\n\nWithout SCHEMA, list the stable public schema names and identifiers.\n",
        )),
        [argument] => {
            let slug = argument
                .to_str()
                .ok_or_else(|| CliError::new("schema name is not valid UTF-8"))?;
            PublicSchema::from_slug(slug)
                .map(|schema| schema.source().to_owned())
                .ok_or_else(|| {
                    let names = PUBLIC_SCHEMAS
                        .iter()
                        .map(|schema| schema.slug())
                        .collect::<Vec<_>>()
                        .join(", ");
                    CliError::new(format!(
                        "unknown public schema `{slug}`; expected one of {names}"
                    ))
                })
        }
        _ => Err(CliError::new(
            "`metewand schema` accepts at most one schema name",
        )),
    }
}

fn manifest_argument(arguments: &[OsString]) -> Result<PathBuf, CliError> {
    match arguments {
        [] => Ok(PathBuf::from("metewand.toml")),
        [flag, path] if flag == "--manifest" => {
            if path.is_empty() {
                Err(CliError::new("`--manifest` requires a nonempty path"))
            } else {
                Ok(PathBuf::from(path))
            }
        }
        [flag] if flag == "--manifest" => {
            Err(CliError::new("`--manifest` requires a path argument"))
        }
        [argument] => Err(CliError::new(format!(
            "unexpected argument `{}`; use `--manifest PATH` to select a manifest",
            argument.to_string_lossy()
        ))),
        _ => Err(CliError::new(
            "expected no arguments or exactly `--manifest PATH`",
        )),
    }
}

fn no_arguments(arguments: &[OsString]) -> Result<(), CliError> {
    if arguments.is_empty() {
        Ok(())
    } else {
        Err(CliError::new("this option accepts no arguments"))
    }
}

fn render_check(repository: &CheckedRepository) -> String {
    format!(
        "Benchmark `{}` is valid.\n  manifest hash: {}\n  problem contracts: {}\n  repository schemas: {}\n  source bundles: {}\n  worker launches: 0\n",
        repository.manifest.name,
        repository.manifest_hash,
        repository.problem_contracts.len(),
        repository.schema_count,
        repository.source_bundle_count,
    )
}

fn render_plan(repository: &CheckedRepository, plan: &LogicalPlan) -> String {
    use fmt::Write as _;

    let mut output = format!(
        "Plan for benchmark `{}`\nManifest hash: {}\n\nDefinition identities:\n",
        repository.manifest.name, repository.manifest_hash
    );
    for (name, id) in &repository.planning_catalog.environment_definitions {
        writeln!(output, "  environment `{name}`: {id}").expect("writing to a string cannot fail");
    }
    for (definition, id) in &repository.planning_catalog.dataset_definitions {
        match definition {
            DatasetConfigurationDefinition::Unit => {
                writeln!(output, "  dataset `<unit>`: {id}")
                    .expect("writing to a string cannot fail");
            }
            DatasetConfigurationDefinition::Named(name) => {
                writeln!(output, "  dataset `{name}`: {id}")
                    .expect("writing to a string cannot fail");
            }
        }
    }
    for (name, id) in &repository.planning_catalog.problem_definitions {
        writeln!(output, "  problem `{name}`: {id}").expect("writing to a string cannot fail");
    }
    for (name, id) in &repository.planning_catalog.implementation_definitions {
        writeln!(output, "  implementation `{name}`: {id}")
            .expect("writing to a string cannot fail");
    }
    writeln!(output, "\nRequired resource identities:").expect("writing to a string cannot fail");
    for (path, id) in &repository.resources.schemas {
        writeln!(output, "  schema `{}`: {id}", path.display())
            .expect("writing to a string cannot fail");
    }
    for (name, id) in &repository.resources.problem_contracts {
        writeln!(output, "  problem contract `{name}`: {id}")
            .expect("writing to a string cannot fail");
    }
    for (paths, id) in &repository.resources.source_bundles {
        let paths = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output, "  source bundle [{paths}]: {id}")
            .expect("writing to a string cannot fail");
    }
    render_requirements(&mut output, repository);

    let mut candidate_count = 0_usize;
    let mut specification_count = 0_usize;
    let mut observation_count = 0_usize;
    let mut attempt_count = 0_usize;
    for experiment in &plan.experiments {
        writeln!(
            output,
            "\nExperiment `{}`\n  scheduling seed: {}\n  execution policy: {}\n  observation policy: {}",
            experiment.name,
            experiment.scheduling_seed.value,
            experiment.execution_policy.id,
            experiment.observation_policy.id,
        )
        .expect("writing to a string cannot fail");
        for candidate in &experiment.candidates {
            candidate_count += 1;
            writeln!(
                output,
                "  logical candidate {} ({})\n    dataset configuration: {}\n    problem configuration: {}\n    implementation configuration: {}",
                candidate.candidate.id,
                candidate.source,
                candidate.dataset_configuration.id,
                candidate.problem_configuration.id,
                candidate.implementation_configuration.id,
            )
            .expect("writing to a string cannot fail");
            for capability in &candidate.implementation_capabilities {
                writeln!(
                    output,
                    "    capability: {} ({})",
                    implementation_capability(capability.capability),
                    capability.evidence,
                )
                .expect("writing to a string cannot fail");
            }
            for specification in &candidate.specifications {
                specification_count += 1;
                writeln!(
                    output,
                    "    logical specification {}",
                    specification.specification.id
                )
                .expect("writing to a string cannot fail");
                for observation in &specification.observation_slots {
                    observation_count += 1;
                    writeln!(output, "      observation slot {}", observation.id)
                        .expect("writing to a string cannot fail");
                }
                for attempt in &specification.attempt_slots {
                    attempt_count += 1;
                    writeln!(output, "      attempt slot {}", attempt.id)
                        .expect("writing to a string cannot fail");
                }
            }
        }
    }
    writeln!(
        output,
        "\nSummary: {}, {}, {}, {}.\nApplicable runs: {}; execution slots: {}.\nApplicability: {} candidates and {} attempt slots known applicable, 0 unchecked, and no known exclusions.\nCapabilities are declarations only; no workers were launched.\nThis command performed no downloads, builds, or filesystem writes.",
        count(candidate_count, "logical candidate"),
        count(specification_count, "logical specification"),
        count(observation_count, "observation slot"),
        count(attempt_count, "attempt slot"),
        specification_count,
        attempt_count,
        candidate_count,
        attempt_count,
    )
    .expect("writing to a string cannot fail");
    output
}

fn render_requirements(output: &mut String, repository: &CheckedRepository) {
    use fmt::Write as _;

    writeln!(
        output,
        "\nDeclared execution requirements:\n  artifacts: {} contracts, {} schemas, {} source bundles\n  executor: local process (unresolved)",
        repository.problem_contracts.len(),
        repository.schema_count,
        repository.source_bundle_count,
    )
    .expect("writing to a string cannot fail");
    for (name, environment) in &repository.manifest.environments {
        writeln!(
            output,
            "  environment `{name}`: {}",
            environment_requirement(environment)
        )
        .expect("writing to a string cannot fail");
    }
    for (name, dataset) in &repository.manifest.datasets {
        match dataset {
            DatasetDefinition::Fixed(_) => {
                writeln!(output, "  dataset `{name}`: read declared local artifact")
                    .expect("writing to a string cannot fail");
            }
            DatasetDefinition::Generated(dataset) => {
                writeln!(
                    output,
                    "  dataset `{name}` materializer: {}{}",
                    worker_description(&dataset.worker),
                    if dataset.source.is_some() {
                        "; pinned download required"
                    } else {
                        ""
                    }
                )
                .expect("writing to a string cannot fail");
            }
        }
    }
    for (name, problem) in &repository.manifest.problems {
        writeln!(
            output,
            "  problem `{name}` evaluator: {}",
            worker_description(&problem.evaluator)
        )
        .expect("writing to a string cannot fail");
    }
    for (name, implementation) in &repository.manifest.implementations {
        writeln!(
            output,
            "  implementation `{name}` worker: {}",
            worker_description(&implementation.worker)
        )
        .expect("writing to a string cannot fail");
    }
    for experiment in &repository.manifest.experiments {
        let policy = &repository.manifest.execution_policies[&experiment.execution_policy];
        writeln!(
            output,
            "  controls for experiment `{}`: cpus={}, threads={}, memory={}, network={}, worker_reuse={}, warmups={}, timeout={}, timing_scope={}, primary_time={}, run_order={}, enforcement={}",
            experiment.name,
            optional_value(policy.cpus),
            optional_value(policy.threads),
            policy.memory.as_deref().unwrap_or("unspecified"),
            policy.resolved_network(),
            policy.resolved_worker_reuse(),
            policy.resolved_warmup_runs(),
            policy.timeout.as_deref().unwrap_or("unspecified"),
            policy.resolved_timing_scope(),
            primary_time(policy.resolved_primary_time()),
            run_order(policy.resolved_run_order()),
            enforcement(policy.resolved_enforcement()),
        )
        .expect("writing to a string cannot fail");
    }
    writeln!(
        output,
        "  writable paths for this plan command: none\n  downloads, builds, and worker launches for this plan command: none"
    )
    .expect("writing to a string cannot fail");
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

#[derive(Debug)]
struct CliError(String);

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn source(error: impl std::error::Error) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
