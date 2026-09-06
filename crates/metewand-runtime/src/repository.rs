//! Read-only loading and validation of a benchmark repository.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

use metewand_core::{
    canonical::{CanonicalJsonError, CanonicalValue},
    identity::{IdentityError, identify_canonical, identify_record},
    manifest::{
        DatasetDefinition, EnvironmentDefinition, Manifest, ManifestParseError, Name,
        RepositoryPath, Sha256Digest, WorkerDefinition, parse_manifest,
    },
    manifest_hash::{ManifestHash, ManifestHashError, hash_manifest},
    planning::{
        ConfigurationExpansionError, DatasetConfigurationDefinition, LogicalPlan,
        LogicalPlanningCatalog, LogicalPlanningError, expand_manifest_configurations,
        expand_manifest_logical_plan, identify_builtin_unit_dataset_definition,
    },
    problem_contract::{
        ProblemContract, ProblemContractError, parse_problem_contract_document,
        validate_problem_contract,
    },
    records::{
        ContentDigest, DatasetDefinitionKind, DatasetDefinitionRecord, EnvironmentDefinitionKind,
        EnvironmentDefinitionRecord, ImplementationDefinitionRecord, ProblemContractResource,
        ProblemDefinitionRecord, RecordId, RemoteSourceRecord, SchemaResource,
        SourceBundleResource, WorkerDefinitionRecord, WorkerLaunch,
    },
    schema::{SchemaCatalog, SchemaCatalogError},
};
use serde_json::{Value, json};
use thiserror::Error;

use crate::source_bundle::{SourceBundleIdentityError, identify_source_bundle};

/// A benchmark repository that has passed every side-effect-free validation pass.
pub struct CheckedRepository {
    /// Canonical repository root containing the manifest.
    pub root: PathBuf,
    /// Canonical path of the loaded manifest.
    pub manifest_path: PathBuf,
    /// Parsed, strict version-1 manifest.
    pub manifest: Manifest,
    /// Whole-manifest hash after defaults and name resolution.
    pub manifest_hash: ManifestHash,
    /// Validated problem contracts keyed by manifest-local problem name.
    pub problem_contracts: BTreeMap<Name, ProblemContract>,
    /// Complete offline catalog of reachable repository schemas.
    pub schemas: SchemaCatalog,
    /// Content-derived definition identities consumed by logical planning.
    pub planning_catalog: LogicalPlanningCatalog,
    /// Content identities for repository resources required by the plan.
    pub resources: RepositoryResources,
    /// Number of repository schema documents in the offline catalog.
    pub schema_count: usize,
    /// Number of distinct declared source bundles inspected.
    pub source_bundle_count: usize,
}

/// Content identities for the repository resources admitted by a check.
pub struct RepositoryResources {
    /// Reachable schemas keyed by logical repository path.
    pub schemas: BTreeMap<PathBuf, RecordId<SchemaResource>>,
    /// Validated contracts keyed by manifest-local problem name.
    pub problem_contracts: BTreeMap<Name, RecordId<ProblemContractResource>>,
    /// Declared source bundles keyed by their normalized source path set.
    pub source_bundles: BTreeMap<Vec<PathBuf>, RecordId<SourceBundleResource>>,
}

impl CheckedRepository {
    /// Expands the checked repository into its complete unresolved logical plan.
    ///
    /// # Errors
    ///
    /// Returns a deterministic planning error if identity construction or
    /// expansion invariants fail.
    pub fn logical_plan(&self) -> Result<LogicalPlan, LogicalPlanningError> {
        expand_manifest_logical_plan(
            &self.manifest,
            &self.problem_contracts,
            &self.schemas,
            &self.planning_catalog,
        )
    }
}

