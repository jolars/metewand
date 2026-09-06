use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, SystemTime},
};

use metewand_core::{
    IDENTITY_VERSION,
    canonical::CanonicalValue,
    identity::{
        CanonicalResource, IdentityRecord, canonical_identity_bytes,
        canonical_resource_identity_bytes, identify_canonical, identify_record,
    },
    manifest::{
        DatasetDefinition, Enforcement, ImplementationCapability, Name, ObservationKind,
        PrimaryTime, ProtocolTransport, RepositoryPath, RunOrder, TimingScope, parse_manifest,
    },
    problem_contract::ScientificBudget,
    records::{
        ArtifactReference, AttemptSlotRole, CompletionStatus, ContentDigest, DatasetArtifact,
        DatasetConfigurationRecord, DatasetDefinitionKind, DatasetDefinitionRecord,
        DatasetInstanceRecord, EnvironmentDefinitionKind, EnvironmentDefinitionRecord,
        ExecutionPolicyRecord, ExecutorDefinitionRecord, ImplementationConfigurationRecord,
        ImplementationDefinitionRecord, LogicalAttemptSlotRecord, LogicalCandidateRecord,
        LogicalObservationSlotRecord, MetricsArtifact, ObservationPolicyRecord,
        ObservationTimingRecord, OneShotLogicalSpecificationRecord, ProblemConfigurationRecord,
        ProblemContractResource, ProblemDefinitionRecord, ProblemInstanceRecord, ProvenanceRecord,
        RecordId, RemoteSourceRecord, ResolvedAttemptSlotRecord, ResolvedEnvironmentRecord,
        ResolvedLaunchRecord, ResolvedObservationSlotRecord, ResolvedOneShotSpecificationRecord,
        ResolvedWorkerRecord, ResultArtifact, RunAttemptOutcome, RunAttemptRecord,
        RunObservationRecord, SchemaResource, SourceBundleArtifact, SourceBundleResource,
        WorkerDefinitionRecord, WorkerLaunch, WorkerWorkingDirectory,
    },
    seed::{derive_dataset_seed, derive_implementation_seed},
};
use serde::{Deserialize, Serialize};

