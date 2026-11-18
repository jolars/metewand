use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use super::{ArchiveLimits, create_archive, import_archive, restore_archive, verify_archive};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/v1/fixtures/archive-executable-v1")
}

#[test]
fn streamed_archive_readback_checks_payload_digests() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("archive.tar.gz");
    create_archive(&fixture(), &path).unwrap();
    let manifest_bytes = fs::read(fixture().join("archive.json")).unwrap();
    let mut manifest: super::ArchiveManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    // Creation records the running tool version in the emitted manifest.
    manifest.tool_version = env!("CARGO_PKG_VERSION").to_owned();
    let manifest_member =
        super::metadata_member("archive.json", &super::canonical_bytes(&manifest).unwrap());
    super::verify_created_archive(&path, &manifest_member, &manifest.files).unwrap();

    let payload = manifest
        .files
        .iter_mut()
        .find(|file| file.path.ends_with("/payload.bin"))
        .unwrap();
    payload.sha256 = super::digest(b"different bytes");
    let error =
        super::verify_created_archive(&path, &manifest_member, &manifest.files).unwrap_err();
    assert!(error.to_string().contains("recorded hash or size"));
}

#[test]
fn archive_publication_child() {
    let Some(root) = std::env::var_os("METEWAND_ARCHIVE_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let target = std::env::var("METEWAND_ARCHIVE_CRASH_POINT")
        .unwrap()
        .parse()
        .unwrap();
    crate::runtime::fault_injection::arm(fs::File::create(root.join("trace")).unwrap(), target);
    match std::env::var("METEWAND_ARCHIVE_CRASH_OPERATION")
        .unwrap()
        .as_str()
    {
        "create" => {
            create_archive(&fixture(), &root.join("created.tar.gz")).unwrap();
        }
        "import" => {
            import_archive(
                &root.join("input.tar.gz"),
                &root.join("imported"),
                ArchiveLimits::default(),
            )
            .unwrap();
        }
        "restore" => {
            restore_archive(
                &root.join("input.tar.gz"),
                &root.join("restored"),
                ArchiveLimits::default(),
            )
            .unwrap();
        }
        _ => panic!("unknown archive crash operation"),
    }
}

fn prepare_input(root: &Path, operation: &str) {
    match operation {
        "import" => {
            create_archive(&fixture(), &root.join("input.tar.gz")).unwrap();
        }
        "restore" => {
            #[cfg(unix)]
            create_archive(&fixture(), &root.join("input.tar.gz")).unwrap();
            #[cfg(windows)]
            {
                let run = root.join("source");
                fs::create_dir(&run).unwrap();
                drop(crate::runtime::run_store::RunStore::open(&run).unwrap());
                create_archive(&run, &root.join("input.tar.gz")).unwrap();
            }
        }
        _ => (),
    }
}

fn invoke(root: &Path, operation: &str, target: usize) -> Output {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::archive::tests::archive_publication_child",
            "--nocapture",
        ])
        .env("METEWAND_ARCHIVE_CRASH_ROOT", root)
        .env("METEWAND_ARCHIVE_CRASH_OPERATION", operation)
        .env("METEWAND_ARCHIVE_CRASH_POINT", target.to_string())
        .output()
        .unwrap()
}

#[test]
fn publication_crashes_leave_no_destination_or_a_complete_verified_archive() {
    check_publication_crashes(false);
}

#[test]
#[ignore = "full archive crash sweep; see CONTRIBUTING.md#crash-conformance"]
fn every_archive_publication_boundary_survives_process_death() {
    check_publication_crashes(true);
}

fn check_publication_crashes(exhaustive: bool) {
    for operation in ["create", "import", "restore"] {
        let baseline = tempfile::tempdir().unwrap();
        prepare_input(baseline.path(), operation);
        let output = invoke(baseline.path(), operation, 0);
        assert!(output.status.success(), "{output:?}");
        let trace = fs::read_to_string(baseline.path().join("trace")).unwrap();
        // The last rename publishes the destination. Earlier renames can
        // publish individual files inside the staging directory.
        let publication_point = |boundary| {
            trace
                .lines()
                .enumerate()
                .filter(|(_, label)| *label == boundary)
                .map(|(index, _)| index + 1)
                .last()
                .unwrap_or_else(|| panic!("{operation}: missing {boundary}"))
        };
        let before_publication = publication_point("rename.before");
        let after_publication = publication_point("rename.after");
        let targets = trace.lines().enumerate().filter_map(|(index, label)| {
            let target = index + 1;
            if !exhaustive {
                return (target == before_publication || target == after_publication)
                    .then_some((target, label));
            }
            let durability = matches!(
                label,
                "file_sync.before"
                    | "file_sync.after"
                    | "directory_sync.before"
                    | "directory_sync.after"
                    | "rename.before"
                    | "rename.after"
            );
            let transaction = operation == "restore"
                && (label.starts_with("database.before COMMIT")
                    || label.starts_with("database.after COMMIT"));
            (durability || transaction).then_some((target, label))
        });
        for (target, label) in targets {
            let root = tempfile::tempdir().unwrap();
            prepare_input(root.path(), operation);
            let output = invoke(root.path(), operation, target);
            assert_eq!(
                output.status.code(),
                Some(86),
                "{operation}: {label}: {output:?}"
            );
            let destination = root.path().join(match operation {
                "create" => "created.tar.gz",
                "import" => "imported",
                "restore" => "restored",
                _ => unreachable!(),
            });
            if operation == "create" && target == before_publication {
                // A crash leaves scratch paths behind. Creation needs only the
                // compressed temporary file, even before publication.
                for entry in fs::read_dir(root.path()).unwrap() {
                    assert!(entry.unwrap().file_type().unwrap().is_file());
                }
            }
            let published = destination.exists();
            if target == before_publication || target == after_publication {
                assert_eq!(
                    published,
                    target == after_publication,
                    "{operation}: {label}"
                );
            }
            if published {
                if operation == "restore" {
                    drop(crate::runtime::run_store::RunStore::open(&destination).unwrap());
                    let archive = root.path().join("round-trip.tar.gz");
                    create_archive(&destination, &archive).unwrap();
                    assert_eq!(
                        fs::read(archive).unwrap(),
                        fs::read(root.path().join("input.tar.gz")).unwrap()
                    );
                } else {
                    verify_archive(&destination, ArchiveLimits::default()).unwrap();
                }
            }
        }
    }
}
