//! `/getupdates` 长轮询与游标解析（TS `weixinProvider.ts:540-575,703-728` 的对齐）。
//!
//! 第二波边界：只做「请求面」——返回**原始消息数组**与 `next_buf`，不做
//! `buildInboundMessage` 投影（附件 kind/用户字段语义留在 TS callback 管线）。
//! 顺序约束（TS :161-165 Bugfix）：游标必须等本批消息全部进入业务处理后持久化——
//! 由调用方负责，本 crate 只给出 `next_buf`。

use std::time::Duration;

use serde_json::Value;

use crate::classify::WeixinError;
use crate::headers::{
    DEFAULT_WEIXIN_ILINK_BASE_URL, WEIXIN_BOT_API_PREFIX, WEIXIN_GET_UPDATES_TIMEOUT_MS,
};
use crate::http::WeixinHttp;

/// 响应容器：`data`（若有）下按多键取消息数组（TS :548-558 的宽容语义）。
pub fn read_messages(payload: &Value) -> Vec<Value> {
    let container = payload.get("data").unwrap_or(payload);
    let raw = container
        .get("msgs")
        .or_else(|| container.get("messages"))
        .or_else(|| container.get("updates"))
        .or_else(|| container.get("items"))
        .or_else(|| container.get("list"));
    match raw {
        Some(Value::Array(items)) => items.clone(),
        Some(item @ Value::Object(_)) => vec![item.clone()],
        _ => Vec::new(),
    }
}

/// 游标多键取首个非空（TS :565-575）。
pub fn next_buf_of(payload: &Value) -> Option<String> {
    let container = payload.get("data").unwrap_or(payload);
    for key in [
        "get_updates_buf",
        "buf",
        "next_buf",
        "nextBuf",
        "getUpdatesBuf",
        "syncKey",
    ] {
        if let Some(buf) = container.get(key).and_then(Value::as_str) {
            if !buf.is_empty() {
                return Some(buf.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct UpdatesResult {
    /// 原始消息（未做语义投影；见 spec §4b 边界）。
    pub raw_messages: Vec<Value>,
    /// 服务端游标；响应缺省时回退请求侧的 buf（TS :726 `?? params.buf`）。
    pub next_buf: Option<String>,
}

/// 一次长轮询：POST `/getupdates`，超时 90s（TS :22,709-718）。
pub async fn get_updates(
    http: &dyn WeixinHttp,
    token: &str,
    buf: Option<&str>,
    base_url: Option<&str>,
) -> Result<UpdatesResult, WeixinError> {
    let base = base_url.unwrap_or(DEFAULT_WEIXIN_ILINK_BASE_URL);
    let request = crate::http::WeixinRequest {
        url: format!(
            "{}{WEIXIN_BOT_API_PREFIX}/getupdates",
            base.trim_end_matches('/')
        ),
        token: token.to_string(),
        body: serde_json::json!({ "get_updates_buf": buf.unwrap_or_default() }),
        timeout: Duration::from_millis(WEIXIN_GET_UPDATES_TIMEOUT_MS),
    };
    let payload = http.post_json(request).await?;
    Ok(UpdatesResult {
        raw_messages: read_messages(&payload),
        next_buf: next_buf_of(&payload).or_else(|| buf.map(str::to_string)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_read_from_all_documented_keys() {
        for key in ["msgs", "messages", "updates", "items", "list"] {
            let payload = json!({ "data": { key: [{ "x": 1 }, { "y": 2 }] } });
            assert_eq!(read_messages(&payload).len(), 2, "key={key}");
        }
        let single = json!({ "msgs": { "x": 1 } });
        assert_eq!(read_messages(&single).len(), 1);
        assert!(read_messages(&json!({ "msgs": "not-array" })).is_empty());
    }

    #[test]
    fn next_buf_reads_first_non_empty_key() {
        let payload = json!({ "data": { "buf": "", "get_updates_buf": "cursor-1" } });
        assert_eq!(next_buf_of(&payload).as_deref(), Some("cursor-1"));
        let sync = json!({ "syncKey": "s" });
        assert_eq!(next_buf_of(&sync).as_deref(), Some("s"));
        assert!(next_buf_of(&json!({})).is_none());
    }

    #[test]
    fn get_updates_falls_back_to_request_buf() {
        // TS :726：响应缺游标时 `?? params.buf`。
        struct EmptyHttp;
        impl crate::http::WeixinHttp for EmptyHttp {
            fn post_json(
                &self,
                _request: crate::http::WeixinRequest,
            ) -> futures_util::future::BoxFuture<'static, Result<Value, WeixinError>> {
                Box::pin(async { Ok(json!({})) })
            }
        }
        let result = tokio_test_block_on(get_updates(&EmptyHttp, "tok", Some("cursor-9"), None))
            .expect("getupdates 应成功");
        assert_eq!(result.next_buf.as_deref(), Some("cursor-9"));
        assert!(result.raw_messages.is_empty());
    }

    fn tokio_test_block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(future)
    }
}
