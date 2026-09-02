//! Resolution of literal parameter defaults before schema validation.

use std::path::Path;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    canonical::{CanonicalJsonError, CanonicalValue},
    schema::{SchemaCatalog, SchemaValidationError},
};

/// Merges literal defaults with supplied parameters and validates the result.
///
/// Object values merge recursively by key. At every other pairing, the
/// supplied value replaces the default, including arrays and explicit `null`.
/// The merged value is validated without applying JSON Schema `default`
/// annotations.
///
/// # Errors
///
/// Returns an error when either parameter root is not an object, when the
/// merged value violates the version-1 canonical JSON domain, or when it does
/// not satisfy the selected schema.
pub fn resolve_parameters(
    schemas: &SchemaCatalog,
    schema_path: &Path,
    parameter_defaults: Option<&CanonicalValue>,
    supplied_parameters: &CanonicalValue,
) -> Result<CanonicalValue, ParameterResolutionError> {
    let supplied = supplied_parameters
        .as_json()
        .as_object()
        .ok_or(ParameterResolutionError::ParametersMustBeObject)?;

    let mut resolved = match parameter_defaults {
        Some(defaults) => defaults
            .as_json()
            .as_object()
            .ok_or(ParameterResolutionError::DefaultsMustBeObject)?
            .clone(),
        None => Map::new(),
    };
    merge_objects(&mut resolved, supplied);

    let resolved = CanonicalValue::try_from(Value::Object(resolved))?;
    schemas.validate(schema_path, resolved.as_json())?;
    Ok(resolved)
}

/// An error encountered while resolving parameters.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ParameterResolutionError {
    /// The definition's literal defaults were not an object.
    #[error("`parameter_defaults` must be an object")]
    DefaultsMustBeObject,

    /// The explicitly supplied parameters were not an object.
    #[error("supplied parameters must be an object")]
    ParametersMustBeObject,

    /// The merged value fell outside the version-1 canonical JSON domain.
    #[error(transparent)]
    CanonicalJson(#[from] CanonicalJsonError),

    /// The merged value did not satisfy its parameter schema.
    #[error(transparent)]
    Validation(#[from] SchemaValidationError),
}

fn merge_objects(target: &mut Map<String, Value>, supplied: &Map<String, Value>) {
    for (key, supplied_value) in supplied {
        match (target.get_mut(key), supplied_value) {
            (Some(Value::Object(target)), Value::Object(supplied)) => {
                merge_objects(target, supplied);
            }
            _ => {
                target.insert(key.clone(), supplied_value.clone());
            }
        }
    }
}
