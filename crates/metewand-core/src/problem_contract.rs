//! Typed parsing and validation for version-1 problem contracts.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor, value::MapAccessDeserializer},
};
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    canonical::CanonicalValue,
    field_declaration::{OutputDeclarations, ParameterBoundary, ParameterDeclarations},
    manifest::{
        Name, ParameterAxes, RepositoryPath, SourceSpan, TimingScope, deserialize_manifest_value,
        deserialize_optional_non_null, deserialize_version_one, ensure_nonempty_unique,
    },
    schema::{SchemaCatalog, SchemaDeclaration, SchemaValidationError},
};

/// An inline problem contract or a reusable repository YAML document.
#[derive(Debug)]
pub enum ContractDeclaration {
    /// A repository-relative contract path.
    Reference(RepositoryPath),
    /// A complete contract in the manifest.
    Inline(Box<ProblemContract>),
}

impl ContractDeclaration {
    /// Returns the source path for a referenced contract.
    #[must_use]
    pub fn reference_path(&self) -> Option<&Path> {
        match self {
            Self::Reference(path) => Some(path.as_path()),
            Self::Inline(_) => None,
        }
    }

    /// Returns the effective contract envelope for manifest hashing.
    ///
    /// The owning problem definition contributes parameter defaults and grids
    /// separately because they select runs rather than change the contract.
    #[must_use]
    pub fn declaration_value(&self) -> Value {
        match self {
            Self::Reference(path) => json!(path.as_path()),
            Self::Inline(contract) => contract.declaration_value(),
        }
    }
}

impl<'de> Deserialize<'de> for ContractDeclaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ContractVisitor;
        impl<'de> Visitor<'de> for ContractVisitor {
            type Value = ContractDeclaration;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a repository contract path or an inline problem contract")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                RepositoryPath::deserialize(de::value::StrDeserializer::<E>::new(value))
                    .map(ContractDeclaration::Reference)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                ProblemContract::deserialize(MapAccessDeserializer::new(map))
                    .map(|contract| ContractDeclaration::Inline(Box::new(contract)))
            }
        }
        deserializer.deserialize_any(ContractVisitor)
    }
}

/// Parses and validates one version-1 problem contract.
///
/// Parsing rejects unknown common-envelope fields and values outside Metewand's
/// canonical JSON domain. Validation requires every referenced schema to be in
/// `schemas`, then validates the complete family-owned `semantics` object
/// against its declared schema. No filesystem or network access is performed.
///
/// # Errors
///
/// Returns a source-spanned error for invalid YAML or an invalid typed envelope,
/// a missing-schema error, or the deterministic violations produced by the
/// family-specific semantics schema.
pub fn parse_problem_contract(
    source_path: &Path,
    source: &str,
    schemas: &SchemaCatalog,
) -> Result<ProblemContract, ProblemContractError> {
    let contract = parse_problem_contract_document(source_path, source)?;

    validate_problem_contract(source_path, &contract, schemas)?;

    Ok(contract)
}

/// Parses one version-1 problem contract without resolving its schema references.
///
/// Repository loaders use this first phase to discover the complete set of
/// schemas before compiling an offline catalog. Callers must subsequently use
/// [`validate_problem_contract`] before treating the document as valid.
///
/// # Errors
///
/// Returns a source-spanned error for invalid YAML or an invalid typed envelope.
pub fn parse_problem_contract_document(
    source_path: &Path,
    source: &str,
) -> Result<ProblemContract, ProblemContractError> {
    let mut contract: ProblemContract =
        crate::yaml::parse(source_path, source).map_err(|error| ProblemContractError::Parse {
            message: error.message,
            source_span: error.source_span,
        })?;
    contract.set_schema_owner(source_path);
    Ok(contract)
}

/// Validates a parsed problem contract against a complete offline schema catalog.
///
/// # Errors
///
/// Returns an error when a referenced schema is absent or the family-owned
/// semantics do not satisfy their schema.
pub fn validate_problem_contract(
    source_path: &Path,
    contract: &ProblemContract,
    schemas: &SchemaCatalog,
) -> Result<(), ProblemContractError> {
    validate_schema_references(source_path, contract, schemas)?;
    schemas
        .validate(
            contract.semantics_schema.as_path(),
            contract.semantics.as_canonical().as_json(),
        )
        .map_err(|source| ProblemContractError::InvalidSemantics {
            contract_path: source_path.to_path_buf(),
            source,
        })?;

    Ok(())
}

