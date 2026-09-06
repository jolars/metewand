use std::{path::Path, str::FromStr};

use metewand_core::{
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    identity::{
        IdentityError, IdentityKind, IdentityRecord, canonical_identity_bytes, identify_canonical,
        identify_record,
    },
    manifest::{Enforcement, Name, PrimaryTime, RunOrder, TimingScope, parse_manifest},
    records::{
        DatasetArtifact, DatasetConfigurationRecord, DatasetDefinitionRecord,
        DatasetInstanceRecord, EnvironmentDefinitionKind, EnvironmentDefinitionRecord,
        ExecutionPolicyRecord, ExecutorDefinitionRecord, ImplementationConfigurationRecord,
        ImplementationDefinitionRecord, LogicalAttemptSlotRecord, LogicalCandidateRecord,
        LogicalObservationSlotRecord, MetricsArtifact, ObservationPolicyRecord,
        OneShotLogicalSpecificationRecord, ProblemConfigurationRecord, ProblemContractResource,
        ProblemDefinitionRecord, ProblemInstanceRecord, RecordId, ResolvedAttemptSlotRecord,
        ResolvedEnvironmentRecord, ResolvedLaunchRecord, ResolvedObservationSlotRecord,
        ResolvedOneShotSpecificationRecord, ResolvedWorkerRecord, ResultArtifact, RunAttemptRecord,
        RunObservationRecord, SchemaResource, SourceBundleArtifact, SourceBundleResource,
        WorkerDefinitionRecord,
    },
};

fn name(value: &str) -> Name {
    parse_manifest(
        Path::new("metewand.toml"),
        &format!("version = 1\nname = {value:?}\n"),
    )
    .unwrap()
    .name
}

fn assert_identity_record<T: IdentityRecord>() {}

#[test]
fn every_gate_one_record_and_resource_has_a_stable_kind() {
    assert_identity_record::<EnvironmentDefinitionRecord>();
    assert_identity_record::<WorkerDefinitionRecord>();
    assert_identity_record::<DatasetDefinitionRecord>();
    assert_identity_record::<ProblemDefinitionRecord>();
    assert_identity_record::<ImplementationDefinitionRecord>();
    assert_identity_record::<ExecutionPolicyRecord>();
    assert_identity_record::<ObservationPolicyRecord>();
    assert_identity_record::<DatasetConfigurationRecord>();
    assert_identity_record::<ProblemConfigurationRecord>();
    assert_identity_record::<ImplementationConfigurationRecord>();
    assert_identity_record::<DatasetInstanceRecord>();
    assert_identity_record::<ProblemInstanceRecord>();
    assert_identity_record::<LogicalCandidateRecord>();
    assert_identity_record::<OneShotLogicalSpecificationRecord>();
    assert_identity_record::<LogicalObservationSlotRecord>();
    assert_identity_record::<LogicalAttemptSlotRecord>();
    assert_identity_record::<ResolvedEnvironmentRecord>();
    assert_identity_record::<ResolvedLaunchRecord>();
    assert_identity_record::<ExecutorDefinitionRecord>();
    assert_identity_record::<ResolvedWorkerRecord>();
    assert_identity_record::<ResolvedOneShotSpecificationRecord>();
    assert_identity_record::<ResolvedObservationSlotRecord>();
    assert_identity_record::<ResolvedAttemptSlotRecord>();
    assert_identity_record::<RunObservationRecord>();
    assert_identity_record::<RunAttemptRecord>();

    fn assert_identity_kind<T: IdentityKind>() {}
    assert_identity_kind::<SchemaResource>();
    assert_identity_kind::<ProblemContractResource>();
    assert_identity_kind::<SourceBundleResource>();
    assert_identity_kind::<SourceBundleArtifact>();
    assert_identity_kind::<DatasetArtifact>();
    assert_identity_kind::<ResultArtifact>();
    assert_identity_kind::<MetricsArtifact>();

    assert_eq!(EnvironmentDefinitionRecord::KIND, "environment-definition");
    assert_eq!(DatasetDefinitionRecord::KIND, "dataset-definition");
}