/// Loads and validates a benchmark repository without modifying it.
///
/// The manifest's parent directory establishes the repository root. The loader
/// reads only declared contracts, reachable schemas, source bundles, and the
/// manifest itself. It performs no downloads, builds, environment resolution,
/// or worker launches.
///
/// # Errors
///
/// Returns a contextual filesystem, parsing, schema, compatibility, source
/// bundle, or identity error.
pub fn check_repository(manifest_path: &Path) -> Result<CheckedRepository, RepositoryCheckError> {
    let manifest_path =
        fs::canonicalize(manifest_path).map_err(|source| RepositoryCheckError::ManifestAccess {
            path: manifest_path.to_path_buf(),
            source,
        })?;
    if !manifest_path.is_file() {
        return Err(RepositoryCheckError::ManifestNotFile {
            path: manifest_path,
        });
    }
    let root = manifest_path
        .parent()
        .expect("a canonical file path must have a parent")
        .to_path_buf();
    let manifest_source = read_utf8_file(&manifest_path, &manifest_path)?;
    let manifest_diagnostic_path = manifest_path
        .file_name()
        .map_or_else(|| PathBuf::from("metewand.toml"), PathBuf::from);
    let manifest = parse_manifest(&manifest_diagnostic_path, &manifest_source)?;
    let manifest_hash = hash_manifest(&manifest)?;

    let mut problem_contracts = BTreeMap::new();
    for (name, definition) in &manifest.problems {
        let source = read_repository_utf8(&root, definition.contract.as_path())?;
        let contract = parse_problem_contract_document(definition.contract.as_path(), &source)?;
        problem_contracts.insert(name.clone(), contract);
    }

    let schema_documents =
        load_schema_documents(&root, schema_roots(&manifest, problem_contracts.values()))?;
    let schemas = SchemaCatalog::try_new(
        schema_documents
            .iter()
            .map(|(path, value)| (path.clone(), value.as_json().clone())),
    )?;
    for (name, contract) in &problem_contracts {
        validate_problem_contract(
            manifest.problems[name].contract.as_path(),
            contract,
            &schemas,
        )?;
    }

    expand_manifest_configurations(&manifest, &problem_contracts, &schemas)?;
    let (planning_catalog, resources) =
        build_planning_catalog(&root, &manifest, &problem_contracts, &schema_documents)?;
    let source_bundle_count = resources
        .source_bundles
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .len();

    Ok(CheckedRepository {
        root,
        manifest_path,
        manifest,
        manifest_hash,
        problem_contracts,
        schemas,
        planning_catalog,
        resources,
        schema_count: schema_documents.len(),
        source_bundle_count,
    })
}

fn schema_roots<'a>(
    manifest: &'a Manifest,
    contracts: impl IntoIterator<Item = &'a ProblemContract>,
) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    for dataset in manifest.datasets.values() {
        match dataset {
            DatasetDefinition::Fixed(dataset) => {
                roots.insert(dataset.output_schema.as_path().to_path_buf());
            }
            DatasetDefinition::Generated(dataset) => {
                roots.extend(
                    dataset
                        .parameter_schema
                        .iter()
                        .map(|path| path.as_path().to_path_buf()),
                );
                roots.insert(dataset.output_schema.as_path().to_path_buf());
            }
        }
    }
    for implementation in manifest.implementations.values() {
        roots.extend(
            implementation
                .parameter_schema
                .iter()
                .map(|path| path.as_path().to_path_buf()),
        );
    }
    for contract in contracts {
        roots.insert(contract.parameter_schema.as_path().to_path_buf());
        roots.extend(
            contract
                .dataset_schemas
                .iter()
                .map(|path| path.as_path().to_path_buf()),
        );
        roots.insert(contract.result_schema.as_path().to_path_buf());
        roots.insert(contract.metric_schema.as_path().to_path_buf());
        roots.insert(contract.semantics_schema.as_path().to_path_buf());
    }
    roots
}

