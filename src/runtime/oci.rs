//! Digest-pinned image evidence and bounded Docker lifecycle operations.

use crate::core::lockfile::LockedOciEnvironment;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) const IMAGE_ENV: &str = "METEWAND_OCI_IMAGE";

pub(crate) trait DockerBackend {
    fn invoke(&self, arguments: &[String]) -> io::Result<Vec<u8>>;
}

pub(crate) struct Docker;
impl DockerBackend for Docker {
    fn invoke(&self, arguments: &[String]) -> io::Result<Vec<u8>> {
        let output = tempfile::tempfile()?;
        let errors = tempfile::tempfile()?;
        let mut child = Command::new("docker")
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(errors.try_clone()?)
            .spawn()?;
        let limit = match arguments.first().map(String::as_str) {
            Some("pull") => 120,
            Some("kill" | "rm" | "inspect") => 2,
            _ => 10,
        };
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                use std::io::{Read, Seek};
                let mut file = if status.success() { output } else { errors };
                file.rewind()?;
                let mut bytes = Vec::new();
                file.take(1_048_576).read_to_end(&mut bytes)?;
                return if status.success() {
                    Ok(bytes)
                } else {
                    Err(io::Error::other(format!(
                        "Docker command failed: {}",
                        String::from_utf8_lossy(&bytes)
                    )))
                };
            }
            if start.elapsed() > Duration::from_secs(limit) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Docker command exceeded its deadline",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub(crate) fn invoke(args: &[&str]) -> io::Result<Vec<u8>> {
    Docker.invoke(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
}

pub(crate) fn preflight() -> io::Result<Value> {
    if !cfg!(target_os = "linux") {
        return Err(io::Error::other(
            "OCI execution requires a local Linux Docker Engine",
        ));
    }
    let contexts: Value = serde_json::from_slice(&invoke(&["context", "inspect"])?)?;
    let endpoint = std::env::var("DOCKER_HOST")
        .ok()
        .filter(|_| std::env::var_os("DOCKER_CONTEXT").is_none())
        .unwrap_or_else(|| {
            contexts[0]["Endpoints"]["docker"]["Host"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        });
    if !endpoint.starts_with("unix://") {
        return Err(io::Error::other(
            "OCI execution requires a local Unix Docker socket",
        ));
    }
    let info: Value = serde_json::from_slice(&invoke(&["info", "--format", "{{json .}}"])?)?;
    if info["OSType"] != "linux"
        || info["SecurityOptions"].as_array().is_some_and(|opts| {
            opts.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.contains("rootless") || s.contains("userns"))
            })
        })
    {
        return Err(io::Error::other(
            "OCI execution requires rootful Linux Docker without user namespace remapping",
        ));
    }
    Ok(
        serde_json::json!({"server_version":info["ServerVersion"],"architecture":info["Architecture"],"security_options":info["SecurityOptions"],"network":"host","uid":unsafe {libc::getuid()},"gid":unsafe {libc::getgid()}}),
    )
}

pub(crate) fn platform() -> io::Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return Err(io::Error::other("unsupported native OCI architecture")),
    };
    Ok(format!("linux/{arch}"))
}

