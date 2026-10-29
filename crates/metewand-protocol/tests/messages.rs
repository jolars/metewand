use std::str::FromStr;

use metewand_protocol::{
    Acknowledgement, Capability, EvaluateRequest, ExecuteBufferedRequest, ExecuteBufferedResponse,
    ExecuteRequest, ExecuteResponse, HandshakeError, HelloRequest, HelloResponse,
    MaterializeRequest, MaterializeResponse, MessageEncodeError, OperationResponse, PrepareRequest,
    RequestId, RequestTracker, RequestTrackerError, ResetRequest, SdkMetadata, ShutdownRequest,
    WorkerFailure, WorkerFailureCode, WorkerFailureResponse, WorkerIdentity, WorkerRole,
    decode_message, decode_operation_response, encode_message, framing::FrameReader,
    framing::MAX_LINE_BYTES, negotiate_protocol, validate_hello_response,
};
use serde_json::json;

const WORKER_ID: &str =
    "mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn buffered_delivery_has_stable_frames_and_strict_envelopes() {
    let id: RequestId = "1".parse().unwrap();
    let request = ExecuteBufferedRequest::new(
        id.clone(),
        "/private/results",
        json!({"instrumentation":{"observer":"gap"}}),
    );
    assert_eq!(
        encode_message(&request).unwrap(),
        b"{\"id\":\"1\",\"method\":\"execute_buffered\",\"profile\":{\"instrumentation\":{\"observer\":\"gap\"}},\"result_root\":\"/private/results\"}\n"
    );
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(
        decode_message::<ExecuteBufferedRequest>(value.clone()).unwrap(),
        request
    );
    for (field, invalid) in [
        ("method", json!("execute")),
        ("result_root", json!("")),
        ("profile", json!([])),
        ("unknown", json!(true)),
    ] {
        let mut invalid_request = value.clone();
        invalid_request[field] = invalid;
        assert!(decode_message::<ExecuteBufferedRequest>(invalid_request).is_err());
    }
    let response = ExecuteBufferedResponse::new(id, "buffer.json");
    assert_eq!(
        encode_message(&response).unwrap(),
        b"{\"buffer\":{\"manifest\":\"buffer.json\"},\"id\":\"1\",\"ok\":true}\n"
    );
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(
        decode_message::<ExecuteBufferedResponse>(value.clone()).unwrap(),
        response
    );
    for (field, invalid) in [
        ("ok", json!(false)),
        ("buffer", json!({"manifest":""})),
        ("buffer", json!({"manifest":"buffer.json", "complete":true})),
        ("unknown", json!(true)),
    ] {
        let mut invalid_response = value.clone();
        invalid_response[field] = invalid;
        assert!(decode_message::<ExecuteBufferedResponse>(invalid_response).is_err());
    }
}

#[test]
fn buffered_capability_and_resource_failure_are_available_in_version_one() {
    let hello = HelloRequest::new(
        "0".parse().unwrap(),
        WorkerRole::Implementation,
        WORKER_ID.parse().unwrap(),
    );
    let response = HelloResponse::new(
        "0".parse().unwrap(),
        WORKER_ID.parse().unwrap(),
        None,
        [Capability::BufferedProfile],
    );
    assert!(validate_hello_response(&hello, &response, &[Capability::BufferedProfile]).is_ok());
    assert!(matches!(
        validate_hello_response(&hello, &response, &[Capability::OneShot]),
        Err(HandshakeError::MissingCapability { .. })
    ));
    let failure = json!({"id":"1", "ok":false, "error":{
        "code":"resource_limit", "message":"Terminal snapshot exceeds capacity."
    }});
    assert_eq!(
        decode_message::<WorkerFailureResponse>(failure)
            .unwrap()
            .error()
            .code(),
        WorkerFailureCode::ResourceLimit
    );
}

