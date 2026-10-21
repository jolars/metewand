//! Incremental component authoring without scientific defaults.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
};

use metewand_core::{
    MANIFEST_VERSION,
    manifest::{Name, SourceSpan, parse_manifest},
    schema::DRAFT_2020_12_DIALECT,
};
use serde::Serialize;
use thiserror::Error;
use toml_edit::{Document, DocumentMut, Item, Key, Table};

use super::CreatedPathKind;

/// The single manifest component a scaffold command may add.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaffoldComponent {
    /// A generated dataset and its materializer.
    Dataset,
    /// A problem contract and its independent evaluator.
    Problem,
    /// An implementation of an explicitly selected problem.
    Implementation,
}

impl ScaffoldComponent {
    /// Parses the public component name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "dataset" => Some(Self::Dataset),
            "problem" => Some(Self::Problem),
            "implementation" => Some(Self::Implementation),
            _ => None,
        }
    }

    fn namespace(self) -> &'static str {
        match self {
            Self::Dataset => "datasets",
            Self::Problem => "problems",
            Self::Implementation => "implementations",
        }
    }
}

/// An explicit component addition, independent of command-line syntax.
#[derive(Debug)]
pub struct ScaffoldRequest {
    /// Kind of component to add.
    pub component: ScaffoldComponent,
    /// New manifest-local component name.
    pub name: String,
    /// Existing environment selected by the author for the worker stub.
    pub environment: String,
    /// Existing problem selected for an implementation, and only for it.
    pub problem: Option<String>,
}

/// Whether an authoring operation creates an entry or modifies the manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathAction {
    /// Create a previously absent filesystem entry.
    Create,
    /// Add the requested component to an existing manifest.
    Modify,
}

/// A filesystem change made or proposed by scaffolding.
#[derive(Clone, Debug, Serialize)]
pub struct ChangedPath {
    /// Path as addressed by the operation.
    pub path: PathBuf,
    /// Entry type at this path.
    pub kind: CreatedPathKind,
    /// How the operation changes this path.
    pub action: PathAction,
}

/// Ordered changes made or proposed for one component addition.
#[derive(Debug, Serialize)]
pub struct ScaffoldReport {
    /// Discriminator in the versioned machine-output envelope.
    pub command: &'static str,
    /// Component added to the manifest.
    pub component: ScaffoldComponent,
    /// Manifest-local name of the component.
    pub name: String,
    /// Whether changes were only proposed.
    pub dry_run: bool,
    /// Manifest edited by the operation.
    pub manifest: PathBuf,
    /// Every changed path in execution order, with the manifest last.
    pub paths: Vec<ChangedPath>,
}

/// Stable failure categories for scaffold operations.
#[derive(Clone, Copy, Debug)]
pub enum ScaffoldErrorKind {
    /// A request omitted or misidentified an explicit component choice.
    InvalidRequest,
    /// The requested manifest component is already defined.
    ComponentExists,
    /// The existing manifest could not be decoded.
    InvalidManifest,
    /// A generated path conflicts with an existing entry or symbolic link.
    PathExists,
    /// Another authoring operation holds the manifest lock.
    Busy,
    /// The manifest changed after its authoring plan was prepared.
    ManifestChanged,
    /// A filesystem operation failed.
    Io,
}

/// A path-aware failure with every change already made.
#[derive(Debug, Error)]
#[error("cannot scaffold component at `{path}`: {message}")]
pub struct ScaffoldError {
    /// Failure category used by CLI diagnostics.
    pub kind: ScaffoldErrorKind,
    /// Failing path.
    pub path: PathBuf,
    /// Explanation suitable for a diagnostic.
    pub message: String,
    /// Original filesystem error, when available.
    #[source]
    pub source: Option<io::Error>,
    /// Original manifest source location, when available.
    pub source_span: Option<Box<SourceSpan>>,
    /// Changes already made, including a possibly partially written file.
    pub changed_paths: Vec<ChangedPath>,
}

impl ScaffoldError {
    fn new(kind: ScaffoldErrorKind, path: &Path, message: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.to_path_buf(),
            message: message.into(),
            source: None,
            source_span: None,
            changed_paths: Vec::new(),
        }
    }

    fn io(path: &Path, source: io::Error) -> Self {
        let kind = if source.kind() == io::ErrorKind::AlreadyExists {
            ScaffoldErrorKind::PathExists
        } else {
            ScaffoldErrorKind::Io
        };
        let mut error = Self::new(kind, path, source.to_string());
        error.source = Some(source);
        error
    }
}

