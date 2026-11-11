//! Local SIF verification and foreground Apptainer execution.

use crate::core::lockfile::LockedApptainerEnvironment;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) const IMAGE_ENV: &str = "METEWAND_APPTAINER_IMAGE";

fn invoke(args: &[&str]) -> io::Result<String> {
    let output = tempfile::tempfile()?;
    let errors = tempfile::tempfile()?;
    let mut child = Command::new("apptainer")
        .args(args)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(errors.try_clone()?)
        .spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            use std::io::Seek;
            let mut file = if status.success() { output } else { errors };
            file.rewind()?;
            let mut bytes = Vec::new();
            file.take(1_048_576).read_to_end(&mut bytes)?;
            let text = String::from_utf8(bytes).map_err(io::Error::other)?;
            return if status.success() {
                Ok(text)
            } else {
                Err(io::Error::other(format!(
                    "Apptainer command failed: {text}"
                )))
            };
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Apptainer command exceeded its deadline",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn preflight() -> io::Result<String> {
    let version = invoke(&["--version"])?;
    let number = version
        .split_whitespace()
        .find(|s| s.starts_with(|c: char| c.is_ascii_digit()))
        .ok_or_else(|| io::Error::other("unrecognized Apptainer version"))?;
    let mut parts = number.split('.');
    let major = parts.next().unwrap_or("").parse::<u32>().unwrap_or(0);
    let minor = parts.next().unwrap_or("").parse::<u32>().unwrap_or(0);
    if major < 1 || (major == 1 && minor < 3) {
        return Err(io::Error::other("Apptainer 1.3 or newer is required"));
    }
    Ok(version.trim().into())
}

pub(crate) fn image_path(root: &Path, image: &str) -> io::Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(image).components() {
        let std::path::Component::Normal(part) = component else {
            return Err(io::Error::other("SIF image must be repository-relative"));
        };
        path.push(part);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(io::Error::other("SIF image paths cannot contain symlinks"));
        }
    }
    if !path.is_file() {
        return Err(io::Error::other("SIF image must be a regular file"));
    }
    Ok(path)
}

fn digest(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(crate::runtime::language_adapter::hex(hash.finalize()))
}

pub(crate) fn prepare(
    root: &Path,
    image: &str,
    definition: &str,
    source: &str,
) -> io::Result<LockedApptainerEnvironment> {
    preflight()?;
    prepare_with(root, image, definition, source, invoke)
}

fn prepare_with(
    root: &Path,
    image: &str,
    definition: &str,
    source: &str,
    invoke: impl Fn(&[&str]) -> io::Result<String>,
) -> io::Result<LockedApptainerEnvironment> {
    let path = image_path(root, image)?;
    let path_text = path
        .to_str()
        .ok_or_else(|| io::Error::other("SIF path must be UTF-8"))?;
    let before = digest(&path)?;
    let header = invoke(&["sif", "header", path_text])?;
    let list = invoke(&["sif", "list", path_text])?;
    let system = crate::runtime::oci::platform()?;
    let arch = system.split('/').nth(1).unwrap_or("");
    if !header.lines().any(|line| {
        line.trim()
            .strip_prefix("Primary Architecture:")
            .is_some_and(|v| v.trim() == arch)
    }) || !list.contains(&format!("(Squashfs/*System/{arch})"))
    {
        return Err(io::Error::other(
            "SIF requires a native unencrypted SquashFS primary system partition",
        ));
    }
    if digest(&path)? != before {
        return Err(io::Error::other("SIF image changed during verification"));
    }
    Ok(LockedApptainerEnvironment {
        definition: definition.into(),
        system,
        source_sha256: source.into(),
        image: image.into(),
        image_sha256: before,
    })
}

pub(crate) fn verify(root: &Path, entry: &LockedApptainerEnvironment) -> io::Result<()> {
    let actual = prepare(root, &entry.image, &entry.definition, &entry.source_sha256)?;
    if serde_json::to_value(actual)? != serde_json::to_value(entry)? {
        return Err(io::Error::other("SIF image differs from metewand.lock"));
    }
    Ok(())
}

