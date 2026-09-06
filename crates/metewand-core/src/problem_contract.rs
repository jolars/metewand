//! Typed parsing and validation for version-1 problem contracts.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Deserializer, de};
use serde_json::Value;
use thiserror::Error;

use crate::{
    canonical::CanonicalValue,
    manifest::{
        Name, RepositoryPath, SourceSpan, TimingScope, deserialize_manifest_value,
        deserialize_version_one, ensure_nonempty_unique,
    },
    schema::{SchemaCatalog, SchemaValidationError},
};

/// Parses and validates one version-1 problem contract.
///
/// Parsing rejects unknown common-envelope fields and values outside Metewand's
/// canonical JSON domain. Validation requires every referenced schema to be in
/// `schemas`, then validates the complete family-owned `semantics` object
/// against its declared schema. No filesystem or network access is performed.
///
/// # Errors
///
/// Returns a source-spanned error for invalid TOML or an invalid typed envelope,
/// a missing-schema error, or the deterministic violations produced by the
/// family-specific semantics schema.
pub fn parse_problem_contract(
    source_path: &Path,
    source: &str,
    schemas: &SchemaCatalog,
) -> Result<ProblemContract, ProblemContractError> {
    let parsed: ParsedProblemContract =
        toml::from_str(source).map_err(|error: toml::de::Error| {
            let byte_span = error.span().unwrap_or(source.len()..source.len());
            ProblemContractError::Parse {
                message: error.message().to_owned(),
                source_span: SourceSpan::from_byte_range(source_path, source, byte_span),
            }
        })?;
    let contract = parsed.0;

    validate_schema_references(source_path, &contract, schemas)?;
    schemas
        .validate(
            contract.semantics_schema.as_path(),
            contract.semantics.as_canonical().as_json(),
        )
        .map_err(|source| ProblemContractError::InvalidSemantics {
            contract_path: source_path.to_path_buf(),
            source,
        })?;

    Ok(contract)
}

/// A validated, language-neutral problem and fairness contract.
#[derive(Debug)]
pub struct ProblemContract {
    /// Problem-contract compatibility version.
    pub version: u32,
    /// Stable problem name declared by the contract.
    pub name: Name,
    /// Open family tag whose meaning is owned by the benchmark.
    pub family: Name,
    /// Schema for problem-owned parameters.
    pub parameter_schema: RepositoryPath,
    /// Dataset schemas accepted by this problem, or an empty list when the
    /// problem is dataset-free.
    pub dataset_schemas: Vec<RepositoryPath>,
    /// Schema for canonical implementation results.
    pub result_schema: RepositoryPath,
    /// Schema for independent evaluator metrics.
    pub metric_schema: RepositoryPath,
    /// Schema that owns the family-specific semantics object.
    pub semantics_schema: RepositoryPath,
    /// Timing scopes under which the problem may be measured.
    pub allowed_timing_scopes: Vec<TimingScope>,
    /// Scientific budgets supported by the contract.
    pub supported_budgets: Vec<ScientificBudget>,
    /// Validated family-specific semantics and operational rules.
    pub semantics: ProblemSemantics,
    /// Small conformance cases for the problem evaluator.
    pub reference_cases: Vec<ReferenceCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProblemContract {
    #[serde(deserialize_with = "deserialize_version_one")]
    version: u32,
    name: Name,
    family: Name,
    parameter_schema: RepositoryPath,
    dataset_schemas: Vec<RepositoryPath>,
    result_schema: RepositoryPath,
    metric_schema: RepositoryPath,
    semantics_schema: RepositoryPath,
    allowed_timing_scopes: Vec<TimingScope>,
    supported_budgets: Vec<ScientificBudget>,
    semantics: ProblemSemantics,
    reference_cases: Vec<ReferenceCase>,
}

struct ParsedProblemContract(ProblemContract);

impl<'de> Deserialize<'de> for ParsedProblemContract {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawProblemContract::deserialize(deserializer)?;
        ensure_unique(&raw.dataset_schemas, "`dataset_schemas`").map_err(de::Error::custom)?;
        ensure_nonempty_unique(&raw.allowed_timing_scopes, "`allowed_timing_scopes`")
            .map_err(de::Error::custom)?;
        if raw.supported_budgets != [ScientificBudget::None] {
            return Err(de::Error::custom(
                "version 1 requires `supported_budgets = [\"none\"]`",
            ));
        }
        if raw.reference_cases.is_empty() {
            return Err(de::Error::custom("`reference_cases` cannot be empty"));
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

        Ok(Self(ProblemContract {
            version: raw.version,
            name: raw.name,
            family: raw.family,
            parameter_schema: raw.parameter_schema,
            dataset_schemas: raw.dataset_schemas,
            result_schema: raw.result_schema,
            metric_schema: raw.metric_schema,
            semantics_schema: raw.semantics_schema,
            allowed_timing_scopes: raw.allowed_timing_scopes,
            supported_budgets: raw.supported_budgets,
            semantics: raw.semantics,
            reference_cases: raw.reference_cases,
        }))
    }
}

/// Family-owned problem semantics with required operational rule groups.
#[derive(Debug)]
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
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceCase {
    /// Dataset artifact used by the case, when the problem accepts datasets.
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
    /// The TOML document did not satisfy the typed common envelope.
    #[error("failed to parse problem contract at `{}`: {message}", source_span.path.display())]
    Parse {
        /// Decoder explanation.
        message: String,
        /// Source range attributed by the TOML decoder.
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
    /// Returns the TOML decoder's explanation for typed-envelope failures.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Parse { message, .. } => Some(message),
            Self::MissingSchema { .. } | Self::InvalidSemantics { .. } => None,
        }
    }

    /// Returns the source range for TOML and typed-envelope failures.
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
    schema_path: &RepositoryPath,
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
    T: Ord,
{
    if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        Err(format!("{description} cannot contain duplicates"))
    } else {
        Ok(())
    }
}
