#![cfg(unix)]

use std::{
    env,
    fs::File,
    io::{self, BufRead, BufReader, Write},
    os::fd::FromRawFd,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use metewand_runtime::worker_process::{PosixWorkerProcess, WorkerLogLimits};
use serde_json::json;

const FIXTURE_MODE: &str = "METEWAND_TEST_WORKER_MODE";
const FLOOD_BYTES: usize = 2 * 1024 * 1024;

#[test]
fn fixture_worker() {
    match env::var(FIXTURE_MODE).as_deref() {
        Ok("exchange") => run_exchange_fixture(),
        Ok("flood") => run_flood_fixture(),
        Ok("closed-stdio-parent") => run_closed_stdio_parent_fixture(),
        _ => {}
    }
}

#[test]
fn protocol_uses_dedicated_inherited_pipes() {
    let mut worker = PosixWorkerProcess::spawn(
        fixture_command("exchange"),
        WorkerLogLimits::new(1024, 1024),
    )
    .expect("fixture worker must spawn");

    worker
        .protocol_writer()
        .write_all(b"{\"id\":\"request-1\",\"method\":\"ping\"}\n")
        .expect("request must be written to the dedicated pipe");
    worker
        .protocol_writer()
        .flush()
        .expect("request must be flushed");

    assert_eq!(
        worker
            .protocol_reader()
            .read_frame()
            .expect("response frame must be valid"),
        Some(json!({"id": "request-1", "ok": true}))
    );

    let output = worker.wait().expect("fixture worker must finish");
    assert!(output.status.success());
    assert!(
        output
            .stdout
            .bytes()
            .windows(b"worker stdout\n".len())
            .any(|window| window == b"worker stdout\n")
    );
    assert!(
        output
            .stderr
            .bytes()
            .windows(b"worker stderr\n".len())
            .any(|window| window == b"worker stderr\n")
    );
}

#[test]
fn stdout_and_stderr_continue_draining_after_capture_limits() {
    let stdout_limit = 31;
    let stderr_limit = 47;
    let mut worker = PosixWorkerProcess::spawn(
        fixture_command("flood"),
        WorkerLogLimits::new(stdout_limit, stderr_limit),
    )
    .expect("fixture worker must spawn");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if worker
            .try_wait()
            .expect("fixture status must remain observable")
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            worker.kill().expect("blocked fixture must be killed");
            let _ = worker.wait();
            panic!("fixture blocked because a full log pipe was not drained");
        }
        thread::sleep(Duration::from_millis(10));
    }

    let output = worker.wait().expect("fixture worker must finish");
    assert!(output.status.success());
    assert_eq!(output.stdout.bytes().len(), stdout_limit);
    assert_eq!(output.stderr.bytes().len(), stderr_limit);
    assert!(output.stdout.truncated());
    assert!(output.stderr.truncated());
    assert!(output.stdout.total_bytes() >= FLOOD_BYTES as u64);
    assert!(output.stderr.total_bytes() >= FLOOD_BYTES as u64);
}

#[test]
fn protocol_descriptors_remain_dedicated_when_parent_stdio_is_closed() {
    let status = fixture_command("closed-stdio-parent")
        .status()
        .expect("isolated parent fixture must run");

    assert!(status.success());
}

fn fixture_command(mode: &str) -> Command {
    let mut command = Command::new(env::current_exe().expect("test executable must have a path"));
    command
        .arg("--exact")
        .arg("fixture_worker")
        .arg("--nocapture")
        .env(FIXTURE_MODE, mode);
    command
}

fn run_exchange_fixture() {
    let read_fd = protocol_fd("METEWAND_PROTOCOL_READ_FD");
    let write_fd = protocol_fd("METEWAND_PROTOCOL_WRITE_FD");
    assert_ne!(read_fd, write_fd);
    assert!(read_fd > 2);
    assert!(write_fd > 2);

    // SAFETY: The runtime grants the worker ownership of each inherited
    // protocol descriptor, and this fixture constructs exactly one owner for
    // each descriptor.
    let read_pipe = unsafe { File::from_raw_fd(read_fd) };
    // SAFETY: See the ownership argument above.
    let mut write_pipe = unsafe { File::from_raw_fd(write_fd) };
    let mut request = String::new();
    BufReader::new(read_pipe)
        .read_line(&mut request)
        .expect("request must be readable from the inherited descriptor");
    assert_eq!(request, "{\"id\":\"request-1\",\"method\":\"ping\"}\n");

    io::stdout()
        .write_all(b"worker stdout\n")
        .expect("stdout log must be writable");
    io::stderr()
        .write_all(b"worker stderr\n")
        .expect("stderr log must be writable");
    write_pipe
        .write_all(b"{\"id\":\"request-1\",\"ok\":true}\n")
        .expect("response must be writable to the inherited descriptor");
    write_pipe.flush().expect("response must be flushed");
}

fn run_flood_fixture() {
    let stdout = thread::spawn(|| write_repeated(io::stdout(), b'o'));
    let stderr = thread::spawn(|| write_repeated(io::stderr(), b'e'));
    stdout.join().expect("stdout writer must not panic");
    stderr.join().expect("stderr writer must not panic");
}

fn run_closed_stdio_parent_fixture() -> ! {
    for descriptor in 0..=2 {
        // SAFETY: This isolated fixture process deliberately takes ownership
        // of each standard descriptor exactly once and exits without returning
        // to the test harness.
        drop(unsafe { File::from_raw_fd(descriptor) });
    }

    let mut worker = PosixWorkerProcess::spawn(
        fixture_command("exchange"),
        WorkerLogLimits::new(1024, 1024),
    )
    .expect("the transport must not reuse standard descriptors");
    worker
        .protocol_writer()
        .write_all(b"{\"id\":\"request-1\",\"method\":\"ping\"}\n")
        .expect("request pipe must remain usable");
    worker
        .protocol_writer()
        .flush()
        .expect("request must be flushed");
    assert_eq!(
        worker
            .protocol_reader()
            .read_frame()
            .expect("response pipe must remain usable"),
        Some(json!({"id": "request-1", "ok": true}))
    );
    assert!(
        worker
            .wait()
            .expect("nested worker must finish")
            .status
            .success()
    );

    std::process::exit(0);
}

fn write_repeated(mut destination: impl Write, byte: u8) {
    let chunk = [byte; 8192];
    for _ in 0..(FLOOD_BYTES / chunk.len()) {
        destination
            .write_all(&chunk)
            .expect("the orchestrator must continue draining logs");
    }
    destination
        .flush()
        .expect("the complete log must be flushed");
}

fn protocol_fd(name: &str) -> i32 {
    env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set"))
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be a descriptor number"))
}