/// A validated, language-neutral problem and fairness contract.
#[derive(Clone, Debug)]
pub struct ProblemContract {
    /// Problem-contract compatibility version.
    pub version: u32,
    /// Stable problem name declared by the contract.
    pub name: Name,
    /// Open family tag whose meaning is owned by the benchmark.
    pub family: Name,
    /// Schema for problem-owned parameters.
    pub parameter_schema: SchemaDeclaration,
    /// Literal defaults authored in compact problem parameters.
    pub parameter_defaults: Option<CanonicalValue>,
    /// Default expansion axes, kept separate from the valid parameter domain.
    pub parameter_grids: ParameterAxes,
    pub(crate) compact_parameters: bool,
    /// Dataset schemas accepted by this problem, or an empty list when the
    /// problem is dataset-free.
    pub dataset_schemas: Vec<SchemaDeclaration>,
    /// Schema for canonical implementation results.
    pub result_schema: SchemaDeclaration,
    /// Schema for independent evaluator metrics.
    pub metric_schema: SchemaDeclaration,
    /// Schema that owns the family-specific semantics object.
    pub semantics_schema: SchemaDeclaration,
    /// Timing scopes under which the problem may be measured.
    pub allowed_timing_scopes: Vec<TimingScope>,
    /// Scientific budgets supported by the contract.
    pub supported_budgets: Vec<ScientificBudget>,
    /// Validated family-specific semantics and operational rules.
    pub semantics: ProblemSemantics,
    /// Small conformance cases for the problem evaluator.
    pub reference_cases: Vec<ReferenceCase>,
}

impl ProblemContract {
    /// Returns every schema declaration in the common envelope.
    pub fn schemas(&self) -> impl Iterator<Item = &SchemaDeclaration> {
        [
            &self.parameter_schema,
            &self.result_schema,
            &self.metric_schema,
            &self.semantics_schema,
        ]
        .into_iter()
        .chain(&self.dataset_schemas)
    }

    pub(crate) fn set_schema_owner(&mut self, owner: &Path) {
        for schema in [
            &mut self.parameter_schema,
            &mut self.result_schema,
            &mut self.metric_schema,
            &mut self.semantics_schema,
        ]
        .into_iter()
        .chain(&mut self.dataset_schemas)
        {
            schema.set_owner(owner);
        }
    }