fn load_schema_documents(
    root: &Path,
    mut pending: BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, CanonicalValue>, RepositoryCheckError> {
    let mut documents = BTreeMap::new();
    while let Some(path) = pending.pop_first() {
        if documents.contains_key(&path) {
            continue;
        }
        let bytes = read_repository_file(root, &path)?;
        let value = CanonicalValue::from_slice(&bytes).map_err(|source| {
            RepositoryCheckError::InvalidSchemaJson {
                path: path.clone(),
                source,
            }
        })?;
        for reference in external_schema_references(&path, value.as_json())? {
            if !documents.contains_key(&reference) {
                pending.insert(reference);
            }
        }
        documents.insert(path, value);
    }
    Ok(documents)
}

fn external_schema_references(
    document_path: &Path,
    document: &Value,
) -> Result<BTreeSet<PathBuf>, RepositoryCheckError> {
    fn visit(value: &Value, references: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    references.push(reference.to_owned());
                }
                for value in object.values() {
                    visit(value, references);
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, references);
                }
            }
            _ => {}
        }
    }

    let mut references = Vec::new();
    visit(document, &mut references);
    references
        .into_iter()
        .filter_map(|reference| {
            let path = reference.split('#').next().unwrap_or_default();
            (!path.is_empty()
                && !path.starts_with('/')
                && !path.starts_with("//")
                && !has_uri_scheme(path))
            .then(|| path.to_owned())
        })
        .map(|reference| resolve_schema_reference(document_path, &reference))
        .collect()
}

fn has_uri_scheme(value: &str) -> bool {
    value.find(':').is_some_and(|colon| {
        value[..colon].bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphabetic()
            } else {
                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')
            }
        })
    })
}

