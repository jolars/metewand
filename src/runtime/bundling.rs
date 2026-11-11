//! Explicit generation of portable Docker build contexts.

use super::{
    repository::{CheckedRepository, check_repository},
    source_bundle::expand_source_bundle,
};
use crate::core::{
    bundling::{BUNDLE_VERSION, BundleEntry, BundleManifest, BundleReport},
    canonical::CanonicalValue,
    manifest::{DatasetDefinition, EnvironmentDefinition, OciImageReference, RepositoryPath},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub const BUILDER_IMAGE: &str =
    "rust:1.98.0-alpine@sha256:a10e64dd139b7387337c7fbe8aca31b959b57b2fd4c8ae20a02cf1d6ea424dce";

pub struct BundleOptions {
    pub destination: PathBuf,
    pub base_image: String,
    pub runner_version: String,
    pub recipe: Option<RepositoryPath>,
    pub includes: Vec<RepositoryPath>,
    pub dry_run: bool,
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}
fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn portable(path: &Path) -> io::Result<String> {
    Ok(path
        .to_str()
        .ok_or_else(|| invalid("input paths must be UTF-8"))?
        .replace(std::path::MAIN_SEPARATOR, "/"))
}
fn exact_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
}

fn file_entry(path: String, source: &Path) -> io::Result<BundleEntry> {
    let metadata = fs::symlink_metadata(source)?;
    if !super::platform_fs::is_regular_file(&metadata) {
        return Err(invalid(
            "bundle inputs must be regular files or directories",
        ));
    }
    let mut file = fs::File::open(source)?;
    let mut hasher = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    if size != metadata.len() {
        return Err(invalid("input changed while hashing"));
    }
    Ok(BundleEntry::File {
        path,
        sha256: hex(hasher.finalize()),
        size_bytes: size,
        executable: super::platform_fs::is_executable(&metadata),
    })
}

fn collect(
    root: &Path,
    path: &Path,
    entries: &mut BTreeMap<String, (BundleEntry, PathBuf)>,
) -> io::Result<()> {
    let mut ancestor = root.to_path_buf();
    for component in path.components() {
        ancestor.push(component);
        let metadata = fs::symlink_metadata(&ancestor)?;
        if !super::platform_fs::is_regular_file(&metadata)
            && !super::platform_fs::is_real_directory(&metadata)
        {
            return Err(invalid("bundle inputs cannot contain symbolic links"));
        }
    }
    let source = root.join(path);
    let metadata = fs::symlink_metadata(&source)?;
    let name = format!("benchmark/{}", portable(path)?);
    if super::platform_fs::is_real_directory(&metadata) {
        entries.insert(
            name.clone(),
            (BundleEntry::Directory { path: name }, source.clone()),
        );
        for child in fs::read_dir(source)? {
            collect(root, &path.join(child?.file_name()), entries)?;
        }
    } else {
        entries.insert(name.clone(), (file_entry(name, &source)?, source));
    }
    Ok(())
}

fn generated_entry(path: &str, bytes: &[u8]) -> BundleEntry {
    BundleEntry::File {
        path: path.into(),
        sha256: hex(Sha256::digest(bytes)),
        size_bytes: bytes.len() as u64,
        executable: false,
    }
}

fn verify_inputs(
    root: &Path,
    paths: &BTreeSet<PathBuf>,
    expected: &BTreeMap<String, (BundleEntry, PathBuf)>,
) -> io::Result<()> {
    let mut current = BTreeMap::new();
    for path in paths {
        collect(root, path, &mut current)?;
    }
    if &current != expected {
        return Err(invalid("input changed during bundling"));
    }
    Ok(())
}

