//! Offline JSON Schema validation for repository-owned contracts.
//!
//! The catalog operates on parsed documents supplied by its caller. This keeps
//! filesystem and lockfile concerns outside core while ensuring that validation
//! can resolve only the repository documents admitted to the catalog.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    path::{Path, PathBuf},
};

use jsonschema::{Draft, Registry, Validator};
use serde_json::Value;
use thiserror::Error;

/// The only JSON Schema dialect supported by the version-1 compatibility contract.
pub const DRAFT_2020_12_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

const REPOSITORY_SCHEMA_BASE_URI: &str = "metewand://repository/";

/// A collection of compiled schemas addressable by normalized repository path.
#[derive(Debug)]
pub struct SchemaCatalog {
    validators: BTreeMap<PathBuf, Validator>,
}

impl SchemaCatalog {
    /// Builds an offline catalog from parsed repository schema documents.
    ///
    /// All document paths must be normalized, relative UTF-8 paths. Every
    /// document must explicitly declare the Draft 2020-12 dialect and satisfy
    /// its meta-schema. External references can resolve only to resources in
    /// the supplied catalog.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid or duplicate paths, missing or unsupported
    /// dialect declarations, invalid schemas, and references that cannot be
    /// resolved from the supplied documents.
    pub fn try_new(
        documents: impl IntoIterator<Item = (PathBuf, Value)>,
    ) -> Result<Self, SchemaCatalogError> {
        let mut documents_by_path = BTreeMap::new();
        for (path, document) in documents {
            validate_repository_path(&path)?;
            match documents_by_path.entry(path) {
                Entry::Vacant(entry) => {
                    entry.insert(document);
                }
                Entry::Occupied(entry) => {
                    return Err(SchemaCatalogError::DuplicatePath {
                        path: entry.key().clone(),
                    });
                }
            }
        }

        for (path, document) in &documents_by_path {
            validate_dialect(path, document)?;
            jsonschema::draft202012::meta::validate(document).map_err(|error| {
                SchemaCatalogError::InvalidSchema {
                    path: path.clone(),
                    message: error.to_string(),
                }
            })?;
        }

        let mut registry = Registry::new().draft(Draft::Draft202012);
        for (path, document) in &documents_by_path {
            registry = registry
                .add(repository_uri(path), document.clone())
                .map_err(|error| SchemaCatalogError::Compilation {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
        }
        let registry = registry
            .prepare()
            .map_err(|error| SchemaCatalogError::Compilation {
                path: PathBuf::from("<catalog>"),
                message: error.to_string(),
            })?;

        let mut validators = BTreeMap::new();
        for (path, document) in &documents_by_path {
            let validator = jsonschema::draft202012::options()
                .with_registry(&registry)
                .with_base_uri(repository_uri(path))
                .offline()
                .build(document)
                .map_err(|error| SchemaCatalogError::Compilation {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
            validators.insert(path.clone(), validator);
        }

        Ok(Self { validators })
    }

    /// Validates an instance with the schema at `schema_path`.
    ///
    /// All violations are returned in deterministic path-and-message order.
    /// Validation does not modify the instance or apply schema annotations.
    ///
    /// # Errors
    ///
    /// Returns an error if the requested schema is absent or the instance does
    /// not satisfy it.
    pub fn validate(
        &self,
        schema_path: &Path,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        let validator = self.validators.get(schema_path).ok_or_else(|| {
            SchemaValidationError::UnknownSchema {
                path: schema_path.to_path_buf(),
            }
        })?;

        let mut violations = validator
            .iter_errors(instance)
            .map(|error| SchemaViolation {
                instance_path: error.instance_path().to_string(),
                schema_path: error
                    .absolute_keyword_location()
                    .map_or_else(|| error.schema_path().to_string(), ToString::to_string),
                message: error.to_string(),
            })
            .collect::<Vec<_>>();
        violations.sort();

        if violations.is_empty() {
            Ok(())
        } else {
            Err(SchemaValidationError::InvalidInstance {
                path: schema_path.to_path_buf(),
                violations,
            })
        }
    }
}

/// An error encountered while constructing a schema catalog.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SchemaCatalogError {
    /// A schema path was absolute, non-normalized, non-UTF-8, or non-portable.
    #[error("schema path `{path}` must be a normalized, relative UTF-8 repository path")]
    InvalidPath {
        /// The rejected logical repository path.
        path: PathBuf,
    },

    /// More than one document was registered at the same path.
    #[error("schema path `{path}` is registered more than once")]
    DuplicatePath {
        /// The duplicated logical repository path.
        path: PathBuf,
    },

    /// A schema omitted its required dialect declaration.
    #[error("schema `{path}` does not declare `$schema`")]
    MissingDialect {
        /// The logical repository path of the schema.
        path: PathBuf,
    },

    /// A schema declared a dialect other than Draft 2020-12.
    #[error("schema `{path}` declares unsupported dialect `{dialect}`")]
    UnsupportedDialect {
        /// The logical repository path of the schema.
        path: PathBuf,
        /// The rejected dialect value.
        dialect: String,
    },

    /// A schema did not satisfy the Draft 2020-12 meta-schema.
    #[error("schema `{path}` is not valid Draft 2020-12 JSON Schema: {message}")]
    InvalidSchema {
        /// The logical repository path of the schema.
        path: PathBuf,
        /// The underlying validation message.
        message: String,
    },

    /// A schema or its references could not be compiled from the offline catalog.
    #[error("schema `{path}` could not be compiled offline: {message}")]
    Compilation {
        /// The logical repository path being compiled.
        path: PathBuf,
        /// The underlying compilation message.
        message: String,
    },
}

/// An error produced while validating an instance.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemaValidationError {
    /// The requested schema was not registered.
    #[error("schema `{path}` is not present in the catalog")]
    UnknownSchema {
        /// The requested logical repository path.
        path: PathBuf,
    },

    /// The instance failed one or more schema constraints.
    #[error("instance does not satisfy schema `{path}`")]
    InvalidInstance {
        /// The logical repository path of the selected schema.
        path: PathBuf,
        /// Every observed validation failure in deterministic order.
        violations: Vec<SchemaViolation>,
    },
}

/// One failed JSON Schema constraint.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchemaViolation {
    /// JSON Pointer to the invalid part of the instance.
    pub instance_path: String,
    /// Absolute schema URI and JSON Pointer, when available.
    pub schema_path: String,
    /// Human-readable description of the violated constraint.
    pub message: String,
}

fn validate_repository_path(path: &Path) -> Result<(), SchemaCatalogError> {
    let valid_components = path.to_str().is_some_and(|path| {
        !path.is_empty()
            && !path.starts_with('/')
            && !path.contains('\\')
            && path
                .split('/')
                .all(|component| !component.is_empty() && component != "." && component != "..")
    });

    if valid_components {
        Ok(())
    } else {
        Err(SchemaCatalogError::InvalidPath {
            path: path.to_path_buf(),
        })
    }
}

fn validate_dialect(path: &Path, document: &Value) -> Result<(), SchemaCatalogError> {
    let Some(dialect) = document
        .as_object()
        .and_then(|object| object.get("$schema"))
    else {
        return Err(SchemaCatalogError::MissingDialect {
            path: path.to_path_buf(),
        });
    };
    let Some(dialect) = dialect.as_str() else {
        return Err(SchemaCatalogError::UnsupportedDialect {
            path: path.to_path_buf(),
            dialect: dialect.to_string(),
        });
    };

    if dialect == DRAFT_2020_12_DIALECT {
        Ok(())
    } else {
        Err(SchemaCatalogError::UnsupportedDialect {
            path: path.to_path_buf(),
            dialect: dialect.to_owned(),
        })
    }
}

fn repository_uri(path: &Path) -> String {
    let path = path
        .to_str()
        .expect("repository path validation guarantees UTF-8");
    let mut uri = String::with_capacity(REPOSITORY_SCHEMA_BASE_URI.len() + path.len());
    uri.push_str(REPOSITORY_SCHEMA_BASE_URI);
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(uri, "%{byte:02X}").expect("writing to a string cannot fail");
        }
    }
    uri
}
