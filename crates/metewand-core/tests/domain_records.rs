use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, SystemTime},
};

use metewand_core::{
    canonical::CanonicalValue,
    manifest::{
        DatasetDefinition, Enforcement, ImplementationCapability, Name, ObservationKind,
        PrimaryTime, ProtocolTransport, RepositoryPath, RunOrder, TimingScope, parse_manifest,
    },
    problem_contract::ScientificBudget,
    records::{
        ArtifactReference, AttemptDiagnostic, AttemptSlotRole, CompletionStatus, ContentDigest,
        DatasetArtifact, DatasetConfigurationRecord, DatasetDefinitionKind,
        DatasetDefinitionRecord, DatasetInstanceRecord, DerivedSeedRecord,
        EnvironmentDefinitionKind, EnvironmentDefinitionRecord, ExecutionPolicyRecord,
        ExecutorDefinitionRecord, IdentifiedRecord, ImplementationConfigurationRecord,
        ImplementationDefinitionRecord, LogicalAttemptSlotRecord, LogicalCandidateRecord,
        LogicalObservationSlotRecord, ObservationPolicyRecord, ObservationTimingRecord,
        OneShotLogicalSpecificationRecord, ProblemConfigurationRecord, ProblemDefinitionRecord,
        ProblemInstanceRecord, ProvenanceRecord, RecordId, ResolvedAttemptSlotRecord,
        ResolvedEnvironmentRecord, ResolvedLaunchRecord, ResolvedObservationSlotRecord,
        ResolvedOneShotSpecificationRecord, ResolvedWorkerRecord, ResultArtifact,
        RunAttemptOutcome, RunAttemptRecord, RunObservationRecord, SchemaResource,
        SourceBundleResource, WorkerDefinitionRecord, WorkerLaunch, WorkerWorkingDirectory,
    },
};

fn name(value: &str) -> Name {
    let source = format!("version = 1\nname = {value:?}\n");
    parse_manifest(Path::new("metewand.toml"), &source)
        .unwrap()
        .name
}

fn value(source: &str) -> CanonicalValue {
    CanonicalValue::from_slice(source.as_bytes()).unwrap()
}

fn repository_path(value: &str) -> RepositoryPath {
    let source = format!(
        "version = 1\nname = \"path\"\n[datasets.path]\noutput_schema = {value:?}\nsources = [\"fixture\"]\n"
    );
    let manifest = parse_manifest(Path::new("metewand.toml"), &source).unwrap();
    let DatasetDefinition::Fixed(dataset) = &manifest.datasets["path"] else {
        panic!("expected a fixed dataset");
    };
    dataset.output_schema.clone()
}

fn id<T>(byte: u8) -> RecordId<T> {
    RecordId::from_digest([byte; 32])
}

fn command_worker(
    environment: RecordId<EnvironmentDefinitionRecord>,
    source_bundle: Option<RecordId<SourceBundleResource>>,
) -> WorkerDefinitionRecord {
    WorkerDefinitionRecord {
        launch: WorkerLaunch::Command {
            program: "bin/worker".to_owned(),
            source_bundle,
        },
        args: vec!["--metewand-worker".to_owned()],
        environment,
        protocol_transport: Some(ProtocolTransport::Pipes),
    }
}

