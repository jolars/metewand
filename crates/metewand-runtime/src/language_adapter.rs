//! CLI-owned language adapters behind the stable binding API.

use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Compatibility version of the author-facing callback registration API.
pub const BINDING_API_VERSION: u32 = 1;

const PYTHON: &[(&str, &str)] = &[
    (
        "__init__.py",
        include_str!("../adapters/python/metewand_adapter/__init__.py"),
    ),
    (
        "_artifacts.py",
        include_str!("../adapters/python/metewand_adapter/_artifacts.py"),
    ),
    (
        "_buffered.py",
        include_str!("../adapters/python/metewand_adapter/_buffered.py"),
    ),
    (
        "_canonical.py",
        include_str!("../adapters/python/metewand_adapter/_canonical.py"),
    ),
    (
        "_worker.py",
        include_str!("../adapters/python/metewand_adapter/_worker.py"),
    ),
    (
        "_bindings.py",
        include_str!("../adapters/python/metewand_adapter/_bindings.py"),
    ),
];
const R: &[(&str, &str)] = &[("adapter.R", include_str!("../adapters/r/adapter.R"))];

fn files(language: &str) -> io::Result<&'static [(&'static str, &'static str)]> {
    match language {
        "python" => Ok(PYTHON),
        "r" => Ok(R),
        _ => Err(io::Error::other("unsupported binding language")),
    }
}

/// Returns the path-independent hash of all adapter code for a language.
///
/// # Errors
/// Returns an error for an unsupported language.
pub fn digest(language: &str) -> io::Result<String> {
    let mut hash = Sha256::new();
    hash.update(BINDING_API_VERSION.to_be_bytes());
    for (name, code) in files(language)? {
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update((code.len() as u64).to_be_bytes());
        hash.update(code.as_bytes());
    }
    Ok(hex(hash.finalize()))
}

/// Stages a bundled adapter beneath a private worker directory.
///
/// # Errors
/// Returns an error for an unsupported language or failed filesystem write.
pub fn stage(language: &str, directory: &Path) -> io::Result<PathBuf> {
    let directory = directory.join("adapter").join(language);
    fs::create_dir_all(&directory)?;
    for (name, code) in files(language)? {
        fs::write(directory.join(name), code)?;
    }
    Ok(directory.join(if language == "r" {
        "adapter.R"
    } else {
        "__init__.py"
    }))
}

pub(crate) fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
