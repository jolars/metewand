use std::str::FromStr;

use metewand_core::{
    SCHEDULING_POLICY_VERSION, SEED_DERIVATION_VERSION,
    canonical::{CanonicalValue, MAX_SAFE_INTEGER},
    manifest::Name,
    records::{
        DatasetConfigurationRecord, DatasetDefinitionRecord, ProblemConfigurationRecord, RecordId,
    },
    seed::{
        SEED_DERIVATION_DOMAIN, SeedDerivationError, canonical_dataset_seed_fields,
        canonical_implementation_seed_fields, canonical_scheduling_seed_fields,
        derive_dataset_seed, derive_implementation_seed, derive_scheduling_seed,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};

const VECTORS: &str = include_str!("../../../fixtures/seeds/v1.json");

#[derive(Debug, Deserialize)]
struct VectorSet {
    version: u32,
    domain: String,
    dataset: Vec<DatasetVector>,
    implementation: Vec<ImplementationVector>,
    scheduling: Vec<SchedulingVector>,
}

#[derive(Debug, Deserialize)]
struct DatasetVector {
    name: String,
    experiment_seed: u64,
    dataset_definition: String,
    dataset_parameters: Value,
    canonical_fields: String,
    digest: String,
    value: u64,
}

#[derive(Debug, Deserialize)]
struct ImplementationVector {
    name: String,
    experiment_seed: u64,
    dataset_configuration: String,
    problem_configuration: String,
    implementation_repetition: u64,
    canonical_fields: String,
    digest: String,
    value: u64,
}

#[derive(Debug, Deserialize)]
struct SchedulingVector {
    name: String,
    experiment_seed: u64,
    benchmark_name: String,
    experiment_name: String,
    scheduling_policy_version: u32,
    canonical_fields: String,
    digest: String,
    value: u64,
}

#[test]
fn version_one_golden_vectors_fix_every_seed_transcript() {
    let vectors: VectorSet = serde_json::from_str(VECTORS).expect("vectors must be valid JSON");
    assert_eq!(vectors.version, SEED_DERIVATION_VERSION);
    assert_eq!(vectors.domain, SEED_DERIVATION_DOMAIN);

    for vector in vectors.dataset {
        let definition = RecordId::<DatasetDefinitionRecord>::from_str(&vector.dataset_definition)
            .unwrap_or_else(|error| {
                panic!(
                    "dataset vector `{}` has an invalid ID: {error}",
                    vector.name
                )
            });
        let parameters =
            CanonicalValue::try_from(vector.dataset_parameters).unwrap_or_else(|error| {
                panic!(
                    "dataset vector `{}` has invalid parameters: {error}",
                    vector.name
                )
            });

        let fields = canonical_dataset_seed_fields(vector.experiment_seed, definition, &parameters)
            .unwrap_or_else(|error| panic!("dataset vector `{}` failed: {error}", vector.name));
        let seed = derive_dataset_seed(vector.experiment_seed, definition, &parameters)
            .unwrap_or_else(|error| panic!("dataset vector `{}` failed: {error}", vector.name));

        assert_eq!(
            fields,
            vector.canonical_fields.as_bytes(),
            "{}",
            vector.name
        );
        assert_eq!(
            hex(seed.derivation_digest.bytes()),
            vector.digest,
            "{}",
            vector.name
        );
        assert_eq!(seed.value, vector.value, "{}", vector.name);
    }

    for vector in vectors.implementation {
        let dataset =
            RecordId::<DatasetConfigurationRecord>::from_str(&vector.dataset_configuration)
                .unwrap_or_else(|error| {
                    panic!(
                        "implementation vector `{}` has an invalid dataset ID: {error}",
                        vector.name
                    )
                });
        let problem =
            RecordId::<ProblemConfigurationRecord>::from_str(&vector.problem_configuration)
                .unwrap_or_else(|error| {
                    panic!(
                        "implementation vector `{}` has an invalid problem ID: {error}",
                        vector.name
                    )
                });

        let fields = canonical_implementation_seed_fields(
            vector.experiment_seed,
            dataset,
            problem,
            vector.implementation_repetition,
        )
        .unwrap_or_else(|error| panic!("implementation vector `{}` failed: {error}", vector.name));
        let seed = derive_implementation_seed(
            vector.experiment_seed,
            dataset,
            problem,
            vector.implementation_repetition,
        )
        .unwrap_or_else(|error| panic!("implementation vector `{}` failed: {error}", vector.name));

        assert_eq!(
            fields,
            vector.canonical_fields.as_bytes(),
            "{}",
            vector.name
        );
        assert_eq!(
            hex(seed.derivation_digest.bytes()),
            vector.digest,
            "{}",
            vector.name
        );
        assert_eq!(seed.value, vector.value, "{}", vector.name);
    }

    for vector in vectors.scheduling {
        let benchmark = name(&vector.benchmark_name);
        let experiment = name(&vector.experiment_name);

        let fields = canonical_scheduling_seed_fields(
            vector.experiment_seed,
            &benchmark,
            &experiment,
            vector.scheduling_policy_version,
        )
        .unwrap_or_else(|error| panic!("scheduling vector `{}` failed: {error}", vector.name));
        let seed = derive_scheduling_seed(
            vector.experiment_seed,
            &benchmark,
            &experiment,
            vector.scheduling_policy_version,
        )
        .unwrap_or_else(|error| panic!("scheduling vector `{}` failed: {error}", vector.name));

        assert_eq!(
            fields,
            vector.canonical_fields.as_bytes(),
            "{}",
            vector.name
        );
        assert_eq!(
            hex(seed.derivation_digest.bytes()),
            vector.digest,
            "{}",
            vector.name
        );
        assert_eq!(seed.value, vector.value, "{}", vector.name);
    }
}

#[test]
fn equivalent_dataset_parameter_objects_derive_the_same_seed() {
    let definition = RecordId::<DatasetDefinitionRecord>::from_digest([7; 32]);
    let left = CanonicalValue::from_str(r#"{"n":1000,"shape":{"p":20,"q":3}}"#).unwrap();
    let right = CanonicalValue::from_str(r#"{"shape":{"q":3,"p":20},"n":1e3}"#).unwrap();

    assert_eq!(
        derive_dataset_seed(19, definition, &left).unwrap(),
        derive_dataset_seed(19, definition, &right).unwrap()
    );
}

#[test]
fn each_declared_seed_field_changes_its_derivation() {
    let dataset_definition = RecordId::<DatasetDefinitionRecord>::from_digest([1; 32]);
    let other_dataset_definition = RecordId::<DatasetDefinitionRecord>::from_digest([2; 32]);
    let parameters = CanonicalValue::try_from(json!({"n": 10})).unwrap();
    let other_parameters = CanonicalValue::try_from(json!({"n": 11})).unwrap();
    let dataset_seed = derive_dataset_seed(5, dataset_definition, &parameters).unwrap();

    assert_ne!(
        dataset_seed,
        derive_dataset_seed(6, dataset_definition, &parameters).unwrap()
    );
    assert_ne!(
        dataset_seed,
        derive_dataset_seed(5, other_dataset_definition, &parameters).unwrap()
    );
    assert_ne!(
        dataset_seed,
        derive_dataset_seed(5, dataset_definition, &other_parameters).unwrap()
    );

    let dataset = RecordId::<DatasetConfigurationRecord>::from_digest([3; 32]);
    let other_dataset = RecordId::<DatasetConfigurationRecord>::from_digest([4; 32]);
    let problem = RecordId::<ProblemConfigurationRecord>::from_digest([5; 32]);
    let other_problem = RecordId::<ProblemConfigurationRecord>::from_digest([6; 32]);
    let implementation_seed = derive_implementation_seed(5, dataset, problem, 0).unwrap();

    assert_ne!(
        implementation_seed,
        derive_implementation_seed(6, dataset, problem, 0).unwrap()
    );
    assert_ne!(
        implementation_seed,
        derive_implementation_seed(5, other_dataset, problem, 0).unwrap()
    );
    assert_ne!(
        implementation_seed,
        derive_implementation_seed(5, dataset, other_problem, 0).unwrap()
    );
    assert_ne!(
        implementation_seed,
        derive_implementation_seed(5, dataset, problem, 1).unwrap()
    );

    let benchmark = name("benchmark");
    let other_benchmark = name("other-benchmark");
    let experiment = name("experiment");
    let other_experiment = name("other-experiment");
    let scheduling_seed =
        derive_scheduling_seed(5, &benchmark, &experiment, SCHEDULING_POLICY_VERSION).unwrap();

    assert_ne!(
        scheduling_seed,
        derive_scheduling_seed(6, &benchmark, &experiment, SCHEDULING_POLICY_VERSION).unwrap()
    );
    assert_ne!(
        scheduling_seed,
        derive_scheduling_seed(5, &other_benchmark, &experiment, SCHEDULING_POLICY_VERSION)
            .unwrap()
    );
    assert_ne!(
        scheduling_seed,
        derive_scheduling_seed(5, &benchmark, &other_experiment, SCHEDULING_POLICY_VERSION)
            .unwrap()
    );
    assert_ne!(
        scheduling_seed,
        derive_scheduling_seed(5, &benchmark, &experiment, SCHEDULING_POLICY_VERSION + 1).unwrap()
    );
}

#[test]
fn exposed_values_are_the_first_53_digest_bits() {
    let parameters = CanonicalValue::try_from(json!({})).unwrap();
    let seed = derive_dataset_seed(
        0,
        RecordId::<DatasetDefinitionRecord>::from_digest([0; 32]),
        &parameters,
    )
    .unwrap();
    let first_eight = u64::from_be_bytes(seed.derivation_digest.bytes()[..8].try_into().unwrap());

    assert_eq!(seed.value, first_eight >> 11);
    assert!(seed.value <= MAX_SAFE_INTEGER);
}

#[test]
fn unsafe_integer_inputs_are_rejected_at_the_canonical_boundary() {
    let parameters = CanonicalValue::try_from(json!({})).unwrap();
    let dataset_definition = RecordId::<DatasetDefinitionRecord>::from_digest([0; 32]);
    let dataset = RecordId::<DatasetConfigurationRecord>::from_digest([0; 32]);
    let problem = RecordId::<ProblemConfigurationRecord>::from_digest([0; 32]);
    let benchmark = name("benchmark");
    let experiment = name("experiment");

    assert!(matches!(
        derive_dataset_seed(MAX_SAFE_INTEGER + 1, dataset_definition, &parameters),
        Err(SeedDerivationError::Canonical(_))
    ));
    assert!(matches!(
        derive_implementation_seed(0, dataset, problem, MAX_SAFE_INTEGER + 1),
        Err(SeedDerivationError::Canonical(_))
    ));
    assert!(matches!(
        derive_scheduling_seed(
            MAX_SAFE_INTEGER + 1,
            &benchmark,
            &experiment,
            SCHEDULING_POLICY_VERSION,
        ),
        Err(SeedDerivationError::Canonical(_))
    ));
}

fn name(value: &str) -> Name {
    serde_json::from_value(json!(value)).expect("test name must be valid")
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}