pub fn bundle_repository(
    repository: &CheckedRepository,
    options: &BundleOptions,
) -> io::Result<BundleReport> {
    let _: OciImageReference =
        serde_json::from_value(json!(options.base_image)).map_err(invalid)?;
    if !exact_version(&options.runner_version) {
        return Err(invalid(
            "runner version must be an exact MAJOR.MINOR.PATCH release",
        ));
    }
    if repository
        .manifest
        .environments
        .values()
        .any(|e| !matches!(e, EnvironmentDefinition::Local { .. }))
    {
        return Err(invalid(
            "bundling requires local environments; supply a local-worker manifest and install its dependencies in the image",
        ));
    }
    if repository
        .manifest
        .datasets
        .values()
        .any(|d| matches!(d, DatasetDefinition::Generated(d) if d.source.is_some()))
    {
        return Err(invalid(
            "bundling requires local dataset inputs; vendor remote sources before bundling",
        ));
    }
    let destination_name = options
        .destination
        .file_name()
        .ok_or_else(|| invalid("destination must name a new directory"))?;
    let parent = fs::canonicalize(
        options
            .destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let destination = parent.join(destination_name);
    if destination.starts_with(&repository.root) {
        return Err(invalid(
            "bundle destination must be outside the benchmark root",
        ));
    }
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "bundle destination already exists",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut paths = BTreeSet::from([PathBuf::from(
        repository
            .manifest_path
            .file_name()
            .expect("manifest filename"),
    )]);
    paths.extend(repository.schema_files.iter().cloned());
    for definition in repository.manifest.problems.values() {
        if let Some(path) = definition.contract.reference_path() {
            paths.insert(path.to_path_buf());
        }
    }
    for contract in repository.problem_contracts.values() {
        for case in &contract.reference_cases {
            for input in case
                .dataset
                .iter()
                .chain([&case.result, &case.expected_metrics])
            {
                if let Some(path) = input.reference_path() {
                    paths.insert(path.to_path_buf());
                }
            }
        }
    }
    for patterns in repository.resources.source_bundles.keys() {
        let patterns = patterns
            .iter()
            .map(|p| serde_json::from_value(json!(portable(p)?)).map_err(invalid))
            .collect::<io::Result<Vec<RepositoryPath>>>()?;
        paths.extend(
            expand_source_bundle(&repository.root, &patterns)
                .map_err(invalid)?
                .paths()
                .iter()
                .cloned(),
        );
    }
    if !options.includes.is_empty() {
        paths.extend(
            expand_source_bundle(&repository.root, &options.includes)
                .map_err(invalid)?
                .paths()
                .iter()
                .cloned(),
        );
    }
    let mut inputs = BTreeMap::new();
    for path in &paths {
        collect(&repository.root, path, &mut inputs)?;
    }
    let manifest = format!(
        "benchmark/{}",
        portable(Path::new(
            repository
                .manifest_path
                .file_name()
                .expect("manifest filename")
        ))?
    );
    let mut generated = BTreeMap::<String, Vec<u8>>::new();
    let mut dockerfile = format!(
        "FROM {BUILDER_IMAGE} AS metewand-runner\nRUN cargo install --locked --version '={}' --bin metewand --root /opt/runner metewand\nFROM {}\nUSER 0:0\nCOPY --from=metewand-runner /opt/runner/bin/metewand /usr/local/bin/metewand\nCOPY [\"benchmark/\", \"/benchmark/\"]\nWORKDIR /benchmark\n",
        options.runner_version, options.base_image
    );
    if let Some(recipe) = &options.recipe {
        let mut checked = BTreeMap::new();
        collect(&repository.root, recipe.as_path(), &mut checked)?;
        let source = repository.root.join(recipe.as_path());
        if !fs::symlink_metadata(&source)?.is_file() {
            return Err(invalid("recipe must be a regular shell script"));
        }
        generated.insert("install.sh".into(), fs::read(source)?);
        dockerfile.push_str("COPY [\"install.sh\", \"/opt/metewand/install.sh\"]\nRUN [\"/bin/sh\", \"-eu\", \"/opt/metewand/install.sh\"]\n");
    }
    dockerfile.push_str(&format!(
        "COPY [\"bundle.json\", \"/opt/metewand/bundle.json\"]\nENTRYPOINT {}\nCMD []\n",
        json!([
            "/usr/local/bin/metewand",
            "run",
            "--manifest",
            format!("/{manifest}"),
            "--output-dir",
            "/output/run"
        ])
    ));
    generated.insert("Dockerfile".into(), dockerfile.into_bytes());
    generated.insert(
        ".dockerignore".into(),
        b"*\n!Dockerfile\n!.dockerignore\n!bundle.json\n!install.sh\n!benchmark/\n!benchmark/**\n"
            .to_vec(),
    );
    let mut inventory = inputs
        .iter()
        .map(|(p, (e, _))| (p.clone(), e.clone()))
        .collect::<BTreeMap<_, _>>();
    for (path, bytes) in &generated {
        inventory.insert(path.clone(), generated_entry(path, bytes));
    }
    let bundle = BundleManifest {
        schema_version: BUNDLE_VERSION,
        runner_version: options.runner_version.clone(),
        builder_image: BUILDER_IMAGE.into(),
        base_image: options.base_image.clone(),
        manifest,
        entries: inventory.into_values().collect(),
    };
    if !options.dry_run {
        let stage = tempfile::Builder::new()
            .prefix(".metewand-bundle-")
            .tempdir_in(&parent)?;
        fs::create_dir(stage.path().join("benchmark"))?;
        for (path, (entry, source)) in &inputs {
            let target = stage.path().join(path);
            match entry {
                BundleEntry::Directory { .. } => fs::create_dir_all(&target)?,
                BundleEntry::File { .. } => {
                    fs::create_dir_all(target.parent().expect("file parent"))?;
                    super::platform_fs::copy_owned_file(source, &target)?;
                    if &file_entry(path.clone(), &target)? != entry
                        || &file_entry(path.clone(), source)? != entry
                    {
                        return Err(invalid("input changed during bundling"));
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let executable = matches!(
                            entry,
                            BundleEntry::File {
                                executable: true,
                                ..
                            }
                        );
                        fs::set_permissions(
                            &target,
                            fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
                        )?;
                    }
                }
            }
        }
        let copied = check_repository(&stage.path().join(&bundle.manifest)).map_err(invalid)?;
        let current = check_repository(&repository.manifest_path).map_err(invalid)?;
        if copied.manifest_hash != repository.manifest_hash
            || copied
                .resources
                .records
                .keys()
                .ne(repository.resources.records.keys())
            || current
                .resources
                .records
                .keys()
                .ne(repository.resources.records.keys())
            || current.manifest_hash != repository.manifest_hash
        {
            return Err(invalid(
                "benchmark inputs or logical identities changed during bundling",
            ));
        }
        verify_inputs(&repository.root, &paths, &inputs)?;
        if let Some(recipe) = &options.recipe {
            let mut checked = BTreeMap::new();
            collect(&repository.root, recipe.as_path(), &mut checked)?;
            if fs::read(repository.root.join(recipe.as_path()))? != generated["install.sh"] {
                return Err(invalid("dependency recipe changed during bundling"));
            }
        }
        for (path, bytes) in generated {
            fs::write(stage.path().join(path), bytes)?;
        }
        let bytes = CanonicalValue::try_from(serde_json::to_value(&bundle).map_err(invalid)?)
            .map_err(invalid)?
            .to_canonical_bytes()
            .map_err(invalid)?;
        fs::write(stage.path().join("bundle.json"), bytes)?;
        super::publication::rename_no_replace(stage.path(), &destination)?;
    }
    Ok(BundleReport {
        command: "bundle",
        dry_run: options.dry_run,
        destination: destination.display().to_string(),
        bundle,
        next_steps: vec![
            "docker build -t metewand-benchmark .".into(),
            "mkdir -p output".into(),
            "docker run --rm --user \"$(id -u):$(id -g)\" --mount type=bind,src=\"$PWD/output\",dst=/output metewand-benchmark".into(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_changed_bytes_and_directory_membership() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("deps")).unwrap();
        fs::write(root.path().join("deps/lock"), "old").unwrap();
        let paths = BTreeSet::from([PathBuf::from("deps")]);
        let mut expected = BTreeMap::new();
        collect(root.path(), Path::new("deps"), &mut expected).unwrap();
        fs::write(root.path().join("deps/lock"), "new").unwrap();
        assert!(verify_inputs(root.path(), &paths, &expected).is_err());
        fs::write(root.path().join("deps/lock"), "old").unwrap();
        fs::write(root.path().join("deps/added"), "extra").unwrap();
        assert!(verify_inputs(root.path(), &paths, &expected).is_err());
        fs::remove_file(root.path().join("deps/added")).unwrap();
        verify_inputs(root.path(), &paths, &expected).unwrap();
    }
}