const GOLDEN_VECTORS: &str = include_str!("../../../fixtures/identities/v1.json");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct GoldenVector {
    kind: String,
    canonical: String,
    identity: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenFile {
    version: u32,
    vectors: BTreeMap<String, GoldenVector>,
}

#[derive(Clone, Copy)]
enum ObjectOrder {
    Forward,
    Reverse,
}

struct IdentityGraph {
    vectors: BTreeMap<String, GoldenVector>,
}

fn name(value: &str) -> Name {
    parse_manifest(
        Path::new("metewand.toml"),
        &format!("version = 1\nname = {value:?}\n"),
    )
    .unwrap()
    .name
}

fn canonical(source: &str) -> CanonicalValue {
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

fn opaque_id<T>(byte: u8) -> RecordId<T> {
    RecordId::from_digest([byte; 32])
}

fn ordered<'a>(order: ObjectOrder, forward: &'a str, reverse: &'a str) -> &'a str {
    match order {
        ObjectOrder::Forward => forward,
        ObjectOrder::Reverse => reverse,
    }
}

fn is_mutated(mutation: Option<&str>, label: &str) -> bool {
    mutation == Some(label)
}

fn add_resource<T: CanonicalResource>(
    vectors: &mut BTreeMap<String, GoldenVector>,
    label: &str,
    value: &CanonicalValue,
) -> RecordId<T> {
    let id = identify_canonical::<T>(value).unwrap();
    vectors.insert(
        label.to_owned(),
        GoldenVector {
            kind: T::KIND.to_owned(),
            canonical: String::from_utf8(canonical_resource_identity_bytes(value).unwrap())
                .unwrap(),
            identity: id.to_string(),
        },
    );
    id
}

fn add_record<T: IdentityRecord>(
    vectors: &mut BTreeMap<String, GoldenVector>,
    label: &str,
    record: T,
) -> RecordId<T> {
    let canonical = String::from_utf8(canonical_identity_bytes(&record).unwrap()).unwrap();
    let identified = identify_record(record).unwrap();
    vectors.insert(
        label.to_owned(),
        GoldenVector {
            kind: T::KIND.to_owned(),
            canonical,
            identity: identified.id.to_string(),
        },
    );
    identified.id
}

fn graph(mutation: Option<&str>, order: ObjectOrder) -> IdentityGraph {
    let mut vectors = BTreeMap::new();

    let parameter_schema_value = if is_mutated(mutation, "parameter-schema") {
        canonical(r#"{"properties":{"rows":{"maximum":1000,"type":"integer"}},"type":"object"}"#)
    } else {
        canonical(ordered(
            order,
            r#"{"type":"object","properties":{"rows":{"type":"integer"}}}"#,
            r#"{"properties":{"rows":{"type":"integer"}},"type":"object"}"#,
        ))
    };
    let parameter_schema =
        add_resource::<SchemaResource>(&mut vectors, "parameter-schema", &parameter_schema_value);

    let output_schema_value = if is_mutated(mutation, "output-schema") {
        canonical(r#"{"properties":{"y":{"type":"array"}},"required":["y"],"type":"object"}"#)
    } else {
        canonical(ordered(
            order,
            r#"{"type":"object","properties":{"x":{"type":"array"}},"required":["x"]}"#,
            r#"{"required":["x"],"properties":{"x":{"type":"array"}},"type":"object"}"#,
        ))
    };
    let output_schema =
        add_resource::<SchemaResource>(&mut vectors, "output-schema", &output_schema_value);

    let contract_value = if is_mutated(mutation, "problem-contract") {
        canonical(r#"{"family":"regression","version":2}"#)
    } else {
        canonical(ordered(
            order,
            r#"{"version":1,"family":"regression"}"#,
            r#"{"family":"regression","version":1}"#,
        ))
    };
    let problem_contract =
        add_resource::<ProblemContractResource>(&mut vectors, "problem-contract", &contract_value);

    let source_bundle_value = if is_mutated(mutation, "source-bundle") {
        canonical(r#"{"files":[{"path":"worker.py","sha256":"changed"}]}"#)
    } else {
        canonical(r#"{"files":[{"path":"worker.py","sha256":"source"}]}"#)
    };
    let source_bundle =
        add_resource::<SourceBundleResource>(&mut vectors, "source-bundle", &source_bundle_value);

    let source_artifact_value = if is_mutated(mutation, "source-artifact") {
        canonical(r#"{"files":["worker-v2.py"],"role":"source"}"#)
    } else {
        canonical(r#"{"files":["worker.py"],"role":"source"}"#)
    };
    let source_artifact = add_resource::<SourceBundleArtifact>(
        &mut vectors,
        "source-artifact",
        &source_artifact_value,
    );

    let dataset_artifact_value = if is_mutated(mutation, "dataset-artifact") {
        canonical(r#"{"files":["dataset-v2.json"],"role":"dataset"}"#)
    } else {
        canonical(r#"{"files":["dataset.json"],"role":"dataset"}"#)
    };
    let dataset_artifact =
        add_resource::<DatasetArtifact>(&mut vectors, "dataset-artifact", &dataset_artifact_value);

    let result_artifact_value = if is_mutated(mutation, "result-artifact") {
        canonical(r#"{"files":["result-v2.json"],"role":"result"}"#)
    } else {
        canonical(r#"{"files":["result.json"],"role":"result"}"#)
    };
    let result_artifact =
        add_resource::<ResultArtifact>(&mut vectors, "result-artifact", &result_artifact_value);

    let metrics_artifact_value = if is_mutated(mutation, "metrics-artifact") {
        canonical(r#"{"files":["metrics-v2.json"],"role":"metrics"}"#)
    } else {
        canonical(r#"{"files":["metrics.json"],"role":"metrics"}"#)
    };
    let metrics_artifact =
        add_resource::<MetricsArtifact>(&mut vectors, "metrics-artifact", &metrics_artifact_value);

    let environment = add_record(
        &mut vectors,
        "environment-definition",
        EnvironmentDefinitionRecord {
            name: name(if is_mutated(mutation, "environment-definition") {
                "runtime-v2"
            } else {
                "runtime"
            }),
            kind: EnvironmentDefinitionKind::Local,
        },
    );

    let worker = WorkerDefinitionRecord {
        launch: WorkerLaunch::Command {
            program: if is_mutated(mutation, "worker-definition") {
                "bin/worker-v2".to_owned()
            } else {
                "bin/worker".to_owned()
            },
            source_bundle: Some(source_bundle),
        },
        args: vec!["--metewand-worker".to_owned()],
        environment,
        protocol_transport: Some(ProtocolTransport::Pipes),
    };
    add_record(&mut vectors, "worker-definition", worker.clone());

    let dataset_definition = add_record(
        &mut vectors,
        "dataset-definition",
        DatasetDefinitionRecord {
            name: name(if is_mutated(mutation, "dataset-definition") {
                "generated-v2"
            } else {
                "generated"
            }),
            parameter_schema: Some(parameter_schema),
            parameter_defaults: Some(canonical(ordered(
                order,
                r#"{"rows":100,"distribution":"normal"}"#,
                r#"{"distribution":"normal","rows":100}"#,
            ))),
            output_schema,
            kind: DatasetDefinitionKind::Generated {
                source: Some(RemoteSourceRecord {
                    url: "https://example.invalid/dataset".to_owned(),
                    sha256: ContentDigest::new([0x11; 32]),
                }),
                materializer: worker.clone(),
            },
        },
    );

    let problem_definition = add_record(
        &mut vectors,
        "problem-definition",
        ProblemDefinitionRecord {
            name: name(if is_mutated(mutation, "problem-definition") {
                "regression-v2"
            } else {
                "regression"
            }),
            contract: problem_contract,
            parameter_defaults: Some(canonical(ordered(
                order,
                r#"{"fit_intercept":true,"lambda":0.1}"#,
                r#"{"lambda":0.1,"fit_intercept":true}"#,
            ))),
            evaluator: worker.clone(),
        },
    );

    let mut problem_contracts = vec![problem_definition, opaque_id(0x32)];
    if matches!(order, ObjectOrder::Reverse) {
        problem_contracts.reverse();
    }
    let implementation_definition = add_record(
        &mut vectors,
        "implementation-definition",
        ImplementationDefinitionRecord {
            name: name(if is_mutated(mutation, "implementation-definition") {
                "solver-v2"
            } else {
                "solver"
            }),
            worker,
            problem_contracts,
            parameter_schema: Some(parameter_schema),
            parameter_defaults: Some(canonical(r#"{"tolerance":1e-8}"#)),
            capabilities: vec![ImplementationCapability::OneShot],
        },
    );

    let execution_policy = add_record(
        &mut vectors,
        "execution-policy",
        ExecutionPolicyRecord {
            name: name("controlled"),
            cpus: Some(if is_mutated(mutation, "execution-policy") {
                2
            } else {
                1
            }),
            threads: Some(1),
            memory: Some("8 GiB".to_owned()),
            network: false,
            worker_reuse: false,
            warmup_runs: 1,
            timeout: Some("10 min".to_owned()),
            timing_scope: TimingScope::PrepareAndExecute,
            primary_time: PrimaryTime::TimedWallTime,
            run_order: RunOrder::Randomized,
            enforcement: Enforcement::BestEffort,
        },
    );

    let observation_policy = add_record(
        &mut vectors,
        "observation-policy",
        ObservationPolicyRecord {
            name: name(if is_mutated(mutation, "observation-policy") {
                "final-v2"
            } else {
                "final"
            }),
            kind: ObservationKind::OneShot,
        },
    );

    let dataset_parameters = canonical(if is_mutated(mutation, "dataset-configuration") {
        r#"{"distribution":"normal","rows":200}"#
    } else {
        ordered(
            order,
            r#"{"rows":100,"distribution":"normal"}"#,
            r#"{"distribution":"normal","rows":100}"#,
        )
    });
    let dataset_seed = derive_dataset_seed(2025, dataset_definition, &dataset_parameters).unwrap();
    let dataset_configuration = add_record(
        &mut vectors,
        "dataset-configuration",
        DatasetConfigurationRecord {
            definition: dataset_definition,
            parameters: dataset_parameters,
            seed: dataset_seed,
        },
    );

    let problem_configuration = add_record(
        &mut vectors,
        "problem-configuration",
        ProblemConfigurationRecord {
            definition: problem_definition,
            parameters: canonical(if is_mutated(mutation, "problem-configuration") {
                r#"{"fit_intercept":true,"lambda":0.2}"#
            } else {
                ordered(
                    order,
                    r#"{"lambda":0.1,"fit_intercept":true}"#,
                    r#"{"fit_intercept":true,"lambda":0.1}"#,
                )
            }),
        },
    );

    let implementation_configuration = add_record(
        &mut vectors,
        "implementation-configuration",
        ImplementationConfigurationRecord {
            definition: implementation_definition,
            parameters: canonical(if is_mutated(mutation, "implementation-configuration") {
                r#"{"method":"cd","tolerance":1e-7}"#
            } else {
                ordered(
                    order,
                    r#"{"tolerance":1e-8,"method":"cd"}"#,
                    r#"{"method":"cd","tolerance":1e-8}"#,
                )
            }),
            environment,
        },
    );

    let dataset_instance = add_record(
        &mut vectors,
        "dataset-instance",
        DatasetInstanceRecord {
            configuration: dataset_configuration,
            artifact: if is_mutated(mutation, "dataset-instance") {
                opaque_id(0x41)
            } else {
                dataset_artifact
            },
            output_schema,
        },
    );

    let problem_instance = add_record(
        &mut vectors,
        "problem-instance",
        ProblemInstanceRecord {
            dataset: if is_mutated(mutation, "problem-instance") {
                opaque_id(0x42)
            } else {
                dataset_instance
            },
            configuration: problem_configuration,
        },
    );

    let logical_candidate = add_record(
        &mut vectors,
        "logical-candidate",
        LogicalCandidateRecord {
            dataset_configuration,
            problem_configuration,
            implementation_configuration,
            execution_policy: if is_mutated(mutation, "logical-candidate") {
                opaque_id(0x43)
            } else {
                execution_policy
            },
            observation_policy,
        },
    );

    let implementation_repetition = if is_mutated(mutation, "logical-specification") {
        1
    } else {
        0
    };
    let implementation_seed = derive_implementation_seed(
        2025,
        dataset_configuration,
        problem_configuration,
        implementation_repetition,
    )
    .unwrap();
    let logical_specification = add_record(
        &mut vectors,
        "logical-specification",
        OneShotLogicalSpecificationRecord {
            candidate: logical_candidate,
            scientific_budget: ScientificBudget::None,
            implementation_repetition,
            implementation_seed,
        },
    );

    let logical_observation_slot = add_record(
        &mut vectors,
        "logical-observation-slot",
        LogicalObservationSlotRecord {
            specification: logical_specification,
            measurement_index: if is_mutated(mutation, "logical-observation-slot") {
                1
            } else {
                0
            },
        },
    );

    let logical_attempt_slot = add_record(
        &mut vectors,
        "logical-attempt-slot",
        LogicalAttemptSlotRecord {
            specification: logical_specification,
            role: AttemptSlotRole::Measured {
                observation_slot: logical_observation_slot,
            },
            scheduling_priority: ContentDigest::new(
                [if is_mutated(mutation, "logical-attempt-slot") {
                    0x45
                } else {
                    0x44
                }; 32],
            ),
        },
    );

    let resolved_environment = add_record(
        &mut vectors,
        "resolved-environment",
        ResolvedEnvironmentRecord {
            definition: environment,
            fingerprint: canonical(if is_mutated(mutation, "resolved-environment") {
                r#"{"kind":"local","platform":"aarch64-linux"}"#
            } else {
                ordered(
                    order,
                    r#"{"platform":"x86_64-linux","kind":"local"}"#,
                    r#"{"kind":"local","platform":"x86_64-linux"}"#,
                )
            }),
        },
    );

    let environment_variables = match order {
        ObjectOrder::Forward => BTreeMap::from([
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("OMP_NUM_THREADS".to_owned(), "1".to_owned()),
        ]),
        ObjectOrder::Reverse => BTreeMap::from([
            ("OMP_NUM_THREADS".to_owned(), "1".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ]),
    };
    let resolved_launch = add_record(
        &mut vectors,
        "resolved-launch",
        ResolvedLaunchRecord {
            environment: resolved_environment,
            program: if is_mutated(mutation, "resolved-launch") {
                "/nix/store/worker-v2/bin/worker".to_owned()
            } else {
                "/nix/store/worker/bin/worker".to_owned()
            },
            args: vec!["--metewand-worker".to_owned()],
            environment_variables,
            working_directory: WorkerWorkingDirectory::PrivateRunDirectory,
        },
    );

    let executor = add_record(
        &mut vectors,
        "executor-definition",
        ExecutorDefinitionRecord {
            kind: name("local-process"),
            configuration: canonical(if is_mutated(mutation, "executor-definition") {
                r#"{"capture_output":true,"serial":false}"#
            } else {
                ordered(
                    order,
                    r#"{"serial":true,"capture_output":true}"#,
                    r#"{"capture_output":true,"serial":true}"#,
                )
            }),
        },
    );

    let resolved_worker = ResolvedWorkerRecord {
        source_bundle: Some(if is_mutated(mutation, "resolved-worker") {
            opaque_id(0x46)
        } else {
            source_artifact
        }),
        environment: resolved_environment,
        launch: resolved_launch,
    };
    add_record(&mut vectors, "resolved-worker", resolved_worker.clone());

    let resolved_specification = add_record(
        &mut vectors,
        "resolved-specification",
        ResolvedOneShotSpecificationRecord {
            logical_specification,
            problem_instance,
            implementation: resolved_worker.clone(),
            evaluator: resolved_worker,
            executor,
            wire_protocol_version: if is_mutated(mutation, "resolved-specification") {
                2
            } else {
                1
            },
            execution_semantics_version: 1,
        },
    );

    let resolved_observation_slot = add_record(
        &mut vectors,
        "resolved-observation-slot",
        ResolvedObservationSlotRecord {
            logical_slot: if is_mutated(mutation, "resolved-observation-slot") {
                opaque_id(0x47)
            } else {
                logical_observation_slot
            },
            specification: resolved_specification,
        },
    );

    let resolved_attempt_slot = add_record(
        &mut vectors,
        "resolved-attempt-slot",
        ResolvedAttemptSlotRecord {
            logical_slot: if is_mutated(mutation, "resolved-attempt-slot") {
                opaque_id(0x48)
            } else {
                logical_attempt_slot
            },
            specification: resolved_specification,
        },
    );

    let provenance = ProvenanceRecord {
        tool_version: "0.1.0".to_owned(),
        wire_protocol_version: 1,
        execution_semantics_version: 1,
        data: canonical(r#"{"host":"fixture"}"#),
    };
    let run_attempt = add_record(
        &mut vectors,
        "attempt",
        RunAttemptRecord {
            slot: resolved_attempt_slot,
            retry_index: if is_mutated(mutation, "attempt") {
                1
            } else {
                0
            },
            outcome: RunAttemptOutcome::Accepted {
                observation: opaque_id(0x49),
            },
            started_at: SystemTime::UNIX_EPOCH,
            finished_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            provenance: provenance.clone(),
        },
    );

    add_record(
        &mut vectors,
        "observation",
        RunObservationRecord {
            slot: resolved_observation_slot,
            producing_attempt: run_attempt,
            result: ArtifactReference {
                id: result_artifact,
                path: repository_path("result.json"),
                sha256: ContentDigest::new([0x51; 32]),
            },
            metrics: ArtifactReference {
                id: metrics_artifact,
                path: repository_path("metrics.json"),
                sha256: ContentDigest::new([0x52; 32]),
            },
            timing: ObservationTimingRecord {
                primary_time: PrimaryTime::TimedWallTime,
                timed_wall_time: Duration::new(2, 3),
                implementation_time: Some(Duration::new(1, 2)),
                evaluation_time: Duration::new(0, 4),
                cpu_time: Some(Duration::new(1, 5)),
                peak_rss_bytes: Some(4096),
            },
            completion: if is_mutated(mutation, "observation") {
                CompletionStatus::NotReached
            } else {
                CompletionStatus::Reached
            },
            provenance,
        },
    );

    IdentityGraph { vectors }
}

#[test]
fn version_one_golden_vectors_pin_the_complete_identity_graph() {
    let golden: GoldenFile = serde_json::from_str(GOLDEN_VECTORS).unwrap();
    let actual = graph(None, ObjectOrder::Forward).vectors;

    assert_eq!(golden.version, IDENTITY_VERSION);
    assert_eq!(
        actual,
        golden.vectors,
        "actual vectors:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}

#[test]
fn nonsemantic_ordering_preserves_every_identity() {
    let forward = graph(None, ObjectOrder::Forward).vectors;
    let reverse = graph(None, ObjectOrder::Reverse).vectors;

    assert_eq!(forward, reverse);
}

const DEPENDENCIES: &[(&str, &[&str])] = &[
    ("parameter-schema", &[]),
    ("output-schema", &[]),
    ("problem-contract", &[]),
    ("source-bundle", &[]),
    ("source-artifact", &[]),
    ("dataset-artifact", &[]),
    ("result-artifact", &[]),
    ("metrics-artifact", &[]),
    ("environment-definition", &[]),
    (
        "worker-definition",
        &["environment-definition", "source-bundle"],
    ),
    (
        "dataset-definition",
        &["parameter-schema", "output-schema", "worker-definition"],
    ),
    (
        "problem-definition",
        &["problem-contract", "worker-definition"],
    ),
    (
        "implementation-definition",
        &[
            "parameter-schema",
            "problem-definition",
            "worker-definition",
        ],
    ),
    ("execution-policy", &[]),
    ("observation-policy", &[]),
    ("dataset-configuration", &["dataset-definition"]),
    ("problem-configuration", &["problem-definition"]),
    (
        "implementation-configuration",
        &["implementation-definition", "environment-definition"],
    ),
    (
        "dataset-instance",
        &["dataset-configuration", "dataset-artifact", "output-schema"],
    ),
    (
        "problem-instance",
        &["dataset-instance", "problem-configuration"],
    ),
    (
        "logical-candidate",
        &[
            "dataset-configuration",
            "problem-configuration",
            "implementation-configuration",
            "execution-policy",
            "observation-policy",
        ],
    ),
    ("logical-specification", &["logical-candidate"]),
    ("logical-observation-slot", &["logical-specification"]),
    (
        "logical-attempt-slot",
        &["logical-specification", "logical-observation-slot"],
    ),
    ("resolved-environment", &["environment-definition"]),
    ("resolved-launch", &["resolved-environment"]),
    ("executor-definition", &[]),
    (
        "resolved-worker",
        &["source-artifact", "resolved-environment", "resolved-launch"],
    ),
    (
        "resolved-specification",
        &[
            "logical-specification",
            "problem-instance",
            "resolved-worker",
            "executor-definition",
        ],
    ),
    (
        "resolved-observation-slot",
        &["logical-observation-slot", "resolved-specification"],
    ),
    (
        "resolved-attempt-slot",
        &["logical-attempt-slot", "resolved-specification"],
    ),
    ("attempt", &["resolved-attempt-slot"]),
    (
        "observation",
        &[
            "resolved-observation-slot",
            "attempt",
            "result-artifact",
            "metrics-artifact",
        ],
    ),
];

fn transitive_dependents(changed: &str) -> BTreeSet<String> {
    let mut affected = BTreeSet::from([changed.to_owned()]);
    loop {
        let before = affected.len();
        for (node, dependencies) in DEPENDENCIES {
            if dependencies
                .iter()
                .any(|dependency| affected.contains(*dependency))
            {
                affected.insert((*node).to_owned());
            }
        }
        if affected.len() == before {
            return affected;
        }
    }
}

#[test]
fn every_dependency_change_invalidates_exactly_its_transitive_dependents() {
    let baseline = graph(None, ObjectOrder::Forward).vectors;
    assert_eq!(
        baseline.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        DEPENDENCIES
            .iter()
            .map(|(node, _)| *node)
            .collect::<BTreeSet<_>>()
    );

    for changed in baseline.keys() {
        let mutated = graph(Some(changed), ObjectOrder::Forward).vectors;
        let actual = baseline
            .iter()
            .filter_map(|(node, vector)| (mutated[node] != *vector).then_some(node.clone()))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual,
            transitive_dependents(changed),
            "unexpected invalidation after changing {changed}"
        );
    }
}
