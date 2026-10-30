use crate::protocol::{ExecuteRequest, ExecuteResponse};
use serde_json::json;

#[test]
fn execute_exchanges_preserve_the_exact_selected_control() {
    let request = json!({"id":"1","method":"execute","result_dir":"result","observation_control":{"iterations":4}});
    let parsed: ExecuteRequest = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(parsed.observation_control(), Some(&json!({"iterations":4})));
    assert_eq!(serde_json::to_value(parsed).unwrap(), request);
    let response = json!({"id":"1","ok":true,"implementation_time_ns":null,"result":{"manifest":"result.json"},"statistics":{},"observation_control":{"iterations":4}});
    let parsed: ExecuteResponse = serde_json::from_value(response.clone()).unwrap();
    assert_eq!(parsed.observation_control(), Some(&json!({"iterations":4})));
    assert_eq!(serde_json::to_value(parsed).unwrap(), response);
}

#[test]
fn control_presence_requires_an_object() {
    for control in [json!(null), json!(1), json!([])] {
        assert!(serde_json::from_value::<ExecuteRequest>(json!({"id":"1","method":"execute","result_dir":"result","observation_control":control})).is_err());
    }
}
