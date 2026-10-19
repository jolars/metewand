//! Secret filtering at worker log and typed-session boundaries.

use std::{fmt, sync::Arc};

use serde_json::Value;

use crate::worker_environment::WorkerSecrets;

#[derive(Clone, Default)]
pub(crate) struct Redactor(Arc<Vec<Vec<u8>>>);

impl Redactor {
    pub(crate) fn new(secrets: &WorkerSecrets) -> Self {
        Self(Arc::new(
            secrets
                .values
                .values()
                .map(|v| v.as_bytes().to_vec())
                .collect(),
        ))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn lookahead(&self) -> usize {
        self.0.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub(crate) fn bytes(&self, bytes: &[u8]) -> Vec<u8> {
        self.prefix(bytes, bytes.len())
    }

    pub(crate) fn prefix(&self, bytes: &[u8], limit: usize) -> Vec<u8> {
        if self.is_empty() {
            return bytes[..limit.min(bytes.len())].to_vec();
        }
        // Mark all original matches before replacing any, so overlapping
        // secrets cannot expose one another's suffixes.
        let mut hidden = vec![false; bytes.len()];
        for secret in self.0.iter() {
            for (start, window) in bytes.windows(secret.len()).enumerate() {
                if window == secret {
                    hidden[start..start + secret.len()].fill(true);
                }
            }
        }
        let mut result = Vec::new();
        let mut previous = false;
        for (byte, hide) in bytes.iter().zip(hidden).take(limit) {
            if hide && !previous {
                result.extend_from_slice(b"[REDACTED]");
            } else if !hide {
                result.push(*byte);
            }
            previous = hide;
        }
        result
    }

    pub(crate) fn value(&self, value: &mut Value) {
        if self.is_empty() {
            return;
        }
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => items.iter_mut().for_each(|v| self.value(v)),
            Value::Object(fields) => {
                *fields = std::mem::take(fields)
                    .into_iter()
                    .map(|(name, mut v)| {
                        self.value(&mut v);
                        (self.text(&name), v)
                    })
                    .collect();
            }
            _ => {}
        }
    }

    fn text(&self, text: &str) -> String {
        String::from_utf8(self.bytes(text.as_bytes())).expect("UTF-8 secret replacement")
    }
}

impl fmt::Debug for Redactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Redactor { .. }")
    }
}