    fn declaration_value(&self) -> Value {
        json!({
            "version": self.version, "name": self.name.as_str(), "family": self.family.as_str(),
            "parameter_schema": self.parameter_schema.declaration_value(),
            "dataset_schemas": self.dataset_schemas.iter().map(SchemaDeclaration::declaration_value).collect::<Vec<_>>(),
            "result_schema": self.result_schema.declaration_value(),
            "metric_schema": self.metric_schema.declaration_value(),
            "semantics_schema": self.semantics_schema.declaration_value(),
            "allowed_timing_scopes": self.allowed_timing_scopes.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "supported_budgets": self.supported_budgets.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "semantics": self.semantics.as_canonical().as_json(),
            "reference_cases": self.reference_cases.iter().map(|case| json!({
                "dataset": case.dataset.as_ref().map(RepositoryPath::as_path),
                "result": case.result.as_path(), "expected_metrics": case.expected_metrics.as_path(),
            })).collect::<Vec<_>>(),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawProblemContract {
    #[serde(deserialize_with = "deserialize_version_one")]
    version: u32,
    name: Name,
    family: Name,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    parameters: Option<ParameterDeclarations>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    parameter_schema: Option<SchemaDeclaration>,
    dataset_schemas: Vec<SchemaDeclaration>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    result: Option<OutputDeclarations>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    result_schema: Option<SchemaDeclaration>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    metrics: Option<OutputDeclarations>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    metric_schema: Option<SchemaDeclaration>,
    semantics_schema: SchemaDeclaration,
    allowed_timing_scopes: Vec<TimingScope>,
    supported_budgets: Vec<ScientificBudget>,
    semantics: ProblemSemantics,
    reference_cases: Vec<ReferenceCase>,
}

impl<'de> Deserialize<'de> for ProblemContract {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawProblemContract::deserialize(deserializer)?;
        let compact_parameters = raw.parameters.is_some();
        let parameters = ParameterBoundary::lower(raw.parameters, raw.parameter_schema, None)
            .map_err(de::Error::custom)?;
        let parameter_schema = parameters.schema.ok_or_else(|| {
            de::Error::custom("a problem contract requires `parameters` or `parameter-schema`")
        })?;
        let result_schema = lower_output(raw.result, raw.result_schema, "result", "result-schema")
            .map_err(de::Error::custom)?;
        let metric_schema =
            lower_output(raw.metrics, raw.metric_schema, "metrics", "metric-schema")
                .map_err(de::Error::custom)?;
        ensure_unique(&raw.dataset_schemas, "`dataset-schemas`").map_err(de::Error::custom)?;
        ensure_nonempty_unique(&raw.allowed_timing_scopes, "`allowed-timing-scopes`")
            .map_err(de::Error::custom)?;
        if raw.supported_budgets != [ScientificBudget::None] {
            return Err(de::Error::custom(
                "version 1 requires `supported-budgets: [\"none\"]`",
            ));
        }
        if raw.reference_cases.is_empty() {
            return Err(de::Error::custom("`reference-cases` cannot be empty"));
        }

        let requires_dataset = !raw.dataset_schemas.is_empty();
        for (index, reference_case) in raw.reference_cases.iter().enumerate() {
            match (requires_dataset, reference_case.dataset.is_some()) {
                (true, false) => {
                    return Err(de::Error::custom(format_args!(
                        "reference case {index} requires `dataset` because the contract accepts datasets"
                    )));
                }
                (false, true) => {
                    return Err(de::Error::custom(format_args!(
                        "reference case {index} must omit `dataset` because the contract is dataset-free"
                    )));
                }
                _ => {}
            }
        }

        Ok(Self {
            version: raw.version,
            name: raw.name,
            family: raw.family,
            parameter_schema,
            parameter_defaults: parameters.defaults,
            parameter_grids: parameters.grids,
            compact_parameters,
            dataset_schemas: raw.dataset_schemas,
            result_schema,
            metric_schema,
            semantics_schema: raw.semantics_schema,
            allowed_timing_scopes: raw.allowed_timing_scopes,
            supported_budgets: raw.supported_budgets,
            semantics: raw.semantics,
            reference_cases: raw.reference_cases,
        })
    }
}

fn lower_output(
    compact: Option<OutputDeclarations>,
    schema: Option<SchemaDeclaration>,
    compact_name: &str,
    schema_name: &str,
) -> Result<SchemaDeclaration, String> {
    match (compact, schema) {
        (Some(_), Some(_)) => Err(format!(
            "`{compact_name}` cannot be combined with `{schema_name}`"
        )),
        (Some(compact), None) => Ok(compact.compile()?.schema),
        (None, Some(schema)) => Ok(schema),
        (None, None) => Err(format!(
            "a problem contract requires `{compact_name}` or `{schema_name}`"
        )),
    }
}

/// Family-owned problem semantics with required operational rule groups.
#[derive(Clone, Debug)]
pub struct ProblemSemantics {
    value: CanonicalValue,
}

impl ProblemSemantics {
    /// Returns the complete family-owned semantics object.
    #[must_use]
    pub fn as_canonical(&self) -> &CanonicalValue {
        &self.value
    }

    /// Returns the contract's result-validity rules.
    #[must_use]
    pub fn validity(&self) -> &Value {
        &self.value.as_json()["validity"]
    }

    /// Returns the contract's one-shot completion rules.
    #[must_use]
    pub fn one_shot_completion(&self) -> &Value {
        &self.value.as_json()["one_shot_completion"]
    }
}

impl<'de> Deserialize<'de> for ProblemSemantics {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = deserialize_manifest_value(deserializer)?;
        let object = value
            .as_json()
            .as_object()
            .ok_or_else(|| de::Error::custom("`semantics` must be an object"))?;

        for rule in ["validity", "one_shot_completion"] {
            let rules = object.get(rule).and_then(Value::as_object).ok_or_else(|| {
                de::Error::custom(format_args!("`semantics.{rule}` must be a present object"))
            })?;
            if rules.is_empty() {
                return Err(de::Error::custom(format_args!(
                    "`semantics.{rule}` cannot be empty"
                )));
            }
        }

        Ok(Self { value })
    }
}

/// A problem/evaluator conformance case.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReferenceCase {
    /// Dataset artifact used by the case, when the problem accepts datasets.
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    pub dataset: Option<RepositoryPath>,
    /// Canonical result artifact evaluated by the case.
    pub result: RepositoryPath,
    /// Expected evaluator metrics for the result.
    pub expected_metrics: RepositoryPath,
}

/// A scientific budget supported by a problem contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScientificBudget {
    /// No problem-defined scientific budget.
    None,
}