pub(crate) fn attach(command: &mut Command) -> io::Result<bool> {
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
    let Some(image) = env.get(IMAGE_ENV) else {
        return Ok(false);
    };
    let root = env
        .get("METEWAND_OCI_ROOT")
        .ok_or_else(|| io::Error::other("missing container staging root"))?;
    let mut wrapper = Command::new("apptainer");
    wrapper.args([
        "exec",
        "--containall",
        "--cleanenv",
        "--no-eval",
        "--disable-cache",
        "--no-mount",
        "hostfs,bind-paths,cwd",
    ]);
    for mount in [
        Some(root),
        env.get("METEWAND_OCI_SOURCE_ROOT").filter(|p| *p != root),
    ]
    .into_iter()
    .flatten()
    {
        wrapper.arg("--mount").arg(format!(
            "type=bind,\"src={}\",\"dst={}\"",
            mount.replace('"', "\"\""),
            mount.replace('"', "\"\"")
        ));
    }
    if let Some(work) = command.get_current_dir() {
        wrapper.arg("--pwd").arg(work).current_dir(work);
    }
    wrapper
        .arg(image)
        .arg(command.get_program())
        .args(command.get_args())
        .env_clear();
    // Host helpers need the host search path; cleanenv keeps it out of the image.
    if let Some(path) = std::env::var_os("PATH") {
        wrapper.env("PATH", path);
    }
    for (key, value) in env {
        if key == IMAGE_ENV || key == "METEWAND_OCI_ROOT" || key == "METEWAND_OCI_SOURCE_ROOT" {
            continue;
        }
        if key == "HOME" || key == "TMPDIR" {
            wrapper.env(&key, &value);
        }
        wrapper.env(format!("APPTAINERENV_{key}"), value);
    }
    *command = wrapper;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sif_metadata_verification_never_executes_or_prepares_images() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("worker.sif"), b"fixture image bytes").unwrap();
        let arch = crate::runtime::oci::platform()
            .unwrap()
            .split('/')
            .nth(1)
            .unwrap()
            .to_owned();
        let calls = std::cell::RefCell::new(Vec::new());
        let entry = prepare_with(root.path(), "worker.sif", "definition", "source", |args| {
            calls.borrow_mut().push(
                args[..2]
                    .iter()
                    .map(|v| (*v).to_owned())
                    .collect::<Vec<_>>(),
            );
            Ok(if args[1] == "header" {
                format!("Primary Architecture: {arch}")
            } else {
                format!("1 |FS (Squashfs/*System/{arch})")
            })
        })
        .unwrap();
        assert_eq!(
            *calls.borrow(),
            [vec!["sif", "header"], vec!["sif", "list"]]
        );
        assert_eq!(
            entry.image_sha256,
            digest(&root.path().join("worker.sif")).unwrap()
        );
        assert!(prepare_with(root.path(), "worker.sif", "", "", |_| Ok("invalid".into())).is_err());
        assert!(
            prepare_with(root.path(), "missing.sif", "", "", |_| panic!(
                "missing files must fail before invocation"
            ))
            .is_err()
        );
        for partition in ["EncryptedSquashfs/*System", "Ext3/*System", "Squashfs/Data"] {
            assert!(
                prepare_with(root.path(), "worker.sif", "", "", |args| Ok(
                    if args[1] == "header" {
                        format!("Primary Architecture: {arch}")
                    } else {
                        format!("1 |FS ({partition}/{arch})")
                    }
                ))
                .is_err()
            );
        }
        assert!(
            prepare_with(root.path(), "worker.sif", "", "", |args| {
                if args[1] == "list" {
                    fs::write(root.path().join("worker.sif"), b"mutated")?;
                }
                Ok(if args[1] == "header" {
                    format!("Primary Architecture: {arch}")
                } else {
                    format!("1 |FS (Squashfs/*System/{arch})")
                })
            })
            .is_err()
        );
        std::os::unix::fs::symlink(root.path().join("worker.sif"), root.path().join("link.sif"))
            .unwrap();
        assert!(image_path(root.path(), "link.sif").is_err());
        assert!(image_path(root.path(), "../worker.sif").is_err());
    }

    #[test]
    fn wrapper_preserves_literals_and_removes_internal_settings() {
        let mut command = Command::new("python3");
        command
            .arg("literal $(date), 'quoted'")
            .current_dir("/staging/worker")
            .env(IMAGE_ENV, "/images/worker.sif")
            .env("METEWAND_OCI_ROOT", "/staging,quoted\"path")
            .env("TOKEN", "$(date)\nsecret")
            .env("METEWAND_PROTOCOL_TOKEN", "token");
        assert!(attach(&mut command).unwrap());
        let args = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args.last().unwrap(), "literal $(date), 'quoted'");
        assert!(args.contains(&"--no-eval".into()));
        let env = command.get_envs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            env[std::ffi::OsStr::new("APPTAINERENV_TOKEN")].unwrap(),
            "$(date)\nsecret"
        );
        assert!(!env.contains_key(std::ffi::OsStr::new(IMAGE_ENV)));
    }
}
