//! Explicit uv preparation and read-only verification of installed Python content.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsString,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use crate::core::{
    canonical::CanonicalValue,
    lockfile::{LockedPython, LockedUvEnvironment, UvFingerprint},
    manifest::{EnvironmentDefinition, Name},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::runtime::{
    lockfile::{LockError, error, sha256},
    repository::CheckedRepository,
};

type Inventory = BTreeMap<String, Entry>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Entry {
    sha256: Option<String>,
    executable: bool,
    target: Option<String>,
    identity_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeRoot {
    path: PathBuf,
    inventory: Inventory,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    environment: Inventory,
    runtime: BTreeMap<String, RuntimeRoot>,
    python: PathBuf,
    source: PathBuf,
    fingerprint: UvFingerprint,
    uv_version: String,
    build_environment: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct PythonInfo {
    implementation: String,
    version: String,
    abi: String,
    platform: String,
    executable: PathBuf,
    roots: BTreeMap<String, PathBuf>,
}

trait Backend {
    fn invoke(
        &self,
        program: &Path,
        args: &[OsString],
        directory: &Path,
        settings: &BTreeMap<String, OsString>,
    ) -> Result<Vec<u8>, LockError>;
}

struct Process;

impl Backend for Process {
    fn invoke(
        &self,
        program: &Path,
        args: &[OsString],
        directory: &Path,
        settings: &BTreeMap<String, OsString>,
    ) -> Result<Vec<u8>, LockError> {
        let output = Command::new(program)
            .args(args)
            .current_dir(directory)
            .env_clear()
            .envs(settings)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                error(
                    "uv_unavailable",
                    format!("cannot invoke {}: {e}", program.display()),
                )
            })?;
        if !output.status.success() {
            return Err(error(
                "uv_operation_failed",
                format!(
                    "{} failed: {}",
                    program.display(),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }
        Ok(output.stdout)
    }
}

pub(crate) fn system() -> String {
    format!("{}-{}", env::consts::OS, env::consts::ARCH)
}

pub(crate) fn data_directory() -> Result<PathBuf, LockError> {
    let root = if let Some(path) = env::var_os("METEWAND_DATA_DIR") {
        PathBuf::from(path)
    } else if cfg!(windows) {
        PathBuf::from(env::var_os("LOCALAPPDATA").ok_or_else(|| {
            error(
                "uv_data_unavailable",
                "set METEWAND_DATA_DIR or LOCALAPPDATA",
            )
        })?)
        .join("metewand")
    } else {
        let home = || {
            env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| error("uv_data_unavailable", "set METEWAND_DATA_DIR or HOME"))
        };
        if cfg!(target_os = "macos") {
            home()?.join("Library/Application Support/metewand")
        } else {
            match env::var_os("XDG_DATA_HOME") {
                Some(path) => PathBuf::from(path).join("metewand"),
                None => home()?.join(".local/share/metewand"),
            }
        }
    };
    if !root.is_absolute() {
        return Err(error(
            "uv_data_unavailable",
            "Metewand's data directory must be absolute",
        ));
    }
    Ok(root)
}

fn generation_in(environment: &LockedUvEnvironment, data: &Path) -> Result<PathBuf, LockError> {
    let path = Path::new(&environment.preparation);
    if path.components().count() != 2
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(error("invalid_lockfile", "invalid uv preparation locator"));
    }
    Ok(data.join("environments/uv").join(path))
}

pub(crate) fn environment_path(environment: &LockedUvEnvironment) -> Result<PathBuf, LockError> {
    Ok(generation_in(environment, &data_directory()?)?.join("environment"))
}

pub(crate) fn executable_directory(root: &Path) -> PathBuf {
    root.join(if cfg!(windows) { "Scripts" } else { "bin" })
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, LockError> {
    CanonicalValue::try_from(
        serde_json::to_value(value).map_err(|e| error("invalid_uv_evidence", e))?,
    )
    .and_then(|v| v.to_canonical_bytes())
    .map_err(|e| error("invalid_uv_evidence", e))
}

fn normalized(bytes: &[u8], replacements: &[(String, String)]) -> Vec<u8> {
    let mut result = bytes.to_vec();
    for (old, replacement) in replacements {
        if old.is_empty() {
            continue;
        }
        let mut output = Vec::new();
        let mut remaining = result.as_slice();
        while let Some(index) = remaining
            .windows(old.len())
            .position(|s| s == old.as_bytes())
        {
            output.extend_from_slice(&remaining[..index]);
            output.extend_from_slice(replacement.as_bytes());
            remaining = &remaining[index + old.len()..];
        }
        output.extend_from_slice(remaining);
        result = output;
    }
    result
}

fn inventory(
    root: &Path,
    runtime: bool,
    replacements: &[(String, String)],
) -> Result<Inventory, LockError> {
    fn visit(
        root: &Path,
        path: &Path,
        runtime: bool,
        replacements: &[(String, String)],
        active: &mut BTreeSet<PathBuf>,
        entries: &mut Inventory,
    ) -> Result<(), LockError> {
        let relative = path.strip_prefix(root).expect("inventory descendant");
        // Workers use a private bytecode prefix and never read the host cache.
        if runtime
            && relative.components().any(|c| {
                c.as_os_str() == "site-packages"
                    || c.as_os_str() == "dist-packages"
                    || c.as_os_str() == "__pycache__"
            })
        {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(path)?;
        let target = metadata
            .is_symlink()
            .then(|| fs::read_link(path))
            .transpose()?;
        let actual = fs::metadata(path)?;
        let name = if relative.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            crate::runtime::platform_fs::relative_path(relative)?
        };
        let executable = crate::runtime::lockfile::is_executable(&actual);
        let content = if actual.is_file() {
            Some(fs::read(path)?)
        } else {
            None
        };
        if !actual.is_file() && !actual.is_dir() {
            return Err(error(
                "invalid_uv_evidence",
                format!("unsupported installed entry: {}", path.display()),
            ));
        }
        let launcher = (name.starts_with("bin/") || name.starts_with("Scripts/"))
            && content.as_ref().is_some_and(|bytes| {
                bytes.starts_with(b"#!")
                    || (cfg!(windows)
                        && !matches!(
                            path.file_name().and_then(|n| n.to_str()),
                            Some("python.exe" | "pythonw.exe")
                        )
                        && replacements
                            .iter()
                            .filter(|(_, new)| new == "{environment}")
                            .any(|(old, _)| {
                                let prefix = format!("#!{old}");
                                bytes
                                    .windows(prefix.len())
                                    .any(|part| part == prefix.as_bytes())
                            }))
            });
        let activation = (name.starts_with("bin/") || name.starts_with("Scripts/"))
            && matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(
                    "activate"
                        | "activate.csh"
                        | "activate.fish"
                        | "activate.nu"
                        | "activate.bat"
                        | "activate.ps1"
                        | "Activate.ps1"
                        | "activate_this.py"
                        | "deactivate.bat"
                )
            );
        let generated = name == "pyvenv.cfg"
            || name.ends_with(".dist-info/direct_url.json")
            || launcher
            || activation;
        let digest = content.as_ref().map(|bytes| sha256(bytes));
        let identity_sha256 = if generated {
            content
                .as_ref()
                .map(|bytes| sha256(&normalized(bytes, replacements)))
        } else {
            digest.clone()
        };
        entries.insert(
            name,
            Entry {
                sha256: digest,
                executable: actual.is_file() && executable,
                target: target.map(|p| p.to_string_lossy().into_owned()),
                identity_sha256,
            },
        );
        if actual.is_dir() {
            let canonical = fs::canonicalize(path)?;
            if !active.insert(canonical.clone()) {
                return Err(error("invalid_uv_evidence", "cycle in installed symlinks"));
            }
            let mut children = fs::read_dir(path)?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()?;
            children.sort();
            for child in children {
                visit(root, &child, runtime, replacements, active, entries)?;
            }
            active.remove(&canonical);
        }
        Ok(())
    }
    let mut entries = BTreeMap::new();
    visit(
        root,
        root,
        runtime,
        replacements,
        &mut BTreeSet::new(),
        &mut entries,
    )?;
    Ok(entries)
}

fn identity_inventory(inventory: &Inventory, replacements: &[(String, String)]) -> Value {
    let entries: BTreeMap<_,_> = inventory.iter().filter(|(name,_)| !name.ends_with(".dist-info/RECORD") && !name.ends_with(".dist-info/uv_cache.json"))
        .map(|(name,e)| (name, json!({"sha256":e.identity_sha256, "executable":e.executable,
            "target":e.target.as_ref().map(|p|String::from_utf8_lossy(&normalized(p.as_bytes(), replacements)).into_owned())}))).collect();
    json!(entries)
}

fn replacements(root: &Path, receipt: &Receipt) -> Vec<(String, String)> {
    let mut entries = vec![
        (root.to_string_lossy().into_owned(), "{environment}".into()),
        (
            receipt.source.to_string_lossy().into_owned(),
            "{sources}".into(),
        ),
        (
            receipt.python.to_string_lossy().into_owned(),
            "{python}".into(),
        ),
    ];
    for (name, runtime) in &receipt.runtime {
        entries.push((
            runtime.path.to_string_lossy().into_owned(),
            format!("{{python_{name}}}"),
        ));
    }
    let originals = entries.clone();
    for (old, replacement) in originals {
        let unprefixed = old.trim_start_matches(r"\\?\").to_owned();
        let forward = unprefixed.replace('\\', "/");
        for variant in [old, unprefixed, forward.clone()] {
            let escaped = serde_json::to_string(&variant).expect("path string");
            entries.push((variant, replacement.clone()));
            entries.push((
                escaped[1..escaped.len() - 1].to_owned(),
                replacement.clone(),
            ));
        }
        let encoded: String = forward
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
                    char::from(byte).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect();
        let url = if forward.starts_with('/') {
            format!("file://{encoded}")
        } else {
            format!("file:///{encoded}")
        };
        entries.push((url, format!("file://{replacement}")));
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
    entries
}

fn read_receipt_in(environment: &LockedUvEnvironment, data: &Path) -> Result<Receipt, LockError> {
    let path = generation_in(environment, data)?.join("receipt.json");
    if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
        return Err(error(
            "uv_environment_unavailable",
            "prepared uv environment is missing; run metewand lock",
        ));
    }
    let bytes = fs::read(path)?;
    if sha256(&bytes) != environment.receipt_sha256 {
        return Err(error(
            "uv_environment_changed",
            "uv integrity receipt differs; run metewand lock",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| error("invalid_uv_evidence", e))
}

fn read_receipt(environment: &LockedUvEnvironment) -> Result<Receipt, LockError> {
    read_receipt_in(environment, &data_directory()?)
}

pub(crate) fn verify(environment: &LockedUvEnvironment) -> Result<(), LockError> {
    verify_in(environment, &data_directory()?)
}

fn verify_in(environment: &LockedUvEnvironment, data: &Path) -> Result<(), LockError> {
    let receipt = read_receipt_in(environment, data)?;
    let root = generation_in(environment, data)?.join("environment");
    let replacements = replacements(&root, &receipt);
    let verify = || -> Result<bool, LockError> {
        if inventory(&root, false, &replacements)? != receipt.environment {
            return Ok(false);
        }
        for runtime in receipt.runtime.values() {
            if inventory(&runtime.path, true, &replacements)? != runtime.inventory {
                return Ok(false);
            }
        }
        Ok(canonical_bytes(&receipt.fingerprint)? == canonical_bytes(&environment.fingerprint)?)
    };
    if !verify().unwrap_or(false) {
        return Err(error(
            "uv_environment_changed",
            "prepared Python runtime or packages differ; run metewand lock",
        ));
    }
    Ok(())
}

pub(crate) fn program(environment: &LockedUvEnvironment, name: &str) -> Result<PathBuf, LockError> {
    program_in(environment, name, &data_directory()?)
}

fn program_in(
    environment: &LockedUvEnvironment,
    name: &str,
    data: &Path,
) -> Result<PathBuf, LockError> {
    if Path::new(name).components().count() != 1 {
        return Err(error(
            "invalid_locked_launch",
            "uv programs must name an installed entry point",
        ));
    }
    let directory = executable_directory(&generation_in(environment, data)?.join("environment"));
    let file = if matches!(name, "python" | "python3" | "python.exe" | "python3.exe") {
        if cfg!(windows) {
            "python.exe"
        } else {
            "python"
        }
    } else {
        name
    };
    let mut path = directory.join(file);
    if cfg!(windows) && path.extension().is_none() && !path.is_file() {
        path.set_extension("exe");
    }
    let receipt = read_receipt_in(environment, data)?;
    let relative = crate::runtime::platform_fs::relative_path(
        path.strip_prefix(generation_in(environment, data)?.join("environment"))
            .expect("installed program"),
    )?;
    if !receipt
        .environment
        .get(&relative)
        .is_some_and(|e| e.sha256.is_some() && (cfg!(windows) || e.executable))
        || !path.is_file()
    {
        return Err(error(
            "invalid_locked_launch",
            format!("program {name} is absent from the prepared uv environment"),
        ));
    }
    Ok(path)
}

pub(crate) fn program_identity(
    environment: &LockedUvEnvironment,
    path: &Path,
) -> Result<(String, String), LockError> {
    let root = environment_path(environment)?;
    if let Ok(relative) = path.strip_prefix(&root) {
        let relative = crate::runtime::platform_fs::relative_path(relative)?;
        let receipt = read_receipt(environment)?;
        let digest = receipt
            .environment
            .get(&relative)
            .and_then(|e| e.identity_sha256.clone())
            .ok_or_else(|| error("invalid_locked_launch", "program is absent from uv receipt"))?;
        Ok((format!("{{environment}}/{relative}"), digest))
    } else {
        Ok((
            format!("sha256:{}", sha256(&fs::read(path)?)),
            sha256(&fs::read(path)?),
        ))
    }
}

const PYTHON_INFO: &str = r#"
import json, pathlib, platform, sys, sysconfig
roots = {'executable': str(pathlib.Path(sys.executable).resolve()), 'stdlib': sysconfig.get_path('stdlib')}
platstdlib = sysconfig.get_path('platstdlib')
if platstdlib != roots['stdlib']: roots['platstdlib'] = platstdlib
library = pathlib.Path(sysconfig.get_config_var('LIBDIR') or sys.base_prefix) / (sysconfig.get_config_var('LDLIBRARY') or '')
if library.is_file(): roots['library'] = str(library)
if sys.platform == 'win32':
    for path in pathlib.Path(sys.base_prefix).glob('python*.dll'): roots[path.name] = str(path)
    if (pathlib.Path(sys.base_prefix) / 'DLLs').is_dir(): roots['extensions'] = str(pathlib.Path(sys.base_prefix) / 'DLLs')
print(json.dumps(dict(implementation=sys.implementation.name, version=platform.python_version(), abi=sysconfig.get_config_var('SOABI') or sys.implementation.cache_tag, platform=sysconfig.get_platform(), executable=roots['executable'], roots=roots)))
"#;

const VALIDATE_SOURCES: &str = r#"
import json, pathlib, sys, tomllib
root, project = map(pathlib.Path, sys.argv[1:])
local = []
for package in tomllib.loads((project / 'uv.lock').read_text())['package']:
    source = package.get('source', {})
    for kind in ('directory', 'editable', 'path', 'virtual'):
        if kind not in source: continue
        path = pathlib.Path(source[kind])
        if path.is_absolute(): raise ValueError('Local dependencies must use repository-relative paths.')
        selected = (project / path).resolve(strict=True)
        if not selected.is_relative_to(root.resolve()): raise ValueError('Local dependency escapes captured sources: ' + str(path))
        local.append(package['name'])
print(json.dumps(sorted(set(local))))
"#;

const DISTRIBUTIONS: &str = r#"
import importlib.metadata, json
print(json.dumps({d.metadata['Name']: d.version for d in importlib.metadata.distributions()}))
"#;

fn settings(generation: &Path, data: &Path) -> Result<BTreeMap<String, OsString>, LockError> {
    let configuration = generation.join("config/uv");
    fs::create_dir_all(&configuration)?;
    // An existing empty config prevents uv from falling back to /etc/uv.
    fs::write(configuration.join("uv.toml"), b"")?;
    let mut values = BTreeMap::new();
    for name in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "CC",
        "CXX",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "PKG_CONFIG_PATH",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "UV_OFFLINE",
        "UV_PYTHON_INSTALL_DIR",
    ] {
        if let Some(value) = env::var_os(name) {
            values.insert(name.into(), value);
        }
    }
    for name in [
        "XDG_CONFIG_HOME",
        "XDG_CONFIG_DIRS",
        "APPDATA",
        "PROGRAMDATA",
    ] {
        values.insert(name.into(), generation.join("config").into_os_string());
    }
    values.insert(
        "UV_CACHE_DIR".into(),
        data.join("uv-cache").into_os_string(),
    );
    values.insert(
        "UV_PROJECT_ENVIRONMENT".into(),
        generation.join("environment").into_os_string(),
    );
    values.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    values.insert("PYTHONNOUSERSITE".into(), "1".into());
    Ok(values)
}

pub(crate) fn prepare(
    repository: &CheckedRepository,
    name: &Name,
    source: &Path,
    source_sha256: &str,
) -> Result<LockedUvEnvironment, LockError> {
    prepare_with(
        repository,
        name,
        source,
        source_sha256,
        &data_directory()?,
        &Process,
    )
}

fn prepare_with(
    repository: &CheckedRepository,
    name: &Name,
    source: &Path,
    source_sha256: &str,
    data: &Path,
    backend: &dyn Backend,
) -> Result<LockedUvEnvironment, LockError> {
    let EnvironmentDefinition::Uv {
        project,
        lockfile,
        groups,
        extras,
    } = &repository.manifest.environments[name]
    else {
        unreachable!("uv backend")
    };
    let project_root = source.join(project.as_path());
    let native_path = source.join(lockfile.as_path());
    if fs::canonicalize(&native_path).ok() != fs::canonicalize(project_root.join("uv.lock")).ok()
        || !native_path.is_file()
        || !project_root.join("pyproject.toml").is_file()
    {
        return Err(error(
            "invalid_uv_project",
            "uv projects require captured pyproject.toml and uv.lock at the selected project or workspace root; create the native lock with uv lock",
        ));
    }
    let native_bytes = fs::read(&native_path)?;
    if data.starts_with(&repository.root) {
        return Err(error(
            "invalid_uv_project",
            "METEWAND_DATA_DIR must be outside captured sources",
        ));
    }
    fs::create_dir_all(data)?;
    let data = fs::canonicalize(data)?;
    if data.starts_with(&repository.root) {
        return Err(error(
            "invalid_uv_project",
            "Metewand's data directory must be outside captured sources",
        ));
    }
    let probe = tempfile::Builder::new()
        .prefix("probe-")
        .tempdir_in(&data)?;
    let probe_settings = settings(probe.path(), &data)?;
    let call = |program: &Path, args: Vec<OsString>, settings: &BTreeMap<String, OsString>| {
        backend.invoke(program, &args, &project_root, settings)
    };
    let uv = Path::new("uv");
    let python = call(
        uv,
        vec![
            "python".into(),
            "find".into(),
            "--system".into(),
            "--no-python-downloads".into(),
        ],
        &probe_settings,
    )?;
    let python = PathBuf::from(
        String::from_utf8(python)
            .map_err(|e| error("invalid_uv_output", e))?
            .trim(),
    );
    let info: PythonInfo = serde_json::from_slice(&call(
        &python,
        vec!["-I".into(), "-B".into(), "-c".into(), PYTHON_INFO.into()],
        &probe_settings,
    )?)
    .map_err(|e| error("invalid_uv_output", e))?;
    let local: Vec<String> = serde_json::from_slice(&call(
        &python,
        vec![
            "-I".into(),
            "-B".into(),
            "-c".into(),
            VALIDATE_SOURCES.into(),
            source.as_os_str().into(),
            project_root.as_os_str().into(),
        ],
        &probe_settings,
    )?)
    .map_err(|e| error("invalid_uv_output", e))?;
    let mut runtime = BTreeMap::new();
    for (key, path) in &info.roots {
        runtime.insert(
            key.clone(),
            RuntimeRoot {
                path: path.clone(),
                inventory: inventory(path, true, &[])?,
            },
        );
    }
    let runtime_document: BTreeMap<_, _> = runtime
        .iter()
        .map(|(name, root)| (name, identity_inventory(&root.inventory, &[])))
        .collect();
    let locked_python = LockedPython {
        implementation: info.implementation,
        version: info.version,
        abi: info.abi,
        platform: info.platform,
        executable_sha256: sha256(&fs::read(&info.executable)?),
        runtime_sha256: sha256(&canonical_bytes(&runtime_document)?),
    };
    let key = sha256(&canonical_bytes(
        &json!({"definition":repository.planning_catalog.environment_definitions[name].to_string(),"source":source_sha256,"native_lock":sha256(&native_bytes),"python":locked_python,"system":system()}),
    )?);
    let parent = data.join("environments/uv").join(&key);
    fs::create_dir_all(&parent)?;
    let guard = crate::runtime::platform_fs::open_regular(
        &parent.join(".lock"),
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true),
    )?;
    guard.lock()?;
    if let Ok(bytes) = fs::read(parent.join("current.json"))
        && let Ok(current) = serde_json::from_slice::<LockedUvEnvironment>(&bytes)
        && current.definition
            == repository.planning_catalog.environment_definitions[name].to_string()
        && current.source_sha256 == source_sha256
        && current.native_lock_sha256 == sha256(&native_bytes)
        && current.system == system()
        && current.preparation.starts_with(&format!("{key}/"))
        && verify_in(&current, &data).is_ok()
    {
        return Ok(current);
    }
    let generation = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(&parent)?;
    let configuration = settings(generation.path(), &data)?;
    let mut arguments: Vec<OsString> = [
        "sync",
        "--locked",
        "--no-editable",
        "--no-default-groups",
        "--link-mode",
        "copy",
        "--no-python-downloads",
        "--no-progress",
        "--python",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    arguments.push(python.into_os_string());
    for group in groups {
        arguments.extend(["--group".into(), group.as_str().into()]);
    }
    for extra in extras {
        arguments.extend(["--extra".into(), extra.as_str().into()]);
    }
    for package in local {
        arguments.extend(["--reinstall-package".into(), package.into()]);
    }
    call(uv, arguments, &configuration)?;
    if fs::read(&native_path)? != native_bytes {
        return Err(error(
            "lockfile_outdated",
            "uv changed the author-owned native lock; preparation was discarded",
        ));
    }
    let root = generation.path().join("environment");
    let venv_python = executable_directory(&root).join(if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    });
    let distributions = serde_json::from_slice(&call(
        &venv_python,
        vec!["-I".into(), "-B".into(), "-c".into(), DISTRIBUTIONS.into()],
        &configuration,
    )?)
    .map_err(|e| error("invalid_uv_output", e))?;
    let uv_version = String::from_utf8(call(uv, vec!["--version".into()], &configuration)?)
        .map_err(|e| error("invalid_uv_output", e))?
        .trim()
        .to_owned();
    let mut receipt = Receipt {
        environment: BTreeMap::new(),
        runtime,
        python: info.executable,
        source: source.to_owned(),
        fingerprint: UvFingerprint {
            python: locked_python,
            packages_sha256: String::new(),
            distributions,
            groups: groups.iter().map(ToString::to_string).collect(),
            extras: extras.iter().map(ToString::to_string).collect(),
        },
        uv_version,
        build_environment: configuration
            .iter()
            .filter(|(key, _)| {
                [
                    "CC",
                    "CXX",
                    "CFLAGS",
                    "CXXFLAGS",
                    "CPPFLAGS",
                    "LDFLAGS",
                    "PKG_CONFIG_PATH",
                    "SDKROOT",
                    "MACOSX_DEPLOYMENT_TARGET",
                ]
                .contains(&key.as_str())
            })
            .map(|(k, v)| (k.clone(), v.to_string_lossy().into_owned()))
            .collect(),
    };
    let replacements = replacements(&root, &receipt);
    receipt.environment = inventory(&root, false, &replacements)?;
    receipt.fingerprint.packages_sha256 = sha256(&canonical_bytes(&identity_inventory(
        &receipt.environment,
        &replacements,
    ))?);
    let bytes = canonical_bytes(&receipt)?;
    let entry = LockedUvEnvironment {
        definition: repository.planning_catalog.environment_definitions[name].to_string(),
        source_sha256: source_sha256.into(),
        native_lock_sha256: sha256(&native_bytes),
        system: system(),
        fingerprint: receipt.fingerprint.clone(),
        preparation: format!(
            "{key}/{}",
            generation
                .path()
                .file_name()
                .expect("generation name")
                .to_string_lossy()
        ),
        receipt_sha256: sha256(&bytes),
    };
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(generation.path().join("receipt.json"))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    // Virtual environments retain absolute paths, so publication never renames them.
    let path = generation.keep();
    verify_in(&entry, &data)?;
    let mut current = tempfile::NamedTempFile::new_in(&parent)?;
    current.write_all(&canonical_bytes(&entry)?)?;
    current.as_file().sync_all()?;
    current
        .persist(parent.join("current.json"))
        .map_err(|e| e.error)?;
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
        fs::File::open(&parent)?.sync_all()?;
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Fake {
        python: PathBuf,
        syncs: Cell<usize>,
        fail_sync: Cell<bool>,
        calls: RefCell<Vec<Vec<OsString>>>,
    }
    impl Backend for Fake {
        fn invoke(
            &self,
            _: &Path,
            args: &[OsString],
            _: &Path,
            settings: &BTreeMap<String, OsString>,
        ) -> Result<Vec<u8>, LockError> {
            self.calls.borrow_mut().push(args.to_vec());
            if args[0] == "python" {
                return Ok(self.python.to_string_lossy().as_bytes().to_vec());
            }
            if args.iter().any(|a| a == PYTHON_INFO) {
                return Ok(serde_json::to_vec(&json!({"implementation":"cpython","version":"3.14.0","abi":"test","platform":"test","executable":self.python,"roots":{"executable":self.python}})).unwrap());
            }
            if args.iter().any(|a| a == VALIDATE_SOURCES) {
                return Ok(b"[\"local-package\"]".to_vec());
            }
            if args[0] == "sync" {
                self.syncs.set(self.syncs.get() + 1);
                if self.fail_sync.get() {
                    return Err(error("uv_operation_failed", "interrupted"));
                }
                let root = PathBuf::from(&settings["UV_PROJECT_ENVIRONMENT"]);
                fs::create_dir_all(executable_directory(&root))?;
                fs::write(
                    executable_directory(&root).join(if cfg!(windows) {
                        "python.exe"
                    } else {
                        "python"
                    }),
                    b"python",
                )?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        executable_directory(&root).join("python"),
                        fs::Permissions::from_mode(0o755),
                    )?;
                }
                fs::write(root.join("package.py"), b"value = 1\n")?;
                fs::write(
                    root.join("pyvenv.cfg"),
                    format!("environment = {}\n", root.display()),
                )?;
                return Ok(vec![]);
            }
            if args.iter().any(|a| a == DISTRIBUTIONS) {
                return Ok(b"{\"local-package\":\"1.0\"}".to_vec());
            }
            assert_eq!(args, [OsString::from("--version")]);
            Ok(b"uv test".to_vec())
        }
    }
    fn fixture(root: &Path) -> CheckedRepository {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("metewand.yaml"),"version: 1\nname: uv\nenvironments: {python: {kind: uv, project: ., lockfile: uv.lock, groups: [benchmark], extras: [checked]}}\n").unwrap();
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname='example'\nversion='1.0'\n",
        )
        .unwrap();
        fs::write(root.join("uv.lock"), "version = 1\n").unwrap();
        crate::runtime::repository::check_repository(&root.join("metewand.yaml")).unwrap()
    }
    fn fake(root: &Path) -> Fake {
        let python = root.join("python");
        fs::write(&python, b"python").unwrap();
        Fake {
            python,
            syncs: Cell::new(0),
            fail_sync: Cell::new(false),
            calls: RefCell::new(vec![]),
        }
    }
    #[test]
    fn preparation_is_reused_only_after_exact_verification_and_never_replaces_a_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = fixture(&temporary.path().join("sources"));
        let fake = fake(temporary.path());
        let data = temporary.path().join("data");
        let name = repository.manifest.environments.keys().next().unwrap();
        let first =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        verify_in(&first, &data).unwrap();
        let second =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        assert_eq!(first.preparation, second.preparation);
        assert_eq!(fake.syncs.get(), 1);
        let path = generation_in(&first, &data)
            .unwrap()
            .join("environment/package.py");
        fs::write(&path, b"value = 2\n").unwrap();
        assert_eq!(
            verify_in(&first, &data).unwrap_err().code,
            "uv_environment_changed"
        );
        let third =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        assert_ne!(first.preparation, third.preparation);
        assert_eq!(fs::read(path).unwrap(), b"value = 2\n");
        let calls = fake.calls.borrow();
        let sync = calls.iter().find(|a| a[0] == "sync").unwrap();
        for flag in [
            "--locked",
            "--no-editable",
            "--no-default-groups",
            "--no-python-downloads",
            "--reinstall-package",
            "--group",
            "--extra",
        ] {
            assert!(sync.iter().any(|a| a == flag));
        }
        assert!(sync.windows(2).any(|a| a == ["--link-mode", "copy"]));
    }
    #[test]
    fn interrupted_preparation_publishes_no_receipt_or_current_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = fixture(&temporary.path().join("sources"));
        let fake = fake(temporary.path());
        fake.fail_sync.set(true);
        let data = temporary.path().join("data");
        let name = repository.manifest.environments.keys().next().unwrap();
        assert!(prepare_with(&repository, name, &repository.root, "source", &data, &fake).is_err());
        for key in fs::read_dir(data.join("environments/uv")).unwrap() {
            let key = key.unwrap().path();
            assert!(!key.join("current.json").exists());
            assert_eq!(fs::read_dir(key).unwrap().count(), 1);
        }
    }
    #[test]
    fn raw_inventory_detects_injection_deletion_and_runtime_drift() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = fixture(&temporary.path().join("sources"));
        let fake = fake(temporary.path());
        let data = temporary.path().join("data");
        let name = repository.manifest.environments.keys().next().unwrap();
        let entry =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        let injected = generation_in(&entry, &data)
            .unwrap()
            .join("environment/injected.py");
        fs::write(&injected, b"pass").unwrap();
        assert!(verify_in(&entry, &data).is_err());
        fs::remove_file(injected).unwrap();
        verify_in(&entry, &data).unwrap();
        fs::write(&fake.python, b"changed").unwrap();
        assert!(verify_in(&entry, &data).is_err());
        fs::write(&fake.python, b"python").unwrap();
        verify_in(&entry, &data).unwrap();
        fs::remove_file(
            generation_in(&entry, &data)
                .unwrap()
                .join("environment/package.py"),
        )
        .unwrap();
        assert!(verify_in(&entry, &data).is_err());
    }
    #[test]
    fn generated_paths_are_normalized_without_rewriting_package_code_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("pyvenv.cfg"), "environment = /one\n").unwrap();
        fs::write(root.join("module.py"), "path = '/one'\n").unwrap();
        let first = inventory(root, false, &[("/one".into(), "{environment}".into())]).unwrap();
        fs::write(root.join("pyvenv.cfg"), "environment = /two\n").unwrap();
        let second = inventory(root, false, &[("/two".into(), "{environment}".into())]).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            first["pyvenv.cfg"].identity_sha256,
            second["pyvenv.cfg"].identity_sha256
        );
        fs::write(root.join("module.py"), "path = '/two'\n").unwrap();
        let third = inventory(root, false, &[("/two".into(), "{environment}".into())]).unwrap();
        assert_ne!(
            first["module.py"].identity_sha256,
            third["module.py"].identity_sha256
        );
    }
    #[test]
    fn installed_programs_have_no_ambient_fallback() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = fixture(&temporary.path().join("sources"));
        let fake = fake(temporary.path());
        let data = temporary.path().join("data");
        let name = repository.manifest.environments.keys().next().unwrap();
        let entry =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        assert!(program_in(&entry, "python3", &data).unwrap().is_file());
        for program in ["Rscript", "../python", "/usr/bin/python3", "python999"] {
            assert_eq!(
                program_in(&entry, program, &data).unwrap_err().code,
                "invalid_locked_launch"
            );
        }
    }

    #[test]
    fn concurrent_preparation_installs_once_and_reuses_the_published_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = fixture(&temporary.path().join("sources"));
        let python = fake(temporary.path()).python;
        let data = temporary.path().join("data");
        let name = repository.manifest.environments.keys().next().unwrap();
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        let fake = Fake {
                            python: python.clone(),
                            syncs: Cell::new(0),
                            fail_sync: Cell::new(false),
                            calls: RefCell::new(vec![]),
                        };
                        barrier.wait();
                        let entry = prepare_with(
                            &repository,
                            name,
                            &repository.root,
                            "source",
                            &data,
                            &fake,
                        )
                        .unwrap();
                        (entry.preparation, fake.syncs.get())
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results[0].0, results[1].0);
        assert_eq!(results[0].1 + results[1].1, 1);
    }
    #[test]
    fn runtime_cache_churn_is_excluded_because_workers_read_private_bytecode() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("module.py"), "value = 1\n").unwrap();
        let before = inventory(root, true, &[]).unwrap();
        fs::create_dir(root.join("__pycache__")).unwrap();
        fs::write(root.join("__pycache__/module.pyc"), b"mutable host cache").unwrap();
        assert_eq!(inventory(root, true, &[]).unwrap(), before);
        fs::write(root.join("module.py"), "value = 2\n").unwrap();
        assert_ne!(inventory(root, true, &[]).unwrap(), before);
    }
    #[test]
    fn activation_scripts_are_normalized_but_arbitrary_binaries_keep_their_content() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir(root.join("bin")).unwrap();
        fs::write(root.join("bin/activate"), "VIRTUAL_ENV='/one'\n").unwrap();
        fs::write(root.join("bin/tool"), b"binary /one").unwrap();
        let first = inventory(root, false, &[("/one".into(), "{environment}".into())]).unwrap();
        fs::write(root.join("bin/activate"), "VIRTUAL_ENV='/two'\n").unwrap();
        fs::write(root.join("bin/tool"), b"binary /two").unwrap();
        let second = inventory(root, false, &[("/two".into(), "{environment}".into())]).unwrap();
        assert_eq!(
            first["bin/activate"].identity_sha256,
            second["bin/activate"].identity_sha256
        );
        assert_ne!(
            first["bin/tool"].identity_sha256,
            second["bin/tool"].identity_sha256
        );
    }
    #[test]
    fn cached_evidence_must_match_the_preparation_inputs() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = fixture(&temporary.path().join("sources"));
        let fake = fake(temporary.path());
        let data = temporary.path().join("data");
        let name = repository.manifest.environments.keys().next().unwrap();
        let first =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        let mut corrupted = first.clone();
        corrupted.definition = "wrong definition".into();
        let current = generation_in(&first, &data)
            .unwrap()
            .parent()
            .unwrap()
            .join("current.json");
        fs::write(current, canonical_bytes(&corrupted).unwrap()).unwrap();
        let repaired =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        assert_eq!(repaired.definition, first.definition);
        assert_ne!(repaired.preparation, first.preparation);
    }
    #[test]
    fn windows_generated_paths_normalize_verbatim_native_json_and_url_forms() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = fixture(&temporary.path().join("sources"));
        let fake = fake(temporary.path());
        let data = temporary.path().join("data");
        let name = repository.manifest.environments.keys().next().unwrap();
        let entry =
            prepare_with(&repository, name, &repository.root, "source", &data, &fake).unwrap();
        let receipt = read_receipt_in(&entry, &data).unwrap();
        let replacements = replacements(
            Path::new(r"\\?\C:\Users\Alice Smith\state\environment"),
            &receipt,
        );
        for value in [
            r"C:\Users\Alice Smith\state\environment",
            r"\\?\C:\Users\Alice Smith\state\environment",
            r"C:/Users/Alice Smith/state/environment",
        ] {
            assert_eq!(
                normalized(value.as_bytes(), &replacements),
                b"{environment}"
            );
            assert_eq!(
                normalized(
                    serde_json::to_string(value).unwrap().as_bytes(),
                    &replacements
                ),
                br#""{environment}""#
            );
        }
        assert_eq!(
            normalized(
                b"file:///C:/Users/Alice%20Smith/state/environment",
                &replacements
            ),
            b"file://{environment}"
        );
    }
}