#[test]
fn scientific_rules_extend_only_negotiated_evaluator_requests() {
    let id = RequestId::from_str("1").unwrap();
    let rules = json!({"preprocessing": {}, "validity": {"finite": true}, "one_shot_completion": {"error": 0.01}});
    let request = EvaluateRequest::new(
        id.clone(),
        "/data",
        "contract",
        json!({}),
        "/result",
        "/metrics",
    )
    .with_problem_semantics(rules.clone());
    let bytes = encode_message(&request).unwrap();
    assert_eq!(bytes, b"{\"dataset_dir\":\"/data\",\"id\":\"1\",\"method\":\"evaluate\",\"metrics_path\":\"/metrics\",\"problem_contract_id\":\"contract\",\"problem_parameters\":{},\"problem_semantics\":{\"one_shot_completion\":{\"error\":0.01},\"preprocessing\":{},\"validity\":{\"finite\":true}},\"result_dir\":\"/result\"}\n");
    let mut value = serde_json::to_value(&request).unwrap();
    assert_eq!(
        decode_message::<EvaluateRequest>(value.clone())
            .unwrap()
            .problem_semantics(),
        Some(&rules)
    );
    for invalid in [json!(null), json!([]), json!(true)] {
        value["problem_semantics"] = invalid;
        assert!(decode_message::<EvaluateRequest>(value.clone()).is_err());
    }
    for role in [
        WorkerRole::Implementation,
        WorkerRole::DatasetMaterializer,
        WorkerRole::ProblemEvaluator,
    ] {
        let hello = HelloRequest::new(id.clone(), role, WORKER_ID.parse().unwrap());
        let response = HelloResponse::new(
            id.clone(),
            WORKER_ID.parse().unwrap(),
            None,
            [Capability::ProblemSemantics],
        );
        let selected = validate_hello_response(&hello, &response, &[Capability::ProblemSemantics]);
        assert_eq!(selected.is_ok(), role == WorkerRole::ProblemEvaluator);
        assert!(
            validate_hello_response(
                &hello,
                &HelloResponse::new(id.clone(), WORKER_ID.parse().unwrap(), None, []),
                &[Capability::ProblemSemantics]
            )
            .is_err()
        );
    }
}

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
fn one_shot_requests_have_exact_version_one_wire_shapes() {
    let id = RequestId::from_str("1").expect("request ID must be valid");
    let cases = [
        serde_json::to_value(MaterializeRequest::new(
            id.clone(),
            "/bundle",
            json!({"rows": 100}),
            17,
            "/output",
        ))
        .expect("materialize request must serialize"),
        serde_json::to_value(PrepareRequest::new(
            id.clone(),
            "/dataset",
            "mw1-problem-contract-abc",
            json!({"lambda": 0.1}),
            json!({"solver": "fast"}),
            23,
        ))
        .expect("prepare request must serialize"),
        serde_json::to_value(ExecuteRequest::new(id.clone(), "/result"))
            .expect("execute request must serialize"),
        serde_json::to_value(ResetRequest::new(id.clone())).expect("reset request must serialize"),
        serde_json::to_value(EvaluateRequest::new(
            id.clone(),
            "/dataset",
            "mw1-problem-contract-abc",
            json!({"lambda": 0.1}),
            "/result",
            "/metrics.json",
        ))
        .expect("evaluate request must serialize"),
        serde_json::to_value(ShutdownRequest::new(id)).expect("shutdown request must serialize"),
    ];

    assert_eq!(
        cases,
        [
            json!({
                "id": "1", "method": "materialize", "source_dir": "/bundle",
                "dataset_parameters": {"rows": 100}, "dataset_seed": 17,
                "output_dir": "/output",
            }),
            json!({
                "id": "1", "method": "prepare", "dataset_dir": "/dataset",
                "problem_contract_id": "mw1-problem-contract-abc",
                "problem_parameters": {"lambda": 0.1},
                "implementation_parameters": {"solver": "fast"},
                "implementation_seed": 23,
            }),
            json!({"id": "1", "method": "execute", "result_dir": "/result"}),
            json!({"id": "1", "method": "reset"}),
            json!({
                "id": "1", "method": "evaluate", "dataset_dir": "/dataset",
                "problem_contract_id": "mw1-problem-contract-abc",
                "problem_parameters": {"lambda": 0.1}, "result_dir": "/result",
                "metrics_path": "/metrics.json",
            }),
            json!({"id": "1", "method": "shutdown"}),
        ]
    );
}

