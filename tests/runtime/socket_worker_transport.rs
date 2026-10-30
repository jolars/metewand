#![cfg(any(unix, windows))]

use crate::protocol::{WorkerIdentity, WorkerRole};
use crate::runtime::{
    worker_process::{WorkerLogLimits, WorkerProcess},
    worker_session::{PhaseTimeouts, WorkerPhase, WorkerSession, WorkerSessionError},
};
use serde_json::json;
use std::{
    io::Write,
    process::Command,
    time::{Duration, Instant},
};

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn python() -> Command {
    if cfg!(windows) {
        let path = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|directory| directory.join("python.exe"))
            .find(|path| path.is_file())
            .expect("Python executable on PATH");
        Command::new(path.canonicalize().unwrap())
    } else {
        Command::new("python3")
    }
}

#[test]
fn authenticated_socket_keeps_fragmented_binary_frames_and_logs_separate() {
    let mut command = python();
    command
        .args([
            "-c",
            r#"
import os, socket, sys
sys.stdout.reconfigure(newline='\n')
sys.stderr.reconfigure(newline='\n')
port = int(os.environ['METEWAND_PROTOCOL_PORT'])
with socket.create_connection(('127.0.0.1', port)) as wrong:
    wrong.sendall(b'wrong token\n')
with socket.create_connection(('127.0.0.1', port)) as connection:
    connection.sendall(os.environ['METEWAND_PROTOCOL_TOKEN'].encode() + b'\n')
    with connection.makefile('rb') as stream:
        frame = stream.readline()
    connection.sendall(frame[:7])
    connection.sendall(frame[7:])
print('stdout log')
print(os.environ['METEWAND_PROTOCOL_TOKEN'])
print('stderr log', file=sys.stderr)
"#,
        ])
        .env("METEWAND_PROTOCOL_TRANSPORT", "socket");
    let mut process = WorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    process
        .protocol_writer()
        .write_all(b"{\"value\":\"\xc3\xa9\",\"number\":1.0000000000000002}\n")
        .unwrap();
    let response = process.protocol_reader().read_frame().unwrap().unwrap();
    assert_eq!(response, json!({"value":"é","number":1.0000000000000002}));
    let output = process.wait().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.bytes(), b"stdout log\n[REDACTED]\n");
    assert_eq!(output.stderr.bytes(), b"stderr log\n");
}

#[cfg(unix)]
#[test]
fn socket_authentication_does_not_require_a_temporary_directory() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime_tests::socket_worker_transport::authenticated_socket_keeps_fragmented_binary_frames_and_logs_separate",
        ])
        .env("TMPDIR", directory.path().join("unavailable"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn startup_deadline_cancels_unconnected_and_unauthenticated_sockets() {
    for source in [
        "import time; time.sleep(30)",
        r#"import os,socket,time
connection=socket.create_connection(('127.0.0.1',int(os.environ['METEWAND_PROTOCOL_PORT'])))
connection.sendall(b'incomplete')
time.sleep(30)"#,
    ] {
        let mut command = python();
        command
            .args(["-c", source])
            .env("METEWAND_PROTOCOL_TRANSPORT", "socket");
        let process = WorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
        let started = Instant::now();
        let error = WorkerSession::connect(
            process,
            WorkerRole::DatasetMaterializer,
            WORKER_ID.parse::<WorkerIdentity>().unwrap(),
            &[],
            PhaseTimeouts {
                hello: Duration::from_millis(50),
                ..PhaseTimeouts::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("hello"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn exited_socket_workers_fail_before_the_hello_deadline() {
    for source in [
        "import sys; sys.exit(0)",
        "import sys; sys.exit(7)",
        "import metewand_nonexistent_worker_dependency",
        r#"import os,socket
connection=socket.create_connection(('127.0.0.1',int(os.environ['METEWAND_PROTOCOL_PORT'])))
connection.sendall(b'incomplete')
os._exit(7)"#,
    ] {
        let mut command = python();
        command
            .args(["-c", source])
            .env("METEWAND_PROTOCOL_TRANSPORT", "socket");
        let process = WorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
        let started = Instant::now();
        let error = WorkerSession::connect(
            process,
            WorkerRole::DatasetMaterializer,
            WORKER_ID.parse().unwrap(),
            &[],
            PhaseTimeouts {
                hello: Duration::from_secs(5),
                ..PhaseTimeouts::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                WorkerSessionError::ProtocolWrite {
                    phase: WorkerPhase::Hello,
                    ref source,
                } if source.kind() == std::io::ErrorKind::BrokenPipe
            ),
            "{error:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn queued_socket_frames_remain_readable_after_worker_exit() {
    let mut command = python();
    command
        .args([
            "-c",
            r#"
import os, socket, sys
with socket.create_connection(('127.0.0.1', int(os.environ['METEWAND_PROTOCOL_PORT']))) as connection:
    connection.sendall(os.environ['METEWAND_PROTOCOL_TOKEN'].encode() + b'\n{"complete":true}\n')
sys.exit(7)
"#,
        ])
        .env("METEWAND_PROTOCOL_TRANSPORT", "socket");
    let mut process = WorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = process.try_wait().unwrap() {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(7));
    let response = process.protocol_reader().read_frame().unwrap().unwrap();
    assert_eq!(response, json!({"complete": true}));
    assert_eq!(process.wait().unwrap().status, status);
}

#[test]
fn managed_outputs_keep_large_payloads_out_of_protocol_frames_and_hash_binary_files() {
    use sha2::{Digest, Sha256};
    let adapter = tempfile::tempdir().unwrap();
    let result = tempfile::tempdir().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let path = crate::runtime::language_adapter::stage("python", adapter.path()).unwrap();
    let mut command = python();
    command.args(["-c", r#"
from metewand import Artifact, Result, serve_implementation

def execute(state, context):
    (context.output_dir / 'payload.bin').write_bytes(bytes([0, 255, 10]))
    return Result({'payload': 'x' * (2 * 1024 * 1024)}, [Artifact('payload.bin', 'application/octet-stream')])

serve_implementation(prepare=lambda context: None, execute=execute)
"#]).env("PYTHONPATH",root.join("packages/python/src"))
        .env("METEWAND_ADAPTER_PATH",path).env("METEWAND_PROTOCOL_TRANSPORT","socket");
    let process = WorkerProcess::spawn(command, WorkerLogLimits::new(4096, 4096)).unwrap();
    let mut session = WorkerSession::connect(
        process,
        WorkerRole::Implementation,
        WORKER_ID.parse().unwrap(),
        &[crate::protocol::Capability::OneShot],
        PhaseTimeouts::default(),
    )
    .unwrap();
    session
        .prepare(
            adapter.path().to_str().unwrap(),
            "contract",
            json!({}),
            json!({}),
            7,
        )
        .unwrap();
    let response = session.execute(result.path().to_str().unwrap()).unwrap();
    assert_eq!(response.manifest(), "result.json");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(result.path().join("result.json")).unwrap()).unwrap();
    assert_eq!(
        document["data"]["payload"].as_str().unwrap().len(),
        2 * 1024 * 1024
    );
    assert_eq!(document["files"][0]["size_bytes"], 3);
    let digest = Sha256::digest([0, 255, 10])
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(document["files"][0]["sha256"], digest);
    assert!(!result.path().join(".metewand-output.json").exists());
    assert!(session.shutdown().unwrap().status.success());
}
