use std::str::FromStr;

use metewand_core::canonical::{
    CanonicalJsonError, CanonicalValue, CanonicalValueViolation, MAX_SAFE_INTEGER,
};
use serde::Deserialize;
use serde_json::{Value, json};

const VECTORS: &str = include_str!("../../../fixtures/canonical-json/v1.json");

#[derive(Debug, Deserialize)]
struct VectorSet {
    version: u32,
    accepted: Vec<AcceptedVector>,
    rejected: Vec<RejectedVector>,
}

#[derive(Debug, Deserialize)]
struct AcceptedVector {
    name: String,
    input: String,
    canonical: String,
}

#[derive(Debug, Deserialize)]
struct RejectedVector {
    name: String,
    kind: String,
    input: String,
}

#[test]
fn version_one_vectors_produce_exact_canonical_bytes() {
    let vectors: VectorSet = serde_json::from_str(VECTORS).expect("vectors must be valid JSON");
    assert_eq!(vectors.version, 1);

    for vector in vectors.accepted {
        let value = CanonicalValue::from_slice(vector.input.as_bytes())
            .unwrap_or_else(|error| panic!("accepted vector `{}` failed: {error}", vector.name));
        let bytes = value.to_canonical_bytes().unwrap_or_else(|error| {
            panic!("vector `{}` failed to serialize: {error}", vector.name)
        });

        assert_eq!(
            bytes,
            vector.canonical.as_bytes(),
            "vector `{}`",
            vector.name
        );
        assert_ne!(bytes.last(), Some(&b'\n'), "vector `{}`", vector.name);

        let reparsed = CanonicalValue::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "canonical vector `{}` failed to parse: {error}",
                vector.name
            )
        });
        assert_eq!(
            reparsed.to_canonical_bytes().unwrap(),
            bytes,
            "vector `{}` is not idempotent",
            vector.name
        );
    }
}

#[test]
fn version_one_vectors_reject_out_of_domain_inputs() {
    let vectors: VectorSet = serde_json::from_str(VECTORS).expect("vectors must be valid JSON");

    for vector in vectors.rejected {
        assert!(
            CanonicalValue::from_slice(vector.input.as_bytes()).is_err(),
            "rejected vector `{}` ({}) was accepted",
            vector.name,
            vector.kind
        );
    }
}

#[test]
fn rejects_invalid_utf8() {
    let input = [b'"', 0xff, b'"'];
    assert!(CanonicalValue::from_slice(&input).is_err());
}

#[test]
fn validates_and_normalizes_programmatic_values() {
    assert_eq!(MAX_SAFE_INTEGER, 9_007_199_254_740_991);

    let value = CanonicalValue::try_from(json!({"zero": -0.0, "integer": 1e3})).unwrap();
    assert_eq!(
        value.to_canonical_bytes().unwrap(),
        br#"{"integer":1000,"zero":0}"#
    );

    let error = CanonicalValue::try_from(json!(9_007_199_254_740_992_u64)).unwrap_err();
    assert!(matches!(
        error,
        CanonicalJsonError::InvalidValue(CanonicalValueViolation::IntegerOutOfRange { .. })
    ));

    let error = CanonicalValue::try_from(json!(9.007_199_254_740_992e15_f64)).unwrap_err();
    assert!(matches!(
        error,
        CanonicalJsonError::InvalidValue(CanonicalValueViolation::IntegerOutOfRange { .. })
    ));
}

#[test]
fn semantically_equivalent_inputs_converge() {
    let left = CanonicalValue::from_str(r#"{"b": 1e3, "a": -0.0}"#).unwrap();
    let right = CanonicalValue::from_str(r#"{"a": 0, "b": 1000}"#).unwrap();

    assert_eq!(left.as_json(), right.as_json());
    assert_eq!(
        left.to_canonical_bytes().unwrap(),
        right.to_canonical_bytes().unwrap()
    );
}

#[test]
fn programmatic_values_are_available_for_later_schema_validation() {
    let source: Value = json!({"parameters": {"tolerance": 1e-7}});
    let canonical = CanonicalValue::try_from(source.clone()).unwrap();

    assert_eq!(canonical.as_json(), &source);
}
