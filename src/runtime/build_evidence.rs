//! Build-time claims are accepted only for the executable bytes they describe.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use crate::core::{
    canonical::CanonicalValue,
    public_schemas::{PublicSchema, public_schema_catalog},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Hashes the executable and attaches an available, matching build receipt.
///
/// A missing or stale receipt never borrows compiler information from the host.
/// The wrapper's claims describe a local build, not an authenticated attestation.
///
/// # Errors
/// Returns an I/O error if the executable itself cannot be read.
pub fn executable_evidence(executable: &Path) -> io::Result<Value> {
    let mut hash = Sha256::new();
    let mut file = File::open(executable)?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let mut sidecar = executable.as_os_str().to_os_string();
    sidecar.push(".metewand-build.json");
    let build = match read_receipt(Path::new(&sidecar)) {
        Ok(evidence) if evidence["executable_sha256"] == digest => {
            json!({"status": "available", "evidence": evidence})
        }
        Ok(_) => unavailable("executable_hash_mismatch"),
        Err(reason) => unavailable(reason),
    };
    Ok(json!({"executable_sha256": digest, "build": build}))
}

fn read_receipt(path: &Path) -> Result<Value, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            "not_recorded_at_build"
        } else {
            "unreadable_receipt"
        }
    })?;
    if !metadata.is_file() {
        return Err("unreadable_receipt");
    }
    let limit = 4 * 1024 * 1024;
    if metadata.len() > limit {
        return Err("receipt_too_large");
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|_| "unreadable_receipt")?;
    if bytes.len() as u64 > limit {
        return Err("receipt_too_large");
    }
    let evidence = CanonicalValue::from_slice(&bytes)
        .map_err(|_| "invalid_receipt")?
        .as_json()
        .clone();
    public_schema_catalog()
        .map_err(|_| "invalid_receipt")?
        .validate(PublicSchema::BuildEvidence.repository_path(), &evidence)
        .map_err(|_| "invalid_receipt")?;
    Ok(evidence)
}

fn unavailable(reason: &str) -> Value {
    json!({"status": "unavailable", "reason": reason})
}

pub(crate) fn host_observation() -> Value {
    let read = |path| {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_owned())
    };
    json!({
        "phase": "run_preflight",
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "kernel": read("/proc/sys/kernel/osrelease"),
        "cpu_model": read("/proc/cpuinfo").and_then(|s| s.lines().find_map(|line| line.strip_prefix("model name").and_then(|s| s.split_once(':')).map(|(_, v)| v.trim().to_owned()))),
        "memory": read("/proc/meminfo").and_then(|s| s.lines().find(|line| line.starts_with("MemTotal:")).map(str::to_owned)),
    })
}