impl fmt::Display for ScientificBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
        })
    }
}

/// The purpose of a schema referenced by a problem contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProblemSchemaRole {
    /// Problem-parameter schema.
    Parameter,
    /// Accepted dataset schema.
    Dataset,
    /// Canonical result schema.
    Result,
    /// Evaluator metric schema.
    Metric,
    /// Family-specific semantics schema.
    Semantics,
}

impl fmt::Display for ProblemSchemaRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Parameter => "parameter",
            Self::Dataset => "dataset",
            Self::Result => "result",
            Self::Metric => "metric",
            Self::Semantics => "semantics",
        })
    }
}

/// A problem-contract parsing or validation failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProblemContractError {
    /// The YAML document did not satisfy the typed common envelope.
    #[error("failed to parse problem contract at `{}`: {message}", source_span.path.display())]
    Parse {
        /// Decoder explanation.
        message: String,
        /// Source range attributed by the YAML decoder.
        source_span: SourceSpan,
    },

    /// A schema referenced by the common envelope was unavailable.
    #[error(
        "problem contract `{}` references missing {role} schema `{}`",
        contract_path.display(),
        schema_path.display()
    )]
    MissingSchema {
        /// Logical repository path of the contract.
        contract_path: PathBuf,
        /// Purpose of the missing schema.
        role: ProblemSchemaRole,
        /// Logical repository path expected in the schema catalog.
        schema_path: PathBuf,
    },

    /// The family-specific semantics object did not satisfy its schema.
    #[error("problem contract `{}` has invalid family semantics: {source}", contract_path.display())]
    InvalidSemantics {
        /// Logical repository path of the contract.
        contract_path: PathBuf,
        /// Deterministically ordered schema violations.
        #[source]
        source: SchemaValidationError,
    },
}

impl ProblemContractError {
    /// Returns the YAML decoder's explanation for typed-envelope failures.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Parse { message, .. } => Some(message),
            Self::MissingSchema { .. } | Self::InvalidSemantics { .. } => None,
        }
    }

    /// Returns the source range for YAML and typed-envelope failures.
    #[must_use]
    pub fn source_span(&self) -> Option<&SourceSpan> {
        match self {
            Self::Parse { source_span, .. } => Some(source_span),
            Self::MissingSchema { .. } | Self::InvalidSemantics { .. } => None,
        }
    }
}

fn validate_schema_references(
    contract_path: &Path,
    contract: &ProblemContract,
    schemas: &SchemaCatalog,
) -> Result<(), ProblemContractError> {
    require_schema(
        contract_path,
        ProblemSchemaRole::Parameter,
        &contract.parameter_schema,
        schemas,
    )?;
    for schema_path in &contract.dataset_schemas {
        require_schema(
            contract_path,
            ProblemSchemaRole::Dataset,
            schema_path,
            schemas,
        )?;
    }
    require_schema(
        contract_path,
        ProblemSchemaRole::Result,
        &contract.result_schema,
        schemas,
    )?;
    require_schema(
        contract_path,
        ProblemSchemaRole::Metric,
        &contract.metric_schema,
        schemas,
    )?;
    require_schema(
        contract_path,
        ProblemSchemaRole::Semantics,
        &contract.semantics_schema,
        schemas,
    )?;
    Ok(())
}

fn require_schema(
    contract_path: &Path,
    role: ProblemSchemaRole,
    schema_path: &SchemaDeclaration,
    schemas: &SchemaCatalog,
) -> Result<(), ProblemContractError> {
    if schemas.contains(schema_path.as_path()) {
        Ok(())
    } else {
        Err(ProblemContractError::MissingSchema {
            contract_path: contract_path.to_path_buf(),
            role,
            schema_path: schema_path.as_path().to_path_buf(),
        })
    }
}

fn ensure_unique<T>(values: &[T], description: &str) -> Result<(), String>
where
    T: PartialEq,
{
    if values
        .iter()
        .enumerate()
        .any(|(index, value)| values[..index].contains(value))
    {
        Err(format!("{description} cannot contain duplicates"))
    } else {
        Ok(())
    }
}
