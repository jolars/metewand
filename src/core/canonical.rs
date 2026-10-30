//! Restricted JSON values and their canonical byte representation.
//!
//! Metewand accepts the JSON subset described by its version-1 compatibility
//! contract and serializes accepted values according to RFC 8785. Callers can
//! inspect the parsed value for schema validation, then use
//! [`CanonicalValue::to_canonical_bytes`] for hashing or transport.

use std::{fmt, str::FromStr};

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use thiserror::Error;

/// Largest integer that Metewand accepts in either direction.
pub const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

const MAX_SAFE_INTEGER_I64: i64 = MAX_SAFE_INTEGER as i64;

/// A JSON value in Metewand's version-1 language-neutral value domain.
///
/// Object keys are unique, strings contain Unicode scalar values, integers are
/// within the safe binary64 range, other numbers are finite binary64 values,
/// and negative zero has been normalized to zero.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalValue {
    value: Value,
}

impl CanonicalValue {
    /// Parses one complete JSON value and enforces the version-1 restrictions.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, trailing input, invalid UTF-8 or
    /// Unicode, duplicate object keys, and numbers outside the supported
    /// domain.
    pub fn from_slice(input: &[u8]) -> Result<Self, CanonicalJsonError> {
        let mut deserializer = serde_json::Deserializer::from_slice(input);
        let value = RestrictedValue::deserialize(&mut deserializer)
            .map_err(CanonicalJsonError::InvalidInput)?;
        deserializer
            .end()
            .map_err(CanonicalJsonError::InvalidInput)?;

        Ok(Self { value: value.0 })
    }

    /// Returns the normalized JSON value for validation and typed conversion.
    #[must_use]
    pub fn as_json(&self) -> &Value {
        &self.value
    }

    /// Serializes this value to its exact RFC 8785 UTF-8 representation.
    ///
    /// The returned bytes contain no insignificant whitespace or trailing
    /// newline and are the representation used by Metewand for hashing and
    /// transport.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical serialization unexpectedly fails.
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        serde_json_canonicalizer::to_vec(&self.value).map_err(CanonicalJsonError::Canonicalization)
    }
}

impl FromStr for CanonicalValue {
    type Err = CanonicalJsonError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::from_slice(input.as_bytes())
    }
}

impl TryFrom<Value> for CanonicalValue {
    type Error = CanonicalJsonError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        normalize_value(value)
            .map(|value| Self { value })
            .map_err(CanonicalJsonError::InvalidValue)
    }
}

/// An error produced while accepting or serializing a canonical JSON value.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CanonicalJsonError {
    /// Raw input is malformed or violates the restricted value domain.
    #[error("input is not valid version-1 Metewand JSON: {0}")]
    InvalidInput(#[source] serde_json::Error),

    /// A programmatically constructed value violates the restricted domain.
    #[error(transparent)]
    InvalidValue(#[from] CanonicalValueViolation),

    /// An accepted value could not be serialized canonically.
    #[error("failed to produce canonical JSON bytes: {0}")]
    Canonicalization(#[source] serde_json::Error),
}

/// A restriction violated by a programmatically constructed JSON value.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum CanonicalValueViolation {
    /// An integral value lies outside the safe binary64 integer range.
    #[error(
        "integer {value} is outside the inclusive range -{MAX_SAFE_INTEGER} to {MAX_SAFE_INTEGER}"
    )]
    IntegerOutOfRange {
        /// The rejected number as represented by `serde_json`.
        value: String,
    },

    /// A number cannot be represented as a finite binary64 value.
    #[error("number {value} is not a finite binary64 value")]
    NotFiniteBinary64 {
        /// The rejected number as represented by `serde_json`.
        value: String,
    },
}

struct RestrictedValue(Value);

impl<'de> Deserialize<'de> for RestrictedValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(RestrictedValueVisitor)
    }
}

struct RestrictedValueVisitor;

impl<'de> Visitor<'de> for RestrictedValueVisitor {
    type Value = RestrictedValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a value in the version-1 Metewand JSON domain")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(RestrictedValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if !(-MAX_SAFE_INTEGER_I64..=MAX_SAFE_INTEGER_I64).contains(&value) {
            return Err(E::custom(CanonicalValueViolation::IntegerOutOfRange {
                value: value.to_string(),
            }));
        }

        Ok(RestrictedValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value > MAX_SAFE_INTEGER {
            return Err(E::custom(CanonicalValueViolation::IntegerOutOfRange {
                value: value.to_string(),
            }));
        }

        Ok(RestrictedValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        normalize_f64(value)
            .map(|number| RestrictedValue(Value::Number(number)))
            .map_err(E::custom)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(RestrictedValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(RestrictedValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
        while let Some(value) = sequence.next_element::<RestrictedValue>()? {
            values.push(value.0);
        }

        Ok(RestrictedValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some((key, value)) = object.next_entry::<String, RestrictedValue>()? {
            if values.contains_key(&key) {
                return Err(<A::Error as de::Error>::custom(format_args!(
                    "duplicate object key {key:?}"
                )));
            }
            values.insert(key, value.0);
        }

        Ok(RestrictedValue(Value::Object(values)))
    }
}

fn normalize_value(value: Value) -> Result<Value, CanonicalValueViolation> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(value),
        Value::Number(number) => normalize_number(number).map(Value::Number),
        Value::Array(values) => values
            .into_iter()
            .map(normalize_value)
            .collect::<Result<_, _>>()
            .map(Value::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| normalize_value(value).map(|value| (key, value)))
            .collect::<Result<_, _>>()
            .map(Value::Object),
    }
}

fn normalize_number(number: Number) -> Result<Number, CanonicalValueViolation> {
    if let Some(value) = number.as_i64() {
        if !(-MAX_SAFE_INTEGER_I64..=MAX_SAFE_INTEGER_I64).contains(&value) {
            return Err(CanonicalValueViolation::IntegerOutOfRange {
                value: number.to_string(),
            });
        }
        return Ok(number);
    }

    if let Some(value) = number.as_u64() {
        if value > MAX_SAFE_INTEGER {
            return Err(CanonicalValueViolation::IntegerOutOfRange {
                value: number.to_string(),
            });
        }
        return Ok(number);
    }

    let value = number
        .as_f64()
        .ok_or_else(|| CanonicalValueViolation::NotFiniteBinary64 {
            value: number.to_string(),
        })?;
    normalize_f64(value)
}

fn normalize_f64(value: f64) -> Result<Number, CanonicalValueViolation> {
    if !value.is_finite() {
        return Err(CanonicalValueViolation::NotFiniteBinary64 {
            value: value.to_string(),
        });
    }

    if value.fract() == 0.0 {
        if value.abs() > MAX_SAFE_INTEGER as f64 {
            return Err(CanonicalValueViolation::IntegerOutOfRange {
                value: value.to_string(),
            });
        }

        // Normalizing integral floats also makes negative zero and alternate
        // exponent spellings converge before schema validation.
        return Ok(Number::from(value as i64));
    }

    Number::from_f64(value).ok_or_else(|| CanonicalValueViolation::NotFiniteBinary64 {
        value: value.to_string(),
    })
}
