use std::{fs, path::Path};

use metewand_core::{
    canonical::CanonicalValue,
    manifest::{Name, RemoteSource, RepositoryPath},
    records::ContentDigest,
};
use metewand_runtime::{
    SOURCE_RESOLUTION_VERSION,
    source_bundle::identify_source_bundle,
    source_resolution::{
        AcquiredSource, SourceAcquirer, SourceFile, SourceOrigin, SourceRequest,
        SourceResolutionError, resolve_source,
    },
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn path(value: &str) -> RepositoryPath {
    serde_json::from_value(json!(value)).unwrap()
}

fn digest(bytes: &[u8]) -> ContentDigest {
    ContentDigest::new(Sha256::digest(bytes).into())
}

fn hex(digest: ContentDigest) -> String {
    digest
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn canonical(value: serde_json::Value) -> CanonicalValue {
    CanonicalValue::try_from(value).unwrap()
}

fn file(name: &str, bytes: &[u8]) -> SourceFile {
    SourceFile {
        path: path(name),
        size_bytes: bytes.len() as u64,
        sha256: digest(bytes),
    }
}

fn url() -> RemoteSource {
    serde_json::from_value(json!({
        "url": "https://example.invalid/data.gz",
        "sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    }))
    .unwrap()
}

struct Export {
    files: Vec<(&'static str, &'static [u8])>,
    inventory: Vec<SourceFile>,
}

impl SourceAcquirer for Export {
    fn acquire(
        &self,
        _request: &SourceRequest<'_>,
        output_dir: &Path,
    ) -> Result<AcquiredSource, SourceResolutionError> {
        for (name, bytes) in &self.files {
            let destination = output_dir.join(name);
            fs::create_dir_all(destination.parent().unwrap())?;
            fs::write(destination, bytes)?;
        }
        Ok(AcquiredSource {
            files: self.inventory.clone(),
            provenance: canonical(json!({"verification": "artifact"})),
        })
    }
}

#[test]
fn repository_resolution_preserves_selected_paths_and_owns_a_verified_copy() {
    let repository = TempDir::new().unwrap();
    let staging = TempDir::new().unwrap();
    fs::create_dir(repository.path().join("data")).unwrap();
    fs::write(repository.path().join("data/input"), b"abc").unwrap();
    fs::write(repository.path().join("unselected"), b"other").unwrap();
    let patterns = [path("data")];
    let expected = identify_source_bundle(repository.path(), &patterns).unwrap();
    let request = SourceRequest::Repository {
        root: repository.path(),
        sources: &patterns,
        expected,
    };

    let resolved = resolve_source(&request, None, staging.path()).unwrap();

    assert_eq!(
        fs::read(resolved.directory().join("data/input")).unwrap(),
        b"abc"
    );
    assert!(!resolved.directory().join("unselected").exists());
    assert_eq!(resolved.files(), &[file("data/input", b"abc")]);
    assert_eq!(resolved.provenance().version, SOURCE_RESOLUTION_VERSION);
    assert_eq!(SOURCE_RESOLUTION_VERSION, 1);
    assert!(matches!(
        resolved.provenance().origin,
        SourceOrigin::Repository { source_bundle } if source_bundle == expected
    ));
    resolved.verify().unwrap();
    fs::write(repository.path().join("data/input"), b"changed").unwrap();
    assert_eq!(
        fs::read(resolved.directory().join("data/input")).unwrap(),
        b"abc"
    );
    fs::write(resolved.directory().join("data/input"), b"mutated").unwrap();
    assert!(matches!(
        resolved.verify(),
        Err(SourceResolutionError::ChangedSource)
    ));
    drop(resolved);
    assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
}

#[test]
fn repository_resolution_rejects_a_changed_pin_without_leaking_staging() {
    let repository = TempDir::new().unwrap();
    let staging = TempDir::new().unwrap();
    fs::write(repository.path().join("input"), b"abc").unwrap();
    let patterns = [path("input")];
    let expected = identify_source_bundle(repository.path(), &patterns).unwrap();
    fs::write(repository.path().join("input"), b"changed").unwrap();
    let request = SourceRequest::Repository {
        root: repository.path(),
        sources: &patterns,
        expected,
    };

    assert!(matches!(
        resolve_source(&request, None, staging.path()),
        Err(SourceResolutionError::ChangedSource)
    ));
    assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
}

#[test]
fn repository_relocation_and_declaration_order_preserve_the_output_digest() {
    let staging = TempDir::new().unwrap();
    let mut digests = Vec::new();
    for patterns in [[path("a"), path("b")], [path("b"), path("a")]] {
        let repository = TempDir::new().unwrap();
        fs::write(repository.path().join("a"), b"abc").unwrap();
        fs::write(repository.path().join("b"), b"def").unwrap();
        let expected = identify_source_bundle(repository.path(), &patterns).unwrap();
        let resolved = resolve_source(
            &SourceRequest::Repository {
                root: repository.path(),
                sources: &patterns,
                expected,
            },
            None,
            staging.path(),
        )
        .unwrap();
        digests.push(resolved.tree_digest());
    }
    assert_eq!(digests[0], digests[1]);
}

#[test]
fn pinned_url_resolution_verifies_the_request_pin_and_preserves_raw_bytes() {
    let staging = TempDir::new().unwrap();
    let source = url();
    let backend = Export {
        files: vec![("source", b"abc")],
        inventory: vec![file("source", b"abc")],
    };
    let resolved = resolve_source(
        &SourceRequest::PinnedUrl(&source),
        Some(&backend),
        staging.path(),
    )
    .unwrap();

    assert_eq!(
        fs::read(resolved.directory().join("source")).unwrap(),
        b"abc"
    );
    assert_eq!(resolved.files(), &[file("source", b"abc")]);
    assert_eq!(
        hex(resolved.tree_digest()),
        "d4f57f4acf99be48561165f547f4f33ec3b984466da469554d117ae349eef790"
    );
    assert!(
        matches!(resolved.provenance().origin, SourceOrigin::PinnedUrl { ref url, .. } if url == &source.url)
    );
    assert_eq!(
        resolved
            .provenance()
            .acquisition
            .as_ref()
            .unwrap()
            .as_json(),
        &json!({"verification": "artifact"})
    );

    let wrong_source = serde_json::from_value(json!({
        "url": source.url, "sha256": "0".repeat(64)
    }))
    .unwrap();
    assert!(matches!(
        resolve_source(
            &SourceRequest::PinnedUrl(&wrong_source),
            Some(&backend),
            staging.path()
        ),
        Err(SourceResolutionError::DigestMismatch { .. })
    ));
}

#[test]
fn external_resolution_retains_the_explicit_selector_and_sorts_the_inventory() {
    let staging = TempDir::new().unwrap();
    let provider: Name = serde_json::from_value(json!("example")).unwrap();
    let selector = canonical(json!({"release": "pinned", "dataset": "example:data@1"}));
    let backend = Export {
        files: vec![("b", b"def"), ("nested/a", b"abc")],
        inventory: vec![file("nested/a", b"abc"), file("b", b"def")],
    };
    let resolved = resolve_source(
        &SourceRequest::External {
            provider: &provider,
            selector: &selector,
        },
        Some(&backend),
        staging.path(),
    )
    .unwrap();

    assert_eq!(
        resolved.files(),
        &[file("b", b"def"), file("nested/a", b"abc")]
    );
    assert!(matches!(resolved.provenance().origin,
        SourceOrigin::External { provider: ref actual, selector: ref actual_selector }
        if actual == &provider && actual_selector == &selector
    ));
    resolved.verify().unwrap();
}

#[test]
fn acquisition_is_explicit_and_unavailable_without_an_acquirer() {
    let staging = TempDir::new().unwrap();
    let source = url();
    let provider: Name = serde_json::from_value(json!("missing")).unwrap();
    let selector = canonical(json!({"revision": 1}));
    for request in [
        SourceRequest::PinnedUrl(&source),
        SourceRequest::External {
            provider: &provider,
            selector: &selector,
        },
    ] {
        assert!(matches!(
            resolve_source(&request, None, staging.path()),
            Err(SourceResolutionError::AcquirerUnavailable)
        ));
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
    }
}

#[test]
fn rejects_incomplete_ambiguous_and_false_acquisition_inventories() {
    let provider: Name = serde_json::from_value(json!("example")).unwrap();
    let selector = canonical(json!({"revision": 1}));
    for inventory in [
        vec![],
        vec![file("missing", b"abc")],
        vec![file("source", b"wrong digest")],
        vec![SourceFile {
            size_bytes: 9,
            ..file("source", b"abc")
        }],
        vec![file("source", b"abc"), file("source", b"abc")],
    ] {
        let staging = TempDir::new().unwrap();
        let backend = Export {
            files: vec![("source", b"abc")],
            inventory,
        };
        assert!(
            resolve_source(
                &SourceRequest::External {
                    provider: &provider,
                    selector: &selector,
                },
                Some(&backend),
                staging.path()
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
    }
}

#[test]
fn pinned_url_requires_exactly_the_unextracted_source_file() {
    let source = url();
    for files in [
        vec![("renamed", b"abc".as_slice())],
        vec![("source", b"abc".as_slice()), ("extra", b"abc".as_slice())],
    ] {
        let staging = TempDir::new().unwrap();
        let backend = Export {
            inventory: files
                .iter()
                .map(|(name, bytes)| file(name, bytes))
                .collect(),
            files,
        };
        assert!(matches!(
            resolve_source(
                &SourceRequest::PinnedUrl(&source),
                Some(&backend),
                staging.path()
            ),
            Err(SourceResolutionError::InvalidInventory)
        ));
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn repository_resolution_rejects_symlinks_even_to_contained_files() {
    let repository = TempDir::new().unwrap();
    let staging = TempDir::new().unwrap();
    fs::write(repository.path().join("input"), b"abc").unwrap();
    std::os::unix::fs::symlink("input", repository.path().join("link")).unwrap();
    let patterns = [path("link")];
    let expected = identify_source_bundle(repository.path(), &patterns).unwrap();

    assert!(matches!(
        resolve_source(
            &SourceRequest::Repository {
                root: repository.path(),
                sources: &patterns,
                expected,
            },
            None,
            staging.path()
        ),
        Err(SourceResolutionError::UnsupportedEntry { .. })
    ));
    assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn acquisition_cannot_substitute_a_symlink_for_verified_bytes() {
    struct Link;
    impl SourceAcquirer for Link {
        fn acquire(
            &self,
            _: &SourceRequest<'_>,
            output: &Path,
        ) -> Result<AcquiredSource, SourceResolutionError> {
            fs::write(output.join("target"), b"abc")?;
            std::os::unix::fs::symlink("target", output.join("source"))?;
            Ok(AcquiredSource {
                files: vec![file("source", b"abc"), file("target", b"abc")],
                provenance: canonical(json!({})),
            })
        }
    }
    let staging = TempDir::new().unwrap();
    assert!(matches!(
        resolve_source(
            &SourceRequest::PinnedUrl(&url()),
            Some(&Link),
            staging.path()
        ),
        Err(SourceResolutionError::UnsupportedEntry { .. })
    ));
    assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
}

#[test]
fn failed_acquisition_drops_partial_outputs() {
    struct Fail;
    impl SourceAcquirer for Fail {
        fn acquire(
            &self,
            _: &SourceRequest<'_>,
            output: &Path,
        ) -> Result<AcquiredSource, SourceResolutionError> {
            fs::write(output.join("partial"), b"partial")?;
            Err(std::io::Error::other("failed acquisition").into())
        }
    }
    let staging = TempDir::new().unwrap();
    assert!(matches!(
        resolve_source(
            &SourceRequest::PinnedUrl(&url()),
            Some(&Fail),
            staging.path()
        ),
        Err(SourceResolutionError::Io(_))
    ));
    assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
}

#[test]
fn repository_staging_cannot_become_part_of_its_own_input() {
    let repository = TempDir::new().unwrap();
    fs::create_dir(repository.path().join("data")).unwrap();
    fs::write(repository.path().join("data/input"), b"abc").unwrap();
    let patterns = [path("data")];
    let expected = identify_source_bundle(repository.path(), &patterns).unwrap();
    assert!(matches!(
        resolve_source(
            &SourceRequest::Repository {
                root: repository.path(),
                sources: &patterns,
                expected,
            },
            None,
            &repository.path().join("data")
        ),
        Err(SourceResolutionError::StagingOverlapsSource)
    ));
    assert_eq!(
        identify_source_bundle(repository.path(), &patterns).unwrap(),
        expected
    );
}

#[test]
fn repository_staging_can_use_an_unselected_directory_in_the_repository() {
    let repository = TempDir::new().unwrap();
    fs::create_dir(repository.path().join("data")).unwrap();
    fs::create_dir(repository.path().join("staging")).unwrap();
    fs::write(repository.path().join("data/input"), b"abc").unwrap();
    let patterns = [path("data/**")];
    let expected = identify_source_bundle(repository.path(), &patterns).unwrap();
    let resolved = resolve_source(
        &SourceRequest::Repository {
            root: repository.path(),
            sources: &patterns,
            expected,
        },
        None,
        &repository.path().join("staging"),
    )
    .unwrap();
    assert_eq!(resolved.files(), &[file("data/input", b"abc")]);
    assert_eq!(
        identify_source_bundle(repository.path(), &patterns).unwrap(),
        expected
    );
}

#[test]
fn acquired_sources_reject_undeclared_empty_directories() {
    struct EmptyDirectory;
    impl SourceAcquirer for EmptyDirectory {
        fn acquire(
            &self,
            request: &SourceRequest<'_>,
            output: &Path,
        ) -> Result<AcquiredSource, SourceResolutionError> {
            let SourceRequest::PinnedUrl(source) = request else {
                panic!("expected pinned URL")
            };
            assert_eq!(source.url, "https://example.invalid/data.gz");
            fs::create_dir(output.join("extra"))?;
            fs::write(output.join("source"), b"abc")?;
            Ok(AcquiredSource {
                files: vec![file("source", b"abc")],
                provenance: canonical(json!({})),
            })
        }
    }
    let staging = TempDir::new().unwrap();
    assert!(matches!(
        resolve_source(
            &SourceRequest::PinnedUrl(&url()),
            Some(&EmptyDirectory),
            staging.path()
        ),
        Err(SourceResolutionError::InvalidInventory)
    ));
    assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
}
