use metewand_protocol::{
    Acknowledgement, EvaluateRequest, EvaluateResponse, ExecuteRequest, ExecuteResponse,
    ExecuteStreamRequest, ExecuteStreamResponse, HelloRequest, HelloResponse, MaterializeRequest,
    MaterializeResponse, PrepareRequest, ResetRequest, ShutdownRequest, StreamCheckpoint,
    StreamDecision, WireMessage, WorkerFailure, WorkerFailureResponse, decode_message,
    encode_message, framing::FrameReader,
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

#[test]
fn version_one_messages_preserve_the_gate3_compatibility_boundary() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../fixtures/worker-protocol/v1/compatibility.json"
    ))
    .unwrap();
    assert_eq!(vectors["version"], metewand_protocol::WIRE_PROTOCOL_VERSION);
    for vector in vectors["messages"].as_array().unwrap() {
        let frame = vector["frame"].as_str().unwrap();
        let valid = vector["valid"].as_bool().unwrap();
        let message = FrameReader::new(frame.as_bytes())
            .read_frame()
            .unwrap()
            .unwrap();
        let accepted = match vector["message_type"].as_str().unwrap() {
            "hello-response" => decode_message::<HelloResponse>(message).is_ok(),
            "execute-request" => decode_message::<ExecuteRequest>(message).is_ok(),
            "execute-response" => decode_message::<ExecuteResponse>(message).is_ok(),
            "execute-stream-request" => decode_message::<ExecuteStreamRequest>(message).is_ok(),
            "execute-stream-response" => decode_message::<ExecuteStreamResponse>(message).is_ok(),
            "stream-checkpoint" => decode_message::<StreamCheckpoint>(message).is_ok(),
            "stream-decision" => decode_message::<StreamDecision>(message).is_ok(),
            kind => panic!("uncovered compatibility message: {kind}"),
        };
        assert_eq!(accepted, valid, "{}", vector["name"]);
        if valid {
            match vector["message_type"].as_str().unwrap() {
                "hello-response" => round_trip::<HelloResponse>(frame),
                "execute-request" => round_trip::<ExecuteRequest>(frame),
                "execute-response" => round_trip::<ExecuteResponse>(frame),
                "execute-stream-request" => round_trip::<ExecuteStreamRequest>(frame),
                "execute-stream-response" => round_trip::<ExecuteStreamResponse>(frame),
                "stream-checkpoint" => round_trip::<StreamCheckpoint>(frame),
                "stream-decision" => round_trip::<StreamDecision>(frame),
                kind => panic!("uncovered compatibility message: {kind}"),
            }
        }
    }
}