pub(crate) fn inspect(
    backend: &dyn DockerBackend,
    image: &str,
    definition: &str,
    source: &str,
) -> io::Result<LockedOciEnvironment> {
    let system = platform()?;
    let value: Value = serde_json::from_slice(&backend.invoke(&[
        "image".into(),
        "inspect".into(),
        image.into(),
    ])?)?;
    let image_value = &value[0];
    if format!(
        "{}/{}",
        image_value["Os"].as_str().unwrap_or(""),
        image_value["Architecture"].as_str().unwrap_or("")
    ) != system
    {
        return Err(io::Error::other(
            "OCI image platform differs from the native host platform",
        ));
    }
    if !image_value["RepoDigests"].as_array().is_some_and(|refs| {
        refs.iter().any(|v| {
            v.as_str().is_some_and(|r| {
                r.rsplit_once('@').map(|(_, d)| d) == image.rsplit_once('@').map(|(_, d)| d)
            })
        })
    }) {
        return Err(io::Error::other(
            "Docker image does not retain the requested registry digest",
        ));
    }
    let mut environment = BTreeMap::new();
    for entry in image_value["Config"]["Env"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let (key, val) = entry
            .as_str()
            .and_then(|v| v.split_once('='))
            .ok_or_else(|| io::Error::other("invalid image environment"))?;
        if key.starts_with("METEWAND_") {
            return Err(io::Error::other(
                "image environment overrides a runtime-owned variable",
            ));
        }
        environment.insert(key.to_owned(), val.to_owned());
    }
    Ok(LockedOciEnvironment {
        definition: definition.into(),
        system,
        source_sha256: source.into(),
        image: image.into(),
        image_id: image_value["Id"]
            .as_str()
            .ok_or_else(|| io::Error::other("missing image ID"))?
            .into(),
        layers: serde_json::from_value(image_value["RootFS"]["Layers"].clone())?,
        environment,
    })
}

