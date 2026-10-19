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
        max_concurrency: 1,
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
        br#"{"cpus":1,"enforcement":"best_effort","max_concurrency":1,"memory":null,"name":"default","network":false,"primary_time":"timed_wall_time","run_order":"sequential","threads":null,"timeout":null,"timing_scope":"prepare_and_execute","version":1,"warmup_runs":0,"worker_reuse":false}"#
    );
    assert_eq!(
        identify_record(record).unwrap().id.to_string(),
        "mw1-execution-policy-a7cb0d75d97d065c897a3858debfb8cea2769abb7ab7d5cb80eda49eeffbf486"
    );
}

#[test]
fn concurrency_policy_identities_match_reviewed_version_one_vectors() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/identities/concurrency-v1.json"
    ))
    .unwrap();
    assert_eq!(fixture["identity_version"], metewand_core::IDENTITY_VERSION);
    assert_eq!(
        fixture["execution_semantics_version"],
        metewand_core::EXECUTION_SEMANTICS_VERSION
    );
    let mut policy = ExecutionPolicyRecord {
        name: name("default"),
        max_concurrency: 1,
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
    for vector in fixture["policies"].as_array().unwrap() {
        policy.max_concurrency = vector["max_concurrency"].as_u64().unwrap();
        assert_eq!(
            canonical_identity_bytes(&policy).unwrap(),
            vector["canonical"].as_str().unwrap().as_bytes()
        );
        assert_eq!(
            identify_record(policy.clone()).unwrap().id.to_string(),
            vector["identity"].as_str().unwrap()
        );
    }
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
        max_concurrency: 1,
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

#[test]
fn pre_execution_attempt_identity_rejects_retry_indices_outside_the_canonical_domain() {
    let slot = RecordId::<ResolvedAttemptSlotRecord>::from_digest([1; 32]);
    assert!(metewand_core::identity::run_attempt_id(slot, MAX_SAFE_INTEGER).is_ok());
    assert!(metewand_core::identity::run_attempt_id(slot, MAX_SAFE_INTEGER + 1).is_err());
    assert!(metewand_core::identity::run_attempt_id(slot, u64::MAX).is_err());
}
