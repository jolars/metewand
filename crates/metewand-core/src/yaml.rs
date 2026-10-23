//! Shared YAML decoding into the version-1 JSON domain and typed records.

use std::{collections::BTreeMap, fmt, path::Path};

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Number, Value};
use serde_path_to_error::Segment;
use serde_saphyr::{Location, Spanned, Tagged};

use crate::{canonical::CanonicalValue, manifest::SourceSpan};

pub(crate) struct ParseError {
    pub message: String,
    pub source_span: SourceSpan,
}

pub(crate) fn parse<T: DeserializeOwned>(
    source_path: &Path,
    source: &str,
) -> Result<T, ParseError> {
    // Parsing into a JSON-domain tree first prevents YAML's typed string
    // coercions and preserves source locations through the typed decoding pass.
    let options = serde_saphyr::options! {
        // Span and tag wrappers add decoding frames at each nesting level.
        // Bound depth before they can exhaust an ordinary worker thread's stack.
        budget: serde_saphyr::budget! { max_depth: 24 },
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
        reject_unsupported_tags: true,
        strict_booleans: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::Reject,
        with_snippet: false,
    };
    let node: Node =
        serde_saphyr::from_str_with_options(source, options).map_err(|error| ParseError {
            source_span: source_span(source_path, source, error.location()),
            message: error.to_string(),
        })?;
    if !matches!(node.0.value, NodeValue::Mapping(_)) {
        return Err(ParseError {
            message: "expected one YAML mapping document".to_owned(),
            source_span: source_span(source_path, source, Some(node.0.referenced)),
        });
    }
    if let Some(location) = node.numeric_string_fallback(source) {
        return Err(ParseError {
            message: "YAML integer is outside the version-1 Metewand JSON domain".to_owned(),
            source_span: source_span(source_path, source, Some(location)),
        });
    }

    serde_path_to_error::deserialize(node.to_json()).map_err(|error| {
        let location = node.error_location(error.path(), &error.inner().to_string());
        ParseError {
            message: error.to_string(),
            source_span: source_span(source_path, source, Some(location)),
        }
    })
}

struct Node(Spanned<NodeValue>, bool);

struct KeyOrigin {
    referenced: Location,
    defined: Location,
    explicit_string: bool,
}

enum NodeValue {
    Scalar(Value),
    Sequence(Vec<Node>),
    Mapping(BTreeMap<String, (KeyOrigin, Node)>),
}

impl Node {
    fn numeric_string_fallback(&self, source: &str) -> Option<Location> {
        match &self.0.value {
            NodeValue::Scalar(Value::String(value))
                if !self.1 && plain_integer_source(source, self.0.defined, value) =>
            {
                Some(self.0.referenced)
            }
            NodeValue::Sequence(values) => values
                .iter()
                .find_map(|value| value.numeric_string_fallback(source)),
            NodeValue::Mapping(values) => values.iter().find_map(|(name, (origin, value))| {
                if !origin.explicit_string && plain_integer_source(source, origin.defined, name) {
                    Some(origin.referenced)
                } else {
                    value.numeric_string_fallback(source)
                }
            }),
            _ => None,
        }
    }

    fn to_json(&self) -> Value {
        match &self.0.value {
            NodeValue::Scalar(value) => value.clone(),
            NodeValue::Sequence(values) => Value::Array(values.iter().map(Self::to_json).collect()),
            NodeValue::Mapping(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, (_, value))| (key.clone(), value.to_json()))
                    .collect(),
            ),
        }
    }

    fn error_location(&self, path: &serde_path_to_error::Path, message: &str) -> Location {
        let mut node = self;
        let mut key_location = self.0.referenced;
        for segment in path {
            match (&node.0.value, segment) {
                (NodeValue::Mapping(values), Segment::Map { key }) => {
                    let Some((location, value)) = values.get(key) else {
                        break;
                    };
                    key_location = location.referenced;
                    node = value;
                }
                (NodeValue::Sequence(values), Segment::Seq { index }) => {
                    let Some(value) = values.get(*index) else {
                        break;
                    };
                    node = value;
                    key_location = node.0.referenced;
                }
                _ => break,
            }
        }
        if message.starts_with("unknown field") {
            key_location
        } else {
            node.0.referenced
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let node = Spanned::<Tagged<NodeValue>>::deserialize(deserializer)?;
        if let Some(tag) = &node.value.1
            && !matches!(
                tag.as_str(),
                "!" | "tag:yaml.org,2002:null"
                    | "tag:yaml.org,2002:bool"
                    | "tag:yaml.org,2002:int"
                    | "tag:yaml.org,2002:float"
                    | "tag:yaml.org,2002:str"
                    | "tag:yaml.org,2002:seq"
                    | "tag:yaml.org,2002:map"
            )
        {
            return Err(de::Error::custom(format_args!(
                "YAML tag {tag:?} is outside the version-1 Metewand JSON domain"
            )));
        }
        let referenced = if node.referenced == Location::UNKNOWN {
            node.defined
        } else {
            node.referenced
        };
        let explicit_string =
            matches!(node.value.1.as_deref(), Some("!" | "tag:yaml.org,2002:str"));
        Ok(Self(
            Spanned::new(node.value.0, referenced, node.defined),
            explicit_string,
        ))
    }
}

