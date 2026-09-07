use std::str::FromStr;

use metewand_protocol::{
    Capability, HandshakeError, HelloRequest, HelloResponse, MessageEncodeError, RequestId,
    RequestTracker, RequestTrackerError, SdkMetadata, WorkerIdentity, WorkerRole, decode_message,
    encode_message, framing::FrameReader, framing::MAX_LINE_BYTES, negotiate_protocol,
    validate_hello_response,
};
use serde_json::json;

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn hello_messages_have_the_exact_version_one_wire_shape() {
    let request = HelloRequest::new(
        RequestId::from_str("0").expect("request ID must be valid"),
        WorkerRole::Implementation,
        WorkerIdentity::from_str(WORKER_ID).expect("worker identity must be valid"),
    );
    assert_eq!(
        serde_json::to_value(&request).expect("request must serialize"),
        json!({
            "id": "0",
            "method": "hello",
            "protocols": [1],
            "role": "implementation",
            "worker_id": WORKER_ID,
        })
    );

    let response = HelloResponse::new(
        request.id().clone(),
        request.worker_id().clone(),
        Some(SdkMetadata::new("metewand-python", "0.1.0").expect("metadata must be valid")),
        [Capability::OneShot, Capability::Applicability],
    );
    assert_eq!(
        serde_json::to_value(&response).expect("response must serialize"),
        json!({
            "id": "0",
            "ok": true,
            "protocol": 1,
            "worker_id": WORKER_ID,
            "sdk": {"name": "metewand-python", "version": "0.1.0"},
            "capabilities": ["one_shot", "applicability"],
        })
    );
}

#[test]
fn messages_are_canonical_newline_terminated_frames() {
    let request = HelloRequest::new(
        RequestId::from_str("0").expect("request ID must be valid"),
        WorkerRole::Implementation,
        WorkerIdentity::from_str(WORKER_ID).expect("worker identity must be valid"),
    );
    let encoded = encode_message(&request).expect("request must encode");
    assert_eq!(
        encoded,
        format!(
            "{{\"id\":\"0\",\"method\":\"hello\",\"protocols\":[1],\"role\":\"implementation\",\"worker_id\":\"{WORKER_ID}\"}}\n"
        )
        .as_bytes()
    );

    let mut frames = FrameReader::new(encoded.as_slice());
    let frame = frames
        .read_frame()
        .expect("frame must be valid")
        .expect("frame must exist");
    let decoded = decode_message::<HelloRequest>(frame).expect("message must decode");
    assert_eq!(decoded, request);
}

#[test]
fn strict_messages_reject_unknown_fields_and_invalid_discriminators() {
    for invalid in [
        json!({
            "id": "0", "method": "hello", "protocols": [1],
            "role": "implementation", "worker_id": WORKER_ID, "extra": true,
        }),
        json!({
            "id": "0", "method": "goodbye", "protocols": [1],
            "role": "implementation", "worker_id": WORKER_ID,
        }),
    ] {
        assert!(serde_json::from_value::<HelloRequest>(invalid).is_err());
    }

    for invalid in [
        json!({
            "id": "0", "ok": false, "protocol": 1, "worker_id": WORKER_ID,
            "sdk": null, "capabilities": ["one_shot"],
        }),
        json!({
            "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
            "sdk": null, "capabilities": ["one_shot"], "extra": true,
        }),
        json!({
            "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
            "capabilities": ["one_shot"],
        }),
    ] {
        assert!(serde_json::from_value::<HelloResponse>(invalid).is_err());
    }
}