/// Adds only the requested component and its structural files.
///
/// All generated schemas reject every instance until the author replaces the
/// placeholder. Workers fail explicitly until implemented. Problem contracts
/// omit scientific choices and reference data, so they require author edits
/// before repository validation can succeed. No external programs are invoked.
///
/// The operation validates every target before writing and uses exclusive file
/// creation. A nonblocking manifest lock serializes cooperating authors without
/// extra repository files. New files precede the manifest edit so an earlier
/// failure cannot install references to absent files.
///
/// # Errors
///
/// Returns an error for invalid requests or manifests, duplicate components,
/// path conflicts, a busy or changed manifest, or filesystem failures. Paths
/// must be nonempty UTF-8. Symbolic links at the manifest or generated paths
/// are rejected. Partial changes are reported and retained for inspection.
pub fn scaffold_component(
    manifest_path: &Path,
    request: &ScaffoldRequest,
    dry_run: bool,
) -> Result<ScaffoldReport, ScaffoldError> {
    validate_request(manifest_path, request)?;
    let mut manifest_file = open_manifest(manifest_path, dry_run)?;
    let mut original = String::new();
    manifest_file
        .read_to_string(&mut original)
        .map_err(|error| ScaffoldError::io(manifest_path, error))?;
    let manifest = parse_manifest(manifest_path, &original).map_err(|error| {
        let mut diagnostic = ScaffoldError::new(
            ScaffoldErrorKind::InvalidManifest,
            manifest_path,
            error.to_string(),
        );
        diagnostic.source_span = error.source_span().cloned().map(Box::new);
        diagnostic
    })?;
    let exists = match request.component {
        ScaffoldComponent::Dataset => manifest.datasets.contains_key(request.name.as_str()),
        ScaffoldComponent::Problem => manifest.problems.contains_key(request.name.as_str()),
        ScaffoldComponent::Implementation => {
            manifest.implementations.contains_key(request.name.as_str())
        }
    };
    if exists {
        return Err(ScaffoldError::new(
            ScaffoldErrorKind::ComponentExists,
            manifest_path,
            format!(
                "{} already contains `{}`",
                request.component.namespace(),
                request.name
            ),
        ));
    }
    if !manifest
        .environments
        .contains_key(request.environment.as_str())
    {
        return Err(ScaffoldError::new(
            ScaffoldErrorKind::InvalidRequest,
            manifest_path,
            format!(
                "environment `{}` is not defined; add it explicitly before scaffolding",
                request.environment
            ),
        ));
    }
    if let Some(problem) = &request.problem
        && !manifest.problems.contains_key(problem.as_str())
    {
        return Err(ScaffoldError::new(
            ScaffoldErrorKind::InvalidRequest,
            manifest_path,
            format!("problem `{problem}` is not defined"),
        ));
    }
    let (definition, files) = templates(request);
    let updated = add_definition(manifest_path, &original, request, definition)?;
    let root = manifest_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new(""));
    let mut paths = Vec::new();
    let mut directories = BTreeSet::new();
    for (relative, _) in &files {
        let target = root.join(relative);
        plan_parents(
            root,
            relative.parent().expect("component file has a parent"),
            &mut directories,
            &mut paths,
        )?;
        require_absent(&target)?;
        paths.push(ChangedPath {
            path: target,
            kind: CreatedPathKind::File,
            action: PathAction::Create,
        });
    }
    paths.push(ChangedPath {
        path: manifest_path.to_path_buf(),
        kind: CreatedPathKind::File,
        action: PathAction::Modify,
    });
    if !dry_run {
        apply_changes(
            manifest_path,
            &mut manifest_file,
            &original,
            &updated,
            &paths,
            &files,
        )?;
    }
    Ok(ScaffoldReport {
        command: "scaffold",
        component: request.component,
        name: request.name.clone(),
        dry_run,
        manifest: manifest_path.to_path_buf(),
        paths,
    })
}

fn validate_request(path: &Path, request: &ScaffoldRequest) -> Result<(), ScaffoldError> {
    let invalid = |message| ScaffoldError::new(ScaffoldErrorKind::InvalidRequest, path, message);
    if path.as_os_str().is_empty() || path.to_str().is_none() {
        return Err(invalid("manifest path must be nonempty UTF-8".to_owned()));
    }
    for (label, value) in [
        ("component", &request.name),
        ("environment", &request.environment),
    ]
    .into_iter()
    .chain(request.problem.as_ref().map(|problem| ("problem", problem)))
    {
        serde_json::from_value::<Name>(serde_json::Value::String(value.clone()))
            .map_err(|error| invalid(format!("invalid {label} name: {error}")))?;
    }
    if (request.component == ScaffoldComponent::Implementation) != request.problem.is_some() {
        return Err(invalid(
            "only implementation scaffolds require and accept an explicit problem".to_owned(),
        ));
    }
    Ok(())
}

