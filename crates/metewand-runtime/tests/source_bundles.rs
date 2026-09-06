use std::{fs, path::Path};

use metewand_core::manifest::RepositoryPath;
use metewand_runtime::source_bundle::{
    SourceBundleError, expand_source_bundle, identify_source_bundle,
};
use tempfile::TempDir;

fn pattern(value: &str) -> RepositoryPath {
    serde_json::from_str(&format!("{value:?}")).expect("test pattern must be a repository path")
}

fn write(root: &Path, path: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"fixture").unwrap();
}

#[test]
fn expands_each_pattern_and_orders_unique_paths_by_utf8_bytes() {
    let repository = TempDir::new().unwrap();
    for path in ["src/a.rs", "src/_.rs", "src/Z.rs", "src/note.txt"] {
        write(repository.path(), path);
    }

    let bundle = expand_source_bundle(
        repository.path(),
        &[pattern("src/a.rs"), pattern("src/*.rs")],
    )
    .unwrap();

    assert_eq!(
        bundle.paths(),
        [
            Path::new("src/Z.rs"),
            Path::new("src/_.rs"),
            Path::new("src/a.rs")
        ]
    );
    assert!(bundle.paths().iter().all(|path| path.is_relative()));
}

#[test]
fn expands_recursive_globs_without_following_discovered_symlink_directories() {
    let repository = TempDir::new().unwrap();
    write(repository.path(), "src/main.py");
    write(repository.path(), "src/nested/helper.py");
    write(repository.path(), "src/nested/readme.txt");

    let bundle = expand_source_bundle(repository.path(), &[pattern("src/**/*.py")]).unwrap();

    assert_eq!(
        bundle.paths(),
        [Path::new("src/main.py"), Path::new("src/nested/helper.py")]
    );
}

#[test]
fn supports_the_documented_glob_forms_and_includes_dotfiles() {
    let repository = TempDir::new().unwrap();
    for path in [
        "src/.hidden.py",
        "src/*.txt",
        "src/a.py",
        "src/b.R",
        "src/c.jl",
    ] {
        write(repository.path(), path);
    }

    let bundle = expand_source_bundle(
        repository.path(),
        &[
            pattern("src/{a.py,b.R}"),
            pattern("src/?.jl"),
            pattern("src/[.]hidden.py"),
            pattern("src/[*].txt"),
        ],
    )
    .unwrap();

    assert_eq!(
        bundle.paths(),
        [
            Path::new("src/*.txt"),
            Path::new("src/.hidden.py"),
            Path::new("src/a.py"),
            Path::new("src/b.R"),
            Path::new("src/c.jl"),
        ]
    );
}

#[test]
fn source_bundle_identity_is_order_independent_and_content_sensitive() {
    let repository = TempDir::new().unwrap();
    write(repository.path(), "src/main.py");
    write(repository.path(), "src/helper.py");

    let forward = identify_source_bundle(
        repository.path(),
        &[pattern("src/main.py"), pattern("src/helper.py")],
    )
    .unwrap();
    let reverse = identify_source_bundle(
        repository.path(),
        &[pattern("src/helper.py"), pattern("src/main.py")],
    )
    .unwrap();
    assert_eq!(forward, reverse);

    fs::write(repository.path().join("src/helper.py"), b"changed").unwrap();
    let changed = identify_source_bundle(
        repository.path(),
        &[pattern("src/main.py"), pattern("src/helper.py")],
    )
    .unwrap();
    assert_ne!(forward, changed);
}

#[test]
fn rejects_an_empty_source_bundle() {
    let repository = TempDir::new().unwrap();

    let error = expand_source_bundle(repository.path(), &[]).unwrap_err();

    assert!(matches!(error, SourceBundleError::Empty));
}

#[test]
fn requires_every_declared_pattern_to_match() {
    let repository = TempDir::new().unwrap();
    write(repository.path(), "src/main.py");

    let error = expand_source_bundle(
        repository.path(),
        &[pattern("src/*.py"), pattern("src/*.jl")],
    )
    .unwrap_err();

    assert!(matches!(
        error,
        SourceBundleError::UnmatchedPattern { ref pattern }
            if pattern == Path::new("src/*.jl")
    ));
}

#[test]
fn rejects_invalid_glob_syntax() {
    let repository = TempDir::new().unwrap();

    let error = expand_source_bundle(repository.path(), &[pattern("src/[abc")]).unwrap_err();

    assert!(matches!(
        error,
        SourceBundleError::InvalidPattern { ref pattern, .. }
            if pattern == Path::new("src/[abc")
    ));
}

#[test]
fn rejects_missing_and_non_directory_repository_roots() {
    let parent = TempDir::new().unwrap();
    let missing = parent.path().join("missing");
    let error = expand_source_bundle(&missing, &[pattern("src")]).unwrap_err();
    assert!(matches!(error, SourceBundleError::RepositoryRoot { .. }));

    let file = parent.path().join("file");
    fs::write(&file, b"not a repository").unwrap();
    let error = expand_source_bundle(&file, &[pattern("src")]).unwrap_err();
    assert!(matches!(
        error,
        SourceBundleError::RepositoryRootNotDirectory { .. }
    ));
}

