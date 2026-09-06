//! Structural parameter ownership and schema-backed parameter resolution.

use std::{fmt, path::Path};

use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    canonical::{CanonicalJsonError, CanonicalValue},
    manifest::{DatasetDefinition, ExperimentDefinition, Manifest, Name, NamedParameterAxes},
    schema::{SchemaCatalog, SchemaValidationError},
};

/// Validates definition-owned defaults and experiment parameter namespaces.
///
/// Dataset and implementation namespaces must name definitions selected by the
/// enclosing experiment. Those definitions must exist and declare a parameter
/// schema. Problem parameters need no name-qualified structural check because
/// an experiment selects exactly one problem, whose contract always declares
/// its parameter schema.
///
/// This validation deliberately does not infer ownership from parameter names
/// or values. The same name may appear in multiple explicit namespaces when the
/// benchmark author assigns distinct scientific meanings to those parameters.
///
/// # Errors
///
/// Returns the first deterministic structural error in definition, experiment,
/// case, and namespace order.
pub fn validate_parameter_namespaces(manifest: &Manifest) -> Result<(), ParameterNamespaceError> {
    for (name, definition) in &manifest.datasets {
        if let DatasetDefinition::Generated(definition) = definition
            && definition.parameter_defaults.is_some()
            && definition.parameter_schema.is_none()
        {
            return Err(ParameterNamespaceError::DefaultsWithoutSchema {
                namespace: ParameterNamespace::Dataset,
                definition: name.clone(),
            });
        }
    }
    for (name, definition) in &manifest.implementations {
        if definition.parameter_defaults.is_some() && definition.parameter_schema.is_none() {
            return Err(ParameterNamespaceError::DefaultsWithoutSchema {
                namespace: ParameterNamespace::Implementation,
                definition: name.clone(),
            });
        }
    }

    for experiment in &manifest.experiments {
        validate_named_parameter_axes(
            manifest,
            experiment,
            None,
            ParameterNamespace::Dataset,
            &experiment.dataset_parameters,
        )?;
        validate_named_parameter_axes(
            manifest,
            experiment,
            None,
            ParameterNamespace::Implementation,
            &experiment.implementation_parameters,
        )?;

        for case in &experiment.cases {
            validate_named_parameter_axes(
                manifest,
                experiment,
                Some(&case.name),
                ParameterNamespace::Dataset,
                &case.dataset_parameters,
            )?;
            validate_named_parameter_axes(
                manifest,
                experiment,
                Some(&case.name),
                ParameterNamespace::Implementation,
                &case.implementation_parameters,
            )?;
        }
    }

    Ok(())
}

/// A name-qualified owner namespace for scientific parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterNamespace {
    /// Parameters that determine acquired, generated, or transformed data.
    Dataset,
    /// Parameters that determine how one implementation performs computation.
    Implementation,
}

impl fmt::Display for ParameterNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Dataset => "dataset",
            Self::Implementation => "implementation",
        })
    }
}

/// The experiment or named case containing a parameter namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterNamespaceLocation {
    /// Enclosing experiment name.
    pub experiment: Name,
    /// Named case, or `None` for experiment-level defaults.
    pub case: Option<Name>,
}

impl fmt::Display for ParameterNamespaceLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.case {
            Some(case) => write!(
                formatter,
                "case `{case}` in experiment `{}`",
                self.experiment
            ),
            None => write!(formatter, "experiment `{}`", self.experiment),
        }
    }
}

/// A structural parameter-ownership or namespace error.
#[derive(Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum ParameterNamespaceError {
    /// A definition supplies literal defaults without a schema to validate them.
    #[error("{namespace} `{definition}` declares `parameter_defaults` without `parameter_schema`")]
    DefaultsWithoutSchema {
        /// Kind of definition that owns the defaults.
        namespace: ParameterNamespace,
        /// Manifest-local definition name.
        definition: Name,
    },

    /// A named namespace does not belong to the experiment's selected members.
    #[error(
        "{location} has {namespace} parameter namespace `{definition}`, but does not select it"
    )]
    UnselectedDefinition {
        /// Location containing the namespace.
        location: ParameterNamespaceLocation,
        /// Kind of definition named by the namespace.
        namespace: ParameterNamespace,
        /// Manifest-local definition name.
        definition: Name,
    },

    /// A selected namespace has no corresponding manifest definition.
    #[error("{location} has {namespace} parameter namespace `{definition}`, which is undefined")]
    UndefinedDefinition {
        /// Location containing the namespace.
        location: ParameterNamespaceLocation,
        /// Kind of definition named by the namespace.
        namespace: ParameterNamespace,
        /// Missing manifest-local definition name.
        definition: Name,
    },

    /// A named namespace belongs to a definition that cannot accept parameters.
    #[error(
        "{location} has {namespace} parameter namespace `{definition}`, but that definition has no `parameter_schema`"
    )]
    DefinitionHasNoParameterSchema {
        /// Location containing the namespace.
        location: ParameterNamespaceLocation,
        /// Kind of definition named by the namespace.
        namespace: ParameterNamespace,
        /// Manifest-local definition name.
        definition: Name,
    },
}

fn validate_named_parameter_axes(
    manifest: &Manifest,
    experiment: &ExperimentDefinition,
    case: Option<&Name>,
    namespace: ParameterNamespace,
    axes: &NamedParameterAxes,
) -> Result<(), ParameterNamespaceError> {
    let selected = match namespace {
        ParameterNamespace::Dataset => &experiment.datasets,
        ParameterNamespace::Implementation => &experiment.implementations,
    };

    for definition in axes.keys() {
        let location = || ParameterNamespaceLocation {
            experiment: experiment.name.clone(),
            case: case.cloned(),
        };

        if !selected.contains(definition) {
            return Err(ParameterNamespaceError::UnselectedDefinition {
                location: location(),
                namespace,
                definition: definition.clone(),
            });
        }

        let has_parameter_schema = match namespace {
            ParameterNamespace::Dataset => {
                let Some(dataset) = manifest.datasets.get(definition) else {
                    return Err(ParameterNamespaceError::UndefinedDefinition {
                        location: location(),
                        namespace,
                        definition: definition.clone(),
                    });
                };
                matches!(
                    dataset,
                    DatasetDefinition::Generated(definition)
                        if definition.parameter_schema.is_some()
                )
            }
            ParameterNamespace::Implementation => {
                let Some(implementation) = manifest.implementations.get(definition) else {
                    return Err(ParameterNamespaceError::UndefinedDefinition {
                        location: location(),
                        namespace,
                        definition: definition.clone(),
                    });
                };
                implementation.parameter_schema.is_some()
            }
        };

        if !has_parameter_schema {
            return Err(ParameterNamespaceError::DefinitionHasNoParameterSchema {
                location: location(),
                namespace,
                definition: definition.clone(),
            });
        }
    }

    Ok(())
}

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
