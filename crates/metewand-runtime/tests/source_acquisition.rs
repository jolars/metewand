use metewand_core::{manifest::RemoteSource, materialization::hex, records::ContentDigest};
use metewand_runtime::{
    source_acquisition::{ArchiveFormat, ArchiveLimits, HttpAcquirer, extract_archive},
    source_resolution::{SourceRequest, resolve_source},
};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

fn serve(bytes: Vec<u8>, status: &str) -> (RemoteSource, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let source = serde_json::from_value(serde_json::json!({
        "url": format!("http://{}/input", listener.local_addr().unwrap()),
        "sha256": hex(ContentDigest::new(Sha256::digest(&bytes).into())),
    }))
    .unwrap();
    let status = status.to_owned();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        let _ = stream.write_all(&bytes);
    });
    (source, handle)
}

fn archive(path: &str, kind: u8, content: &[u8]) -> Vec<u8> {
    let mut header = tar::Header::new_ustar();
    header.set_size(content.len() as u64);
    header.set_mode(0o7777);
    header.set_entry_type(tar::EntryType::new(kind));
    // Raw path bytes deliberately bypass the builder's traversal validation.
    header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
    header.set_cksum();
    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, content).unwrap();
    builder.into_inner().unwrap()
}

#[test]
fn pinned_download_rejects_mismatch_limit_and_http_failure() {
    for case in 0..3 {
        let parent = tempfile::tempdir().unwrap();
        let (mut source, handle) = serve(
            b"abc".to_vec(),
            if case == 2 { "302 Found" } else { "200 OK" },
        );
        if case == 0 {
            source.sha256 = serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap();
        }
        let backend =
            HttpAcquirer::new(Duration::from_secs(3), if case == 1 { 2 } else { 100 }).unwrap();
        assert!(
            resolve_source(
                &SourceRequest::PinnedUrl(&source),
                Some(&backend),
                parent.path()
            )
            .is_err()
        );
        handle.join().unwrap();
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    }
}

#[test]
fn extracts_verified_tar_and_gzip_with_private_cleanup() {
    for gzip in [false, true] {
        let mut bytes = archive("nested/data", b'0', b"abc");
        if gzip {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&bytes).unwrap();
            bytes = encoder.finish().unwrap();
        }
        let parent = tempfile::tempdir().unwrap();
        let (source, handle) = serve(bytes, "200 OK");
        let backend = HttpAcquirer::new(Duration::from_secs(3), 10000).unwrap();
        let resolved = resolve_source(
            &SourceRequest::PinnedUrl(&source),
            Some(&backend),
            parent.path(),
        )
        .unwrap();
        handle.join().unwrap();
        let format = if gzip {
            ArchiveFormat::TarGzip
        } else {
            ArchiveFormat::Tar
        };
        let output = extract_archive(
            &resolved,
            format,
            ArchiveLimits {
                max_entries: 1,
                max_expanded_bytes: 3,
            },
            parent.path(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(output.path().join("nested/data")).unwrap(),
            b"abc"
        );
        drop(output);
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
    }
}

#[test]
fn rejects_unsafe_entries_and_budgets_without_leaving_output() {
    for (path, kind, entries, size) in [
        ("../escape", b'0', 1, 3),
        ("/absolute", b'0', 1, 3),
        ("a/../../escape", b'0', 1, 3),
        ("a\\b", b'0', 1, 3),
        ("link", b'1', 1, 3),
        ("link", b'2', 1, 3),
        ("device", b'3', 1, 3),
        ("device", b'4', 1, 3),
        ("fifo", b'6', 1, 3),
        ("unknown", b'X', 1, 3),
        ("pax", b'x', 1, 3),
        ("longname", b'L', 1, 3),
        ("data", b'0', 0, 3),
        ("data", b'0', 1, 2),
    ] {
        let parent = tempfile::tempdir().unwrap();
        let (source, handle) = serve(archive(path, kind, b"abc"), "200 OK");
        let backend = HttpAcquirer::new(Duration::from_secs(3), 10000).unwrap();
        let resolved = resolve_source(
            &SourceRequest::PinnedUrl(&source),
            Some(&backend),
            parent.path(),
        )
        .unwrap();
        handle.join().unwrap();
        assert!(
            extract_archive(
                &resolved,
                ArchiveFormat::Tar,
                ArchiveLimits {
                    max_entries: entries,
                    max_expanded_bytes: size
                },
                parent.path()
            )
            .is_err(),
            "{path} {kind}"
        );
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
    }
}

#[test]
fn extraction_rechecks_source_digest() {
    let parent = tempfile::tempdir().unwrap();
    let (source, handle) = serve(archive("data", b'0', b"abc"), "200 OK");
    let backend = HttpAcquirer::new(Duration::from_secs(3), 10000).unwrap();
    let resolved = resolve_source(
        &SourceRequest::PinnedUrl(&source),
        Some(&backend),
        parent.path(),
    )
    .unwrap();
    handle.join().unwrap();
    std::fs::write(resolved.directory().join("source"), b"corrupt").unwrap();
    assert!(
        extract_archive(
            &resolved,
            ArchiveFormat::Tar,
            ArchiveLimits {
                max_entries: 1,
                max_expanded_bytes: 3
            },
            parent.path()
        )
        .is_err()
    );
}

#[test]
fn rejects_duplicate_paths_conflicts_and_truncated_payloads() {
    let first = archive("data", b'0', b"abc");
    let mut duplicate = first[..1024].to_vec();
    duplicate.extend_from_slice(&first);
    let mut conflict = first[..1024].to_vec();
    conflict.extend_from_slice(&archive("data/child", b'0', b"abc"));
    let truncated = first[..513].to_vec();
    for bytes in [duplicate, conflict, truncated] {
        let parent = tempfile::tempdir().unwrap();
        let (source, handle) = serve(bytes, "200 OK");
        let backend = HttpAcquirer::new(Duration::from_secs(3), 10000).unwrap();
        let resolved = resolve_source(
            &SourceRequest::PinnedUrl(&source),
            Some(&backend),
            parent.path(),
        )
        .unwrap();
        handle.join().unwrap();
        assert!(
            extract_archive(
                &resolved,
                ArchiveFormat::Tar,
                ArchiveLimits {
                    max_entries: 10,
                    max_expanded_bytes: 100
                },
                parent.path()
            )
            .is_err()
        );
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
    }
}