fn resolve_schema_reference(
    document_path: &Path,
    reference: &str,
) -> Result<PathBuf, RepositoryCheckError> {
    let decoded =
        percent_decode(reference).ok_or_else(|| RepositoryCheckError::InvalidSchemaReference {
            document: document_path.to_path_buf(),
            reference: reference.to_owned(),
        })?;
    if decoded.contains('\\') || decoded.contains('?') {
        return Err(RepositoryCheckError::InvalidSchemaReference {
            document: document_path.to_path_buf(),
            reference: reference.to_owned(),
        });
    }

    let mut components = document_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
        .filter_map(|component| match component {
            Component::Normal(component) => Some(component.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in Path::new(&decoded).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if components.pop().is_none() {
                    return Err(RepositoryCheckError::InvalidSchemaReference {
                        document: document_path.to_path_buf(),
                        reference: reference.to_owned(),
                    });
                }
            }
            Component::Normal(component) => components.push(component.to_owned()),
            Component::RootDir | Component::Prefix(_) => {
                return Err(RepositoryCheckError::InvalidSchemaReference {
                    document: document_path.to_path_buf(),
                    reference: reference.to_owned(),
                });
            }
        }
    }
    if components.is_empty() {
        return Err(RepositoryCheckError::InvalidSchemaReference {
            document: document_path.to_path_buf(),
            reference: reference.to_owned(),
        });
    }
    Ok(components.into_iter().collect())
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_nibble(*bytes.get(index + 1)?)?;
            let low = hex_nibble(*bytes.get(index + 2)?)?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn build_planning_catalog(
    root: &Path,
    manifest: &Manifest,
    contracts: &BTreeMap<Name, ProblemContract>,
    schema_documents: &BTreeMap<PathBuf, CanonicalValue>,
) -> Result<(LogicalPlanningCatalog, RepositoryResources), RepositoryCheckError> {
    let mut schema_ids = BTreeMap::new();
    for path in schema_documents.keys() {
        let value = schema_identity_value(path, schema_documents)?;
        schema_ids.insert(path.clone(), identify_canonical(&value)?);
    }
    let environment_definitions = manifest
        .environments
        .iter()
        .map(|(name, definition)| {
            let record = environment_record(name.clone(), definition);
            Ok((name.clone(), identify_record(record)?.id))
        })
        .collect::<Result<BTreeMap<_, _>, RepositoryCheckError>>()?;
    let mut source_bundles = BTreeMap::new();

    let mut dataset_definitions = BTreeMap::new();
    if manifest
        .experiments
        .iter()
        .any(|experiment| experiment.datasets.is_empty())
    {
        let unit = identify_builtin_unit_dataset_definition()?;
        dataset_definitions.insert(DatasetConfigurationDefinition::Unit, unit.id);
    }
    for (name, definition) in &manifest.datasets {
        let (parameter_schema, output_schema, parameter_defaults, kind) = match definition {
            DatasetDefinition::Fixed(definition) => (
                None,
                schema_id(&schema_ids, definition.output_schema.as_path())?,
                None,
                DatasetDefinitionKind::Fixed {
                    source_bundle: cached_source_bundle(
                        root,
                        &definition.sources,
                        &mut source_bundles,
                    )?,
                },
            ),
            DatasetDefinition::Generated(definition) => (
                definition
                    .parameter_schema
                    .as_ref()
                    .map(|path| schema_id(&schema_ids, path.as_path()))
                    .transpose()?,
                schema_id(&schema_ids, definition.output_schema.as_path())?,
                definition.parameter_defaults.clone(),
                DatasetDefinitionKind::Generated {
                    source: definition
                        .source
                        .as_ref()
                        .map(remote_source_record)
                        .transpose()?,
                    materializer: worker_record(
                        root,
                        &definition.worker,
                        &environment_definitions,
                        &mut source_bundles,
                    )?,
                },
            ),
        };
        let identified = identify_record(DatasetDefinitionRecord {
            name: name.clone(),
            parameter_schema,
            parameter_defaults,
            output_schema,
            kind,
        })?;
        dataset_definitions.insert(
            DatasetConfigurationDefinition::Named(name.clone()),
            identified.id,
        );
    }

    let contract_ids = contracts
        .iter()
        .map(|(name, contract)| {
            let value = contract_identity_value(root, contract, &schema_ids)?;
            Ok((name.clone(), identify_canonical(&value)?))
        })
        .collect::<Result<BTreeMap<Name, RecordId<ProblemContractResource>>, RepositoryCheckError>>(
        )?;
    let mut problem_definitions = BTreeMap::new();
    for (name, definition) in &manifest.problems {
        let evaluator = worker_record(
            root,
            &definition.evaluator,
            &environment_definitions,
            &mut source_bundles,
        )?;
        let identified = identify_record(ProblemDefinitionRecord {
            name: name.clone(),
            contract: contract_ids[name],
            parameter_defaults: definition.parameter_defaults.clone(),
            evaluator,
        })?;
        problem_definitions.insert(name.clone(), identified.id);
    }

    let mut implementation_definitions = BTreeMap::new();
    for (name, definition) in &manifest.implementations {
        let worker = worker_record(
            root,
            &definition.worker,
            &environment_definitions,
            &mut source_bundles,
        )?;
        let problem_contracts = definition
            .problem_contracts
            .iter()
            .map(|problem| problem_definitions[problem])
            .collect();
        let identified = identify_record(ImplementationDefinitionRecord {
            name: name.clone(),
            worker,
            problem_contracts,
            parameter_schema: definition
                .parameter_schema
                .as_ref()
                .map(|path| schema_id(&schema_ids, path.as_path()))
                .transpose()?,
            parameter_defaults: definition.parameter_defaults.clone(),
            capabilities: definition.capabilities.clone(),
        })?;
        implementation_definitions.insert(name.clone(), identified.id);
    }

    Ok((
        LogicalPlanningCatalog {
            dataset_definitions,
            problem_definitions,
            implementation_definitions,
            environment_definitions,
        },
        RepositoryResources {
            schemas: schema_ids,
            problem_contracts: contract_ids,
            source_bundles,
        },
    ))
}

fn schema_id(
    schema_ids: &BTreeMap<PathBuf, RecordId<SchemaResource>>,
    path: &Path,
) -> Result<RecordId<SchemaResource>, RepositoryCheckError> {
    schema_ids
        .get(path)
        .copied()
        .ok_or_else(|| RepositoryCheckError::MissingSchemaIdentity {
            path: path.to_path_buf(),
        })
}

fn environment_record(
    name: Name,
    definition: &EnvironmentDefinition,
) -> EnvironmentDefinitionRecord {
    let kind = match definition {
        EnvironmentDefinition::Local {} => EnvironmentDefinitionKind::Local,
        EnvironmentDefinition::Uv { project, lockfile } => EnvironmentDefinitionKind::Uv {
            project: project.clone(),
            lockfile: lockfile.clone(),
        },
        EnvironmentDefinition::Renv { project, lockfile } => EnvironmentDefinitionKind::Renv {
            project: project.clone(),
            lockfile: lockfile.clone(),
        },
        EnvironmentDefinition::Nix {
            flake,
            installable,
            system,
            output_kind,
            output,
        } => EnvironmentDefinitionKind::Nix {
            flake: flake.clone(),
            installable: installable.clone(),
            system: system.clone(),
            output_kind: *output_kind,
            output: output.clone(),
        },
        EnvironmentDefinition::Oci { image } => EnvironmentDefinitionKind::Oci {
            image: image.clone(),
        },
    };
    EnvironmentDefinitionRecord { name, kind }
}

fn worker_record(
    root: &Path,
    definition: &WorkerDefinition,
    environments: &BTreeMap<Name, RecordId<EnvironmentDefinitionRecord>>,
    source_bundles: &mut BTreeMap<Vec<PathBuf>, RecordId<SourceBundleResource>>,
) -> Result<WorkerDefinitionRecord, RepositoryCheckError> {
    let (launch, args, environment, protocol_transport) = match definition {
        WorkerDefinition::Interpreted(worker) => (
            WorkerLaunch::Interpreted {
                runner: worker.runner,
                entrypoint: worker.entrypoint.clone(),
                source_bundle: cached_source_bundle(root, &worker.sources, source_bundles)?,
            },
            worker.args.clone(),
            worker.environment.clone(),
            worker.protocol_transport,
        ),
        WorkerDefinition::Command(worker) => (
            WorkerLaunch::Command {
                program: worker.program.clone(),
                source_bundle: worker
                    .sources
                    .as_ref()
                    .map(|sources| cached_source_bundle(root, sources, source_bundles))
                    .transpose()?,
            },
            worker.args.clone(),
            worker.environment.clone(),
            worker.protocol_transport,
        ),
    };
    Ok(WorkerDefinitionRecord {
        launch,
        args,
        environment: environments[&environment],
        protocol_transport,
    })
}

fn cached_source_bundle(
    root: &Path,
    sources: &[RepositoryPath],
    cache: &mut BTreeMap<Vec<PathBuf>, RecordId<SourceBundleResource>>,
) -> Result<RecordId<SourceBundleResource>, RepositoryCheckError> {
    let mut key = sources
        .iter()
        .map(|path| path.as_path().to_path_buf())
        .collect::<Vec<_>>();
    key.sort();
    key.dedup();
    if let Some(id) = cache.get(&key) {
        return Ok(*id);
    }
    let id = identify_source_bundle(root, sources)?;
    cache.insert(key, id);
    Ok(id)
}

fn remote_source_record(
    source: &metewand_core::manifest::RemoteSource,
) -> Result<RemoteSourceRecord, RepositoryCheckError> {
    Ok(RemoteSourceRecord {
        url: source.url.clone(),
        sha256: ContentDigest::new(decode_sha256(&source.sha256)?),
    })
}

fn decode_sha256(digest: &Sha256Digest) -> Result<[u8; 32], RepositoryCheckError> {
    let mut output = [0_u8; 32];
    let (pairs, remainder) = digest.as_str().as_bytes().as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    for (byte, pair) in output.iter_mut().zip(pairs) {
        let high = hex_nibble(pair[0]).ok_or(RepositoryCheckError::InvalidTypedDigest)?;
        let low = hex_nibble(pair[1]).ok_or(RepositoryCheckError::InvalidTypedDigest)?;
        *byte = (high << 4) | low;
    }
    Ok(output)
}

fn schema_identity_value(
    root_path: &Path,
    documents: &BTreeMap<PathBuf, CanonicalValue>,
) -> Result<CanonicalValue, RepositoryCheckError> {
    let root =
        documents
            .get(root_path)
            .ok_or_else(|| RepositoryCheckError::MissingSchemaIdentity {
                path: root_path.to_path_buf(),
            })?;
    let mut pending = external_schema_references(root_path, root.as_json())?;
    let mut visited = BTreeSet::from([root_path.to_path_buf()]);
    let mut dependencies = Vec::new();
    while let Some(path) = pending.pop_first() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let document = documents
            .get(&path)
            .ok_or_else(|| RepositoryCheckError::MissingSchemaIdentity { path: path.clone() })?;
        pending.extend(external_schema_references(&path, document.as_json())?);
        dependencies.push(json!({
            "document": document.as_json(),
            "path": path.to_str().expect("schema paths are validated as UTF-8"),
        }));
    }
    CanonicalValue::try_from(json!({
        "dependencies": dependencies,
        "document": root.as_json(),
        "path": root_path.to_str().expect("schema paths are validated as UTF-8"),
    }))
    .map_err(RepositoryCheckError::Canonical)
}