fn open_manifest(path: &Path, dry_run: bool) -> Result<File, ScaffoldError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| ScaffoldError::io(path, error))?;
    if !metadata.is_file() {
        return Err(ScaffoldError::new(
            ScaffoldErrorKind::PathExists,
            path,
            "manifest must be a regular file, not a symbolic link or directory",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(!dry_run);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|error| ScaffoldError::io(path, error))?;
    let lock = if dry_run {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    lock.map_err(|error| match error {
        TryLockError::WouldBlock => ScaffoldError::new(
            ScaffoldErrorKind::Busy,
            path,
            "another operation holds the manifest lock; retry after it finishes",
        ),
        TryLockError::Error(error) => ScaffoldError::io(path, error),
    })?;
    verify_manifest_file(path, &file)?;
    Ok(file)
}

fn verify_manifest_file(path: &Path, file: &File) -> Result<(), ScaffoldError> {
    let current = fs::symlink_metadata(path).map_err(|error| ScaffoldError::io(path, error))?;
    let opened = file
        .metadata()
        .map_err(|error| ScaffoldError::io(path, error))?;
    let mut same = current.is_file() && opened.is_file();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        same &= current.dev() == opened.dev() && current.ino() == opened.ino();
    }
    if !same {
        return Err(ScaffoldError::new(
            ScaffoldErrorKind::ManifestChanged,
            path,
            "manifest path changed while preparing the component addition",
        ));
    }
    Ok(())
}

fn add_definition(
    path: &Path,
    source: &str,
    request: &ScaffoldRequest,
    definition: Table,
) -> Result<String, ScaffoldError> {
    let document = Document::parse(source).map_err(|error| {
        ScaffoldError::new(ScaffoldErrorKind::InvalidManifest, path, error.to_string())
    })?;
    let namespace = request.component.namespace();
    let mut updated = source.to_owned();
    if let Some(inline) = document.get(namespace).and_then(Item::as_inline_table) {
        let insertion = inline.span().expect("parsed inline table has a span").start + 1;
        let separator = if inline.is_empty() { " " } else { ", " };
        let value = definition.into_inline_table();
        updated.insert_str(
            insertion,
            &format!(" {} = {value}{separator}", Key::new(&request.name)),
        );
    } else {
        // Append an absolute table header without reformatting existing definitions.
        let mut addition = DocumentMut::new();
        let mut table = Table::new();
        table.set_implicit(true);
        table.insert(&request.name, Item::Table(definition));
        addition[namespace] = Item::Table(table);
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push('\n');
        updated.push_str(&addition.to_string());
    }
    parse_manifest(path, &updated).map_err(|error| {
        ScaffoldError::new(ScaffoldErrorKind::InvalidManifest, path, error.to_string())
    })?;
    Ok(updated)
}

type TemplateFiles = Vec<(PathBuf, String)>;

fn templates(request: &ScaffoldRequest) -> (Table, TemplateFiles) {
    let directory = format!("{}/{}", request.component.namespace(), request.name);
    let quote = |text: &str| toml_edit::Value::from(text).to_string();
    let schema_path = |role: &str| format!("{directory}/{role}.schema.json");
    let worker_name = match request.component {
        ScaffoldComponent::Dataset => "materializer.sh",
        ScaffoldComponent::Problem => "evaluator.sh",
        ScaffoldComponent::Implementation => "worker.sh",
    };
    let worker = format!("{directory}/{worker_name}");
    let launch = format!(
        "runner = \"command\"\nprogram = \"sh\"\nargs = [{}]\nsources = [{}]\nenvironment = {}\n",
        quote(&format!("{{sources}}/{worker}")),
        quote(&worker),
        quote(&request.environment),
    );
    let mut files = Vec::new();
    let definition = match request.component {
        ScaffoldComponent::Dataset => format!(
            "parameter_schema = {}\noutput_schema = {}\n{launch}",
            quote(&schema_path("parameters")),
            quote(&schema_path("dataset"))
        ),
        ScaffoldComponent::Problem => {
            files.push((PathBuf::from(format!("{directory}/contract.toml")), format!(
                "# TODO: Define the scientific contract before checking or running this problem.\nversion = {MANIFEST_VERSION}\nname = {}\nparameter_schema = {}\nresult_schema = {}\nmetric_schema = {}\nsemantics_schema = {}\nsupported_budgets = [\"none\"]\n\n# TODO: Add a family name, dataset_schemas (or [] for a dataset-free problem),\n# and allowed_timing_scopes chosen for your measurement contract.\n\n[semantics.preprocessing]\n# TODO: Define preprocessing and any work allowed outside the timed scope.\n\n[semantics.validity]\n# TODO: Define what makes a result scientifically valid.\n\n[semantics.one_shot_completion]\n# TODO: Define when a valid result completes the computation.\n\n# TODO: Add reference cases with author-supplied results and expected metrics.\n# [[reference_cases]]\n# Include a dataset path only when dataset_schemas is nonempty.\n# Required fields: result, expected_metrics.\n",
                quote(&request.name), quote(&schema_path("parameters")), quote(&schema_path("result")), quote(&schema_path("metrics")), quote(&schema_path("semantics")),
            )));
            format!(
                "contract = {}\n\n[evaluator]\n{launch}",
                quote(&format!("{directory}/contract.toml"))
            )
        }
        ScaffoldComponent::Implementation => format!(
            "parameter_schema = {}\nproblem_contracts = [{}]\ncapabilities = [\"one_shot\"]\n{launch}",
            quote(&schema_path("parameters")),
            quote(
                request
                    .problem
                    .as_deref()
                    .expect("implementation problem was validated")
            )
        ),
    };
    let roles: &[&str] = match request.component {
        ScaffoldComponent::Dataset => &["parameters", "dataset"],
        ScaffoldComponent::Problem => &["parameters", "result", "metrics", "semantics"],
        ScaffoldComponent::Implementation => &["parameters"],
    };
    for role in roles {
        files.push((PathBuf::from(schema_path(role)), format!(
            "{{\n  \"$schema\": \"{DRAFT_2020_12_DIALECT}\",\n  \"$comment\": \"TODO: Define this {role} schema, then remove the rejecting not clause. No scientific constraints have been supplied.\",\n  \"not\": {{}}\n}}\n"
        )));
    }
    files.push((PathBuf::from(worker), format!(
        "#!/bin/sh\n# Fail explicitly until the author supplies a version-1 protocol worker.\nprintf '%s\\n' 'TODO: Implement {worker_name} using the Metewand worker protocol or an SDK.' >&2\nexit 1\n"
    )));
    let table = definition
        .parse::<DocumentMut>()
        .expect("scaffold templates contain valid TOML")
        .as_table()
        .clone();
    (table, files)
}

fn require_absent(path: &Path) -> Result<(), ScaffoldError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(ScaffoldError::new(
            ScaffoldErrorKind::PathExists,
            path,
            "target path already exists",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ScaffoldError::io(path, error)),
    }
}