#[test]
fn one_shot_responses_and_worker_failures_have_exact_wire_shapes() {
    let id = RequestId::from_str("2").expect("request ID must be valid");
    let materialized = MaterializeResponse::new(id.clone(), "dataset-manifest.json");
    let executed = ExecuteResponse::new(
        id.clone(),
        Some(18_342_011),
        "result.json",
        serde_json::from_value(json!({"iterations": 37})).expect("statistics must be an object"),
    );
    let acknowledged = Acknowledgement::new(id.clone());
    let failure = WorkerFailureResponse::new(
        id,
        WorkerFailure::new(
            WorkerFailureCode::OperationFailed,
            "the solver did not converge",
            Some(
                serde_json::from_value(json!({"iterations": 100}))
                    .expect("details must be an object"),
            ),
        )
        .expect("worker failure must be valid"),
    );

    assert_eq!(
        serde_json::to_value(materialized).expect("response must serialize"),
        json!({"id": "2", "ok": true, "dataset": {"manifest": "dataset-manifest.json"}})
    );
    assert_eq!(
        serde_json::to_value(executed).expect("response must serialize"),
        json!({
            "id": "2", "ok": true, "implementation_time_ns": 18342011,
            "result": {"manifest": "result.json"}, "statistics": {"iterations": 37},
        })
    );
    assert_eq!(
        serde_json::to_value(acknowledged).expect("response must serialize"),
        json!({"id": "2", "ok": true})
    );
    assert_eq!(
        serde_json::to_value(failure).expect("response must serialize"),
        json!({
            "id": "2", "ok": false,
            "error": {
                "code": "operation_failed", "message": "the solver did not converge",
                "details": {"iterations": 100},
            },
        })
    );
}

#[test]
fn operation_response_decoding_correlates_and_distinguishes_failures() {
    let mut requests = RequestTracker::new();
    let id = requests.begin_request().expect("request must begin");
    let response = decode_operation_response::<Acknowledgement>(
        json!({"id": id.as_str(), "ok": true}),
        &mut requests,
    )
    .expect("response must decode");
    assert!(matches!(response, OperationResponse::Success(_)));

    let id = requests.begin_request().expect("request must begin");
    let response = decode_operation_response::<Acknowledgement>(
        json!({
            "id": id.as_str(), "ok": false,
            "error": {"code": "internal_error", "message": "fixture failed"},
        }),
        &mut requests,
    )
    .expect("response must decode");
    assert!(matches!(
        response,
        OperationResponse::Failure(ref failure)
            if failure.code() == WorkerFailureCode::InternalError
                && failure.message() == "fixture failed"
                && failure.details().is_none()
    ));
}

