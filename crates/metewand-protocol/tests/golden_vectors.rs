use metewand_protocol::{
    Acknowledgement, EvaluateRequest, EvaluateResponse, ExecuteRequest, ExecuteResponse,
    HelloRequest, HelloResponse, MaterializeRequest, MaterializeResponse, PrepareRequest,
    ResetRequest, ShutdownRequest, WireMessage, WorkerFailure, WorkerFailureResponse,
    decode_message, encode_message, framing::FrameReader,
};
use serde_json::Value;

fn round_trip<M: WireMessage>(frame: &str) {
    let value = FrameReader::new(frame.as_bytes())
        .read_frame()
        .unwrap()
        .unwrap();
    let message = decode_message::<M>(value).unwrap();
    assert_eq!(encode_message(&message).unwrap(), frame.as_bytes());
}

fn request_round_trip(method: &str, frame: &str, valid: bool) {
    fn check<M: WireMessage>(frame: &str, valid: bool) {
        if valid {
            round_trip::<M>(frame);
        } else {
            let value = FrameReader::new(frame.as_bytes())
                .read_frame()
                .unwrap()
                .unwrap();
            assert!(decode_message::<M>(value).is_err(), "{frame}");
        }
    }
    match method {
        "hello" => check::<HelloRequest>(frame, valid),
        "materialize" => check::<MaterializeRequest>(frame, valid),
        "prepare" => check::<PrepareRequest>(frame, valid),
        "execute" => check::<ExecuteRequest>(frame, valid),
        "reset" => check::<ResetRequest>(frame, valid),
        "evaluate" => check::<EvaluateRequest>(frame, valid),
        "shutdown" => check::<ShutdownRequest>(frame, valid),
        _ => panic!("uncovered golden method: {method}"),
    }
}

fn response_round_trip(method: &str, frame: &str) {
    let response: Value = serde_json::from_str(frame).unwrap();
    if response["ok"] == false {
        round_trip::<WorkerFailureResponse>(frame);
        return;
    }
    match method {
        "hello" => round_trip::<HelloResponse>(frame),
        "materialize" => round_trip::<MaterializeResponse>(frame),
        "execute" => round_trip::<ExecuteResponse>(frame),
        "evaluate" => round_trip::<EvaluateResponse>(frame),
        "prepare" | "reset" | "shutdown" => round_trip::<Acknowledgement>(frame),
        _ => panic!("uncovered golden response: {method}"),
    }
}

#[test]
fn shared_sdk_protocol_vectors_have_exact_typed_frames() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../fixtures/worker-protocol/v1/golden.json"
    ))
    .unwrap();
    assert_eq!(vectors["version"], metewand_protocol::WIRE_PROTOCOL_VERSION);
    for session in vectors["sessions"].as_array().unwrap() {
        for step in session["exchanges"].as_array().unwrap() {
            let request = step["request"].as_str().unwrap();
            let value: Value = serde_json::from_str(request).unwrap();
            let method = value["method"].as_str().unwrap();
            request_round_trip(method, request, step["request_valid"].as_bool().unwrap());
            for sdk in ["metewand-python", "metewand-r"] {
                response_round_trip(
                    method,
                    &step["response"].as_str().unwrap().replace("@SDK@", sdk),
                );
            }
        }
    }
    for vector in vectors["responses"].as_array().unwrap() {
        response_round_trip(
            vector["type"].as_str().unwrap(),
            vector["frame"].as_str().unwrap(),
        );
    }
    for vector in vectors["rejected_frames"].as_array().unwrap() {
        let hex = vector["hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert!(
            FrameReader::new(bytes.as_slice()).read_frame().is_err(),
            "{}",
            vector["name"]
        );
    }
}

#[test]
fn shared_sdk_error_vectors_preserve_codes_messages_and_details() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../fixtures/worker-protocol/v1/errors.json"
    ))
    .unwrap();
    assert_eq!(vectors["version"], metewand_protocol::WIRE_PROTOCOL_VERSION);
    for vector in vectors["accepted"].as_array().unwrap() {
        let failure: WorkerFailure = serde_json::from_value(vector["error"].clone()).unwrap();
        let constructed = WorkerFailure::new(
            failure.code(),
            failure.message(),
            failure.details().cloned(),
        )
        .unwrap();
        assert_eq!(constructed, failure);
        round_trip::<WorkerFailureResponse>(vector["frame"].as_str().unwrap());
    }
    for vector in vectors["rejected"].as_array().unwrap() {
        assert!(
            serde_json::from_value::<WorkerFailure>(vector.clone()).is_err(),
            "{vector}"
        );
    }
}
