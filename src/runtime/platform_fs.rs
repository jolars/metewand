//! Host filesystem operations and the evidence their storage contract supports.

use std::{
    fs::{self, File, Metadata, OpenOptions},
    io,
    path::{Component, Path},
};

use serde::Serialize;
use tempfile::{Builder, TempDir};

/// Storage operations performed before accepting an attempt.
///
/// Neither profile promises recovery of compound publications after power loss.
/// Historical provenance without this evidence remains unspecified.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct StorageEvidence {
    pub backend: &'static str,
    pub crash_recovery: &'static str,
    pub file_sync: &'static str,
    pub namespace_sync: &'static str,
}

pub(crate) const fn storage_evidence() -> StorageEvidence {
    StorageEvidence {
        backend: if cfg!(windows) {
            "windows_ntfs_process_recovery"
        } else {
            "unix_file_and_directory_sync"
        },
        crash_recovery: "process_termination",
        file_sync: if cfg!(windows) {
            "flush_file_buffers"
        } else {
            "fsync"
        },
        namespace_sync: if cfg!(windows) {
            "write_through_rename"
        } else {
            "directory_fsync"
        },
    }
}

pub(crate) fn is_real_directory(metadata: &Metadata) -> bool {
    metadata.is_dir() && !is_reparse_point(metadata)
}

pub(crate) fn is_regular_file(metadata: &Metadata) -> bool {
    metadata.is_file() && !is_reparse_point(metadata)
}

#[cfg(unix)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    metadata.is_symlink()
}

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

pub(crate) fn open_regular(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !is_regular_file(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file without links",
        ));
    }
    Ok(file)
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW).bits() as i32,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !is_real_directory(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a directory without links",
        ));
    }
    Ok(file)
}

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().mode(0o700).create(path)?;
        // A restrictive ambient umask must not make the owned directory unusable.
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        windows::create_private_directory(path)
    }
}

pub(crate) fn set_private_directory(path: &Path) -> io::Result<()> {
    if !is_real_directory(&fs::symlink_metadata(path)?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory must not be a link",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        windows::set_private_directory(path)
    }
}

/// Checks the same protection applied when allocating private worker storage.
pub(crate) fn verify_private_directory(path: &Path) -> io::Result<()> {
    let directory = open_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if directory.metadata()?.permissions().mode() & 0o777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private directory permissions changed",
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    windows::verify_private_directory(&directory)
}

/// Allocates an empty directory and protects it before callers populate it.
/// The parent must be a Metewand-owned private directory so its inherited ACL
/// already prevents access during allocation on Windows.
pub(crate) fn private_tempdir_in(parent: &Path, prefix: &str) -> io::Result<TempDir> {
    let mut builder = Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir_in(parent)?;
    set_private_directory(directory.path())?;
    Ok(directory)
}

#[cfg(unix)]
pub(crate) fn is_executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
pub(crate) fn is_executable(_: &Metadata) -> bool {
    // Version-1 local-tree identities record no executable bit on Windows.
    false
}

pub(crate) fn set_executable(path: &Path, executable: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
        )
    }
    #[cfg(windows)]
    {
        let _ = executable;
        make_owned_file_writable(path)
    }
}

/// Copies a file into newly owned storage without importing Windows attributes
/// or ACLs that could make the private copy unwritable or expose it to others.
pub(crate) fn copy_owned_file(source: &Path, destination: &Path) -> io::Result<u64> {
    #[cfg(unix)]
    return fs::copy(source, destination);
    #[cfg(windows)]
    {
        let mut source = open_regular(source, OpenOptions::new().read(true))?;
        let mut destination =
            open_regular(destination, OpenOptions::new().write(true).create_new(true))?;
        io::copy(&mut source, &mut destination)
    }
}

