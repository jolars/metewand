//! Nix preparation is explicit; locked execution only inspects prepared paths.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use metewand_core::{
    canonical::CanonicalValue,
    lockfile::{
        LOCKFILE_VERSION, LockedEnvironment, LockedSource, LockedWorker, Lockfile, SourceEntry,
        StorePath,
    },
    manifest::{DatasetDefinition, EnvironmentDefinition, Name, NixOutputKind, WorkerDefinition},
    planning::LogicalPlan,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::repository::{CheckedRepository, worker_definition_id};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct LockError {
    pub code: &'static str,
    message: String,
}

fn error(code: &'static str, message: impl ToString) -> LockError {
    LockError {
        code,
        message: message.to_string(),
    }
}

impl From<std::io::Error> for LockError {
    fn from(value: std::io::Error) -> Self {
        error("lockfile_io", value)
    }
}

#[derive(Debug, Serialize)]
pub struct LockReport {
    pub command: &'static str,
    pub operation: &'static str,
    pub lockfile: String,
    pub sha256: String,
    pub environments: usize,
    pub workers: usize,
}

trait NixBackend {
    fn invoke(&self, program: &str, arguments: &[OsString]) -> Result<Vec<u8>, LockError>;
}

struct Nix;

impl NixBackend for Nix {
    fn invoke(&self, program: &str, arguments: &[OsString]) -> Result<Vec<u8>, LockError> {
        let mut command = Command::new(program);
        if program == "nix" {
            command.args([
                "--extra-experimental-features",
                "nix-command flakes",
                "--option",
                "accept-flake-config",
                "false",
                "--option",
                "use-registries",
                "false",
            ]);
        }
        let output = command
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| error("nix_unavailable", format!("cannot invoke {program}: {e}")))?;
        if !output.status.success() {
            return Err(error(
                "nix_operation_failed",
                format!(
                    "{program} failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }
        Ok(output.stdout)
    }
}

fn invoke(nix: &dyn NixBackend, args: &[&str]) -> Result<Vec<u8>, LockError> {
    nix.invoke("nix", &args.iter().map(OsString::from).collect::<Vec<_>>())
}

fn text(bytes: Vec<u8>) -> Result<String, LockError> {
    String::from_utf8(bytes)
        .map(|s| s.trim().to_owned())
        .map_err(|e| error("invalid_nix_output", e))
}

fn json_output(nix: &dyn NixBackend, args: &[&str]) -> Result<Value, LockError> {
    serde_json::from_slice(&invoke(nix, args)?).map_err(|e| error("invalid_nix_output", e))
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn environment_name(worker: &WorkerDefinition) -> &Name {
    match worker {
        WorkerDefinition::Command(w) => &w.environment,
        WorkerDefinition::Interpreted(w) => &w.environment,
    }
}

fn workers<'a>(
    repository: &'a CheckedRepository,
    plan: &LogicalPlan,
) -> Result<BTreeMap<String, &'a WorkerDefinition>, LockError> {
    let mut workers = BTreeMap::new();
    let catalog = &repository.planning_catalog;
    for experiment in &plan.experiments {
        for candidate in &experiment.candidates {
            let implementation = catalog
                .implementation_definitions
                .iter()
                .find(|(_, id)| **id == candidate.implementation_configuration.record.definition)
                .expect("checked implementation")
                .0;
            let problem = catalog
                .problem_definitions
                .iter()
                .find(|(_, id)| **id == candidate.problem_configuration.record.definition)
                .expect("checked problem")
                .0;
            let mut selected = vec![
                &repository.manifest.implementations[implementation].worker,
                &repository.manifest.problems[problem].evaluator,
            ];
            if let Some((metewand_core::planning::DatasetConfigurationDefinition::Named(name), _)) =
                catalog
                    .dataset_definitions
                    .iter()
                    .find(|(_, id)| **id == candidate.dataset_configuration.record.definition)
                && let DatasetDefinition::Generated(dataset) = &repository.manifest.datasets[name]
            {
                selected.push(&dataset.worker);
            }
            for worker in selected {
                let patterns = match worker {
                    WorkerDefinition::Command(worker) => {
                        worker.sources.as_deref().unwrap_or_default()
                    }
                    WorkerDefinition::Interpreted(worker) => &worker.sources,
                };
                for pattern in patterns {
                    let glob = globset::GlobBuilder::new(
                        pattern.as_path().to_str().expect("checked source path"),
                    )
                    .literal_separator(true)
                    .backslash_escape(false)
                    .build()
                    .map_err(|e| error("invalid_lock_source", e))?;
                    if glob.compile_matcher().is_match("metewand.lock") {
                        return Err(error(
                            "invalid_lock_source",
                            "worker source bundles must exclude metewand.lock to avoid self-reference",
                        ));
                    }
                }
                let id = worker_definition_id(repository, worker)
                    .map_err(|e| error("lockfile_outdated", e))?;
                workers.insert(id, worker);
            }
        }
    }
    if workers.is_empty() {
        return Err(error(
            "unsupported_locked_environment",
            "locking requires at least one planned worker",
        ));
    }
    for worker in workers.values() {
        if !matches!(
            repository.manifest.environments[environment_name(worker)],
            EnvironmentDefinition::Nix { .. }
        ) {
            return Err(error(
                "unsupported_locked_environment",
                format!(
                    "worker environment `{}` must use kind: nix for locked execution",
                    environment_name(worker)
                ),
            ));
        }
    }
    Ok(workers)
}

fn source_files(root: &Path) -> Result<LockedSource, LockError> {
    let tracked = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--cached", "-z", "--", "."])
        .output();
    let git = tracked.as_ref().is_ok_and(|o| o.status.success());
    let mut paths = BTreeSet::new();
    if git {
        for file in tracked
            .expect("successful git command")
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
        {
            let file = std::str::from_utf8(file).map_err(|e| error("invalid_lock_source", e))?;
            if file == "metewand.lock" {
                continue;
            }
            if fs::symlink_metadata(root.join(file))?.is_dir() {
                return Err(error(
                    "invalid_lock_source",
                    "Git submodule sources are not supported by Nix snapshots",
                ));
            }
            paths.insert(PathBuf::from(file));
            let mut parent = Path::new(file).parent();
            while let Some(path) = parent.filter(|p| !p.as_os_str().is_empty()) {
                paths.insert(path.to_owned());
                parent = path.parent();
            }
        }
    } else {
        fn visit(
            root: &Path,
            directory: &Path,
            paths: &mut BTreeSet<PathBuf>,
        ) -> Result<(), LockError> {
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                let relative = path.strip_prefix(root).expect("source descendant");
                if relative == Path::new("metewand.lock")
                    || path.file_name().is_some_and(|n| n == ".git")
                {
                    continue;
                }
                paths.insert(relative.to_owned());
                if fs::symlink_metadata(&path)?.is_dir() {
                    visit(root, &path, paths)?;
                }
            }
            Ok(())
        }
        visit(root, root, &mut paths)?;
    }
    let mut files = BTreeMap::new();
    for relative in paths {
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(error(
                "invalid_lock_source",
                format!(
                    "locked sources require regular files and directories: {}",
                    relative.display()
                ),
            ));
        }
        let name = crate::platform_fs::relative_path(&relative)?;
        let executable = is_executable(&metadata);
        files.insert(
            name,
            SourceEntry {
                sha256: metadata
                    .is_file()
                    .then(|| fs::read(&path).map(|b| sha256(&b)))
                    .transpose()?,
                executable: metadata.is_file() && executable,
            },
        );
    }
    Ok(LockedSource { git, files })
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn snapshot(root: &Path, source: &LockedSource) -> Result<TempDir, LockError> {
    let directory = TempDir::new()?;
    for (relative, entry) in &source.files {
        let path = directory.path().join(relative);
        if entry.sha256.is_some() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(root.join(relative), &path)?;
            // Nix must see the same executable marker for equivalent source evidence.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    &path,
                    fs::Permissions::from_mode(if entry.executable { 0o755 } else { 0o644 }),
                )?;
            }
        } else {
            fs::create_dir_all(&path)?;
        }
    }
    if source_files(root)? != *source {
        return Err(error(
            "lockfile_outdated",
            "sources changed while preparing the lockfile",
        ));
    }
    if source_files(directory.path())?.files != source.files {
        return Err(error(
            "lockfile_outdated",
            "staged source contents differ from their recorded hashes",
        ));
    }
    Ok(directory)
}