pub(crate) fn verify(entry: &LockedOciEnvironment) -> io::Result<()> {
    preflight()?;
    let actual = inspect(
        &Docker,
        &entry.image,
        &entry.definition,
        &entry.source_sha256,
    )?;
    if serde_json::to_value(actual)? != serde_json::to_value(entry)? {
        return Err(io::Error::other(
            "prepared OCI image differs from metewand.lock",
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Container {
    id: String,
    owner: String,
}
impl Container {
    pub(crate) fn signal(&self, signal: &str) -> io::Result<()> {
        invoke(&["kill", "--signal", signal, &self.id]).map(|_| ())
    }
    pub(crate) fn status(&self) -> io::Result<std::process::ExitStatus> {
        use std::os::unix::process::ExitStatusExt;
        let value: Value = serde_json::from_slice(&invoke(&[
            "inspect",
            "--format",
            "{{json .State}}",
            &self.id,
        ])?)?;
        if value["Running"] == true {
            return Err(io::Error::other(
                "Docker client exited while its container is still running",
            ));
        }
        let code = value["ExitCode"]
            .as_i64()
            .ok_or_else(|| io::Error::other("missing container exit status"))?;
        Ok(std::process::ExitStatus::from_raw((code as i32) << 8))
    }
}
impl Drop for Container {
    fn drop(&mut self) {
        // A create timeout can leave only the reserved name. Check ownership
        // before removing it, including the unlikely case of a name collision.
        if let Ok(owner) = invoke(&[
            "inspect",
            "--format",
            "{{index .Config.Labels \"org.metewand.owner\"}}",
            &self.id,
        ]) && owner.strip_suffix(b"\n").unwrap_or(&owner) == self.owner.as_bytes()
        {
            let _ = invoke(&["rm", "--force", "--volumes", &self.id]);
        }
    }
}

pub(crate) fn attach(command: &mut Command) -> io::Result<Option<Container>> {
    let env: BTreeMap<String, String> = command
        .get_envs()
        .filter_map(|(k, v)| {
            v.map(|v| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    let Some(image) = env.get(IMAGE_ENV).cloned() else {
        return Ok(None);
    };
    let args: Vec<String> = command
        .get_args()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    let root = env
        .get("METEWAND_OCI_ROOT")
        .ok_or_else(|| io::Error::other("missing container staging root"))?;
    let work = command
        .get_current_dir()
        .unwrap_or(Path::new(root))
        .to_string_lossy()
        .into_owned();
    // Docker mount values use CSV quoting, independently of shell quoting.
    let mount = format!(
        "type=bind,\"source={}\",\"target={}\"",
        root.replace('"', "\"\""),
        root.replace('"', "\"\"")
    );
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(io::Error::other)?;
    let owner = crate::runtime::language_adapter::hex(nonce);
    let mut container = Container {
        id: format!("metewand-{owner}"),
        owner,
    };
    let mut create = vec![
        "create".into(),
        "--name".into(),
        container.id.clone(),
        "--label".into(),
        format!("org.metewand.owner={}", container.owner),
        "--pull=never".into(),
        "--network=host".into(),
        "--init".into(),
        "--no-healthcheck".into(),
        "--restart=no".into(),
        "--user".into(),
        format!("{}:{}", unsafe { libc::getuid() }, unsafe {
            libc::getgid()
        }),
        "--mount".into(),
        mount,
        "--workdir".into(),
        work,
        "--entrypoint".into(),
        command.get_program().to_string_lossy().into_owned(),
    ];
    if let Some(source_root) = env.get("METEWAND_OCI_SOURCE_ROOT")
        && source_root != root
    {
        create.extend([
            "--mount".into(),
            format!(
                "type=bind,\"source={}\",\"target={}\"",
                source_root.replace('"', "\"\""),
                source_root.replace('"', "\"\"")
            ),
        ]);
    }
    let mut environment_file = tempfile::NamedTempFile::new()?;
    use std::io::Write;
    for (k, v) in env {
        if k != IMAGE_ENV && k != "METEWAND_OCI_ROOT" && k != "METEWAND_OCI_SOURCE_ROOT" {
            if v.contains(['\n', '\r']) {
                return Err(io::Error::other(
                    "OCI environment values cannot contain newlines",
                ));
            }
            writeln!(environment_file, "{k}={v}")?;
        }
    }
    environment_file.flush()?;
    create.extend([
        "--env-file".into(),
        environment_file.path().to_string_lossy().into_owned(),
        "--label".into(),
        "org.metewand.worker=true".into(),
    ]);
    create.push(image);
    create.extend(args);
    let id = String::from_utf8(Docker.invoke(&create)?)
        .map_err(io::Error::other)?
        .trim()
        .to_owned();
    if id.len() != 64 || !id.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid Docker container ID"));
    }
    container.id = id;
    *command = Command::new("docker");
    command.args(["start", "--attach", &container.id]);
    Ok(Some(container))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    struct Fake {
        image: Value,
        calls: RefCell<Vec<Vec<String>>>,
    }
    impl DockerBackend for Fake {
        fn invoke(&self, args: &[String]) -> io::Result<Vec<u8>> {
            self.calls.borrow_mut().push(args.to_vec());
            Ok(serde_json::to_vec(&serde_json::json!([self.image]))?)
        }
    }
    fn fixture() -> Fake {
        Fake {
            image: serde_json::json!({"Os":"linux","Architecture":platform().unwrap().split('/').nth(1),"Id":format!("sha256:{}","1".repeat(64)),"RepoDigests":[format!("example@sha256:{}","0".repeat(64))],"RootFS":{"Layers":[]},"Config":{"Env":["PATH=/bin","LANG=C.UTF-8"]}}),
            calls: RefCell::new(vec![]),
        }
    }
    #[test]
    fn image_verification_inspects_without_preparing_or_launching() {
        let backend = fixture();
        let entry = inspect(
            &backend,
            &format!("example@sha256:{}", "0".repeat(64)),
            "definition",
            "sources",
        )
        .unwrap();
        assert_eq!(entry.environment["PATH"], "/bin");
        assert_eq!(backend.calls.borrow()[0][..2], ["image", "inspect"]);
        assert_eq!(backend.calls.borrow().len(), 1);
    }
    #[test]
    fn image_evidence_rejects_platform_digest_and_reserved_environment_mismatches() {
        for field in ["platform", "digest", "environment"] {
            let mut backend = fixture();
            match field {
                "platform" => backend.image["Os"] = Value::from("windows"),
                "digest" => backend.image["RepoDigests"] = serde_json::json!([]),
                _ => {
                    backend.image["Config"]["Env"] =
                        serde_json::json!(["METEWAND_PROTOCOL_TOKEN=override"])
                }
            }
            assert!(
                inspect(
                    &backend,
                    &format!("example@sha256:{}", "0".repeat(64)),
                    "definition",
                    "sources"
                )
                .is_err()
            );
        }
    }
}