fn contract_identity_value(
    root: &Path,
    contract: &ProblemContract,
    schemas: &BTreeMap<PathBuf, RecordId<SchemaResource>>,
) -> Result<CanonicalValue, RepositoryCheckError> {
    let dataset_schemas = contract
        .dataset_schemas
        .iter()
        .map(|path| schema_id(schemas, path.as_path()).map(|id| id.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let reference_cases = contract
        .reference_cases
        .iter()
        .map(|case| -> Result<Value, RepositoryCheckError> {
            Ok(json!({
                "dataset": case.dataset.as_ref().map(|path| reference_resource(root, path)).transpose()?,
                "expected_metrics": reference_resource(root, &case.expected_metrics)?,
                "result": reference_resource(root, &case.result)?,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    CanonicalValue::try_from(json!({
        "allowed_timing_scopes": contract.allowed_timing_scopes.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "dataset_schemas": dataset_schemas,
        "family": contract.family.as_str(),
        "metric_schema": schema_id(schemas, contract.metric_schema.as_path())?.to_string(),
        "name": contract.name.as_str(),
        "parameter_schema": schema_id(schemas, contract.parameter_schema.as_path())?.to_string(),
        "reference_cases": reference_cases,
        "result_schema": schema_id(schemas, contract.result_schema.as_path())?.to_string(),
        "semantics": contract.semantics.as_canonical().as_json(),
        "semantics_schema": schema_id(schemas, contract.semantics_schema.as_path())?.to_string(),
        "supported_budgets": contract.supported_budgets.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "version": contract.version,
    }))
    .map_err(RepositoryCheckError::Canonical)
}

fn reference_resource(root: &Path, path: &RepositoryPath) -> Result<Value, RepositoryCheckError> {
    Ok(json!({
        "content": identify_source_bundle(root, std::slice::from_ref(path))?.to_string(),
        "path": path.as_path().to_str().expect("repository paths are validated as UTF-8"),
    }))
}

fn read_repository_utf8(root: &Path, path: &Path) -> Result<String, RepositoryCheckError> {
    let absolute = contained_path(root, path)?;
    read_utf8_file(&absolute, path)
}

fn read_repository_file(root: &Path, path: &Path) -> Result<Vec<u8>, RepositoryCheckError> {
    let absolute = contained_path(root, path)?;
    fs::read(&absolute).map_err(|source| RepositoryCheckError::RepositoryFileAccess {
        path: path.to_path_buf(),
        source,
    })
}

fn read_utf8_file(absolute: &Path, diagnostic: &Path) -> Result<String, RepositoryCheckError> {
    let bytes =
        fs::read(absolute).map_err(|source| RepositoryCheckError::RepositoryFileAccess {
            path: diagnostic.to_path_buf(),
            source,
        })?;
    String::from_utf8(bytes).map_err(|source| RepositoryCheckError::NonUtf8File {
        path: diagnostic.to_path_buf(),
        source,
    })
}

fn contained_path(root: &Path, path: &Path) -> Result<PathBuf, RepositoryCheckError> {
    let absolute = fs::canonicalize(root.join(path)).map_err(|source| {
        RepositoryCheckError::RepositoryFileAccess {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if absolute.starts_with(root) {
        Ok(absolute)
    } else {
        Err(RepositoryCheckError::RepositoryPathEscape {
            path: path.to_path_buf(),
            target: absolute,
        })
    }
}

/// A failure produced by the read-only repository validation pipeline.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RepositoryCheckError {
    /// The manifest path could not be resolved.
    #[error("failed to access manifest `{}`: {source}", path.display())]
    ManifestAccess {
        /// Supplied manifest path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// The manifest path did not resolve to a regular file.
    #[error("manifest path `{}` is not a file", path.display())]
    ManifestNotFile {
        /// Resolved manifest path.
        path: PathBuf,
    },

    /// A declared repository file could not be read.
    #[error("failed to read repository file `{}`: {source}", path.display())]
    RepositoryFileAccess {
        /// Logical repository path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// A declared path resolved outside the repository root.
    #[error(
        "repository path `{}` resolves outside the repository to `{}`",
        path.display(),
        target.display()
    )]
    RepositoryPathEscape {
        /// Logical repository path.
        path: PathBuf,
        /// Resolved path outside the repository.
        target: PathBuf,
    },

    /// A text contract or manifest was not valid UTF-8.
    #[error("repository file `{}` is not valid UTF-8", path.display())]
    NonUtf8File {
        /// Logical or supplied path.
        path: PathBuf,
        /// UTF-8 decoding failure.
        #[source]
        source: std::string::FromUtf8Error,
    },

    /// A repository schema was not valid canonical-domain JSON.
    #[error("schema `{}` is not valid JSON: {source}", path.display())]
    InvalidSchemaJson {
        /// Logical schema path.
        path: PathBuf,
        /// JSON parsing failure.
        #[source]
        source: CanonicalJsonError,
    },

    /// A relative schema reference could not resolve to a repository path.
    #[error("schema `{}` contains invalid repository reference `{reference}`", document.display())]
    InvalidSchemaReference {
        /// Referencing logical schema path.
        document: PathBuf,
        /// Rejected reference.
        reference: String,
    },

    /// A schema needed for an identity was unexpectedly absent after loading.
    #[error("schema `{}` has no loaded content identity", path.display())]
    MissingSchemaIdentity {
        /// Logical schema path.
        path: PathBuf,
    },

    /// A digest accepted by the manifest type could not be decoded.
    #[error("a validated SHA-256 digest could not be decoded")]
    InvalidTypedDigest,

    /// The manifest did not satisfy the strict typed contract.
    #[error(transparent)]
    Manifest(#[from] ManifestParseError),

    /// The manifest contained unresolved names or could not be canonicalized.
    #[error(transparent)]
    ManifestHash(#[from] ManifestHashError),

    /// A problem contract was malformed or failed schema validation.
    #[error(transparent)]
    ProblemContract(#[from] ProblemContractError),

    /// The repository schema catalog was invalid or incomplete.
    #[error(transparent)]
    SchemaCatalog(#[from] SchemaCatalogError),

    /// A configuration or cross-definition invariant failed.
    #[error(transparent)]
    Configuration(Box<ConfigurationExpansionError>),

    /// A declared source bundle could not be identified.
    #[error(transparent)]
    SourceBundle(#[from] SourceBundleIdentityError),

    /// A canonical resource representation was invalid.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),

    /// A definition or resource identity could not be constructed.
    #[error(transparent)]
    Identity(#[from] IdentityError),
}

impl From<ConfigurationExpansionError> for RepositoryCheckError {
    fn from(error: ConfigurationExpansionError) -> Self {
        Self::Configuration(Box::new(error))
    }
}