fn source_digest(source: &LockedSource) -> Result<String, LockError> {
    Ok(sha256(
        &CanonicalValue::try_from(json!(source.files))
            .and_then(|v| v.to_canonical_bytes())
            .map_err(|e| error("invalid_lock_source", e))?,
    ))
}

fn store_path(path: &str) -> Result<&str, LockError> {
    let suffix = path
        .strip_prefix("/nix/store/")
        .ok_or_else(|| error("invalid_nix_output", "Nix paths must belong to /nix/store"))?;
    let name = suffix.split('/').next().unwrap_or("");
    if name.is_empty() || name == "." || name == ".." {
        return Err(error("invalid_nix_output", "invalid store path"));
    }
    Ok(&path[.."/nix/store/".len() + name.len()])
}

fn path_info(
    nix: &dyn NixBackend,
    roots: &[String],
) -> Result<BTreeMap<String, StorePath>, LockError> {
    let mut args = vec!["path-info", "--offline", "--recursive", "--json"];
    args.extend(roots.iter().map(String::as_str));
    let value = json_output(nix, &args)?;
    let mut entries = Vec::new();
    match &value {
        Value::Object(paths) => {
            for (path, entry) in paths {
                entries.push((path.as_str(), entry));
            }
        }
        Value::Array(paths) => {
            for entry in paths {
                entries.push((
                    entry["path"].as_str().ok_or_else(|| {
                        error("invalid_nix_output", "path-info entry has no path")
                    })?,
                    entry,
                ));
            }
        }
        _ => {
            return Err(error(
                "invalid_nix_output",
                "expected path-info JSON objects",
            ));
        }
    }
    let mut paths = BTreeMap::new();
    for (path, entry) in entries {
        if store_path(path)? != path {
            return Err(error("invalid_nix_output", "expected a store root"));
        }
        let hash = entry["narHash"].as_str().ok_or_else(|| {
            error(
                "locked_path_unavailable",
                format!("missing NAR hash for {path}"),
            )
        })?;
        let mut references: Vec<String> = serde_json::from_value(entry["references"].clone())
            .map_err(|e| error("invalid_nix_output", e))?;
        references.sort();
        references.dedup();
        paths.insert(
            path.to_owned(),
            StorePath {
                nar_hash: hash.to_owned(),
                references,
            },
        );
    }
    for root in roots {
        if !paths.contains_key(root) {
            return Err(error(
                "locked_path_unavailable",
                format!("missing Nix path {root}"),
            ));
        }
    }
    Ok(paths)
}