#[cfg(unix)]
mod unix {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, os::unix::fs::symlink};

    use super::*;

    #[test]
    fn rejects_a_matched_symlink_that_escapes_the_repository() {
        let repository = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(outside.path(), "secret.py");
        fs::create_dir(repository.path().join("src")).unwrap();
        symlink(
            outside.path().join("secret.py"),
            repository.path().join("src/secret.py"),
        )
        .unwrap();

        let error = expand_source_bundle(repository.path(), &[pattern("src/*.py")]).unwrap_err();

        assert!(matches!(
            error,
            SourceBundleError::SymlinkEscape { ref path, .. }
                if path == Path::new("src/secret.py")
        ));
    }

    #[test]
    fn rejects_a_path_that_traverses_an_escaping_symlink() {
        let repository = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(outside.path(), "secret.py");
        symlink(outside.path(), repository.path().join("alias")).unwrap();

        let error =
            expand_source_bundle(repository.path(), &[pattern("alias/secret.py")]).unwrap_err();

        assert!(matches!(
            error,
            SourceBundleError::SymlinkEscape { ref path, .. }
                if path == Path::new("alias/secret.py")
        ));
    }

    #[test]
    fn checks_the_complete_tree_below_a_matched_directory() {
        let repository = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(repository.path(), "src/main.py");
        write(outside.path(), "secret.py");
        symlink(
            outside.path().join("secret.py"),
            repository.path().join("src/secret.py"),
        )
        .unwrap();

        let error = expand_source_bundle(repository.path(), &[pattern("src")]).unwrap_err();

        assert!(matches!(
            error,
            SourceBundleError::SymlinkEscape { ref path, .. }
                if path == Path::new("src/secret.py")
        ));
    }

    #[test]
    fn accepts_contained_symlinks_and_ignores_undeclared_trees() {
        let repository = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(repository.path(), "shared/helper.py");
        write(repository.path(), "src/main.py");
        write(outside.path(), "secret.py");
        symlink(
            "../shared/helper.py",
            repository.path().join("src/helper.py"),
        )
        .unwrap();
        fs::create_dir(repository.path().join("unrelated")).unwrap();
        symlink(
            outside.path().join("secret.py"),
            repository.path().join("unrelated/secret.py"),
        )
        .unwrap();

        let bundle = expand_source_bundle(repository.path(), &[pattern("src/*.py")]).unwrap();

        assert_eq!(
            bundle.paths(),
            [Path::new("src/helper.py"), Path::new("src/main.py")]
        );
    }

    #[test]
    fn canonicalizes_a_symlinked_repository_root() {
        let parent = TempDir::new().unwrap();
        let repository = parent.path().join("repository");
        fs::create_dir(&repository).unwrap();
        write(&repository, "src/main.py");
        let root_alias = parent.path().join("repository-alias");
        symlink(&repository, &root_alias).unwrap();

        let bundle = expand_source_bundle(&root_alias, &[pattern("src/main.py")]).unwrap();

        assert_eq!(bundle.paths(), [Path::new("src/main.py")]);
    }

    #[test]
    fn wildcard_traversal_does_not_follow_symlinked_directories() {
        let repository = TempDir::new().unwrap();
        write(repository.path(), "shared/helper.py");
        symlink("shared", repository.path().join("alias")).unwrap();

        let error = expand_source_bundle(repository.path(), &[pattern("alias/*.py")]).unwrap_err();

        assert!(matches!(
            error,
            SourceBundleError::UnmatchedPattern { ref pattern }
                if pattern == Path::new("alias/*.py")
        ));
    }

    #[test]
    fn rejects_broken_symlinks_because_containment_cannot_be_proved() {
        let repository = TempDir::new().unwrap();
        fs::create_dir(repository.path().join("src")).unwrap();
        symlink("missing.py", repository.path().join("src/broken.py")).unwrap();

        let error = expand_source_bundle(repository.path(), &[pattern("src/*.py")]).unwrap_err();

        assert!(matches!(
            error,
            SourceBundleError::SymlinkResolution { ref path, .. }
                if path == Path::new("src/broken.py")
        ));
    }

    #[test]
    fn rejects_matched_paths_outside_the_utf8_repository_contract() {
        let repository = TempDir::new().unwrap();
        let path = repository
            .path()
            .join(OsString::from_vec(vec![b's', b'r', b'c', 0xff]));
        fs::write(path, b"fixture").unwrap();

        let error = expand_source_bundle(repository.path(), &[pattern("**")]).unwrap_err();

        assert!(matches!(error, SourceBundleError::NonUtf8Path { .. }));
    }
}
