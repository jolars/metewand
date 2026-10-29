use metewand_protocol::{
    ExecuteStreamResponse, StreamCheckpoint, StreamDecision, decode_message, encode_message,
};
use serde_json::json;

#[test]
fn checkpoint_decision_and_terminal_frames_are_strict_and_bounded() {
    let checkpoint = json!({"id":"2","event":"checkpoint","sequence":0,"check_index":1,
        "work":{"updates":1},"terminal":false,"manifest":"checkpoints/0/result.json"});
    let value: StreamCheckpoint = decode_message(checkpoint.clone()).unwrap();
    assert_eq!(encode_message(&value).unwrap(), b"{\"check_index\":1,\"event\":\"checkpoint\",\"id\":\"2\",\"manifest\":\"checkpoints/0/result.json\",\"sequence\":0,\"terminal\":false,\"work\":{\"updates\":1}}\n");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encode_message(&value).unwrap()).unwrap(),
        checkpoint
    );
    let _: StreamDecision = decode_message(
        json!({"id":"2","method":"stream_decision","sequence":0,"action":"continue"}),
    )
    .unwrap();
    let _: ExecuteStreamResponse =
        decode_message(json!({"id":"2","ok":true,"checkpoint_count":1})).unwrap();
    for field in ["sequence", "check_index"] {
        let mut invalid = checkpoint.clone();
        invalid[field] = json!(9007199254740992u64);
        assert!(decode_message::<StreamCheckpoint>(invalid).is_err());
    }
    let mut invalid = checkpoint;
    invalid["unexpected"] = json!(true);
    assert!(decode_message::<StreamCheckpoint>(invalid).is_err());
}

#[test]
fn checkpoint_references_and_work_counters_match_the_schema() {
    let valid = json!({"id":"1","event":"checkpoint","sequence":0,"check_index":0,"work":{},"terminal":false,"manifest":"checkpoints/0/result.json"});
    for case in ["missing", "both", "null", "unsafe_work"] {
        let mut invalid = valid.clone();
        match case {
            "missing" => {
                invalid.as_object_mut().unwrap().remove("manifest");
            }
            "both" => invalid["draft"] = json!("draft.json"),
            "null" => invalid["manifest"] = serde_json::Value::Null,
            "unsafe_work" => invalid["work"] = json!({"updates":9007199254740992u64}),
            _ => unreachable!(),
        }
        assert!(
            decode_message::<StreamCheckpoint>(invalid).is_err(),
            "{case}"
        );
    }
}