fn verify_paths(
    nix: &dyn NixBackend,
    expected: &BTreeMap<String, StorePath>,
) -> Result<(), LockError> {
    if expected.is_empty() {
        return Err(error(
            "invalid_lockfile",
            "locked Nix path inventory is empty",
        ));
    }
    let roots = expected.keys().cloned().collect::<Vec<_>>();
    if path_info(nix, &roots)? != *expected {
        return Err(error(
            "locked_closure_mismatch",
            "Nix closure hashes or references differ from metewand.lock",
        ));
    }
    let mut args = vec!["store", "verify", "--offline", "--no-trust"];
    args.extend(roots.iter().map(String::as_str));
    invoke(nix, &args).map_err(|e| error("locked_closure_mismatch", e))?;
    Ok(())
}

fn pin(nix: &dyn NixBackend, paths: &BTreeSet<String>, directory: &Path) -> Result<(), LockError> {
    for (index, path) in paths.iter().enumerate() {
        // Store paths cannot build derivations, and substitution is disabled.
        nix.invoke(
            "nix-store",
            &[
                OsString::from("--realise"),
                path.into(),
                "--add-root".into(),
                directory.join(index.to_string()).into_os_string(),
                "--indirect".into(),
                "--option".into(),
                "substitute".into(),
                "false".into(),
                "--option".into(),
                "builders".into(),
                "".into(),
                "--option".into(),
                "max-jobs".into(),
                "0".into(),
            ],
        )?;
    }
    Ok(())
}

