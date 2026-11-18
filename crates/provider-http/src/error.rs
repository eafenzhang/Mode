//! `provider-http` 的错误构造：把 HTTP 状态、传输失败、流中断、响应契约违规与本地
//! 错误统一成 [`ProviderError`]（C3 T2 起带分类字段：`kind`/`status`/`retryable`）。
//!
//! 分类只在这里做一次：会话层按 `retryable` 决定是否重试，按「本尝试是否已提交
//! delta」决定走 adapter 退避还是 stream recovery（契约 §2 / §7.7）。

use agent_core::ProviderError;
use serde_json::Value;

/// 错误文案里正文/原文的字符上限：错误要可诊断，但超长 HTML 错误页不该原样刷进事件流。
const DETAIL_MAX_CHARS: usize = 512;

/// 非 2xx HTTP 响应 → 错误。正文尽力提取错误消息（`error.message` / `error` 字符串 /
/// `message` 字段），提取失败回退原文截断——错误必须可诊断，不静默吞正文。
///
/// 状态码分类（`ProviderError::http_status`）：401/403 auth、5xx server（retryable）、
/// 408 transport / 429 rate limited（均 retryable）、其余 4xx/3xx 非 retryable。
/// 408 归 transport 不是 rate limited——旧注释把两者并为「rate limited」与
/// `agent-core::ProviderError::http_status` 的实际分类不符（T5 更正）。
pub fn http_status(status: u16, body: &str) -> ProviderError {
    let message = serde_json::from_str::<Value>(body.trim())
        .ok()
        .and_then(|value| payload_message(&value))
        .unwrap_or_else(|| truncate(body));
    ProviderError::http_status(status, format!("HTTP {status}: {message}")).with_underlying(message)
}

/// 传输失败（连接、TLS、DNS；请求未发出或响应头未到）。retryable。
/// reqwest 顶层文案不含根因（挂在 `source()` 链上），调用方应先把整条链拼进 detail。
pub fn transport(detail: String) -> ProviderError {
    ProviderError::transport(format!("模型请求传输失败：{detail}"))
}

/// 流中断（响应已开始、delta 可能已提交，读流失败或连接在终止事件前关闭）。
///
/// 这是 C3 T2 新增分类：**retryable**，但会话层只有在没有已提交 delta 时才走
/// adapter 退避；否则走 stream recovery（`streamRecovery.updated`，立即重试）。
pub fn stream_abort(detail: String) -> ProviderError {
    ProviderError::stream_abort(format!("模型流传输中断：{detail}"))
}

/// 响应不合契约（非法 JSON、缺关键字段、工具入参不可解析）。非 retryable。
pub fn malformed(detail: String) -> ProviderError {
    ProviderError::malformed(format!("模型响应不合契约：{detail}"))
}

/// 本地错误（delta sink 已关闭、请求体序列化失败）。非 retryable。
pub fn local(detail: String) -> ProviderError {
    ProviderError::local(detail)
}

/// 从错误载荷提取消息；不是已知形态时返回 `None`（HTTP 侧回退原文截断）。
///
/// 兼容三种已见形态：`{"error": {"message": …}}`（OpenAI）、`{"error": "…"}`、
/// `{"message": …}`（部分兼容网关）。`error: null` 视为没有错误。
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