#[test]
fn represents_component_definitions_with_typed_dependencies() {
    let environment_id = id::<EnvironmentDefinitionRecord>(1);
    let problem_id = id::<ProblemDefinitionRecord>(2);
    let parameter_schema = id::<SchemaResource>(3);
    let dataset_schema = id::<SchemaResource>(4);
    let source_bundle = id::<SourceBundleResource>(5);

    let environment = EnvironmentDefinitionRecord {
        name: name("local"),
        kind: EnvironmentDefinitionKind::Local,
    };
    let dataset = DatasetDefinitionRecord {
        name: name("generated"),
        parameter_schema: Some(parameter_schema),
        parameter_defaults: Some(value(r#"{"rows":100}"#)),
        output_schema: dataset_schema,
        kind: DatasetDefinitionKind::Generated {
            source: None,
            materializer: command_worker(environment_id, Some(source_bundle)),
        },
    };
    let problem = ProblemDefinitionRecord {
        name: name("regression"),
        contract: id(6),
        parameter_defaults: Some(value(r#"{"centered":true}"#)),
        evaluator: command_worker(environment_id, Some(source_bundle)),
    };
    let implementation = ImplementationDefinitionRecord {
        name: name("solver"),
        worker: command_worker(environment_id, Some(source_bundle)),
        problem_contracts: vec![problem_id],
        parameter_schema: Some(parameter_schema),
        parameter_defaults: None,
        capabilities: vec![ImplementationCapability::OneShot],
    };

    assert!(matches!(environment.kind, EnvironmentDefinitionKind::Local));
    assert_eq!(dataset.output_schema.digest(), &[4; 32]);
    assert_eq!(problem.contract.digest(), &[6; 32]);
    assert_eq!(implementation.problem_contracts, [problem_id]);
}

#[test]
fn separates_logical_records_from_resolved_runtime_dependencies() {
    let dataset_configuration = DatasetConfigurationRecord {
        definition: id::<DatasetDefinitionRecord>(10),
        parameters: value(r#"{"rows":100}"#),
        seed: DerivedSeedRecord {
            value: 2025,
            derivation_digest: ContentDigest::new([11; 32]),
        },
    };
    let problem_configuration = ProblemConfigurationRecord {
        definition: id::<ProblemDefinitionRecord>(12),
        parameters: value(r#"{"lambda":0.1}"#),
    };
    let implementation_configuration = ImplementationConfigurationRecord {
        definition: id::<ImplementationDefinitionRecord>(13),
        parameters: value(r#"{"tolerance":1e-8}"#),
        environment: id::<EnvironmentDefinitionRecord>(14),
    };
    let dataset_instance = DatasetInstanceRecord {
        configuration: id::<DatasetConfigurationRecord>(15),
        artifact: id::<DatasetArtifact>(16),
        output_schema: id::<SchemaResource>(17),
    };
    let problem_instance = ProblemInstanceRecord {
        dataset: id::<DatasetInstanceRecord>(18),
        configuration: id::<ProblemConfigurationRecord>(19),
    };
    let execution_policy = ExecutionPolicyRecord {
        name: name("controlled"),
        cpus: Some(1),
        threads: Some(1),
        memory: Some("8 GiB".to_owned()),
        network: false,
        worker_reuse: false,
        warmup_runs: 0,
        timeout: Some("10 min".to_owned()),
        timing_scope: TimingScope::PrepareAndExecute,
        primary_time: PrimaryTime::TimedWallTime,
        run_order: RunOrder::Randomized,
        enforcement: Enforcement::BestEffort,
    };
    let observation_policy = ObservationPolicyRecord {
        name: name("final"),
        kind: ObservationKind::OneShot,
    };
    let candidate = LogicalCandidateRecord {
        dataset_configuration: id::<DatasetConfigurationRecord>(20),
        problem_configuration: id::<ProblemConfigurationRecord>(21),
        implementation_configuration: id::<ImplementationConfigurationRecord>(22),
        execution_policy: id::<ExecutionPolicyRecord>(23),
        observation_policy: id::<ObservationPolicyRecord>(24),
    };
    let specification = OneShotLogicalSpecificationRecord {
        candidate: id::<LogicalCandidateRecord>(25),
        scientific_budget: ScientificBudget::None,
        implementation_repetition: 2,
        implementation_seed: DerivedSeedRecord {
            value: 404,
            derivation_digest: ContentDigest::new([26; 32]),
        },
    };
    let observation_slot = LogicalObservationSlotRecord {
        specification: id::<OneShotLogicalSpecificationRecord>(27),
        measurement_index: 3,
    };
    let measured_slot = LogicalAttemptSlotRecord {
        specification: id::<OneShotLogicalSpecificationRecord>(27),
        role: AttemptSlotRole::Measured {
            observation_slot: id::<LogicalObservationSlotRecord>(28),
        },
        scheduling_priority: ContentDigest::new([29; 32]),
    };
    let warmup_slot = LogicalAttemptSlotRecord {
        specification: id::<OneShotLogicalSpecificationRecord>(27),
        role: AttemptSlotRole::Warmup { warmup_index: 0 },
        scheduling_priority: ContentDigest::new([30; 32]),
    };

    let resolved_environment = ResolvedEnvironmentRecord {
        definition: id::<EnvironmentDefinitionRecord>(31),
        fingerprint: value(r#"{"kind":"local"}"#),
    };
    let resolved_launch = ResolvedLaunchRecord {
        environment: id::<ResolvedEnvironmentRecord>(32),
        program: "/nix/store/example/bin/worker".to_owned(),
        args: vec!["--metewand-worker".to_owned()],
        environment_variables: BTreeMap::from([("LANG".to_owned(), "C.UTF-8".to_owned())]),
        working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
    };
    let executor = ExecutorDefinitionRecord {
        kind: name("local-process"),
        configuration: value(r#"{"serial":true}"#),
    };
    let resolved_specification = ResolvedOneShotSpecificationRecord {
        logical_specification: id::<OneShotLogicalSpecificationRecord>(33),
        problem_instance: id::<ProblemInstanceRecord>(34),
        implementation: ResolvedWorkerRecord {
            source_bundle: Some(id(35)),
            environment: id::<ResolvedEnvironmentRecord>(36),
            launch: id::<ResolvedLaunchRecord>(37),
        },
        evaluator: ResolvedWorkerRecord {
            source_bundle: Some(id(38)),
            environment: id::<ResolvedEnvironmentRecord>(39),
            launch: id::<ResolvedLaunchRecord>(40),
        },
        executor: id::<ExecutorDefinitionRecord>(41),
        wire_protocol_version: 1,
        execution_semantics_version: 1,
    };
    let resolved_observation_slot = ResolvedObservationSlotRecord {
        logical_slot: id::<LogicalObservationSlotRecord>(42),
        specification: id::<ResolvedOneShotSpecificationRecord>(43),
    };
    let resolved_attempt_slot = ResolvedAttemptSlotRecord {
        logical_slot: id::<LogicalAttemptSlotRecord>(44),
        specification: id::<ResolvedOneShotSpecificationRecord>(43),
    };

    assert_eq!(
        dataset_configuration.seed.derivation_digest.bytes(),
        &[11; 32]
    );
    assert_eq!(problem_configuration.definition.digest(), &[12; 32]);
    assert_eq!(implementation_configuration.environment.digest(), &[14; 32]);
    assert_eq!(dataset_instance.artifact.digest(), &[16; 32]);
    assert_eq!(problem_instance.configuration.digest(), &[19; 32]);
    assert_eq!(
        execution_policy.timing_scope,
        TimingScope::PrepareAndExecute
    );
    assert_eq!(observation_policy.kind, ObservationKind::OneShot);
    assert_eq!(candidate.execution_policy.digest(), &[23; 32]);
    assert_eq!(specification.implementation_repetition, 2);
    assert_eq!(observation_slot.measurement_index, 3);
    assert!(matches!(
        measured_slot.role,
        AttemptSlotRole::Measured { .. }
    ));
    assert!(matches!(
        warmup_slot.role,
        AttemptSlotRole::Warmup { warmup_index: 0 }
    ));
    assert_eq!(resolved_environment.definition.digest(), &[31; 32]);
    assert_eq!(resolved_launch.environment.digest(), &[32; 32]);
    assert_eq!(executor.kind.as_str(), "local-process");
    assert_eq!(resolved_specification.problem_instance.digest(), &[34; 32]);
    assert_eq!(resolved_specification.executor.digest(), &[41; 32]);
    assert_eq!(resolved_observation_slot.logical_slot.digest(), &[42; 32]);
    assert_eq!(resolved_attempt_slot.logical_slot.digest(), &[44; 32]);
}

#[test]
fn records_one_shot_observations_and_terminal_attempts_without_losing_failures() {
    let observation_id = id::<RunObservationRecord>(50);
    let provenance = ProvenanceRecord {
        tool_version: "0.1.0".to_owned(),
        wire_protocol_version: 1,
        execution_semantics_version: 1,
        data: value(r#"{"environment_class":"development"}"#),
    };
    let observation = RunObservationRecord {
        slot: id::<ResolvedObservationSlotRecord>(51),
        producing_attempt: id::<RunAttemptRecord>(52),
        result: ArtifactReference::<ResultArtifact> {
            id: id(53),
            path: repository_path("result"),
            sha256: ContentDigest::new([54; 32]),
        },
        metrics: ArtifactReference {
            id: id(55),
            path: repository_path("metrics.json"),
            sha256: ContentDigest::new([56; 32]),
        },
        timing: ObservationTimingRecord {
            primary_time: PrimaryTime::TimedWallTime,
            timed_wall_time: Duration::from_millis(10),
            implementation_time: Some(Duration::from_millis(8)),
            evaluation_time: Duration::from_millis(1),
            cpu_time: None,
            peak_rss_bytes: Some(1024),
        },
        completion: CompletionStatus::Reached,
        provenance: provenance.clone(),
    };
    let identified_observation = IdentifiedRecord {
        id: observation_id,
        record: observation.clone(),
    };
    let accepted = RunAttemptRecord {
        slot: id::<ResolvedAttemptSlotRecord>(57),
        retry_index: 0,
        outcome: RunAttemptOutcome::Accepted {
            observation: observation_id,
        },
        started_at: SystemTime::UNIX_EPOCH,
        finished_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        provenance: provenance.clone(),
    };
    let retained_observation_id = id::<RunObservationRecord>(58);
    let not_reached_observation = RunObservationRecord {
        producing_attempt: id::<RunAttemptRecord>(59),
        completion: CompletionStatus::NotReached,
        ..observation.clone()
    };
    let failed = RunAttemptRecord {
        slot: id::<ResolvedAttemptSlotRecord>(57),
        retry_index: 1,
        outcome: RunAttemptOutcome::Failed {
            diagnostic: AttemptDiagnostic {
                code: "completion_not_reached".to_owned(),
                message: "the valid result did not reach the required target".to_owned(),
                details: None,
            },
            observation: Some(retained_observation_id),
        },
        started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
        finished_at: SystemTime::UNIX_EPOCH + Duration::from_secs(3),
        provenance,
    };

    assert_eq!(observation.completion, CompletionStatus::Reached);
    assert_eq!(identified_observation.id, observation_id);
    assert_eq!(
        not_reached_observation.completion,
        CompletionStatus::NotReached
    );
    assert!(matches!(
        accepted.outcome,
        RunAttemptOutcome::Accepted { observation } if observation == observation_id
    ));
    assert!(matches!(
        failed.outcome,
        RunAttemptOutcome::Failed {
            ref diagnostic,
            observation: Some(retained),
        } if diagnostic.code == "completion_not_reached" && retained == retained_observation_id
    ));
}
