//! 两个常量频道：`client-config` 与 `client-scenes` 的审计版语义。

use host_channels::client_metadata::{GET_SNAPSHOT, LIST};
use host_channels::{ClientConfigChannel, ClientScenesChannel};
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;

fn body(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn client_config_short_circuits_to_null_plugin_store_order() {
    let value = ClientConfigChannel
        .call(
            "ctx".into(),
            GET_SNAPSHOT.into(),
            Value::Array(Vec::new()),
            CancellationToken::default(),
        )
        .await
        .expect("调用成功");
    let payload = body(&value);
    assert_eq!(payload["pluginStoreOrder"], serde_json::Value::Null);
    assert_eq!(
        payload.as_object().map(|object| object.len()),
        Some(1),
        "审计版只返回这一个字段"
    );
}

#[tokio::test]
async fn client_scenes_returns_an_empty_success_body() {
    let value = ClientScenesChannel
        .call(
            "ctx".into(),
            LIST.into(),
            Value::Array(Vec::new()),
            CancellationToken::default(),
        )
        .await
        .expect("调用成功");
    let payload = body(&value);
    assert_eq!(payload["code"], 0);
    assert_eq!(payload["msg"], "");
    assert_eq!(payload["data"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn unknown_methods_are_rejected() {
    for channel in [
        &ClientConfigChannel as &dyn ServerChannel,
        &ClientScenesChannel as &dyn ServerChannel,
    ] {
        assert!(
            channel
                .call(
                    "ctx".into(),
                    "nope".into(),
                    Value::Undefined,
                    CancellationToken::default(),
                )
                .await
                .is_err()
        );
    }
}
