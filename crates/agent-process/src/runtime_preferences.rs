//! 运行时偏好反向请求的宿主侧应答。
//!
//! 真源：`packages/services/src/mode-agent/modeAgentService.ts` 里
//! `session/requestRuntimePreferences` 的 `sessionRuntimePreferencesAuthority === "local"` 分支：
//! 宿主直接回 `{askUserQuestionAutoResolutionEnabled: true, nativeSearchEnhancementsEnabled: true,
//! memoryEnabled: false}`，参数非法回 `-32602`，解析失败回 `-32603`。
//!
//! 规范见 `docs/specs/session-write-group.md` §「反向请求」。

use std::sync::Arc;

use agent_protocol::session::{
    RuntimePreferencesRequestParams, RuntimePreferencesResult, RuntimePreferencesScope,
};
use mode_protocol_client::{RequestHandler, RequestReply};
use serde_json::Value;

pub const SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD: &str = "session/requestRuntimePreferences";

/// TS `local` 权威下的默认应答。
pub fn default_runtime_preferences() -> RuntimePreferencesResult {
    RuntimePreferencesResult {
        native_search_enhancements_enabled: true,
        memory_enabled: Some(false),
        ask_user_question_auto_resolution_enabled: Some(true),
        integrated_terminal_shell: None,
        model_context_budget_strategy: None,
    }
}

/// 用给定的 scoped resolver 构建入站请求处理器。
///
/// `resolve` 抛错时回 `-32603`（消息原样），与 TS 一致；参数不合法回 `-32602`；
/// 其它方法返回 `NotHandled`（由协议客户端回 `-32601`）。
pub fn runtime_preferences_handler<F>(resolve: F) -> RequestHandler
where
    F: Fn(RuntimePreferencesScope) -> Result<RuntimePreferencesResult, String> + Send + Sync + 'static,
{
    Arc::new(move |request: &Value| {
        if request.get("method").and_then(Value::as_str)
            != Some(SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD)
        {
            return RequestReply::NotHandled;
        }
        let Some(params) = request.get("params").filter(|value| value.is_object()) else {
            return invalid_params();
        };
        let parsed: RuntimePreferencesRequestParams =
            match serde_json::from_value(params.clone()) {
                Ok(parsed) => parsed,
                Err(_) => return invalid_params(),
            };
        match resolve(parsed.scope) {
            Ok(preferences) => match serde_json::to_value(&preferences) {
                Ok(value) => RequestReply::Result(value),
                Err(error) => RequestReply::Error {
                    code: -32603,
                    message: error.to_string(),
                },
            },
            Err(message) => RequestReply::Error {
                code: -32603,
                message,
            },
        }
    })
}

/// 恒用默认值的处理器。
pub fn default_runtime_preferences_handler() -> RequestHandler {
    runtime_preferences_handler(|_| Ok(default_runtime_preferences()))
}

fn invalid_params() -> RequestReply {
    RequestReply::Error {
        code: -32602,
        message: "Invalid session runtime preferences request params".to_string(),
    }
}
