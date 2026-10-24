//! Compact authoring declarations lowered into schemas and parameter selections.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, de};
use serde_json::{Map, Value, json};

use crate::{
    canonical::CanonicalValue,
    manifest::{ParameterAxes, ParameterAxis},
    schema::SchemaDeclaration,
};

#[derive(Debug, Deserialize)]
pub(crate) struct FieldDeclarations<const PARAMETERS: bool>(
    BTreeMap<String, FieldDeclaration<PARAMETERS>>,
);

pub(crate) type ParameterDeclarations = FieldDeclarations<true>;
pub(crate) type OutputDeclarations = FieldDeclarations<false>;

pub(crate) struct CompiledDeclarations {
    pub schema: SchemaDeclaration,
    pub defaults: Option<CanonicalValue>,
    pub grids: ParameterAxes,
}

impl<const PARAMETERS: bool> FieldDeclarations<PARAMETERS> {
    pub(crate) fn compile(self) -> Result<CompiledDeclarations, String> {
        let mut properties = Map::new();
        let mut required = Vec::new();
        let mut defaults = Map::new();
        let mut grids = ParameterAxes::new();
        for (name, field) in self.0 {
            if !field.optional {
                required.push(name.clone());
            }
            if let Some(default) = field.default {
                defaults.insert(name.clone(), default);
            }
            if let Some(grid) = field.grid {
                grids.insert(name.clone(), ParameterAxis::Grid(grid));
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
        Ok(CompiledDeclarations {
            schema,
            defaults,
            grids,
        })
    }
}

pub(crate) struct ParameterBoundary {
    pub schema: Option<SchemaDeclaration>,
    pub defaults: Option<CanonicalValue>,
    pub grids: ParameterAxes,
}

impl ParameterBoundary {
    pub(crate) fn lower(
        parameters: Option<ParameterDeclarations>,
        schema: Option<SchemaDeclaration>,
        defaults: Option<CanonicalValue>,
    ) -> Result<Self, String> {
        if let Some(parameters) = parameters {
            if schema.is_some() || defaults.is_some() {
                return Err(
                    "`parameters` cannot be combined with `parameter-schema` or `parameter-defaults`"
                        .to_owned(),
                );
            }
            let compiled = parameters.compile()?;
            Ok(Self {
                schema: Some(compiled.schema),
                defaults: compiled.defaults,
                grids: compiled.grids,
            })
        } else {
            Ok(Self {
                schema,
                defaults,
                grids: ParameterAxes::new(),
            })
        }
    }
}

#[derive(Debug)]
struct FieldDeclaration<const PARAMETERS: bool> {
    schema: Value,
    default: Option<Value>,
    grid: Option<Vec<CanonicalValue>>,
    optional: bool,
}

impl<'de, const PARAMETERS: bool> Deserialize<'de> for FieldDeclaration<PARAMETERS> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        compile_field::<PARAMETERS>(Value::deserialize(deserializer)?, true)
            .map_err(de::Error::custom)
    }
}

fn compile_field<const PARAMETERS: bool>(
    value: Value,
    field_options: bool,
) -> Result<FieldDeclaration<PARAMETERS>, String> {
    let value = CanonicalValue::try_from(value).map_err(|error| error.to_string())?;
    let mut schema = match value.as_json() {
        Value::String(kind) => Map::from_iter([("type".to_owned(), Value::String(kind.clone()))]),
        Value::Object(mapping) => mapping.clone(),
        _ => return Err("expected a type name or a compact field mapping".to_owned()),
    };
    if !field_options
        && ["default", "grid", "optional"]
            .iter()
            .any(|key| schema.contains_key(*key))
    {
        return Err("array `items` cannot declare `default`, `grid`, or `optional`".to_owned());
    }
    if !PARAMETERS && (schema.contains_key("default") || schema.contains_key("grid")) {
        return Err("`default` and `grid` are only supported on parameter fields".to_owned());
    }
    let optional = match schema.remove("optional") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(_) => return Err("`optional` must be a boolean".to_owned()),
    };
    // Removing by key preserves an explicit null default as a supplied value.
    let default = schema.remove("default");
    let grid = schema.remove("grid");
    if default.is_some() && grid.is_some() {
        return Err("a parameter cannot declare both `default` and `grid`".to_owned());
    }
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or("a compact field requires an explicit `type`")?;
    if !matches!(
        kind,
        "number" | "integer" | "string" | "boolean" | "null" | "array"
    ) {
        return Err(format!(
            "unsupported compact type `{kind}`; use an explicit schema for complex fields"
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
                "unsupported compact field keyword `{key}` for type `{kind}`; use an explicit schema for complex fields"
            ));
        }
    }
    if kind == "array" && !schema.contains_key("items") {
        return Err("a compact array requires `items`".to_owned());
    }
    if let Some(items) = schema.remove("items") {
        schema.insert(
            "items".to_owned(),
            compile_field::<false>(items, false)?.schema,
        );
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
    let grid = grid
        .map(|grid| {
            let values = grid
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or("`grid` must be a nonempty array")?;
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    validator
                        .validate(value)
                        .map_err(|error| format!("invalid `grid[{index}]`: {error}"))?;
                    CanonicalValue::try_from(value.clone()).map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?;
    Ok(FieldDeclaration {
        schema,
        default,
        grid,
        optional,
    })
}
