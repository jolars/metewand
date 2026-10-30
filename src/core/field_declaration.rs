//! Compact authoring declarations lowered into schemas and parameter selections.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Deserializer, de};
use serde_json::{Map, Value, json};

use crate::core::{
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
                schema: Some(schema.unwrap_or_else(|| {
                    FieldDeclarations::<true>(BTreeMap::new())
                        .compile()
                        .expect("empty declarations are valid")
                        .schema
                })),
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

pub(crate) struct FieldError {
    pub pointer: String,
    pub message: String,
}

impl FieldError {
    fn new(pointer: &str, message: impl Into<String>) -> Self {
        Self {
            pointer: pointer.to_owned(),
            message: message.into(),
        }
    }

    fn within(mut self, prefix: &str) -> Self {
        self.pointer.insert_str(0, prefix);
        self
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Recovers the authored location lost by Serde's value-based field lowering.
pub(crate) fn diagnose(document: &Value) -> Option<FieldError> {
    fn boundary<const PARAMETERS: bool>(value: &Value, pointer: &str) -> Option<FieldError> {
        value.as_object()?.iter().find_map(|(name, value)| {
            compile_field::<PARAMETERS>(value.clone(), true)
                .err()
                .map(|error| {
                    error.within(&format!("{pointer}/{}", crate::core::yaml::escape_pointer(name)))
                })
        })
    }
    fn contract(value: &Value, pointer: &str) -> Option<FieldError> {
        boundary::<true>(&value["parameters"], &format!("{pointer}/parameters"))
            .or_else(|| boundary::<false>(&value["result"], &format!("{pointer}/result")))
            .or_else(|| boundary::<false>(&value["metrics"], &format!("{pointer}/metrics")))
    }
    for namespace in ["datasets", "implementations"] {
        if let Some(definitions) = document[namespace].as_object() {
            for (name, value) in definitions {
                if let Some(error) = boundary::<true>(
                    &value["parameters"],
                    &format!(
                        "/{namespace}/{}/parameters",
                        crate::core::yaml::escape_pointer(name)
                    ),
                ) {
                    return Some(error);
                }
            }
        }
    }
    if let Some(problems) = document["problems"].as_object() {
        for (name, problem) in problems {
            let pointer = format!("/problems/{}", crate::core::yaml::escape_pointer(name));
            let error = if problem["contract"].is_object() {
                contract(&problem["contract"], &format!("{pointer}/contract"))
            } else {
                contract(problem, &pointer)
            };
            if error.is_some() {
                return error;
            }
        }
    }
    contract(document, "")
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
) -> Result<FieldDeclaration<PARAMETERS>, FieldError> {
    let value =
        CanonicalValue::try_from(value).map_err(|error| FieldError::new("", error.to_string()))?;
    let mut schema = match value.as_json() {
        Value::String(kind) => Map::from_iter([("type".to_owned(), Value::String(kind.clone()))]),
        Value::Object(mapping) => mapping.clone(),
        _ => {
            return Err(FieldError::new(
                "",
                "expected a type name or a compact field mapping",
            ));
        }
    };
    if !field_options
        && ["default", "grid", "optional"]
            .iter()
            .any(|key| schema.contains_key(*key))
    {
        let key = ["default", "grid", "optional"]
            .into_iter()
            .find(|key| schema.contains_key(*key))
            .unwrap();
        return Err(FieldError::new(
            &format!("/{key}"),
            "array `items` cannot declare `default`, `grid`, or `optional`",
        ));
    }
    if !PARAMETERS && (schema.contains_key("default") || schema.contains_key("grid")) {
        let key = if schema.contains_key("default") {
            "default"
        } else {
            "grid"
        };
        return Err(FieldError::new(
            &format!("/{key}"),
            "`default` and `grid` are only supported on parameter fields",
        ));
    }
    let optional = match schema.remove("optional") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(_) => return Err(FieldError::new("/optional", "`optional` must be a boolean")),
    };
    // Removing by key preserves an explicit null default as a supplied value.
    let default = schema.remove("default");
    let grid = schema.remove("grid");
    if default.is_some() && grid.is_some() {
        return Err(FieldError::new(
            "/grid",
            "a parameter cannot declare both `default` and `grid`",
        ));
    }
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| FieldError::new("/type", "a compact field requires an explicit `type`"))?;
    if !matches!(
        kind,
        "number" | "integer" | "string" | "boolean" | "null" | "array"
    ) {
        return Err(FieldError::new(
            "/type",
            format!("unsupported compact type `{kind}`; use an explicit schema for complex fields"),
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
            return Err(FieldError::new(
                &format!("/{}", crate::core::yaml::escape_pointer(key)),
                format!(
                    "unsupported compact field keyword `{key}` for type `{kind}`; use an explicit schema for complex fields"
                ),
            ));
        }
    }
    if kind == "array" && !schema.contains_key("items") {
        return Err(FieldError::new(
            "/items",
            "a compact array requires `items`",
        ));
    }
    if let Some(items) = schema.remove("items") {
        schema.insert(
            "items".to_owned(),
            compile_field::<false>(items, false)
                .map_err(|error| error.within("/items"))?
                .schema,
        );
    }
    if let Some(values) = schema.get("enum")
        && values.as_array().is_none_or(Vec::is_empty)
    {
        return Err(FieldError::new("/enum", "`enum` must be a nonempty array"));
    }
    let schema = Value::Object(schema);
    jsonschema::draft202012::meta::validate(&schema)
        .map_err(|error| FieldError::new(&error.instance_path().to_string(), error.to_string()))?;
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(&schema)
        .map_err(|error| FieldError::new(&error.schema_path().to_string(), error.to_string()))?;
    if let Some(default) = &default {
        validator.validate(default).map_err(|error| {
            FieldError::new(
                &format!("/default{}", error.instance_path()),
                format!("invalid `default`: {error}"),
            )
        })?;
    }
    let grid = grid
        .map(|grid| {
            let values = grid
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or_else(|| FieldError::new("/grid", "`grid` must be a nonempty array"))?;
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    validator.validate(value).map_err(|error| {
                        FieldError::new(
                            &format!("/grid/{index}{}", error.instance_path()),
                            format!("invalid `grid[{index}]`: {error}"),
                        )
                    })?;
                    CanonicalValue::try_from(value.clone()).map_err(|error| {
                        FieldError::new(&format!("/grid/{index}"), error.to_string())
                    })
                })
                .collect::<Result<Vec<_>, FieldError>>()
        })
        .transpose()?;
    Ok(FieldDeclaration {
        schema,
        default,
        grid,
        optional,
    })
}
