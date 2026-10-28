use std::{fs::File, io};

/// Keep each synchronization observable without changing production behavior.
pub(crate) fn sync_all(file: &File) -> io::Result<()> {
    #[cfg(windows)]
    if file.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the Windows storage profile does not provide directory synchronization",
        ));
    }
    #[cfg(test)]
    let kind = if file.metadata()?.is_dir() {
        "directory_sync"
    } else {
        "file_sync"
    };
    #[cfg(test)]
    crate::fault_injection::hit(&format!("{kind}.before"));
    let result = file.sync_all();
    #[cfg(test)]
    crate::fault_injection::hit(&format!("{kind}.after"));
    result
}
