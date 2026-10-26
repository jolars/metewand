//! Offline JSON Schema validation for repository-owned contracts.
//!
//! The catalog operates on parsed documents supplied by its caller. This keeps
//! filesystem and lockfile concerns outside core while ensuring that validation
//! can resolve only the repository documents admitted to the catalog.

use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonschema::{Draft, Registry, Validator};
use serde::{Deserialize, Deserializer, de};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{canonical::CanonicalValue, manifest::RepositoryPath};

/// The only JSON Schema dialect supported by the version-1 compatibility contract.
pub const DRAFT_2020_12_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

const REPOSITORY_SCHEMA_BASE_URI: &str = "metewand://repository/";

/// A reusable repository schema or an inline Draft 2020-12 schema.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaDeclaration {
    path: PathBuf,
    document: Option<Box<CanonicalValue>>,
}

impl SchemaDeclaration {
    /// Returns the catalog key, including a content-derived key for inline schemas.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// Returns the normalized inline document, or `None` for a file reference.
    #[must_use]
    pub fn inline_document(&self) -> Option<&CanonicalValue> {
        self.document.as_deref()
    }

    /// Returns the normalized authored declaration used by manifest hashing.
    #[must_use]
    pub fn declaration_value(&self) -> Value {
        self.document.as_ref().map_or_else(
            || Value::String(self.path.to_string_lossy().into_owned()),
            |document| document.as_json().clone(),
        )
    }

    pub(crate) fn set_owner(&mut self, owner: &Path) {
        if self.document.is_some() {
            // A virtual sibling keeps relative references rooted at the owning
            // document without inserting schema files into the repository.
            let filename = self.path.file_name().expect("inline schema has a key");
            self.path = owner
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(filename);
        }
    }
}

impl<'de> Deserialize<'de> for SchemaDeclaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = CanonicalValue::try_from(Value::deserialize(deserializer)?)
            .map_err(de::Error::custom)?;
        let mut document = match value.as_json() {
            Value::String(_) => {
                let path = RepositoryPath::deserialize(value.as_json().clone())
                    .map_err(de::Error::custom)?;
                return Ok(Self {
                    path: path.as_path().to_path_buf(),
                    document: None,
                });
            }
            Value::Object(_) => value.as_json().clone(),
            Value::Bool(true) => serde_json::json!({}),
            Value::Bool(false) => serde_json::json!({"allOf": [false]}),
            _ => {
                return Err(de::Error::custom(
                    "expected a schema path, object, or boolean",
                ));
            }
        };
        document
            .as_object_mut()
            .expect("normalized schema is an object")
            .entry("$schema")
            .or_insert_with(|| Value::String(DRAFT_2020_12_DIALECT.to_owned()));
        let document = CanonicalValue::try_from(document).map_err(de::Error::custom)?;
        let bytes = document.to_canonical_bytes().map_err(de::Error::custom)?;
        let digest = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(Self {
            path: PathBuf::from(format!(".metewand-inline-{digest}.schema.json")),
            document: Some(Box::new(document)),
        })
    }
}

/// A collection of compiled schemas addressable by normalized repository path.
#[derive(Clone, Debug)]
pub struct SchemaCatalog {
    validators: Arc<BTreeMap<PathBuf, Validator>>,
    normalized: Arc<BTreeMap<PathBuf, Value>>,
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

        let normalized = documents_by_path
            .iter()
            .map(|(path, document)| {
                Ok((path.clone(), schema_resources(path, document)?.normalized))
            })
            .collect::<Result<_, SchemaCatalogError>>()?;
        Ok(Self {
            validators: Arc::new(validators),
            normalized: Arc::new(normalized),
        })
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
                value_path: match error.kind() {
                    jsonschema::error::ValidationErrorKind::Required { property } => format!(
                        "{}/{}",
                        error.instance_path(),
                        crate::yaml::escape_pointer(property.as_str().unwrap_or_default())
                    ),
                    jsonschema::error::ValidationErrorKind::AdditionalProperties { unexpected }
                    | jsonschema::error::ValidationErrorKind::UnevaluatedProperties {
                        unexpected,
                    } => unexpected.iter().min().map_or_else(
                        || error.instance_path().to_string(),
                        |property| {
                            format!(
                                "{}/{}",
                                error.instance_path(),
                                crate::yaml::escape_pointer(property)
                            )
                        },
                    ),
                    _ => error.instance_path().to_string(),
                },
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

    /// Returns whether a schema is registered at `schema_path`.
    #[must_use]
    pub fn contains(&self, schema_path: &Path) -> bool {
        self.validators.contains_key(schema_path)
    }

    /// Compares normalized declarations without attempting schema implication.
    ///
    /// Inline schemas can match reusable files by their contents. Two explicit
    /// file references retain their existing path-based matching behavior.
    #[must_use]
    pub fn matches(&self, left: &SchemaDeclaration, right: &SchemaDeclaration) -> bool {
        left == right
            || ((left.inline_document().is_some() || right.inline_document().is_some())
                && self
                    .normalized
                    .get(left.as_path())
                    .is_some_and(|document| Some(document) == self.normalized.get(right.as_path())))
    }
}