#[test]
fn hashes_the_typed_versioned_canonical_representation() {
    let record = ExecutionPolicyRecord {
        name: name("default"),
        cpus: Some(1),
        threads: None,
        memory: None,
        network: false,
        worker_reuse: false,
        warmup_runs: 0,
        timeout: None,
        timing_scope: TimingScope::PrepareAndExecute,
        primary_time: PrimaryTime::TimedWallTime,
        run_order: RunOrder::Sequential,
        enforcement: Enforcement::BestEffort,
    };

    assert_eq!(
        canonical_identity_bytes(&record).unwrap(),
        br#"{"cpus":1,"enforcement":"best_effort","memory":null,"name":"default","network":false,"primary_time":"timed_wall_time","run_order":"sequential","threads":null,"timeout":null,"timing_scope":"prepare_and_execute","version":1,"warmup_runs":0,"worker_reuse":false}"#
    );
    assert_eq!(
        identify_record(record).unwrap().id.to_string(),
        "mw1-execution-policy-79195d67021c7f14bbf4e2d2acd946b76e2e7e8f895842e56c6403fc1fad6e3f"
    );
}

#[test]
fn canonical_resource_identity_is_typed_and_round_trips() {
    let schema = CanonicalValue::from_slice(br#"{"type":"object"}"#).unwrap();
    let id = identify_canonical::<SchemaResource>(&schema).unwrap();
    let contract = identify_canonical::<ProblemContractResource>(&schema).unwrap();
    let text = id.to_string();

    assert!(text.starts_with("mw1-schema-"));
    assert_ne!(id.digest(), contract.digest());
    assert_eq!(RecordId::<SchemaResource>::from_str(&text).unwrap(), id);
    assert!(RecordId::<EnvironmentDefinitionRecord>::from_str(&text).is_err());
    assert!(RecordId::<SchemaResource>::from_str(&text.to_uppercase()).is_err());

    let serialized = serde_json::to_string(&id).unwrap();
    assert_eq!(serialized, format!("{text:?}"));
    assert_eq!(
        serde_json::from_str::<RecordId<SchemaResource>>(&serialized).unwrap(),
        id
    );
}

#[test]
fn dependency_identity_changes_invalidate_parent_identities() {
    let first_environment = identify_record(EnvironmentDefinitionRecord {
        name: name("runtime"),
        kind: EnvironmentDefinitionKind::Local,
    })
    .unwrap();
    let second_environment = identify_record(EnvironmentDefinitionRecord {
        name: name("runtime-v2"),
        kind: EnvironmentDefinitionKind::Local,
    })
    .unwrap();

    let first = identify_record(metewand_core::records::WorkerDefinitionRecord {
        launch: metewand_core::records::WorkerLaunch::Command {
            program: "worker".to_owned(),
            source_bundle: None,
        },
        args: Vec::new(),
        environment: first_environment.id,
        protocol_transport: None,
    })
    .unwrap();
    let second = identify_record(metewand_core::records::WorkerDefinitionRecord {
        launch: metewand_core::records::WorkerLaunch::Command {
            program: "worker".to_owned(),
            source_bundle: None,
        },
        args: Vec::new(),
        environment: second_environment.id,
        protocol_transport: None,
    })
    .unwrap();

    assert_ne!(first.id, second.id);
    let bytes = String::from_utf8(canonical_identity_bytes(&first.record).unwrap()).unwrap();
    assert!(bytes.contains(&first_environment.id.to_string()));
    assert!(!bytes.contains("RecordId"));
}

#[test]
fn rejects_record_fields_outside_the_canonical_domain() {
    let record = ExecutionPolicyRecord {
        name: name("invalid"),
        cpus: Some(MAX_SAFE_INTEGER + 1),
        threads: None,
        memory: None,
        network: false,
        worker_reuse: false,
        warmup_runs: 0,
        timeout: None,
        timing_scope: TimingScope::PrepareAndExecute,
        primary_time: PrimaryTime::TimedWallTime,
        run_order: RunOrder::Sequential,
        enforcement: Enforcement::BestEffort,
    };

    assert!(matches!(
        identify_record(record),
        Err(IdentityError::Canonical(_))
    ));
}
