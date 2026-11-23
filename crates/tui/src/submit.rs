//! Enter → `session/send`：把输入行提交变成一次真实 RPC（LT-8）。
//!
//! 真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `sendPrompt` 与
//! `docs/specs/session-write-group.md`。这里只做**可测的那一半**：构造参数、解释回执、
//! 维护「等回执」状态；进程 IO 留在 `main.rs`。
//!
//! 三条语义，各自对应一个真会犯的错：
//!
//! 1. **没有选中会话就不能发**。TUI 的输入行是全局的，但 `session/send` 必须带
//!    `sessionId`；空列表时发送只会拿到 agent 的 `unknown session`。
//! 2. **`inputId` 要由客户端生成且稳定**。它是 agent 侧的幂等键：重复 Enter（例如
//!    UI 卡顿）必须带同一个 id，否则同一条输入会被受理两次。
//! 3. **回执是「受理」不是「完成」**。`accepted:true` 只表示进入队列；turn 的失败走
//!    事件流的 `turn.failed`。状态栏因此写「已提交」，不写「已完成」。
//!
//! 提交期间输入行**不清空**：等回执回来再清，否则 RPC 失败时用户输入就丢了。

use serde_json::{json, Value};

/// 一次提交的客户端状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SubmitState {
    /// 空闲：可以接受新的 Enter。
    #[default]
    Idle,
    /// 已发出、等回执；`request_id` 是 JSON-RPC id，用于把回执对回来。
    InFlight {
        request_id: u64,
        input_id: String,
        session_id: String,
    },
}

impl SubmitState {
    pub fn is_in_flight(&self) -> bool {
        matches!(self, Self::InFlight { .. })
    }

    /// 在途请求的 RPC id（用于匹配回执）。
    pub fn request_id(&self) -> Option<u64> {
        match self {
            Self::Idle => None,
            Self::InFlight { request_id, .. } => Some(*request_id),
        }
    }
}

/// 构造 `session/send` 的 params。
///
/// 只带必需字段：`sessionId` / `content` / `inputId`。`modelSelection` 等可选字段
/// 留给 agent 用会话当前值补齐——TUI 不替用户选模型。
pub fn build_send_params(session_id: &str, content: &str, input_id: &str) -> Value {
    json!({
        "sessionId": session_id,
        "content": content,
        "inputId": input_id,
    })
}

/// 提交被拒绝的原因（决定状态栏文案，不决定是否重试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitRejection {
    /// 还没有任何会话：先 Ctrl-N 新建。
    NoSession,
    /// 上一次提交还没回执。
    InFlight,
    /// 输入为空（含纯空白）。
    EmptyInput,
}

impl SubmitRejection {
    pub fn status_message(&self) -> &'static str {
        match self {
            Self::NoSession => "没有选中会话：先按 Ctrl-N 新建任务",
            Self::InFlight => "上一次提交还在等待受理，请稍候",
            Self::EmptyInput => "输入为空，未提交",
        }
    }
}

/// 把「按下 Enter」翻译成一次待发请求。
///
/// 返回 `(请求参数, 新的在途状态)`；调用方随后用 `AgentClient::request` 发出去，
/// 拿到 RPC id 之后再用 [`SubmitState::InFlight`] 里的 id 对齐（见 `main.rs`）。
///
/// **输入行不清空**——清空由 [`apply_send_reply`] 在成功时做。
pub fn prepare_submit(
    state: &SubmitState,
    session_id: Option<&str>,
    input: &str,
    input_id: &str,
) -> Result<Value, SubmitRejection> {
    if state.is_in_flight() {
        return Err(SubmitRejection::InFlight);
    }
    let Some(session_id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return Err(SubmitRejection::NoSession);
    };
    let content = input.trim();
    if content.is_empty() {
        return Err(SubmitRejection::EmptyInput);
    }
    Ok(build_send_params(session_id, content, input_id))
}

/// 解释一次 `session/send` 回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// `accepted:true`：已进队列（**不是**已完成）。
    Accepted { state_revision: u64 },
    /// agent 明确拒绝（参数不合法 / 版本冲突 / 未知会话）。
    Rejected { message: String },
}

impl SendOutcome {
    pub fn status_message(&self) -> String {
        match self {
            Self::Accepted { .. } => "已提交，等待模型响应…".to_string(),
            Self::Rejected { message } => format!("提交失败：{message}"),
        }
    }

    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted { .. })
    }
}