#[cfg(windows)]
fn make_owned_file_writable(path: &Path) -> io::Result<()> {
    use std::{
        mem::size_of,
        os::windows::{
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        },
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_BASIC_INFO, FILE_READ_ATTRIBUTES,
        FILE_WRITE_ATTRIBUTES, FileBasicInfo, SetFileInformationByHandle,
    };

    // GENERIC_WRITE fails on a read-only file. Attribute access can clear that
    // flag through the verified no-reparse handle before opening it for flush.
    let file = open_regular(
        path,
        OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES),
    )?;
    let attributes = file.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_READONLY != 0 {
        let writable = attributes & !FILE_ATTRIBUTE_READONLY;
        let info = FILE_BASIC_INFO {
            // Zero means "leave unchanged" here, so an otherwise attribute-free
            // file needs NORMAL to clear its last existing attribute.
            FileAttributes: if writable == 0 {
                FILE_ATTRIBUTE_NORMAL
            } else {
                writable
            },
            ..FILE_BASIC_INFO::default()
        };
        // SAFETY: The no-reparse handle owns a regular file with attribute-write
        // access, and the initialized structure remains valid for the call.
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileBasicInfo,
                (&info as *const FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Converts native relative paths without admitting drive prefixes or traversal.
pub(crate) fn relative_path(path: &Path) -> io::Result<String> {
    let parts = path
        .components()
        .map(|component| {
            let Component::Normal(part) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "expected a normalized relative path",
                ));
            };
            let part = part
                .to_str()
                .filter(|part| !part.contains(['\\', '\0']))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "expected a UTF-8 relative path",
                    )
                })?;
            #[cfg(windows)]
            if !windows_component(part) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path contains an unsupported Windows filename",
                ));
            }
            Ok(part)
        })
        .collect::<io::Result<Vec<_>>>()?;
    if parts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "relative path must not be empty",
        ));
    }
    Ok(parts.join("/"))
}

/// Rejects names that cannot be unambiguously reopened through Windows paths.
pub(crate) fn validate_directory_names(directory: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let names = fs::read_dir(directory)?
            .map(|entry| {
                entry?.file_name().into_string().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "expected UTF-8 filenames")
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        validate_windows_names(names.iter().map(String::as_str))?;
    }
    #[cfg(unix)]
    let _ = directory;
    Ok(())
}

#[cfg(any(windows, test))]
fn validate_windows_names<'a>(names: impl IntoIterator<Item = &'a str>) -> io::Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        if !windows_component(name) || !seen.insert(name.to_uppercase()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory contains unsupported or case-colliding Windows filenames",
            ));
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn windows_component(part: &str) -> bool {
    if part.ends_with(['.', ' '])
        || part
            .chars()
            .any(|value| value <= '\u{1f}' || "<>:\"|?*".contains(value))
    {
        return false;
    }
    let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

/// Finishes a namespace change according to the selected platform contract.
/// Windows relies on completed NTFS operations for process-death recovery. This
/// deliberately does not claim to flush a directory or a whole volume.
pub(crate) fn complete_namespace_change(directory: &Path) -> io::Result<()> {
    let directory = open_directory(directory)?;
    #[cfg(unix)]
    return crate::runtime::durability::sync_all(&directory);
    #[cfg(windows)]
    {
        let _ = directory;
        #[cfg(test)]
        crate::runtime::fault_injection::hit("namespace_change.before");
        #[cfg(test)]
        crate::runtime::fault_injection::hit("namespace_change.after");
        Ok(())
    }
}

pub(crate) fn sync_owned_file(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        make_owned_file_writable(path)?;
        options.write(true);
    }
    crate::runtime::durability::sync_all(&open_regular(path, &mut options)?)
}

#[cfg(unix)]
pub(crate) fn file_signature(file: &File) -> io::Result<[u64; 7]> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok([
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
    ])
}

#[cfg(windows)]
pub(crate) fn file_signature(file: &File) -> io::Result<[u64; 7]> {
    windows::file_signature(file)
}