impl<'de> Deserialize<'de> for NodeValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = NodeValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a value in the version-1 Metewand JSON domain")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(NodeValue::Scalar(Value::Null))
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(NodeValue::Scalar(Value::Bool(value)))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        canonical_number(value.into())
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        canonical_number(value.into())
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        let number = Number::from_f64(value)
            .ok_or_else(|| E::custom("expected a finite binary64 number"))?;
        canonical_number(number)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(NodeValue::Scalar(Value::String(value.to_owned())))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(NodeValue::Scalar(Value::String(value)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element()? {
            values.push(value);
        }
        Ok(NodeValue::Sequence(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut mapping: A) -> Result<Self::Value, A::Error> {
        let mut values = BTreeMap::new();
        while let Some(key) = mapping.next_key::<Node>()? {
            let NodeValue::Scalar(Value::String(name)) = key.0.value else {
                return Err(de::Error::custom("YAML mapping keys must be strings"));
            };
            let value = mapping.next_value()?;
            let origin = KeyOrigin {
                referenced: key.0.referenced,
                defined: key.0.defined,
                explicit_string: key.1,
            };
            if values.insert(name.clone(), (origin, value)).is_some() {
                return Err(de::Error::custom(format_args!(
                    "duplicate object key {name:?}"
                )));
            }
        }
        Ok(NodeValue::Mapping(values))
    }
}

fn plain_integer_source(source: &str, location: Location, value: &str) -> bool {
    if !integer_spelling(value) {
        return false;
    }
    let span = location.span();
    let range = if let (Some(start), Some(len)) = (span.byte_offset(), span.byte_len()) {
        let Ok(start) = usize::try_from(start) else {
            return false;
        };
        let Ok(len) = usize::try_from(len) else {
            return false;
        };
        start..start.saturating_add(len)
    } else {
        let byte_offset = |offset| {
            usize::try_from(offset)
                .ok()
                .and_then(|offset| source.char_indices().nth(offset))
                .map_or(source.len(), |(offset, _)| offset)
        };
        byte_offset(span.offset())..byte_offset(span.offset().saturating_add(span.len()))
    };
    // The YAML decoder falls back to strings when its integer accumulator
    // overflows. Check the source token so quoted and block strings retain
    // their explicitly chosen type, including through aliases and merges.
    source.get(range) == Some(value)
}

fn integer_spelling(value: &str) -> bool {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    let (radix, digits) = match unsigned.get(..2) {
        Some("0x" | "0X") => (16, &unsigned[2..]),
        Some("0o" | "0O") => (8, &unsigned[2..]),
        Some("0b" | "0B") => (2, &unsigned[2..]),
        _ if unsigned.starts_with('0') && unsigned != "0" => return false,
        _ => (10, unsigned),
    };
    let mut previous_digit = false;
    for byte in digits.bytes() {
        if byte == b'_' && previous_digit {
            previous_digit = false;
            continue;
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return false,
        };
        if digit >= radix {
            return false;
        }
        previous_digit = true;
    }
    previous_digit
}

fn canonical_number<E: de::Error>(number: Number) -> Result<NodeValue, E> {
    let value = Value::Number(number);
    // Integer-typed fields must still reject floats. Literal parameter values
    // normalize their numbers later through their CanonicalValue decoder.
    CanonicalValue::try_from(value.clone()).map_err(E::custom)?;
    Ok(NodeValue::Scalar(value))
}

fn source_span(path: &Path, source: &str, location: Option<Location>) -> SourceSpan {
    let Some(location) = location.filter(|location| *location != Location::UNKNOWN) else {
        return SourceSpan::from_byte_range(path, source, source.len()..source.len());
    };
    let span = location.span();
    let offset = usize::try_from(span.offset()).unwrap_or(usize::MAX);
    let end_offset =
        usize::try_from(span.offset().saturating_add(span.len())).unwrap_or(usize::MAX);
    let byte_offset = |character_offset| {
        source
            .char_indices()
            .nth(character_offset)
            .map_or(source.len(), |(offset, _)| offset)
    };
    SourceSpan::from_byte_range(path, source, byte_offset(offset)..byte_offset(end_offset))
}