/// 解析 `session/send` 的 `result`。
///
/// `accepted` 字段缺失或不为 `true` 一律按拒绝处理：受理确认只能由 `true` 表达，
/// 缺字段当成成功会让失败的提交在状态栏显示成「已提交」。
pub fn parse_send_result(result: &Value) -> SendOutcome {
    let accepted = result.get("accepted").and_then(Value::as_bool) == Some(true);
    if !accepted {
        let message = result
            .get("message")
            .or_else(|| result.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("对端未确认受理")
            .to_string();
        return SendOutcome::Rejected { message };
    }
    let state_revision = result
        .get("stateRevision")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    SendOutcome::Accepted { state_revision }
}

/// 解析 agent 回的 `error` 对象（JSON-RPC 错误）。
pub fn parse_error_reply(error: &Value) -> SendOutcome {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("agent 返回错误")
        .to_string();
    SendOutcome::Rejected { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_requires_a_selected_session() {
        let state = SubmitState::Idle;
        assert_eq!(
            prepare_submit(&state, None, "hello", "in-1"),
            Err(SubmitRejection::NoSession)
        );
        assert_eq!(
            prepare_submit(&state, Some("   "), "hello", "in-1"),
            Err(SubmitRejection::NoSession)
        );
        assert!(prepare_submit(&state, Some("s1"), "hello", "in-1").is_ok());
    }

    #[test]
    fn submit_rejects_blank_input_so_whitespace_never_reaches_the_agent() {
        let state = SubmitState::Idle;
        assert_eq!(
            prepare_submit(&state, Some("s1"), "   \n ", "in-1"),
            Err(SubmitRejection::EmptyInput)
        );
    }

    #[test]
    fn in_flight_blocks_a_second_enter() {
        let state = SubmitState::InFlight {
            request_id: 7,
            input_id: "in-1".to_string(),
            session_id: "s1".to_string(),
        };
        assert!(state.is_in_flight());
        assert_eq!(state.request_id(), Some(7));
        // 第二次 Enter 不再发请求：否则同一条输入会被受理两次。
        assert_eq!(
            prepare_submit(&state, Some("s1"), "hello", "in-2"),
            Err(SubmitRejection::InFlight)
        );
        assert_eq!(SubmitState::Idle.request_id(), None);
    }

    #[test]
    fn params_carry_exactly_the_three_required_fields() {
        let params = build_send_params("s1", "你好", "in-1");
        assert_eq!(params["sessionId"], "s1");
        assert_eq!(params["content"], "你好");
        assert_eq!(params["inputId"], "in-1");
        // 不替用户选模型：可选字段整个缺席。
        assert!(params.get("modelSelection").is_none());
        assert_eq!(params.as_object().map(|map| map.len()), Some(3));
    }

    #[test]
    fn content_is_trimmed_before_sending() {
        let params =
            prepare_submit(&SubmitState::Idle, Some("s1"), "  hi  ", "in-1").expect("params");
        assert_eq!(params["content"], "hi");
    }

    #[test]
    fn accepted_reply_reports_revision_and_is_not_terminal_success() {
        let outcome = parse_send_result(&json!({
            "accepted": true,
            "sessionId": "s1",
            "stateRevision": 42,
        }));
        assert_eq!(outcome, SendOutcome::Accepted { state_revision: 42 });
        assert!(outcome.is_accepted());
        // 「受理」不是「完成」：文案只说到等待响应。
        assert_eq!(outcome.status_message(), "已提交，等待模型响应…");
    }

    #[test]
    fn missing_or_false_accepted_is_a_rejection() {
        for result in [
            json!({}),
            json!({ "accepted": false }),
            json!({ "accepted": "true" }),
            json!({ "accepted": null }),
        ] {
            let outcome = parse_send_result(&result);
            assert!(
                !outcome.is_accepted(),
                "缺字段或非 true 必须按拒绝处理：{result}"
            );
        }
    }

    #[test]
    fn rejection_message_prefers_message_then_error_then_fallback() {
        assert_eq!(
            parse_send_result(&json!({ "accepted": false, "message": "版本冲突" }))
                .status_message(),
            "提交失败：版本冲突"
        );
        assert_eq!(
            parse_send_result(&json!({ "accepted": false, "error": "未知会话" })).status_message(),
            "提交失败：未知会话"
        );
        assert_eq!(
            parse_send_result(&json!({ "accepted": false })).status_message(),
            "提交失败：对端未确认受理"
        );
    }

    #[test]
    fn error_reply_is_surfaced_verbatim() {
        let outcome = parse_error_reply(&json!({
            "code": -32602,
            "message": "session/send 参数不合法: content 不能为空",
        }));
        assert_eq!(
            outcome.status_message(),
            "提交失败：session/send 参数不合法: content 不能为空"
        );
    }

    #[test]
    fn every_rejection_has_a_status_message() {
        for rejection in [
            SubmitRejection::NoSession,
            SubmitRejection::InFlight,
            SubmitRejection::EmptyInput,
        ] {
            assert!(!rejection.status_message().is_empty(), "{rejection:?}");
        }
    }
}
