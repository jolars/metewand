use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use super::{ArchiveLimits, create_archive, import_archive, verify_archive};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/v1/fixtures/archive-executable-v1")
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
    if std::env::var("METEWAND_ARCHIVE_CRASH_OPERATION").unwrap() == "create" {
        create_archive(&fixture(), &root.join("created.tar.gz")).unwrap();
    } else {
        import_archive(
            &root.join("input.tar.gz"),
            &root.join("imported"),
            ArchiveLimits::default(),
        )
        .unwrap();
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
    for operation in ["create", "import"] {
        let baseline = tempfile::tempdir().unwrap();
        if operation == "import" {
            create_archive(&fixture(), &baseline.path().join("input.tar.gz")).unwrap();
        }
        let output = invoke(baseline.path(), operation, 0);
        assert!(output.status.success(), "{output:?}");
        let trace = fs::read_to_string(baseline.path().join("trace")).unwrap();
        for label in [
            "file_sync.before",
            "file_sync.after",
            "rename.before",
            "rename.after",
        ] {
            let target = trace.lines().position(|line| line == label).unwrap() + 1;
            let root = tempfile::tempdir().unwrap();
            if operation == "import" {
                create_archive(&fixture(), &root.path().join("input.tar.gz")).unwrap();
            }
            let output = invoke(root.path(), operation, target);
            assert_eq!(
                output.status.code(),
                Some(86),
                "{operation}: {label}: {output:?}"
            );
            let destination = root.path().join(if operation == "create" {
                "created.tar.gz"
            } else {
                "imported"
            });
            if destination.exists() {
                verify_archive(&destination, ArchiveLimits::default()).unwrap();
            }
        }
    }
}
