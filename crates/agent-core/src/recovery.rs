//! C3 T2：重试策略与恢复事件的载荷投影。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §2 与 T0 夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c3/retry-*.json`。
//!
//! 两条路径刻意分开（契约 §7.7）：
//! - **HTTP 5xx / 传输失败（未提交 delta）**：adapter 式重试，指数退避 +
//!   `session.updated(model_retry_scheduled)`（带 `_meta.mode.apiRetry`）；
//! - **流中断（已提交 delta）**：stream recovery，立即重试（`retryDelayMs:0`），
//!   4 条 `streamRecovery.updated`，`attempt` 仍为 1、`retryNumber` 递增。
//!
//! 次数上限来自夹具观察：`maxRetries:10` / `maxAttempts:11`。耗尽后的终态形状
//! **未观察到**（契约 §2.7），这里只保证「不再发起新尝试 + 恰好一个终态」。
//! 退避的具体毫秒序列被夹具折叠为 `<retry-delay>`；这里实现带抖动的指数退避，
//! 只保证量级与单调增长，不声称逐毫秒复刻。

use serde_json::{json, Value};

use crate::provider::ProviderError;

/// 重试策略（次数来自夹具观察；延迟可在测试/宿主注入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// `maxRetries`：初始尝试之外允许的重试次数（夹具 10）。
    pub max_retries: u32,
    /// 第一次退避的基数（毫秒）；实际值 = base × 2^(attempt-1) × jitter。
    pub base_delay_ms: u64,
    /// 单次退避上限（毫秒）。
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            // 夹具 `maxRetries:10` / `maxAttempts:11`。
            max_retries: 10,
            // 观察量级：第一次 ≈1.3–1.5s、第二次 ≈2.0–2.6s（夹具不写死毫秒）。
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
        }
    }
}

impl RetryPolicy {
    /// `maxAttempts = maxRetries + 1`（夹具字段）。
    pub fn max_attempts(&self) -> u32 {
        self.max_retries.saturating_add(1)
    }

    /// 第 `failed_attempt`（1 起）次失败后的退避毫秒数。
    ///
    /// 指数增长 + 0–50% 抖动（观察到的退避带抖动）；`base_delay_ms:0` 时返回 0
    /// （测试注入的立即重试）。抖动用 uuid 的低位，不引入新的随机依赖。
    pub fn backoff_delay_ms(&self, failed_attempt: u32) -> u64 {
        if self.base_delay_ms == 0 {
            return 0;
        }
        let shift = failed_attempt.saturating_sub(1).min(20);
        let exponential = self
            .base_delay_ms
            .saturating_mul(1_u64 << shift)
            .min(self.max_delay_ms);
        let jitter = (uuid::Uuid::new_v4().as_u128() & 0xffff) as u64;
        let factor = 1.0 + (jitter as f64 / 65_535.0) * 0.5;
        ((exponential as f64) * factor).round() as u64
    }
}

/// `_meta.mode.apiRetry`（`mode-api-retry` 的归一化输入形状）。
fn api_retry_meta(
    attempt: u32,
    max_retries: u32,
    retry_delay_ms: u64,
    error_status: Option<u16>,
    error: &str,
) -> Value {
    json!({
        "mode": {
            "apiRetry": {
                "kind": "api_retry",
                "attempt": attempt,
                "maxRetries": max_retries,
                "retryDelayMs": retry_delay_ms,
                "errorStatus": error_status,
                "error": error,
            }
        }
    })
}

fn api_retry_cleared_meta() -> Value {
    json!({ "mode": { "apiRetry": null } })
}

/// HTTP 重试调度事件（`session.updated` 的载荷）。
///
/// 观察字段（`retry-5xx-then-success.json`）：`type`/`attempt`/`maxAttempts`/
/// `nextAttempt`/`delayMs`/`statusCode`/`reason`/`message`/`errorCode`/
/// `_meta.mode.apiRetry`。请求级追踪字段（providerId/modelId/span 等）由宿主
/// 投影面补齐，agent-core 不伪造。
pub(crate) fn model_retry_scheduled_payload(
    policy: &RetryPolicy,
    failed_attempt: u32,
    delay_ms: u64,
    error: &ProviderError,
) -> Value {
    let message = error.canonical_message();
    let mut payload = json!({
        "type": "model_retry_scheduled",
        "attempt": failed_attempt,
        "maxAttempts": policy.max_attempts(),
        "nextAttempt": failed_attempt + 1,
        "delayMs": delay_ms,
        "reason": error.reason(),
        "message": message,
        "errorCode": "model_request_failed",
        "_meta": api_retry_meta(
            failed_attempt,
            policy.max_retries,
            delay_ms,
            error.status,
            message,
        ),
    });
    if let Some(status) = error.status {
        payload["statusCode"] = json!(status);
    }
    payload
}

/// 一次 stream recovery 的追踪 id（成功收口事件的 `streamRecovery` 字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamRecoverySummary {
    pub attempt_id: String,
    pub anchor_id: String,
    pub max_retries: u32,
    pub retry_number: u32,
    pub failed_request_id: String,
}

impl StreamRecoverySummary {
    pub fn to_value(&self) -> Value {
        json!({
            "attemptId": self.attempt_id,
            "anchorId": self.anchor_id,
            "maxRetries": self.max_retries,
            "retryNumber": self.retry_number,
            "recoveredFromRequestId": self.failed_request_id,
        })
    }
}