/// Schema resources and references interpreted under Draft 2020-12.
pub struct SchemaResources {
    /// Declared resource URIs, including nested `$id` declarations.
    pub identifiers: BTreeSet<String>,
    /// Resolved reference URIs without fragments.
    pub references: BTreeSet<String>,
    normalized: Value,
}

/// Resolves schema URIs without accessing the filesystem or network.
///
/// Only schema keywords are traversed, so literal data in `const`, `enum`,
/// `default`, and examples never becomes a resource dependency.
///
/// # Errors
///
/// Returns an error for invalid schema URIs.
pub fn schema_resources(
    path: &Path,
    document: &Value,
) -> Result<SchemaResources, SchemaCatalogError> {
    let mut resources = SchemaResources {
        identifiers: BTreeSet::from([repository_uri(path)]),
        references: BTreeSet::new(),
        normalized: document.clone(),
    };
    normalize_schema_uris(
        &mut resources.normalized,
        &repository_uri(path),
        &mut resources.identifiers,
        &mut resources.references,
    )
    .map_err(|message| SchemaCatalogError::Compilation {
        path: path.to_path_buf(),
        message,
    })?;
    Ok(resources)
}

fn resolve_uri(reference: &str, base: &str) -> Result<String, String> {
    let reference = fluent_uri::UriRef::parse(reference).map_err(|error| error.to_string())?;
    let base = fluent_uri::Uri::parse(base).map_err(|error| error.to_string())?;
    if base.as_str().starts_with(REPOSITORY_SCHEMA_BASE_URI)
        && reference.scheme().is_none()
        && reference.authority().is_none()
    {
        let path = reference.path().as_str();
        if path.starts_with('/') {
            return Err("schema references must stay relative to the repository".to_owned());
        }
        let mut depth = base
            .path()
            .as_str()
            .split('/')
            .filter(|part| !part.is_empty())
            .count();
        if !base.path().as_str().ends_with('/') {
            depth = depth.saturating_sub(1);
        }
        for part in path.split('/') {
            match part {
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| "schema reference escapes the repository".to_owned())?
                }
                "" | "." => {}
                _ => depth += 1,
            }
        }
    }
    reference
        .resolve_against(&base)
        .map(|uri| uri.to_string())
        .map_err(|error| error.to_string())
}

fn normalize_schema_uris(
    value: &mut Value,
    base: &str,
    identifiers: &mut BTreeSet<String>,
    references: &mut BTreeSet<String>,
) -> Result<(), String> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    let base = match object.get_mut("$id") {
        Some(Value::String(id)) => {
            *id = resolve_uri(id, base)?;
            identifiers.insert(id.clone());
            id.clone()
        }
        _ => base.to_owned(),
    };
    for keyword in ["$ref", "$dynamicRef"] {
        if let Some(Value::String(reference)) = object.get_mut(keyword) {
            let resolved = resolve_uri(reference, &base)?;
            references.insert(resolved.split('#').next().unwrap_or_default().to_owned());
            // Local fragments belong to this schema regardless of its catalog key.
            if !reference.starts_with('#') {
                *reference = resolved;
            }
        }
    }
    for (keyword, child) in object {
        match keyword.as_str() {
            "$defs" | "definitions" | "dependentSchemas" | "patternProperties" | "properties" => {
                if let Some(children) = child.as_object_mut() {
                    for child in children.values_mut() {
                        normalize_schema_uris(child, &base, identifiers, references)?;
                    }
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(children) = child.as_array_mut() {
                    for child in children {
                        normalize_schema_uris(child, &base, identifiers, references)?;
                    }
                }
            }
            "additionalProperties"
            | "unevaluatedProperties"
            | "propertyNames"
            | "items"
            | "contains"
            | "unevaluatedItems"
            | "not"
            | "if"
            | "then"
            | "else"
            | "contentSchema" => {
                normalize_schema_uris(child, &base, identifiers, references)?;
            }
            _ => {}
        }
    }
    Ok(())
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
    /// JSON Pointer to the offending or missing value, including object keys.
    pub value_path: String,
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

#[cfg(test)]
mod tests {
    use crate::public_schemas::{PUBLIC_SCHEMAS, public_schema_catalog};

    #[test]
    fn public_catalogs_share_compiled_validators_across_threads() {
        let catalogs = std::thread::scope(|scope| {
            let callers = (0..8)
                .map(|_| scope.spawn(|| public_schema_catalog().unwrap()))
                .collect::<Vec<_>>();
            callers
                .into_iter()
                .map(|caller| caller.join().unwrap())
                .collect::<Vec<_>>()
        });

        for schema in PUBLIC_SCHEMAS {
            let first = catalogs[0]
                .validators
                .get(schema.repository_path())
                .unwrap();
            for catalog in &catalogs[1..] {
                assert!(std::ptr::eq(
                    first,
                    catalog.validators.get(schema.repository_path()).unwrap()
                ));
            }
        }
    }
}
