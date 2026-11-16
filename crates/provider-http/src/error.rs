//! `provider-http` 的错误构造：把 HTTP 状态、传输失败、响应契约违规与本地错误统一成
//! [`ProviderError`]。
//!
//! 当前 `agent-core` 契约的 `ProviderError` 只有 `message` 一个字段（brief 里的
//! `kind`/`status` 分字段是旧口径，Task 3 收敛后不再存在）；状态码与根因都拼进文案，
//! 保证 `turn.failed`（Task 6）拿到一行可诊断的话。

use agent_core::ProviderError;
use serde_json::Value;

/// 错误文案里正文/原文的字符上限：错误要可诊断，但超长 HTML 错误页不该原样刷进事件流。
const DETAIL_MAX_CHARS: usize = 512;

/// 非 2xx HTTP 响应 → 错误。正文尽力提取错误消息（`error.message` / `error` 字符串 /
/// `message` 字段），提取失败回退原文截断——错误必须可诊断，不静默吞正文。
pub fn http_status(status: u16, body: &str) -> ProviderError {
    let message = serde_json::from_str::<Value>(body.trim())
        .ok()
        .and_then(|value| payload_message(&value))
        .unwrap_or_else(|| truncate(body));
    ProviderError {
        message: format!("HTTP {status}: {message}"),
    }
}

/// 传输失败（连接、TLS、读流中断）。reqwest 顶层文案不含根因（挂在 `source()` 链上），
/// 调用方应先把整条链拼进 detail（见 openai_chat 的 `describe_transport`）。
pub fn transport(detail: String) -> ProviderError {
    ProviderError {
        message: format!("模型请求传输失败：{detail}"),
    }
}

/// 响应不合契约（非法 JSON、缺关键字段、工具入参不可解析）。
pub fn malformed(detail: String) -> ProviderError {
    ProviderError {
        message: format!("模型响应不合契约：{detail}"),
    }
}

/// 本地错误（delta sink 已关闭、请求体序列化失败）。
pub fn local(detail: String) -> ProviderError {
    ProviderError { message: detail }
}

/// 从错误载荷提取消息；不是已知形态时返回 `None`（HTTP 侧回退原文截断）。
///
/// 兼容三种已见形态：`{"error": {"message": …}}`（OpenAI）、`{"error": "…"}`、
/// `{"message": "…"}`（部分兼容网关）。`error: null` 视为没有错误。
pub(crate) fn payload_message(value: &Value) -> Option<String> {
    let message = match value.get("error") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(error_object) => error_object
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
        None => value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    message.map(|text| truncate(&text))
}

/// 按字符边界截断到 [`DETAIL_MAX_CHARS`]（不截半字）。
pub(crate) fn truncate(text: &str) -> String {
    if text.chars().count() <= DETAIL_MAX_CHARS {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(DETAIL_MAX_CHARS).collect();
    truncated.push('…');
    truncated
}