fn plan_parents(
    root: &Path,
    relative: &Path,
    planned: &mut BTreeSet<PathBuf>,
    paths: &mut Vec<ChangedPath>,
) -> Result<(), ScaffoldError> {
    let mut directory = root.to_path_buf();
    for component in relative.components() {
        directory.push(component);
        if planned.contains(&directory) {
            continue;
        }
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() => continue,
            Ok(_) => {
                return Err(ScaffoldError::new(
                    ScaffoldErrorKind::PathExists,
                    &directory,
                    "required directory is a file or symbolic link",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ScaffoldError::io(&directory, error)),
        }
        planned.insert(directory.clone());
        paths.push(ChangedPath {
            path: directory.clone(),
            kind: CreatedPathKind::Directory,
            action: PathAction::Create,
        });
    }
    Ok(())
}

fn apply_changes(
    manifest_path: &Path,
    manifest_file: &mut File,
    original: &str,
    updated: &str,
    paths: &[ChangedPath],
    files: &TemplateFiles,
) -> Result<(), ScaffoldError> {
    let mut changed_paths = Vec::new();
    let mut contents = files.iter();
    for entry in paths {
        let io_error = |error| ScaffoldError::io(&entry.path, error);
        let result = (|| {
            match (entry.action, entry.kind) {
                (PathAction::Create, CreatedPathKind::Directory) => {
                    fs::create_dir(&entry.path).map_err(io_error)?;
                    changed_paths.push(entry.clone());
                }
                (PathAction::Create, CreatedPathKind::File) => {
                    let (_, content) = contents.next().expect("every planned file has a template");
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&entry.path)
                        .map_err(io_error)?;
                    // Record the path before writing so failed writes remain reviewable.
                    changed_paths.push(entry.clone());
                    file.write_all(content.as_bytes()).map_err(io_error)?;
                }
                (PathAction::Modify, CreatedPathKind::File) => {
                    verify_manifest_file(manifest_path, manifest_file)?;
                    manifest_file.rewind().map_err(io_error)?;
                    let mut current = String::new();
                    manifest_file
                        .read_to_string(&mut current)
                        .map_err(io_error)?;
                    if current != original {
                        return Err(ScaffoldError::new(
                            ScaffoldErrorKind::ManifestChanged,
                            manifest_path,
                            "manifest contents changed before the component could be added",
                        ));
                    }
                    manifest_file.rewind().map_err(io_error)?;
                    changed_paths.push(entry.clone());
                    manifest_file
                        .write_all(updated.as_bytes())
                        .map_err(io_error)?;
                    manifest_file
                        .set_len(updated.len() as u64)
                        .map_err(io_error)?;
                }
                (PathAction::Modify, CreatedPathKind::Directory) => {
                    unreachable!("only the manifest may be modified")
                }
            }
            Ok::<_, ScaffoldError>(())
        })();
        if let Err(mut error) = result {
            error.changed_paths = changed_paths;
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &Path, kind: CreatedPathKind, action: PathAction) -> ChangedPath {
        ChangedPath {
            path: path.to_path_buf(),
            kind,
            action,
        }
    }

    #[test]
    fn late_file_conflicts_preserve_the_manifest_and_report_prior_changes() {
        let root = tempfile::TempDir::new().unwrap();
        let manifest = root.path().join("metewand.toml");
        let original = "version = 1\nname = 'benchmark'\n";
        fs::write(&manifest, original).unwrap();
        let mut file = open_manifest(&manifest, false).unwrap();
        let directory = root.path().join("created");
        let conflict = root.path().join("conflict.sh");
        fs::write(&conflict, "preserve\n").unwrap();
        let paths = vec![
            change(&directory, CreatedPathKind::Directory, PathAction::Create),
            change(&conflict, CreatedPathKind::File, PathAction::Create),
            change(&manifest, CreatedPathKind::File, PathAction::Modify),
        ];
        let templates = vec![(conflict.clone(), "replace\n".to_owned())];
        let error = apply_changes(
            &manifest,
            &mut file,
            original,
            "replace manifest\n",
            &paths,
            &templates,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ScaffoldErrorKind::PathExists));
        assert_eq!(error.path, conflict);
        assert_eq!(error.changed_paths.len(), 1);
        assert_eq!(error.changed_paths[0].path, directory);
        assert!(directory.is_dir());
        assert_eq!(fs::read_to_string(conflict).unwrap(), "preserve\n");
        assert_eq!(fs::read_to_string(manifest).unwrap(), original);
    }

    #[test]
    fn intervening_manifest_edits_are_preserved_and_created_files_are_reported() {
        for replace in [false, true] {
            let root = tempfile::TempDir::new().unwrap();
            let manifest = root.path().join("metewand.toml");
            let original = "version = 1\nname = 'benchmark'\n";
            fs::write(&manifest, original).unwrap();
            let mut file = open_manifest(&manifest, false).unwrap();
            let current = if replace {
                fs::remove_file(&manifest).unwrap();
                original
            } else {
                "version = 1\nname = 'author edit'\n"
            };
            fs::write(&manifest, current).unwrap();
            let created = root.path().join("worker.sh");
            let paths = vec![
                change(&created, CreatedPathKind::File, PathAction::Create),
                change(&manifest, CreatedPathKind::File, PathAction::Modify),
            ];
            let templates = vec![(created.clone(), "stub\n".to_owned())];
            let error = apply_changes(
                &manifest,
                &mut file,
                original,
                "replace manifest\n",
                &paths,
                &templates,
            )
            .unwrap_err();
            assert!(matches!(error.kind, ScaffoldErrorKind::ManifestChanged));
            assert_eq!(error.path, manifest);
            assert_eq!(error.changed_paths.len(), 1);
            assert_eq!(error.changed_paths[0].path, created);
            assert_eq!(fs::read_to_string(manifest).unwrap(), current);
            assert_eq!(fs::read_to_string(created).unwrap(), "stub\n");
        }
    }
}
