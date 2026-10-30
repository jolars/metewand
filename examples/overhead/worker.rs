//! A controlled worker with separately measured computation and result writing.

use std::{
    fs::{self, File},
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

use crate::core::canonical::CanonicalValue;
use crate::protocol::{EvaluateRequest, EvaluationVerdict, framing::FrameReader};
use crate::runtime::worker_sdk::{Evaluation, WorkerResult, read_json, serve_evaluator};
use serde_json::{Value, json};

fn protocol_file(direction: &str) -> Result<File, Box<dyn std::error::Error>> {
    #[cfg(unix)]
    let inherited = {
        use std::os::fd::FromRawFd;
        let descriptor = std::env::var(format!("METEWAND_PROTOCOL_{direction}_FD"))?.parse()?;
        // The launcher transfers ownership of this dedicated descriptor.
        unsafe { File::from_raw_fd(descriptor) }
    };
    #[cfg(windows)]
    let inherited = {
        use std::os::windows::io::{FromRawHandle, RawHandle};
        let handle: usize =
            std::env::var(format!("METEWAND_PROTOCOL_{direction}_HANDLE"))?.parse()?;
        // The launcher transfers ownership of this dedicated handle.
        unsafe { File::from_raw_handle(handle as RawHandle) }
    };
    // Cloning creates a noninheritable endpoint before benchmark code can spawn children.
    Ok(inherited.try_clone()?)
}

fn evaluate(request: &EvaluateRequest) -> WorkerResult<Evaluation> {
    let result = read_json(&Path::new(request.result_dir()).join("result.json"))?;
    let data = &result["data"];
    let payload = data["payload"].as_str().unwrap_or("");
    let valid = data["answer"] == 42 && payload.bytes().all(|b| b == b'x');
    Ok(Evaluation::new(
        json!({"payload_bytes": payload.len()}),
        if valid {
            EvaluationVerdict::Reached
        } else {
            EvaluationVerdict::Invalid
        },
    ))
}

pub fn serve(role: &str) -> Result<(), Box<dyn std::error::Error>> {
    if role == "problem_evaluator" {
        serve_evaluator(&mut evaluate)?;
        return Ok(());
    }
    let reader = protocol_file("READ")?;
    let mut writer = protocol_file("WRITE")?;
    let mut reader = FrameReader::new(reader);
    let mut parameters = Value::Null;
    while let Some(request) = reader.read_frame()? {
        let method = request["method"].as_str().ok_or("missing method")?;
        let mut response = json!({"id": request["id"], "ok": true});
        match method {
            "hello" => {
                response["protocol"] = json!(1);
                response["worker_id"] = request["worker_id"].clone();
                response["capabilities"] = json!(["one_shot"]);
                response["sdk"] = Value::Null;
            }
            "prepare" => parameters = request["implementation_parameters"].clone(),
            "execute" => {
                let started = Instant::now();
                let work_ns = parameters["work_ns"].as_u64().ok_or("missing work_ns")?;
                let size = parameters["payload_bytes"]
                    .as_u64()
                    .ok_or("missing payload_bytes")?;
                let mut state = 1_u64;
                while started.elapsed() < Duration::from_nanos(work_ns) {
                    state = std::hint::black_box(
                        state.wrapping_mul(6364136223846793005).wrapping_add(1),
                    );
                }
                let work_time = started.elapsed().as_nanos() as u64;
                let encoding = Instant::now();
                let document = CanonicalValue::try_from(json!({
                    "schema_version": 1, "files": [],
                    "data": {"answer": 42, "payload": "x".repeat(usize::try_from(size)?)},
                }))?
                .to_canonical_bytes()?;
                let encoding_time = encoding.elapsed().as_nanos() as u64;
                let writing = Instant::now();
                fs::write(
                    Path::new(request["result_dir"].as_str().ok_or("missing result_dir")?)
                        .join("result.json"),
                    &document,
                )?;
                let writing_time = writing.elapsed().as_nanos() as u64;
                response["result"] = json!({"manifest": "result.json"});
                response["implementation_time_ns"] = json!(work_time);
                response["statistics"] = json!({
                    "work_time_ns": work_time,
                    "result_encoding_time_ns": encoding_time,
                    "result_write_time_ns": writing_time,
                    "worker_execute_time_ns": started.elapsed().as_nanos() as u64,
                    "result_bytes": document.len(),
                });
            }
            "reset" => parameters = Value::Null,
            "shutdown" => {}
            _ => return Err(format!("unexpected method {method}").into()),
        }
        writer.write_all(&serde_json::to_vec(&response)?)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        if method == "shutdown" {
            break;
        }
    }
    Ok(())
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve(&std::env::args().nth(1).ok_or("expected worker role")?)
}