fn check_native_inputs(value: &Value) -> Result<(), LockError> {
    if let Some(nodes) = value.get("nodes").and_then(Value::as_object) {
        for node in nodes.values() {
            if let Some(locked) = node.get("locked") {
                if locked["type"] == "path" {
                    let path = locked["path"].as_str().unwrap_or("");
                    if Path::new(path).is_absolute() {
                        return Err(error(
                            "invalid_lock_source",
                            "absolute local flake inputs are not supported; use repository-relative path inputs",
                        ));
                    }
                } else if locked.get("narHash").and_then(Value::as_str).is_none() {
                    return Err(error(
                        "invalid_lock_source",
                        "flake inputs must have locked content hashes",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn prepare_environment(
    repository: &CheckedRepository,
    name: &Name,
    snapshot_path: &str,
    source_sha256: &str,
    roots: &Path,
    nix: &dyn NixBackend,
) -> Result<LockedEnvironment, LockError> {
    let EnvironmentDefinition::Nix {
        flake,
        installable,
        system,
        output_kind,
        output,
    } = &repository.manifest.environments[name]
    else {
        return Err(error(
            "unsupported_locked_environment",
            "expected Nix environment",
        ));
    };
    let native_path = repository.root.join(flake.as_path()).join("flake.lock");
    let native_bytes = match fs::read(&native_path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(bytes) = &native_bytes {
        check_native_inputs(
            &serde_json::from_slice::<Value>(bytes).map_err(|e| error("invalid_lock_source", e))?,
        )?;
    }
    let (reference, attribute) = installable.split_once('#').ok_or_else(|| {
        error(
            "invalid_nix_installable",
            "installable must name a local flake and an explicit #attribute",
        )
    })?;
    let selected = if reference.is_empty() {
        repository.root.join(flake.as_path())
    } else {
        repository.root.join(reference)
    };
    if fs::canonicalize(selected)? != fs::canonicalize(repository.root.join(flake.as_path()))? {
        return Err(error(
            "invalid_nix_installable",
            "installable must refer to its declared local flake",
        ));
    }
    let flake_url = if flake.as_path() == Path::new(".") {
        format!("path:{snapshot_path}")
    } else {
        let directory: String = flake
            .as_path()
            .to_string_lossy()
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        format!("path:{snapshot_path}?dir={}", directory)
    };
    let archive = json_output(
        nix,
        &[
            "flake",
            "archive",
            "--json",
            "--no-update-lock-file",
            "--no-write-lock-file",
            &flake_url,
        ],
    )?;
    fn collect(value: &Value, paths: &mut Vec<String>) {
        if let Some(path) = value["path"].as_str() {
            paths.push(path.to_owned());
        }
        if let Some(inputs) = value["inputs"].as_object() {
            for input in inputs.values() {
                collect(input, paths);
            }
        }
    }
    let mut source_paths = vec![snapshot_path.to_owned()];
    collect(&archive, &mut source_paths);
    source_paths.sort();
    source_paths.dedup();
    let source_roots = roots.join("sources");
    fs::create_dir_all(&source_roots)?;
    pin(nix, &source_paths.iter().cloned().collect(), &source_roots)?;
    let mut result = LockedEnvironment {
        definition: repository.planning_catalog.environment_definitions[name].to_string(),
        system: system.clone(),
        source_sha256: source_sha256.to_owned(),
        native_lock_sha256: native_bytes.as_ref().map(|b| sha256(b)),
        outputs: vec![],
        app_program: None,
        sources: path_info(nix, &source_paths)?,
        closure: BTreeMap::new(),
    };
    let attr = match output_kind {
        NixOutputKind::Package => {
            if attribute.starts_with("packages.") || attribute.starts_with("legacyPackages.") {
                attribute.to_owned()
            } else {
                format!("packages.{system}.{attribute}")
            }
        }
        NixOutputKind::App => {
            let expected = format!("apps.{system}.{output}");
            if attribute != output && attribute != expected {
                return Err(error(
                    "invalid_nix_installable",
                    "app installable must agree with output and system",
                ));
            }
            expected
        }
    };
    if !attr.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }) {
        return Err(error(
            "invalid_nix_installable",
            "Nix attributes must use dot-separated names",
        ));
    }
    if matches!(output_kind, NixOutputKind::Package)
        && !attr.starts_with(&format!("packages.{system}."))
        && !attr.starts_with(&format!("legacyPackages.{system}."))
    {
        return Err(error(
            "invalid_nix_installable",
            "package attribute must match the declared system",
        ));
    }
    let installable = format!("{flake_url}#{attr}");
    match output_kind {
        NixOutputKind::Package => {
            let target = format!("{installable}^{output}");
            let built = json_output(
                nix,
                &[
                    "build",
                    "--json",
                    "--out-link",
                    roots
                        .join("package")
                        .to_str()
                        .ok_or_else(|| error("lockfile_io", "GC root must be UTF-8"))?,
                    "--no-update-lock-file",
                    "--no-write-lock-file",
                    &target,
                ],
            )?;
            for build in built
                .as_array()
                .ok_or_else(|| error("invalid_nix_output", "expected build array"))?
            {
                let path = build["outputs"][output].as_str().ok_or_else(|| {
                    error("invalid_nix_output", "selected package output is missing")
                })?;
                result.outputs.push(path.to_owned());
            }
        }
        NixOutputKind::App => {
            let app = json_output(
                nix,
                &[
                    "eval",
                    "--json",
                    "--no-update-lock-file",
                    "--no-write-lock-file",
                    &installable,
                    "--apply",
                    "app: if app.type != \"app\" then throw \"expected app\" else { program = app.program; context = builtins.getContext app.program; }",
                ],
            )?;
            let program = app["program"]
                .as_str()
                .ok_or_else(|| error("invalid_nix_output", "app has no program"))?;
            for (index, (path, context)) in app["context"]
                .as_object()
                .ok_or_else(|| error("invalid_nix_output", "app has no context"))?
                .iter()
                .enumerate()
            {
                let target = if let Some(outputs) = context["outputs"].as_array() {
                    format!(
                        "{path}^{}",
                        outputs
                            .iter()
                            .map(|v| v.as_str().unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                } else if context["allOutputs"] == true {
                    format!("{path}^*")
                } else {
                    path.to_owned()
                };
                invoke(
                    nix,
                    &[
                        "build",
                        "--out-link",
                        roots
                            .join(format!("app-{index}"))
                            .to_str()
                            .ok_or_else(|| error("lockfile_io", "GC root must be UTF-8"))?,
                        &target,
                    ],
                )?;
            }
            result.outputs.push(store_path(program)?.to_owned());
            result.app_program = Some(program.to_owned());
        }
    }
    result.outputs.sort();
    result.outputs.dedup();
    if result.outputs.is_empty() {
        return Err(error("invalid_nix_output", "no Nix output was realized"));
    }
    result.closure = path_info(nix, &result.outputs)?;
    verify_paths(nix, &result.sources)?;
    verify_paths(nix, &result.closure)?;
    Ok(result)
}

/// Verified evidence and temporary roots retained for the complete invocation.
pub(crate) struct VerifiedLock {
    pub lock: Lockfile,
    pub digest: String,
    _roots: Option<TempDir>,
}

impl VerifiedLock {
    pub(crate) fn load(
        repository: &CheckedRepository,
        plan: &LogicalPlan,
    ) -> Result<Self, LockError> {
        let mut verified = load(repository, plan, &Nix)?;
        let roots = TempDir::new()?;
        let names: BTreeSet<_> = workers(repository, plan)?
            .values()
            .map(|worker| environment_name(worker).to_string())
            .collect();
        let paths = names
            .iter()
            .flat_map(|name| {
                let e = &verified.lock.environments[name];
                e.outputs.iter().chain(e.sources.keys())
            })
            .cloned()
            .collect();
        pin(&Nix, &paths, roots.path())?;
        verified._roots = Some(roots);
        Ok(verified)
    }

    pub(crate) fn environment(&self, name: &Name) -> Result<&LockedEnvironment, LockError> {
        self.lock.environments.get(name.as_str()).ok_or_else(|| {
            error(
                "lockfile_outdated",
                format!("no lock entry for environment {name}"),
            )
        })
    }

    pub(crate) fn verify_worker(
        &self,
        repository: &CheckedRepository,
        worker: &WorkerDefinition,
        actual: &LockedWorker,
    ) -> Result<(), LockError> {
        let id =
            worker_definition_id(repository, worker).map_err(|e| error("lockfile_outdated", e))?;
        if self.lock.workers.get(&id) != Some(actual) {
            return Err(error(
                "lockfile_outdated",
                "worker launch differs from metewand.lock; run metewand lock",
            ));
        }
        Ok(())
    }
}

fn load(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    nix: &dyn NixBackend,
) -> Result<VerifiedLock, LockError> {
    let path = repository.root.join("metewand.lock");
    if !fs::symlink_metadata(&path)
        .map_err(|e| error("lockfile_unavailable", e))?
        .is_file()
    {
        return Err(error(
            "invalid_lockfile",
            "metewand.lock must be a regular file",
        ));
    }
    let bytes = fs::read(&path).map_err(|e| error("lockfile_unavailable", e))?;
    let lock = Lockfile::parse(&bytes).map_err(|e| error("invalid_lockfile", e))?;
    if source_files(&repository.root)? != lock.source {
        return Err(error(
            "lockfile_outdated",
            "repository source contents differ from metewand.lock",
        ));
    }
    let selected = workers(repository, plan)?;
    let system = text(invoke(
        nix,
        &[
            "eval",
            "--offline",
            "--raw",
            "--impure",
            "--expr",
            "builtins.currentSystem",
        ],
    )?)?;
    let mut environments = BTreeSet::new();
    for (id, worker) in &selected {
        let name = environment_name(worker);
        let entry = lock
            .environments
            .get(name.as_str())
            .ok_or_else(|| error("lockfile_outdated", format!("missing environment {name}")))?;
        if entry.definition != repository.planning_catalog.environment_definitions[name].to_string()
            || !lock.workers.contains_key(id)
            || entry.source_sha256 != source_digest(&lock.source)?
        {
            return Err(error(
                "lockfile_outdated",
                "environment or worker definition differs from metewand.lock",
            ));
        }
        if entry.system != system {
            return Err(error(
                "locked_system_mismatch",
                format!(
                    "environment {name} requires {}, host is {system}",
                    entry.system
                ),
            ));
        }
        environments.insert(name);
    }
    let mut paths = BTreeMap::new();
    for name in environments {
        let entry = &lock.environments[name.as_str()];
        paths.extend(entry.sources.clone());
        paths.extend(entry.closure.clone());
    }
    verify_paths(nix, &paths)?;
    let digest = sha256(&lock.to_bytes().map_err(|e| error("invalid_lockfile", e))?);
    Ok(VerifiedLock {
        lock,
        digest,
        _roots: None,
    })
}

/// Creates or refreshes all reachable Nix environments; checking never prepares them.
pub fn lock_repository(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    check: bool,
) -> Result<LockReport, LockError> {
    lock_with(repository, plan, check, &Nix)
}

fn lock_with(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    check: bool,
    nix: &dyn NixBackend,
) -> Result<LockReport, LockError> {
    let path = repository.root.join("metewand.lock");
    if check {
        let verified = load(repository, plan, nix)?;
        resolve_workers(repository, plan, &verified)?;
        return report(&path, "verified", &verified.lock);
    }
    let selected = workers(repository, plan)?;
    let source = source_files(&repository.root)?;
    let directory = snapshot(&repository.root, &source)?;
    let snapshot_path = text(invoke(
        nix,
        &[
            "store",
            "add",
            "--name",
            "source",
            directory
                .path()
                .to_str()
                .ok_or_else(|| error("invalid_lock_source", "source path must be UTF-8"))?,
        ],
    )?)?;
    let roots = TempDir::new()?;
    let snapshot_root = roots.path().join("snapshot");
    fs::create_dir_all(&snapshot_root)?;
    pin(
        nix,
        &BTreeSet::from([snapshot_path.clone()]),
        &snapshot_root,
    )?;
    let system = text(invoke(
        nix,
        &[
            "eval",
            "--offline",
            "--raw",
            "--impure",
            "--expr",
            "builtins.currentSystem",
        ],
    )?)?;
    let mut environments = BTreeMap::new();
    let digest = source_digest(&source)?;
    for worker in selected.values() {
        let name = environment_name(worker);
        if environments.contains_key(name.as_str()) {
            continue;
        }
        if let EnvironmentDefinition::Nix {
            system: requested, ..
        } = &repository.manifest.environments[name]
            && requested != &system
        {
            return Err(error(
                "locked_system_mismatch",
                format!("environment {name} requires {requested}, host is {system}"),
            ));
        }
        let environment_roots = roots.path().join(environments.len().to_string());
        fs::create_dir_all(&environment_roots)?;
        environments.insert(
            name.to_string(),
            prepare_environment(
                repository,
                name,
                &snapshot_path,
                &digest,
                &environment_roots,
                nix,
            )?,
        );
    }
    let mut verified = VerifiedLock {
        lock: Lockfile {
            schema_version: LOCKFILE_VERSION,
            source,
            environments,
            workers: BTreeMap::new(),
        },
        digest: String::new(),
        _roots: None,
    };
    verified.lock.workers = resolve_workers(repository, plan, &verified)?;
    if source_files(&repository.root)? != verified.lock.source {
        return Err(error(
            "lockfile_outdated",
            "sources changed during environment preparation",
        ));
    }
    let bytes = verified
        .lock
        .to_bytes()
        .map_err(|e| error("invalid_lockfile", e))?;
    Lockfile::parse(&bytes).map_err(|e| error("invalid_lockfile", e))?;
    let previous = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => Some(fs::read(&path)?),
        Ok(_) => {
            return Err(error(
                "invalid_lockfile",
                "metewand.lock must be a regular file",
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let operation = if previous.as_deref() == Some(bytes.as_slice()) {
        "unchanged"
    } else {
        if let Some(old) = &previous {
            Lockfile::parse(old).map_err(|e| error("invalid_lockfile", e))?;
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&repository.root)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&path)
            .map_err(|e| LockError::from(e.error))?;
        #[cfg(unix)]
        fs::File::open(&repository.root)?.sync_all()?;
        if previous.is_some() {
            "updated"
        } else {
            "created"
        }
    };
    report(&path, operation, &verified.lock)
}

fn resolve_workers(
    repository: &CheckedRepository,
    plan: &LogicalPlan,
    lock: &VerifiedLock,
) -> Result<BTreeMap<String, LockedWorker>, LockError> {
    let private = TempDir::new()?;
    workers(repository, plan)?
        .into_iter()
        .map(|(id, worker)| {
            let resolved = crate::run::resolution::resolve_worker_with_lock(
                repository,
                worker,
                private.path(),
                Some(lock),
            )
            .map_err(|e| error("lockfile_outdated", e))?;
            Ok((id, resolved.lock_record(environment_name(worker))))
        })
        .collect()
}

fn report(path: &Path, operation: &'static str, lock: &Lockfile) -> Result<LockReport, LockError> {
    Ok(LockReport {
        command: "lock",
        operation,
        lockfile: path.display().to_string(),
        sha256: sha256(&lock.to_bytes().map_err(|e| error("invalid_lockfile", e))?),
        environments: lock.environments.len(),
        workers: lock.workers.len(),
    })
}

pub(crate) fn locked_program(
    environment: &LockedEnvironment,
    program: &str,
) -> Result<PathBuf, LockError> {
    let path = if let Some(app) = &environment.app_program {
        if Path::new(program).file_name() != Path::new(app).file_name() {
            return Err(error(
                "invalid_locked_launch",
                "worker program must name the selected app entrypoint",
            ));
        }
        PathBuf::from(app)
    } else if Path::new(program).is_absolute() {
        PathBuf::from(program)
    } else {
        let relative = if Path::new(program).components().count() == 1 {
            Path::new("bin").join(program)
        } else {
            PathBuf::from(program)
        };
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(error(
                "invalid_locked_launch",
                "package programs must be normalized paths",
            ));
        }
        let matches = environment
            .outputs
            .iter()
            .map(|p| Path::new(p).join(&relative))
            .filter(|p| p.is_file())
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(error(
                "invalid_locked_launch",
                format!("cannot uniquely resolve {program} in the selected Nix output"),
            ));
        }
        matches[0].clone()
    };
    if !environment
        .closure
        .contains_key(store_path(path.to_str().ok_or_else(|| {
            error("invalid_locked_launch", "program path must be UTF-8")
        })?)?)
    {
        return Err(error(
            "invalid_locked_launch",
            "worker entrypoint must belong to the locked runtime closure",
        ));
    }
    let canonical = fs::canonicalize(&path)?;
    if !environment.closure.contains_key(store_path(
        canonical
            .to_str()
            .ok_or_else(|| error("invalid_locked_launch", "program path must be UTF-8"))?,
    )?) || !fs::metadata(&canonical)?.is_file()
        || !is_executable(&fs::metadata(&canonical)?)
    {
        return Err(error(
            "invalid_locked_launch",
            "worker executable must belong to the locked runtime closure",
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque};

    struct FakeNix {
        replies: RefCell<VecDeque<Result<Vec<u8>, LockError>>>,
        calls: RefCell<Vec<Vec<OsString>>>,
    }
    impl NixBackend for FakeNix {
        fn invoke(&self, _: &str, args: &[OsString]) -> Result<Vec<u8>, LockError> {
            self.calls.borrow_mut().push(args.to_vec());
            self.replies
                .borrow_mut()
                .pop_front()
                .expect("unexpected Nix call")
        }
    }
    fn fake(replies: Vec<Result<Vec<u8>, LockError>>) -> FakeNix {
        FakeNix {
            replies: RefCell::new(replies.into()),
            calls: RefCell::new(vec![]),
        }
    }

    #[test]
    fn verification_checks_transitive_hashes_references_and_contents_without_building() {
        let root = "/nix/store/root";
        let dependency = "/nix/store/dependency";
        let document = json!({root: {"narHash":"sha256-root", "references":[dependency]}, dependency: {"narHash":"sha256-dependency", "references":[]}});
        let expected = BTreeMap::from([
            (
                root.into(),
                StorePath {
                    nar_hash: "sha256-root".into(),
                    references: vec![dependency.into()],
                },
            ),
            (
                dependency.into(),
                StorePath {
                    nar_hash: "sha256-dependency".into(),
                    references: vec![],
                },
            ),
        ]);
        let nix = fake(vec![Ok(serde_json::to_vec(&document).unwrap()), Ok(vec![])]);
        verify_paths(&nix, &expected).unwrap();
        assert_eq!(nix.calls.borrow()[1][0], "store");
        assert!(
            nix.calls
                .borrow()
                .iter()
                .all(|call| !call.iter().any(|a| a == "build" || a == "--repair"))
        );
        let mut changed = document.clone();
        changed[dependency]["narHash"] = json!("sha256-changed");
        let nix = fake(vec![Ok(serde_json::to_vec(&changed).unwrap())]);
        assert_eq!(
            verify_paths(&nix, &expected).unwrap_err().code,
            "locked_closure_mismatch"
        );
        let nix = fake(vec![
            Ok(serde_json::to_vec(&document).unwrap()),
            Err(error("nix_operation_failed", "corrupt contents")),
        ]);
        assert_eq!(
            verify_paths(&nix, &expected).unwrap_err().code,
            "locked_closure_mismatch"
        );
        let nix = fake(vec![Ok(b"{}".to_vec())]);
        assert_eq!(
            verify_paths(&nix, &expected).unwrap_err().code,
            "locked_path_unavailable"
        );
    }

    #[test]
    fn root_snapshots_exclude_lockfiles_and_preserve_dirty_tracked_sources() {
        let root = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(root.path())
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.path().join("flake.nix"), "original").unwrap();
        assert!(
            Command::new("git")
                .current_dir(root.path())
                .args(["add", "flake.nix"])
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.path().join("flake.nix"), "uncommitted edits").unwrap();
        fs::write(root.path().join("metewand.lock"), "excluded").unwrap();
        assert!(
            Command::new("git")
                .current_dir(root.path())
                .args(["add", "metewand.lock"])
                .status()
                .unwrap()
                .success()
        );
        let source = source_files(root.path()).unwrap();
        assert_eq!(source.files.len(), 1);
        let staged = snapshot(root.path(), &source).unwrap();
        assert_eq!(
            fs::read(staged.path().join("flake.nix")).unwrap(),
            b"uncommitted edits"
        );
        fs::write(root.path().join("metewand.lock"), "new lock bytes").unwrap();
        assert_eq!(source_files(root.path()).unwrap(), source);
        fs::write(root.path().join("flake.nix"), "changed").unwrap();
        assert_ne!(source_files(root.path()).unwrap(), source);
    }

    #[test]
    #[cfg(unix)]
    fn equivalent_executable_modes_produce_equivalent_nix_sources() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempDir::new().unwrap();
        let worker = root.path().join("worker");
        fs::write(&worker, "source bytes").unwrap();
        let mut modes = Vec::new();
        for mode in [0o610, 0o711] {
            fs::set_permissions(&worker, fs::Permissions::from_mode(mode)).unwrap();
            let source = source_files(root.path()).unwrap();
            let staged = snapshot(root.path(), &source).unwrap();
            modes.push(
                fs::metadata(staged.path().join("worker"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
            );
        }
        assert_eq!(modes[0], modes[1]);
        assert_ne!(modes[0] & 0o100, 0);
    }

    #[test]
    fn native_inputs_require_content_hashes_and_repository_local_paths() {
        assert!(
            check_native_inputs(&json!({"nodes":{"input":{"locked":{"type":"github"}}}})).is_err()
        );
        assert!(
            check_native_inputs(
                &json!({"nodes":{"input":{"locked":{"type":"path", "path":"/outside"}}}})
            )
            .is_err()
        );
        check_native_inputs(
            &json!({"nodes":{"input":{"locked":{"type":"path", "path":"./library"}}}}),
        )
        .unwrap();
    }

    #[test]
    fn failed_preparation_preserves_the_old_lockfile_or_absence() {
        fn copy(source: &Path, target: &Path) {
            fs::create_dir_all(target).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &target.join(entry.file_name()));
                } else {
                    fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
                }
            }
        }
        let root = TempDir::new().unwrap();
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/fixed-dataset"),
            root.path(),
        );
        let manifest = root.path().join("metewand.yaml");
        fs::write(&manifest, fs::read_to_string(&manifest).unwrap().replace("kind: \"local\"", "kind: nix\n    flake: .\n    installable: '.#python'\n    system: x86_64-linux\n    output-kind: package\n    output: out")).unwrap();
        let repository = crate::repository::check_repository(&manifest).unwrap();
        let plan = repository.logical_plan().unwrap();
        let path = root.path().join("metewand.lock");
        let old = include_bytes!("../../../schemas/v1/fixtures/lockfile-v1.json");
        for present in [true, false] {
            if present {
                fs::write(&path, old).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            let nix = fake(vec![Err(error(
                "nix_operation_failed",
                "preparation failed",
            ))]);
            assert!(lock_with(&repository, &plan, false, &nix).is_err());
            if present {
                assert_eq!(fs::read(&path).unwrap(), old);
            } else {
                assert!(!path.exists());
            }
        }
    }
}
