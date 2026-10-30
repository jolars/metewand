//! Bounded JSON-Lines framing for worker protocol messages.

use std::{
    cell::RefCell,
    fmt,
    io::{self, BufRead, BufReader, Read},
    str,
};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use thiserror::Error;

/// Maximum size of a version-1 protocol line, including its final newline.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// A bounded reader for version-1 JSON-Lines protocol frames.
///
/// The reader buffers at most one protocol line and accepts arbitrary
/// fragmentation from the underlying byte stream.
#[derive(Debug)]
pub struct FrameReader<R> {
    reader: BufReader<R>,
}

impl<R: Read> FrameReader<R> {
    /// Wraps a protocol byte stream.
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
        }
    }

    /// Reads and validates the next JSON-Lines frame.
    ///
    /// Clean EOF between frames returns `None`. EOF after any bytes of a new
    /// frame is an error because every message requires a final newline.
    ///
    /// # Errors
    ///
    /// Returns a typed framing error for I/O failure, a line beyond the
    /// version-1 limit, early EOF, a byte-order mark, invalid UTF-8, duplicate
    /// object keys, or malformed JSON.
    pub fn read_frame(&mut self) -> Result<Option<Value>, FrameError> {
        let Some(bytes) = self.read_line()? else {
            return Ok(None);
        };

        parse_frame(&bytes).map(Some)
    }

    fn read_line(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let mut line = Vec::new();

        loop {
            let available = self.reader.fill_buf().map_err(FrameError::Io)?;
            if available.is_empty() {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    Err(FrameError::EarlyEof {
                        received: line.len(),
                    })
                };
            }

            if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
                let consumed = newline + 1;
                if line.len().saturating_add(consumed) > MAX_LINE_BYTES {
                    return Err(FrameError::LineTooLong {
                        limit: MAX_LINE_BYTES,
                    });
                }

                line.extend_from_slice(&available[..newline]);
                self.reader.consume(consumed);
                return Ok(Some(line));
            }

            let remaining = MAX_LINE_BYTES - 1 - line.len();
            if available.len() > remaining {
                return Err(FrameError::LineTooLong {
                    limit: MAX_LINE_BYTES,
                });
            }

            let consumed = available.len();
            line.extend_from_slice(available);
            self.reader.consume(consumed);
        }
    }
}

fn parse_frame(input: &[u8]) -> Result<Value, FrameError> {
    if input.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(FrameError::ByteOrderMark);
    }

    let text = str::from_utf8(input).map_err(|error| FrameError::InvalidUtf8 {
        valid_up_to: error.valid_up_to(),
    })?;
    parse_unique_json(text)
}

fn parse_unique_json(input: &str) -> Result<Value, FrameError> {
    let duplicate = RefCell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value = UniqueValueSeed {
        duplicate: &duplicate,
    }
    .deserialize(&mut deserializer)
    .map_err(|source| malformed_or_duplicate(source, &duplicate))?;
    deserializer
        .end()
        .map_err(|source| malformed_or_duplicate(source, &duplicate))?;
    Ok(value)
}

fn malformed_or_duplicate(
    source: serde_json::Error,
    duplicate: &RefCell<Option<String>>,
) -> FrameError {
    duplicate
        .borrow_mut()
        .take()
        .map_or(FrameError::MalformedJson { source }, |key| {
            FrameError::DuplicateKey { key }
        })
}

#[derive(Clone, Copy)]
struct UniqueValueSeed<'a> {
    duplicate: &'a RefCell<Option<String>>,
}

impl<'de> DeserializeSeed<'de> for UniqueValueSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor {
            duplicate: self.duplicate,
        })
    }
}

struct UniqueValueVisitor<'a> {
    duplicate: &'a RefCell<Option<String>>,
}

impl<'de> Visitor<'de> for UniqueValueVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("JSON numbers must be finite"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(UniqueValueSeed {
            duplicate: self.duplicate,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                *self.duplicate.borrow_mut() = Some(key.clone());
                return Err(de::Error::custom(format_args!(
                    "duplicate object key `{key}`"
                )));
            }

            let value = object.next_value_seed(UniqueValueSeed {
                duplicate: self.duplicate,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

/// Matches worker responses to the one request currently awaiting a response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseCorrelator {
    expected_id: Option<String>,
}

impl ResponseCorrelator {
    /// Starts correlation for one pending request.
    pub fn new(expected_id: impl Into<String>) -> Self {
        Self {
            expected_id: Some(expected_id.into()),
        }
    }

    /// Validates and consumes a response for the pending request.
    ///
    /// A mismatched response does not consume the pending request. Once the
    /// expected response is accepted, every subsequent response is extra.
    ///
    /// # Errors
    ///
    /// Returns an error if `response.id` is not a string, does not match the
    /// pending request, or arrives after that request was satisfied.
    pub fn accept(&mut self, response: &Value) -> Result<(), CorrelationError> {
        let actual = response
            .as_object()
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str)
            .ok_or(CorrelationError::InvalidRequestId)?;

        let Some(expected) = self.expected_id.as_deref() else {
            return Err(CorrelationError::ExtraResponse {
                id: actual.to_owned(),
            });
        };
        if actual != expected {
            return Err(CorrelationError::MismatchedRequestId {
                expected: expected.to_owned(),
                actual: actual.to_owned(),
            });
        }

        self.expected_id = None;
        Ok(())
    }

    /// Reports whether the pending request has received its response.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.expected_id.is_none()
    }
}

/// A version-1 JSON-Lines framing failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FrameError {
    /// The protocol stream could not be read.
    #[error("failed to read the worker protocol stream: {0}")]
    Io(#[source] io::Error),

    /// A line exceeded the inclusive version-1 byte limit.
    #[error("worker protocol line exceeds the {limit}-byte limit")]
    LineTooLong {
        /// Inclusive line limit, including the final newline.
        limit: usize,
    },

    /// EOF arrived after a partial, unterminated line.
    #[error("worker protocol stream ended after {received} bytes without a newline")]
    EarlyEof {
        /// Number of bytes received for the partial line.
        received: usize,
    },

    /// A UTF-8 byte-order mark prefixed the JSON value.
    #[error("worker protocol messages must not begin with a UTF-8 byte-order mark")]
    ByteOrderMark,

    /// The line was not valid UTF-8.
    #[error("worker protocol message is not valid UTF-8 at byte {valid_up_to}")]
    InvalidUtf8 {
        /// Length of the valid UTF-8 prefix.
        valid_up_to: usize,
    },

    /// A JSON object repeated a key.
    #[error("worker protocol message contains duplicate object key `{key}`")]
    DuplicateKey {
        /// Repeated object key.
        key: String,
    },

    /// The line was not one complete JSON value.
    #[error("worker protocol message is malformed JSON: {source}")]
    MalformedJson {
        /// Parser failure, including its line and column.
        #[source]
        source: serde_json::Error,
    },
}

/// A response-to-request correlation failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum CorrelationError {
    /// The response is not an object with a string `id` field.
    #[error("worker response must contain a string request ID")]
    InvalidRequestId,

    /// A response did not name the pending request.
    #[error("worker response ID `{actual}` does not match pending request `{expected}`")]
    MismatchedRequestId {
        /// Pending request ID.
        expected: String,
        /// Response ID received from the worker.
        actual: String,
    },

    /// A response arrived when no request remained pending.
    #[error("worker sent extra response `{id}` with no pending request")]
    ExtraResponse {
        /// Unexpected response ID.
        id: String,
    },
}