/// 重试/恢复成功后清空 apiRetry 的收口事件（`session.updated` 的载荷）。
///
/// 正常（无重试）turn 不产出该事件族（C1 登记缺省）；只有真正重试过的 turn
/// 需要它把 `mode-api-retry` 的状态清回 `null`。
pub(crate) fn model_request_completed_payload(
    policy: &RetryPolicy,
    attempt: u32,
    response: &crate::provider::ModelResponse,
    recovery: Option<&StreamRecoverySummary>,
) -> Value {
    let mut payload = json!({
        "type": "model_request_completed",
        "attempt": attempt,
        "maxAttempts": policy.max_attempts(),
        "finishReason": response.stop_reason,
        "streamOutputCommitted": true,
        "usage": {
            "inputTokens": response.usage.input_tokens,
            "outputTokens": response.usage.output_tokens,
            "totalTokens": response.usage.input_tokens + response.usage.output_tokens,
            "cacheReadTokens": response.usage.cache_read_tokens,
            "cacheWriteTokens": response.usage.cache_write_tokens,
        },
        "_meta": api_retry_cleared_meta(),
    });
    if let Some(recovery) = recovery {
        payload["streamRecovery"] = recovery.to_value();
    }
    payload
}

/// 退避期间取消的收口事件（`session.updated` 的载荷）。
///
/// 观察（`retry-cancel-during-backoff.json`）：`reason:"cancelled"`、
/// `message:"Model request was cancelled."`、`retryable:false`。
pub(crate) fn model_request_cancelled_payload(policy: &RetryPolicy, failed_attempt: u32) -> Value {
    json!({
        "type": "model_request_failed",
        "attempt": failed_attempt,
        "maxAttempts": policy.max_attempts(),
        "reason": "cancelled",
        "message": "Model request was cancelled.",
        "retryable": false,
        "errorCode": "model_request_cancelled",
        "errorPhase": "connect",
        "_meta": api_retry_cleared_meta(),
    })
}

/// 一次 stream recovery 的观察事实。
pub(crate) struct StreamRecoveryFacts<'a> {
    pub policy: &'a RetryPolicy,
    /// `retryNumber`（1 起）。网络层 `attempt` 在夹具里仍是 1，不随恢复递增；
    /// 该值只出现在 `model_request_*` 事件族（本任务不产出），这里不建模。
    pub retry_number: u32,
    pub failure_kind: &'static str,
    pub failure_message: String,
    pub assistant_message_id: String,
    pub discarded_text_bytes: u64,
    pub discarded_reasoning_bytes: u64,
    pub committed_tool_call_ids: Vec<String>,
}

/// 4 条 `streamRecovery.updated` 载荷（顺序即夹具顺序，§2.2）与本次恢复的追踪 id。
pub(crate) fn stream_recovery_payloads(
    facts: &StreamRecoveryFacts<'_>,
) -> ([Value; 4], StreamRecoverySummary) {
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let anchor_id = uuid::Uuid::new_v4().to_string();
    let failed_request_id = uuid::Uuid::new_v4().to_string();
    let max_retries = facts.policy.max_retries;

    let failure = json!({
        "attemptId": attempt_id,
        "assistantMessageId": facts.assistant_message_id,
        "failureKind": facts.failure_kind,
        "message": facts.failure_message,
        "retryNumber": facts.retry_number,
        "maxRetries": max_retries,
        "failedRequestId": failed_request_id,
        "_meta": api_retry_meta(
            facts.retry_number,
            max_retries,
            0,
            None,
            &facts.failure_message,
        ),
    });

    // 工具调用提交锚点：夹具只观察到无工具调用的 `no_tool_committed`；
    // 带工具调用的恢复未观察到（契约 §2.7），这里按「已提交的 toolCallId」
    // 如实投影并登记，不伪造 `no_tool_committed`。
    let reason = if facts.committed_tool_call_ids.is_empty() {
        "no_tool_committed"
    } else {
        "tool_committed"
    };
    let anchor = json!({
        "attemptId": attempt_id,
        "anchorId": anchor_id,
        "reason": reason,
        "committedToolCallIds": facts.committed_tool_call_ids,
    });

    let discard = json!({
        "attemptId": attempt_id,
        "anchorId": anchor_id,
        "assistantMessageId": facts.assistant_message_id,
        "discardedReasoningBytes": facts.discarded_reasoning_bytes,
        "discardedTextBytes": facts.discarded_text_bytes,
        "discardedToolCallIds": facts.committed_tool_call_ids,
    });

    let retry_started = json!({
        "attemptId": attempt_id,
        "anchorId": anchor_id,
        "retryNumber": facts.retry_number,
        "maxRetries": max_retries,
        "streamMode": "sse",
        "failedRequestId": failed_request_id,
        "_meta": api_retry_meta(
            facts.retry_number,
            max_retries,
            0,
            None,
            "Model stream recovery retry started",
        ),
    });

    let summary = StreamRecoverySummary {
        attempt_id: attempt_id.clone(),
        anchor_id: anchor_id.clone(),
        max_retries,
        retry_number: facts.retry_number,
        failed_request_id: failed_request_id.clone(),
    };

    ([failure, anchor, discard, retry_started], summary)
}

/// 流恢复的 `failureKind`：仅网络类是观察值；HTTP 状态类未观察到（登记）。
pub(crate) fn stream_recovery_failure_kind(error: &ProviderError) -> &'static str {
    match error.kind {
        crate::provider::ProviderErrorKind::Transport
        | crate::provider::ProviderErrorKind::StreamAbort => "provider_network_error",
        crate::provider::ProviderErrorKind::Server => "provider_server_error",
        crate::provider::ProviderErrorKind::RateLimited => "provider_rate_limited",
        _ => "provider_error",
    }
}

/// 流恢复事件里的 `message`：与 `turn.failed` 共用规范文案（原始细节留在
/// `ProviderError.message`/`underlying_message`）。
pub(crate) fn stream_recovery_failure_message(error: &ProviderError) -> String {
    error.canonical_message().to_string()
}