#[test]
fn message_fields_obey_the_schema_boundaries() {
    for invalid in [
        json!({
            "id": "0", "method": "hello", "protocols": [],
            "role": "implementation", "worker_id": WORKER_ID,
        }),
        json!({
            "id": "0", "method": "hello", "protocols": [1, 1],
            "role": "implementation", "worker_id": WORKER_ID,
        }),
        json!({
            "id": "0", "method": "hello", "protocols": [1],
            "role": "implementation",
            "worker_id": "mw1-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        }),
    ] {
        assert!(serde_json::from_value::<HelloRequest>(invalid).is_err());
    }

    for invalid in [
        json!({
            "id": "0", "ok": true, "protocol": 2, "worker_id": WORKER_ID,
            "sdk": null, "capabilities": [],
        }),
        json!({
            "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
            "sdk": {"name": "metewand-python", "version": "0.1.0", "extra": true},
            "capabilities": [],
        }),
        json!({
            "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
            "sdk": null, "capabilities": ["unknown"],
        }),
    ] {
        assert!(serde_json::from_value::<HelloResponse>(invalid).is_err());
    }
}

#[test]
fn encoder_enforces_the_complete_line_limit() {
    let request = HelloRequest::new(
        RequestId::from_str(&"a".repeat(MAX_LINE_BYTES)).expect("request ID must be nonempty"),
        WorkerRole::Implementation,
        WorkerIdentity::from_str(WORKER_ID).expect("worker identity must be valid"),
    );

    assert!(matches!(
        encode_message(&request),
        Err(MessageEncodeError::LineTooLong {
            limit: MAX_LINE_BYTES
        })
    ));
}

#[test]
fn response_capabilities_are_unique_and_normalized() {
    let parsed = serde_json::from_value::<HelloResponse>(json!({
        "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
        "sdk": null, "capabilities": ["streaming_profile", "one_shot"],
    }))
    .expect("unordered unique capabilities must decode");
    assert_eq!(
        parsed.capabilities(),
        &[Capability::OneShot, Capability::StreamingProfile]
    );
    assert!(
        serde_json::from_value::<HelloResponse>(json!({
            "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
            "sdk": null, "capabilities": ["one_shot", "one_shot"],
        }))
        .is_err()
    );
}

#[test]
fn request_ids_and_metadata_must_not_be_empty() {
    assert!(RequestId::from_str("").is_err());
    assert!(SdkMetadata::new("", "0.1.0").is_err());
    assert!(SdkMetadata::new("metewand-python", "").is_err());

    for invalid in [
        json!({
            "id": "", "method": "hello", "protocols": [1],
            "role": "implementation", "worker_id": WORKER_ID,
        }),
        json!({
            "id": "0", "ok": true, "protocol": 1, "worker_id": WORKER_ID,
            "sdk": {"name": "", "version": "0.1.0"}, "capabilities": [],
        }),
    ] {
        assert!(serde_json::from_value::<serde_json::Value>(invalid.clone()).is_ok());
        assert!(
            serde_json::from_value::<HelloRequest>(invalid.clone()).is_err()
                || serde_json::from_value::<HelloResponse>(invalid).is_err()
        );
    }
}

#[test]
fn every_version_one_role_and_capability_has_a_stable_spelling() {
    assert_eq!(
        serde_json::to_value([
            WorkerRole::DatasetMaterializer,
            WorkerRole::Implementation,
            WorkerRole::ProblemEvaluator,
        ])
        .expect("roles must serialize"),
        json!([
            "dataset_materializer",
            "implementation",
            "problem_evaluator"
        ])
    );
    assert_eq!(
        serde_json::to_value([
            Capability::OneShot,
            Capability::Applicability,
            Capability::FreshSequence,
            Capability::StreamingProfile,
        ])
        .expect("capabilities must serialize"),
        json!([
            "one_shot",
            "applicability",
            "fresh_sequence",
            "streaming_profile"
        ])
    );
}

#[test]
fn version_one_negotiation_requires_a_common_version() {
    assert_eq!(negotiate_protocol(&[2, 1]).expect("version 1 is common"), 1);
    assert!(matches!(
        negotiate_protocol(&[2, 3]),
        Err(HandshakeError::NoCommonProtocol { .. })
    ));
}

#[test]
fn handshake_validation_checks_request_worker_and_selected_capabilities() {
    let request = HelloRequest::new(
        RequestId::from_str("0").expect("request ID must be valid"),
        WorkerRole::Implementation,
        WorkerIdentity::from_str(WORKER_ID).expect("worker identity must be valid"),
    );
    let response = HelloResponse::new(
        request.id().clone(),
        request.worker_id().clone(),
        None,
        [Capability::OneShot, Capability::Applicability],
    );

    let negotiated = validate_hello_response(&request, &response, &[Capability::OneShot])
        .expect("matching handshake must succeed");
    assert_eq!(negotiated.role(), WorkerRole::Implementation);
    assert_eq!(negotiated.protocol(), 1);
    assert_eq!(negotiated.sdk(), None);
    assert_eq!(negotiated.selected_capabilities(), &[Capability::OneShot]);
    assert_eq!(
        negotiated.reported_capabilities(),
        &[Capability::OneShot, Capability::Applicability]
    );

    let wrong_id = HelloResponse::new(
        RequestId::from_str("other").expect("request ID must be valid"),
        request.worker_id().clone(),
        None,
        [Capability::OneShot],
    );
    assert!(matches!(
        validate_hello_response(&request, &wrong_id, &[Capability::OneShot]),
        Err(HandshakeError::MismatchedRequestId { .. })
    ));

    let wrong_worker = HelloResponse::new(
        request.id().clone(),
        WorkerIdentity::from_str(
            "mw1-resolved-worker-ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        )
        .expect("worker identity must be valid"),
        None,
        [Capability::OneShot],
    );
    assert!(matches!(
        validate_hello_response(&request, &wrong_worker, &[Capability::OneShot]),
        Err(HandshakeError::WorkerIdentityMismatch { .. })
    ));

    let missing_capability = HelloResponse::new(
        request.id().clone(),
        request.worker_id().clone(),
        None,
        [Capability::Applicability],
    );
    assert!(matches!(
        validate_hello_response(&request, &missing_capability, &[Capability::OneShot]),
        Err(HandshakeError::MissingCapability {
            capability: Capability::OneShot
        })
    ));
}

#[test]
fn one_session_permits_only_one_pending_orchestrator_request() {
    let mut requests = RequestTracker::new();
    let hello = requests
        .begin_request()
        .expect("the first request must start");
    assert_eq!(hello.as_str(), "0");
    assert!(matches!(
        requests.begin_request(),
        Err(RequestTrackerError::RequestAlreadyPending { ref id }) if id == &hello
    ));

    assert!(matches!(
        requests.complete_response(&RequestId::from_str("late").expect("request ID must be valid")),
        Err(RequestTrackerError::MismatchedRequestId { .. })
    ));
    assert_eq!(requests.pending_request(), Some(&hello));
    requests
        .complete_response(&hello)
        .expect("the matching response must complete the request");

    let next = requests
        .begin_request()
        .expect("the next serial request must start");
    assert_eq!(next.as_str(), "1");
}