pub(crate) fn check_storage_root(path: &Path) -> io::Result<()> {
    let directory = open_directory(path)?;
    #[cfg(target_os = "linux")]
    {
        let kind = rustix::fs::fstatfs(&directory)
            .map_err(io::Error::from)?
            .f_type;
        if matches!(
            kind as u32,
            0x6969
                | 0x517b
                | 0xff534d42
                | 0xfe534d42
                | 0x5346414f
                | 0x6b414653
                | 0x00c36400
                | 0x73757245
                | 0x564c
                | 0x01021997
        ) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SQLite WAL on a network filesystem",
            ));
        }
    }
    #[cfg(windows)]
    windows::check_storage_root(path, &directory)?;
    let _ = directory;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    windows::rename_no_replace(source, destination)
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        mem::size_of,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, LocalFree},
        Security::{
            ACE_HEADER, ACL,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SE_FILE_OBJECT, SetSecurityInfo,
            },
            DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
            SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            CreateDirectoryW, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FileBasicInfo,
            FileIdInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetVolumeInformationByHandleW,
            GetVolumePathNameW, MOVEFILE_WRITE_THROUGH, MoveFileExW, READ_CONTROL, WRITE_DAC,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut text = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if text.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL",
            ));
        }
        text.push(0);
        Ok(text)
    }

    struct LocalAllocation(*mut core::ffi::c_void);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            // SAFETY: These allocations come from Win32 security conversion APIs.
            unsafe {
                LocalFree(self.0);
            }
        }
    }

    fn private_descriptor() -> io::Result<LocalAllocation> {
        let mut token = ptr::null_mut();
        // SAFETY: The output is initialized and the pseudo-process handle is valid.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: A successful call transferred ownership of a token handle.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut bytes = 0;
        // SAFETY: The first call queries the required aligned buffer size.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut bytes,
            );
        }
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        // SAFETY: The buffer is aligned and at least the queried number of bytes.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation initialized TOKEN_USER at this aligned address.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid = ptr::null_mut();
        // SAFETY: The token buffer retains the SID while its string is allocated.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid_allocation = LocalAllocation(sid.cast());
        let mut length = 0;
        // SAFETY: The conversion returned a NUL-terminated UTF-16 string.
        unsafe {
            while *sid.add(length) != 0 {
                length += 1;
            }
        }
        let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid, length) });
        drop(sid_allocation);
        let sddl = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid})\0")
            .encode_utf16()
            .collect::<Vec<_>>();
        let mut descriptor = ptr::null_mut();
        // SAFETY: The string remains valid while Win32 allocates the descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(LocalAllocation(descriptor))
    }

    pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
        let descriptor = private_descriptor()?;
        let path = wide(path)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: Path and security attributes outlive the call.
        if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn set_private_directory(path: &Path) -> io::Result<()> {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if !is_real_directory(&directory.metadata()?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "private directory must not be a link",
            ));
        }
        let descriptor = private_descriptor()?;
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        // SAFETY: The descriptor remains owned and outputs are initialized.
        if unsafe {
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if present == 0 || dacl.is_null() {
            return Err(io::Error::other("private directory descriptor has no DACL"));
        }
        // SAFETY: The no-reparse directory handle and descriptor stay valid.
        let result = unsafe {
            SetSecurityInfo(
                directory.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null(),
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        Ok(())
    }

    pub(super) fn verify_private_directory(directory: &File) -> io::Result<()> {
        let expected = private_descriptor()?;
        let mut actual = ptr::null_mut();
        // SAFETY: The owned directory handle is valid and the output is writable.
        let result = unsafe {
            GetSecurityInfo(
                directory.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut actual,
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let actual = LocalAllocation(actual);
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: GetSecurityInfo returned a valid descriptor retained above.
        if unsafe { GetSecurityDescriptorControl(actual.0, &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if control & SE_DACL_PROTECTED == 0
            || descriptor_aces(actual.0)? != descriptor_aces(expected.0)?
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private directory ACL changed",
            ));
        }
        Ok(())
    }

    fn descriptor_aces(descriptor: *mut core::ffi::c_void) -> io::Result<Vec<Vec<u8>>> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl: *mut ACL = ptr::null_mut();
        // SAFETY: The caller retains the descriptor for the duration of this function.
        if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        if present == 0 || acl.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private directory has no restricting DACL",
            ));
        }
        let mut aces = Vec::new();
        // SAFETY: The valid descriptor contains the ACL header and its entries.
        for index in 0..u32::from(unsafe { (*acl).AceCount }) {
            let mut ace = ptr::null_mut();
            // SAFETY: Each index is inside the ACL and the output is writable.
            if unsafe { GetAce(acl, index, &mut ace) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: GetAce returned an ACE whose header supplies its byte length.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    ace.cast::<u8>(),
                    usize::from((*ace.cast::<ACE_HEADER>()).AceSize),
                )
            };
            aces.push(bytes.to_vec());
        }
        aces.sort();
        Ok(aces)
    }

    pub(super) fn file_signature(file: &File) -> io::Result<[u64; 7]> {
        let mut id = FILE_ID_INFO::default();
        let mut basic = FILE_BASIC_INFO::default();
        // SAFETY: Both fixed-sized outputs are writable and the file is owned.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
            || unsafe {
                GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileBasicInfo,
                    (&mut basic as *mut FILE_BASIC_INFO).cast(),
                    size_of::<FILE_BASIC_INFO>() as u32,
                )
            } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok([
            id.VolumeSerialNumber,
            u64::from_le_bytes(id.FileId.Identifier[..8].try_into().expect("fixed file ID")),
            u64::from_le_bytes(id.FileId.Identifier[8..].try_into().expect("fixed file ID")),
            file.metadata()?.len(),
            basic.LastWriteTime as u64,
            basic.ChangeTime as u64,
            u64::from(basic.FileAttributes),
        ])
    }

    pub(super) fn check_storage_root(path: &Path, directory: &File) -> io::Result<()> {
        let path = wide(path)?;
        let mut volume = vec![0u16; 32768];
        // SAFETY: The output buffer is writable and includes space for NUL.
        if unsafe { GetVolumePathNameW(path.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        // DRIVE_FIXED and DRIVE_REMOVABLE both represent local volumes.
        if !matches!(unsafe { GetDriveTypeW(volume.as_ptr()) }, 2 | 3) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "storage requires a local NTFS volume",
            ));
        }
        let mut filesystem = [0u16; 32];
        // SAFETY: Only the filesystem-name output is requested.
        if unsafe {
            GetVolumeInformationByHandleW(
                directory.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                filesystem.as_mut_ptr(),
                filesystem.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let end = filesystem
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(filesystem.len());
        if String::from_utf16_lossy(&filesystem[..end]) != "NTFS" {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "storage requires a local NTFS volume",
            ));
        }
        Ok(())
    }

    pub(super) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
        let source = wide(source)?;
        let destination_wide = wide(destination)?;
        // Omitting COPY_ALLOWED keeps publication on one volume; omitting
        // REPLACE_EXISTING preserves both empty and populated competing objects.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error().map(|code| code as u32),
            Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS)
        ) || error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)
            && fs::symlink_metadata(destination).is_ok()
        {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, error));
        }
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_relative_paths_use_the_canonical_separator() {
        assert_eq!(
            relative_path(&Path::new("nested").join("data.json")).unwrap(),
            "nested/data.json"
        );
        assert!(relative_path(Path::new("../data.json")).is_err());
        assert!(relative_path(Path::new("")).is_err());
    }

    #[test]
    fn windows_components_reject_devices_streams_and_aliases() {
        for name in [
            "CON",
            "con.txt",
            "COM1.json",
            "LPT¹",
            "CONOUT$",
            "a:b",
            "tail.",
            "tail ",
            "wild*",
            "a\u{1f}b",
        ] {
            assert!(!windows_component(name), "{name:?}");
        }
        for name in ["data.json", "COM10", "CONSOLE", "a b", "café"] {
            assert!(windows_component(name), "{name:?}");
        }
        #[cfg(unix)]
        assert_eq!(relative_path(Path::new("a:b")).unwrap(), "a:b");
    }

    #[test]
    fn windows_directory_names_reject_case_collisions_without_normalizing_unicode() {
        assert!(validate_windows_names(["Data", "data"]).is_err());
        assert!(validate_windows_names(["café", "CAFÉ"]).is_err());
        assert!(validate_windows_names(["data", "NUL.txt"]).is_err());
        assert!(validate_windows_names(["data", "a:b"]).is_err());
        assert!(validate_windows_names(["data", "caf\u{e9}", "cafe\u{301}"]).is_ok());
    }

    #[test]
    fn private_directories_and_regular_opens_reject_wrong_kinds() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("private");
        create_private_directory(&directory).unwrap();
        set_private_directory(&directory).unwrap();
        verify_private_directory(&directory).unwrap();
        let path = directory.join("data");
        open_regular(&path, OpenOptions::new().write(true).create_new(true)).unwrap();
        assert!(open_regular(&directory, OpenOptions::new().read(true)).is_err());
        assert!(open_directory(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            symlink(&path, directory.join("link")).unwrap();
            assert!(open_regular(&directory.join("link"), OpenOptions::new().read(true)).is_err());
        }
    }

    #[test]
    fn file_signatures_distinguish_replaced_files_with_identical_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data");
        fs::write(&path, b"same bytes").unwrap();
        let original = open_regular(&path, OpenOptions::new().read(true)).unwrap();
        let before = file_signature(&original).unwrap();
        fs::rename(&path, directory.path().join("original")).unwrap();
        fs::write(&path, b"same bytes").unwrap();
        let replaced = open_regular(&path, OpenOptions::new().read(true)).unwrap();
        assert_ne!(file_signature(&replaced).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn readonly_owned_files_can_be_flushed_without_changing_their_identity() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_READONLY, SetFileAttributesW,
        };

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("payload");
        fs::write(&path, b"immutable bytes").unwrap();
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: The test owns this regular file and the path is NUL-terminated.
        assert_ne!(
            unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_READONLY) },
            0
        );
        let digest = crate::runtime::local_tree::hash_local_tree(directory.path()).unwrap();
        sync_owned_file(&path).unwrap();
        assert!(!fs::metadata(&path).unwrap().permissions().readonly());
        assert_eq!(
            crate::runtime::local_tree::hash_local_tree(directory.path()).unwrap(),
            digest
        );
        assert_eq!(fs::read(&path).unwrap(), b"immutable bytes");
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_verification_rejects_widened_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("private");
        create_private_directory(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(verify_private_directory(&path).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn private_directory_verification_rejects_unrestricted_acl() {
        use std::{os::windows::io::AsRawHandle, ptr};
        use windows_sys::Win32::Security::{
            Authorization::{SE_FILE_OBJECT, SetSecurityInfo},
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        };

        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("private");
        create_private_directory(&path).unwrap();
        verify_private_directory(&path).unwrap();
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, READ_CONTROL, WRITE_DAC,
        };
        let directory = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)
            .unwrap();
        // SAFETY: This test owns the empty directory and deliberately gives it a NULL DACL.
        let result = unsafe {
            SetSecurityInfo(
                directory.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert_eq!(result, 0);
        assert!(verify_private_directory(&path).is_err());
        set_private_directory(&path).unwrap();
        verify_private_directory(&path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn junctions_cannot_be_storage_roots_or_published_entries() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("outside target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"outside bytes").unwrap();
        let stage = root.path().join("staging");
        fs::create_dir(&stage).unwrap();
        let junction = stage.join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction creation failed: {output:?}"
        );
        assert!(!is_real_directory(
            &fs::symlink_metadata(&junction).unwrap()
        ));
        assert!(open_directory(&junction).is_err());
        assert!(set_private_directory(&junction).is_err());
        assert!(crate::runtime::run_store::RunStore::open(&junction).is_err());
        assert!(crate::runtime::artifact_store::ArtifactStore::open(&junction).is_err());
        assert!(crate::runtime::publication::prepare_directory(&stage).is_err());
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"outside bytes");
    }
}
