//! `broadcast` 频道适配器：发送、claim 状态机与 `onMessage` 事件。

use std::sync::{Arc, Mutex};

use broadcast_service::BroadcastService;
use host_channels::broadcast::{ACQUIRE_CLAIM, COMMIT_CLAIM, RELEASE_CLAIM, SEND, TRY_CLAIM};
use host_channels::BroadcastChannel;
use rpc_host::{CancellationToken, ChannelError, EventCallback, ServerChannel};
use rpc_wire::Value;

fn channel() -> BroadcastChannel {
    BroadcastChannel::new(Arc::new(BroadcastService::new()))
}

async fn call(channel: &BroadcastChannel, method: &str, arg: Value) -> Result<Value, ChannelError> {
    channel
        .call(
            "ctx".into(),
            method.into(),
            arg,
            CancellationToken::default(),
        )
        .await
}

fn object(value: serde_json::Value) -> Value {
    Value::Array(vec![Value::Json(value.to_string())])
}

fn string_arg(value: &str) -> Value {
    Value::Array(vec![Value::Str(value.to_string())])
}

fn json(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn send_notifies_subscribers_and_unsubscribe_stops() {
    let channel = channel();
    let received: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    let emit: EventCallback = Arc::new(move |value: Value| {
        sink.lock().unwrap().push(json(&value));
    });
    let subscription = channel
        .listen("ctx".into(), "onMessage".into(), Value::Undefined, emit)
        .unwrap();

    call(
        &channel,
        SEND,
        object(serde_json::json!({
            "channel": "state:theme",
            "payload": { "theme": "dark" },
            "sourceWindowId": 2,
        })),
    )
    .await
    .unwrap();
    assert_eq!(received.lock().unwrap().len(), 1);
    assert_eq!(received.lock().unwrap()[0]["channel"], "state:theme");
    assert_eq!(received.lock().unwrap()[0]["sourceWindowId"], 2);

    subscription();
    call(
        &channel,
        SEND,
        object(serde_json::json!({ "channel": "state:locale", "payload": "zh-CN" })),
    )
    .await
    .unwrap();
    assert_eq!(received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn claims_round_trip() {
    let channel = channel();

    let acquired = json(
        &call(&channel, ACQUIRE_CLAIM, string_arg("k"))
            .await
            .unwrap(),
    );
    assert_eq!(acquired["status"], "acquired");
    let lease = acquired["lease"].clone();
    assert_eq!(lease["key"], "k");

    let busy = json(
        &call(&channel, ACQUIRE_CLAIM, string_arg("k"))
            .await
            .unwrap(),
    );
    assert_eq!(busy["status"], "busy");
    assert!(busy["retryAfterMs"].as_u64().unwrap() <= 250);

    let committed = call(
        &channel,
        COMMIT_CLAIM,
        object(serde_json::json!({ "key": lease["key"], "token": lease["token"] })),
    )
    .await
    .unwrap();
    assert_eq!(committed, Value::Undefined);
    let after = json(
        &call(&channel, ACQUIRE_CLAIM, string_arg("k"))
            .await
            .unwrap(),
    );
    assert_eq!(after["status"], "committed");

    // 已 commit 的 lease 释放不影响 committed 状态。
    call(
        &channel,
        RELEASE_CLAIM,
        object(serde_json::json!({ "key": lease["key"], "token": lease["token"] })),
    )
    .await
    .unwrap();
    let after = json(
        &call(&channel, ACQUIRE_CLAIM, string_arg("k"))
            .await
            .unwrap(),
    );
    assert_eq!(after["status"], "committed");

    let empty = json(
        &call(&channel, ACQUIRE_CLAIM, string_arg("   "))
            .await
            .unwrap(),
    );
    assert_eq!(empty["status"], "unavailable");

    assert_eq!(
        call(&channel, TRY_CLAIM, string_arg("once")).await.unwrap(),
        Value::Json("true".to_string())
    );
    assert_eq!(
        call(&channel, TRY_CLAIM, string_arg("once")).await.unwrap(),
        Value::Json("false".to_string())
    );
}

#[tokio::test]
async fn invalid_and_unknown_calls_are_errors() {
    let channel = channel();

    let missing = call(&channel, SEND, Value::Array(Vec::new()))
        .await
        .unwrap_err();
    match missing {
        ChannelError::Error { message, .. } => assert_eq!(message, "missing arguments"),
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let invalid_message = call(&channel, SEND, object(serde_json::json!({ "payload": 1 })))
        .await
        .unwrap_err();
    match invalid_message {
        ChannelError::Error { message, .. } => {
            assert!(message.starts_with("Invalid broadcast message"))
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let missing_key = call(&channel, ACQUIRE_CLAIM, Value::Array(Vec::new()))
        .await
        .unwrap_err();
    match missing_key {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "acquireClaim requires a key")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unknown = call(&channel, "nope", object(serde_json::json!({})))
        .await
        .unwrap_err();
    match unknown {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "broadcast channel: unknown command nope")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unknown_event = channel.listen(
        "ctx".into(),
        "nope".into(),
        Value::Undefined,
        Arc::new(|_| {}),
    );
    match unknown_event {
        Err(ChannelError::Error { message, .. }) => {
            assert_eq!(message, "broadcast channel: unknown event nope")
        }
        Ok(_) => panic!("期望 Error，实际 Ok"),
        Err(other) => panic!("期望 Error，实际 {other:?}"),
    }
}
