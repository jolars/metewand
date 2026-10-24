//! Compact authoring declarations lowered into existing parameter boundaries.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, de};
use serde_json::{Map, Value, json};

use crate::{canonical::CanonicalValue, schema::SchemaDeclaration};

#[derive(Debug, Deserialize)]
pub(crate) struct ParameterDeclarations(BTreeMap<String, FieldDeclaration>);

impl ParameterDeclarations {
    pub(crate) fn into_parts(self) -> Result<(SchemaDeclaration, Option<CanonicalValue>), String> {
        let mut properties = Map::new();
        let mut required = Vec::new();
        let mut defaults = Map::new();
        for (name, field) in self.0 {
            if !field.optional {
                required.push(name.clone());
            }
            if let Some(default) = field.default {
                defaults.insert(name.clone(), default);
            }
            properties.insert(name, field.schema);
        }
        let schema = serde_json::from_value(json!({
            "type": "object", "properties": properties,
            "required": required, "additionalProperties": false
        }))
        .map_err(|error| error.to_string())?;
        let defaults = if defaults.is_empty() {
            None
        } else {
            Some(
                CanonicalValue::try_from(Value::Object(defaults))
                    .map_err(|error| error.to_string())?,
            )
        };
        Ok((schema, defaults))
    }
}

#[derive(Debug)]
struct FieldDeclaration {
    schema: Value,
    default: Option<Value>,
    optional: bool,
}

impl<'de> Deserialize<'de> for FieldDeclaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        compile_field(Value::deserialize(deserializer)?, true).map_err(de::Error::custom)
    }
}

fn compile_field(value: Value, field_options: bool) -> Result<FieldDeclaration, String> {
    let value = CanonicalValue::try_from(value).map_err(|error| error.to_string())?;
    let mut schema = match value.as_json() {
        Value::String(kind) => Map::from_iter([("type".to_owned(), Value::String(kind.clone()))]),
        Value::Object(mapping) => mapping.clone(),
        _ => return Err("expected a type name or a compact field mapping".to_owned()),
    };
    if !field_options && (schema.contains_key("default") || schema.contains_key("optional")) {
        return Err("array `items` cannot declare `default` or `optional`".to_owned());
    }
    let optional = match schema.remove("optional") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(_) => return Err("`optional` must be a boolean".to_owned()),
    };
    // Removing by key preserves an explicit null default as a supplied value.
    let default = schema.remove("default");
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or("a compact field requires an explicit `type`")?;
    if !matches!(
        kind,
        "number" | "integer" | "string" | "boolean" | "null" | "array"
    ) {
        return Err(format!(
            "unsupported compact type `{kind}`; use `parameter-schema` for complex schemas"
        ));
    }
    for key in schema.keys() {
        let supported = match key.as_str() {
            "type" | "enum" => true,
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
                matches!(kind, "number" | "integer")
            }
            "minLength" | "maxLength" | "pattern" => kind == "string",
            "items" | "minItems" | "maxItems" | "uniqueItems" => kind == "array",
            _ => false,
        };
        if !supported {
            return Err(format!(
                "unsupported compact field keyword `{key}` for type `{kind}`; use `parameter-schema` for complex schemas"
            ));
        }
    }
    if kind == "array" && !schema.contains_key("items") {
        return Err("a compact array requires `items`".to_owned());
    }
    if let Some(items) = schema.remove("items") {
        schema.insert("items".to_owned(), compile_field(items, false)?.schema);
    }
    if let Some(values) = schema.get("enum")
        && values.as_array().is_none_or(Vec::is_empty)
    {
        return Err("`enum` must be a nonempty array".to_owned());
    }
    let schema = Value::Object(schema);
    jsonschema::draft202012::meta::validate(&schema).map_err(|error| error.to_string())?;
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(&schema)
        .map_err(|error| error.to_string())?;
    if let Some(default) = &default {
        validator
            .validate(default)
            .map_err(|error| format!("invalid `default`: {error}"))?;
    }
    Ok(FieldDeclaration {
        schema,
        default,
        optional,
    })
}
