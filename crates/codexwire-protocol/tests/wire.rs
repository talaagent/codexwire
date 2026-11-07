use codexwire_protocol::{CodexErrorInfo, ThreadItem};
use serde_json::json;

#[test]
fn thread_item_keeps_shared_lifecycle_timestamps() {
    let raw = json!({
        "type": "userMessage", "id": "example", "content": [],
        "startedAtMs": 1000, "completedAtMs": 2000
    });
    let item: ThreadItem = serde_json::from_value(raw.clone()).unwrap();
    let ThreadItem::UserMessage(body) = &item else {
        panic!("known item fell back to raw JSON");
    };
    assert_eq!(body.started_at_ms, Some(1000));
    assert_eq!(body.completed_at_ms, Some(2000));
    assert_eq!(serde_json::to_value(item).unwrap(), raw);
}

#[test]
fn error_any_of_decodes_known_and_future_values() {
    let raw = json!({"httpConnectionFailed": {"httpStatusCode": 503}});
    let error: CodexErrorInfo = serde_json::from_value(raw.clone()).unwrap();
    assert!(matches!(error, CodexErrorInfo::HttpConnectionFailed(_)));
    assert_eq!(serde_json::to_value(error).unwrap(), raw);

    let future = json!({"futureError": {"code": 123}});
    let error: CodexErrorInfo = serde_json::from_value(future.clone()).unwrap();
    assert!(matches!(error, CodexErrorInfo::Unknown(_)));
    assert_eq!(serde_json::to_value(error).unwrap(), future);
}

#[test]
fn incompatible_known_item_retains_raw_json() {
    let raw = json!({"type": "userMessage", "id": "example", "content": 42});
    let item: ThreadItem = serde_json::from_value(raw.clone()).unwrap();
    assert!(matches!(item, ThreadItem::Unknown(_)));
    assert_eq!(serde_json::to_value(item).unwrap(), raw);
}