#[test]
fn operation_messages_reject_wrong_methods_shapes_and_discriminators() {
    assert!(
        serde_json::from_value::<ExecuteRequest>(
            json!({"id": "1", "method": "execute", "result_dir": "/result", "extra": true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<PrepareRequest>(json!({
            "id": "1", "method": "execute", "dataset_dir": "/dataset",
            "problem_contract_id": "contract", "problem_parameters": {},
            "implementation_parameters": {}, "implementation_seed": 1,
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<ExecuteResponse>(json!({
            "id": "1", "ok": false, "implementation_time_ns": null,
            "result": {"manifest": "result.json"}, "statistics": {},
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkerFailureResponse>(json!({
            "id": "1", "ok": false,
            "error": {"code": "not valid", "message": "failed"},
        }))
        .is_err()
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
            Capability::BufferedProfile,
            Capability::FreshSequence,
            Capability::StreamingProfile,
        ])
        .expect("capabilities must serialize"),
        json!([
            "one_shot",
            "applicability",
            "buffered_profile",
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
fn reported_capabilities_are_selected_explicitly() {
    let request = HelloRequest::new(
        RequestId::from_str("0").expect("request ID must be valid"),
        WorkerRole::Implementation,
        WorkerIdentity::from_str(WORKER_ID).expect("worker identity must be valid"),
    );
    let response = HelloResponse::new(
        request.id().clone(),
        request.worker_id().clone(),
        None,
        [
            Capability::OneShot,
            Capability::Applicability,
            Capability::FreshSequence,
            Capability::StreamingProfile,
        ],
    );

    let negotiated = validate_hello_response(&request, &response, &[Capability::OneShot])
        .expect("reported future capabilities must remain inert");
    assert_eq!(negotiated.selected_capabilities(), &[Capability::OneShot]);
    assert_eq!(
        validate_hello_response(&request, &response, &[Capability::FreshSequence])
            .unwrap()
            .selected_capabilities(),
        &[Capability::FreshSequence]
    );
    assert_eq!(
        validate_hello_response(&request, &response, &[Capability::StreamingProfile])
            .unwrap()
            .selected_capabilities(),
        &[Capability::StreamingProfile]
    );
}

#[test]
fn capabilities_can_be_selected_only_for_implementation_workers() {
    let request = HelloRequest::new(
        RequestId::from_str("0").expect("request ID must be valid"),
        WorkerRole::ProblemEvaluator,
        WorkerIdentity::from_str(WORKER_ID).expect("worker identity must be valid"),
    );
    let response = HelloResponse::new(
        request.id().clone(),
        request.worker_id().clone(),
        None,
        [Capability::OneShot],
    );

    assert!(matches!(
        validate_hello_response(&request, &response, &[Capability::OneShot]),
        Err(HandshakeError::CapabilityRoleMismatch {
            role: WorkerRole::ProblemEvaluator,
            capability: Capability::OneShot,
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

#[test]
fn evaluation_requires_an_explicit_scientific_verdict() {
    use metewand_protocol::{EvaluateResponse, EvaluationVerdict};
    let response = EvaluateResponse::new(
        RequestId::from_str("1").unwrap(),
        EvaluationVerdict::Reached,
    );
    assert_eq!(
        encode_message(&response).unwrap(),
        b"{\"id\":\"1\",\"ok\":true,\"verdict\":\"reached\"}\n"
    );
    for verdict in ["invalid", "reached", "not_reached", "not_required"] {
        let value = json!({"id": "1", "ok": true, "verdict": verdict});
        assert!(serde_json::from_value::<EvaluateResponse>(value).is_ok());
    }
    for value in [
        json!({"id":"1", "ok":true}),
        json!({"id":"1", "ok":true, "verdict":"unknown"}),
    ] {
        assert!(serde_json::from_value::<EvaluateResponse>(value).is_err());
    }
}

#[test]
fn applicability_exchange_preserves_checked_inputs_and_structured_reasons() {
    use metewand_protocol::{CheckApplicabilityRequest, CheckApplicabilityResponse};
    let request = json!({
        "id": "check-1", "method": "check_applicability",
        "dataset_dir": "/private/data", "dataset_manifest": "dataset-manifest.json",
        "problem_contract_id": "contract", "problem_parameters": {"intercept": true},
        "implementation_parameters": {"representation": "dense"}
    });
    let checked = decode_message::<CheckApplicabilityRequest>(request.clone()).unwrap();
    assert_eq!(serde_json::to_value(checked).unwrap(), request);
    let response = json!({"id":"check-1", "ok":true, "outcome":"not_applicable",
        "code":"unsupported_intercept", "reason":"This solver requires a zero intercept.",
        "details":{"intercept":true}});
    let parsed = decode_message::<CheckApplicabilityResponse>(response.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), response);
    for (field, invalid) in [
        ("outcome", json!("unchecked")),
        ("code", json!("")),
        ("reason", json!("")),
        ("details", json!([])),
    ] {
        let mut malformed = response.clone();
        malformed[field] = invalid;
        assert!(decode_message::<CheckApplicabilityResponse>(malformed).is_err());
    }
    let hello = HelloRequest::new(
        "0".parse().unwrap(),
        WorkerRole::Implementation,
        WORKER_ID.parse().unwrap(),
    );
    let response = HelloResponse::new(
        "0".parse().unwrap(),
        WORKER_ID.parse().unwrap(),
        None,
        [Capability::Applicability],
    );
    assert!(validate_hello_response(&hello, &response, &[Capability::Applicability]).is_ok());
}

#[test]
fn managed_output_responses_have_exact_canonical_bytes_and_strict_shapes() {
    use metewand_protocol::ManagedOutputResponse;
    let cases = [
        (
            json!({"id":"1","ok":true,"draft":"draft.json","implementation_time_ns":null,"statistics":{}}),
            "{\"draft\":\"draft.json\",\"id\":\"1\",\"implementation_time_ns\":null,\"ok\":true,\"statistics\":{}}\n",
        ),
        (
            json!({"id":"1","ok":true,"draft":"draft.json","verdict":"reached"}),
            "{\"draft\":\"draft.json\",\"id\":\"1\",\"ok\":true,\"verdict\":\"reached\"}\n",
        ),
        (
            json!({"id":"1","ok":true,"draft":"draft.json"}),
            "{\"draft\":\"draft.json\",\"id\":\"1\",\"ok\":true}\n",
        ),
    ];
    for (value, canonical) in cases {
        let message: ManagedOutputResponse = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(encode_message(&message).unwrap(), canonical.as_bytes());
        let mut extra = value;
        extra["unknown"] = json!(true);
        assert!(serde_json::from_value::<ManagedOutputResponse>(extra).is_err());
    }
    for invalid in [
        json!({"id":"1","ok":true,"draft":""}),
        json!({"id":"1","ok":false,"draft":"draft.json"}),
        json!({"id":"1","ok":true,"draft":"draft.json","statistics":[],"implementation_time_ns":null}),
    ] {
        assert!(serde_json::from_value::<ManagedOutputResponse>(invalid).is_err());
    }
}
