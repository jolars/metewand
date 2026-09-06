use std::{fs, path::Path};

use metewand_core::records::ContentDigest;
use metewand_runtime::local_tree::{LocalTreeHashError, hash_local_tree};
use tempfile::TempDir;

fn write(root: &Path, path: &str, contents: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn hex(digest: &ContentDigest) -> String {
    digest
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn hashes_empty_directories_and_file_bytes_in_normalized_path_order() {
    let first = TempDir::new().unwrap();
    fs::create_dir(first.path().join("empty")).unwrap();
    write(first.path(), "src/z.txt", b"last\n");
    write(first.path(), "src/a.txt", b"first\0with binary");

    let second = TempDir::new().unwrap();
    write(second.path(), "src/a.txt", b"first\0with binary");
    write(second.path(), "src/z.txt", b"last\n");
    fs::create_dir(second.path().join("empty")).unwrap();

    assert_eq!(
        hash_local_tree(first.path()).unwrap(),
        hash_local_tree(second.path()).unwrap()
    );
}

#[test]
fn has_a_golden_portable_version_one_digest() {
    let tree = TempDir::new().unwrap();
    fs::create_dir(tree.path().join("empty")).unwrap();
    write(tree.path(), "data.bin", b"\0data");

    assert_eq!(
        hex(&hash_local_tree(tree.path()).unwrap()),
        "bcf747bafaea76059fbc0c7e16a8e5be01f94ecbce155a85db1c99458293b848"
    );
}

#[test]
fn every_semantic_tree_change_changes_the_digest() {
    fn digest(mut build: impl FnMut(&Path)) -> ContentDigest {
        let tree = TempDir::new().unwrap();
        build(tree.path());
        hash_local_tree(tree.path()).unwrap()
    }

    let baseline = digest(|root| write(root, "entry", b"contents"));
    let changed_bytes = digest(|root| write(root, "entry", b"changed"));
    let changed_path = digest(|root| write(root, "renamed", b"contents"));
    let changed_type = digest(|root| fs::create_dir(root.join("entry")).unwrap());

    assert_ne!(baseline, changed_bytes);
    assert_ne!(baseline, changed_path);
    assert_ne!(baseline, changed_type);
}

#[test]
fn excludes_ordinary_permissions_timestamps_and_root_spelling() {
    let parent = TempDir::new().unwrap();
    let first = parent.path().join("first");
    let second = parent.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    write(&first, "data.txt", b"same");
    write(&second, "data.txt", b"same");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(first.join("data.txt"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(second.join("data.txt"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&first, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&second, fs::Permissions::from_mode(0o755)).unwrap();
    }

    assert_eq!(
        hash_local_tree(&first).unwrap(),
        hash_local_tree(parent.path().join("first/.")).unwrap()
    );
    assert_eq!(
        hash_local_tree(&first).unwrap(),
        hash_local_tree(&second).unwrap()
    );
}

#[test]
fn reports_missing_and_non_directory_roots() {
    let parent = TempDir::new().unwrap();
    let missing = parent.path().join("missing");
    assert!(matches!(
        hash_local_tree(&missing),
        Err(LocalTreeHashError::Root { .. })
    ));

    let file = parent.path().join("file");
    fs::write(&file, b"contents").unwrap();
    assert!(matches!(
        hash_local_tree(&file),
        Err(LocalTreeHashError::RootNotDirectory { .. })
    ));
}

#[cfg(unix)]
mod unix {
    use std::{
        ffi::OsString,
        os::unix::{
            ffi::OsStringExt,
            fs::{PermissionsExt, symlink},
            net::UnixListener,
        },
    };

    use super::*;

    #[test]
    fn hashes_exact_symlink_targets_without_following_them() {
        fn digest(target: &str) -> ContentDigest {
            let tree = TempDir::new().unwrap();
            symlink(target, tree.path().join("link")).unwrap();
            hash_local_tree(tree.path()).unwrap()
        }

        assert_eq!(digest("missing"), digest("missing"));
        assert_ne!(digest("missing"), digest("./missing"));
    }

    #[test]
    fn only_the_presence_of_a_regular_file_executable_bit_is_semantic() {
        fn digest(mode: u32) -> ContentDigest {
            let tree = TempDir::new().unwrap();
            write(tree.path(), "program", b"contents");
            fs::set_permissions(
                tree.path().join("program"),
                fs::Permissions::from_mode(mode),
            )
            .unwrap();
            hash_local_tree(tree.path()).unwrap()
        }

        assert_eq!(digest(0o600), digest(0o644));
        assert_eq!(digest(0o700), digest(0o711));
        assert_ne!(digest(0o644), digest(0o744));
    }

    #[test]
    fn has_a_golden_version_one_digest() {
        let tree = TempDir::new().unwrap();
        fs::create_dir(tree.path().join("empty")).unwrap();
        write(tree.path(), "bin/tool", b"#!/bin/sh\nexit 0\n");
        fs::set_permissions(
            tree.path().join("bin/tool"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        symlink("bin/tool", tree.path().join("tool")).unwrap();

        assert_eq!(
            hex(&hash_local_tree(tree.path()).unwrap()),
            "f8abaea4758185deb72f163f1adb3c9993cb060a20a9a2a544c2e97b0745a1c6"
        );
    }

    #[test]
    fn rejects_non_utf8_paths_and_symlink_targets() {
        let path_tree = TempDir::new().unwrap();
        fs::write(
            path_tree.path().join(OsString::from_vec(vec![b'f', 0xff])),
            b"contents",
        )
        .unwrap();
        assert!(matches!(
            hash_local_tree(path_tree.path()),
            Err(LocalTreeHashError::NonUtf8Path { .. })
        ));

        let target_tree = TempDir::new().unwrap();
        symlink(
            OsString::from_vec(vec![b't', 0xff]),
            target_tree.path().join("link"),
        )
        .unwrap();
        assert!(matches!(
            hash_local_tree(target_tree.path()),
            Err(LocalTreeHashError::NonUtf8SymlinkTarget { .. })
        ));
    }

    #[test]
    fn rejects_backslashes_in_normalized_paths() {
        let tree = TempDir::new().unwrap();
        write(tree.path(), "not\\portable", b"contents");

        assert!(matches!(
            hash_local_tree(tree.path()),
            Err(LocalTreeHashError::NonNormalizedPath { ref path })
                if path == Path::new("not\\portable")
        ));
    }

    #[test]
    fn rejects_unsupported_entry_types() {
        let tree = TempDir::new().unwrap();
        let socket = tree.path().join("socket");
        let _listener = UnixListener::bind(&socket).unwrap();

        assert!(matches!(
            hash_local_tree(tree.path()),
            Err(LocalTreeHashError::UnsupportedEntryType { ref path })
                if path == Path::new("socket")
        ));
    }
}
