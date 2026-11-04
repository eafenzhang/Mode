//! 运行时偏好反向请求的宿主侧应答。
//!
//! 真源：`packages/services/src/mode-agent/modeAgentService.ts` 里
//! `session/requestRuntimePreferences` 的 `sessionRuntimePreferencesAuthority === "local"` 分支：
//! 宿主直接回 `{askUserQuestionAutoResolutionEnabled: true, nativeSearchEnhancementsEnabled: true,
//! memoryEnabled: false}`，参数非法回 `-32602`，解析失败回 `-32603`。
//!
//! 规范见 `docs/specs/session-write-group.md` §「反向请求」。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_protocol::session::{
    RuntimePreferencesRequestParams, RuntimePreferencesResult, RuntimePreferencesScope,
};
use mode_protocol_client::{ReplyHandle, RequestHandler, RequestReply};
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
    F: Fn(RuntimePreferencesScope) -> Result<RuntimePreferencesResult, String>
        + Send
        + Sync
        + 'static,
{
    Arc::new(move |request: &Value, _reply: ReplyHandle| {
        runtime_preferences_reply(request, &resolve)
    })
}

/// 单条入站请求的同步应答（handler 与测试共用）。
pub fn runtime_preferences_reply<F>(request: &Value, resolve: &F) -> RequestReply
where
    F: Fn(RuntimePreferencesScope) -> Result<RuntimePreferencesResult, String>,
{
    if request.get("method").and_then(Value::as_str)
        != Some(SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD)
    {
        return RequestReply::NotHandled;
    }
    let Some(params) = request.get("params").filter(|value| value.is_object()) else {
        return invalid_params();
    };
    let parsed: RuntimePreferencesRequestParams = match serde_json::from_value(params.clone()) {
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
}

/// 外部权威（desktop-attached-remote）的待回复请求表。
pub struct PendingSessionRuntimePreferences {
    pending: Mutex<HashMap<String, ReplyHandle>>,
    next_id: AtomicU64,
}

/// `respondSessionRuntimePreferences` 的两种解析结果。
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimePreferencesResolution {
    Resolved(RuntimePreferencesResult),
    Failed(String),
}

impl Default for PendingSessionRuntimePreferences {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
        }
    }
}

impl PendingSessionRuntimePreferences {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn mint_request_id(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let sequence = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("runtime-preferences-{now:x}-{sequence:x}")
    }

    /// 回复一条 pending 请求；未知 id 返回与 TS 相同的错误文案。
    pub fn respond(
        &self,
        request_id: &str,
        resolution: RuntimePreferencesResolution,
    ) -> Result<(), String> {
        let handle = self
            .pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(request_id)
            .ok_or_else(|| {
                format!("Mode session runtime preferences request not found: {request_id}")
            })?;
        match resolution {
            RuntimePreferencesResolution::Resolved(preferences) => {
                match serde_json::to_value(&preferences) {
                    Ok(value) => handle.respond(RequestReply::Result(value)),
                    Err(error) => handle.respond(RequestReply::Error {
                        code: -32603,
                        message: error.to_string(),
                    }),
                }
            }
            RuntimePreferencesResolution::Failed(message) => handle.respond(RequestReply::Error {
                code: -32603,
                message,
            }),
        }
        Ok(())
    }

    fn take(&self, request_id: &str) -> Option<ReplyHandle> {
        self.pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(request_id)
    }
}

/// 外部权威处理器：把请求交给订阅方（renderer），等 `respond` 回复。
///
/// `on_request` 收到带 `requestId` 的完整参数；没有订阅方时立即回 -32603。
pub fn external_runtime_preferences_handler<F>(
    pending: Arc<PendingSessionRuntimePreferences>,
    on_request: F,
) -> RequestHandler
where
    F: Fn(Value) -> bool + Send + Sync + 'static,
{
    Arc::new(move |request: &Value, reply: ReplyHandle| {
        if request.get("method").and_then(Value::as_str)
            != Some(SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD)
        {
            return RequestReply::NotHandled;
        }
        let Some(params) = request
            .get("params")
            .and_then(Value::as_object)
            .cloned()
        else {
            return invalid_params();
        };
        let parsed: RuntimePreferencesRequestParams =
            match serde_json::from_value(Value::Object(params.clone())) {
                Ok(parsed) => parsed,
                Err(_) => return invalid_params(),
            };
        // 校验后的字段原样回传，保留 scope/sessionId。
        let _ = parsed;
        let request_id = pending.mint_request_id();
        let mut payload = params;
        payload.insert("requestId".to_owned(), Value::String(request_id.clone()));
        pending
            .pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(request_id.clone(), reply);
        if !on_request(Value::Object(payload)) {
            if let Some(handle) = pending.take(&request_id) {
                handle.respond(RequestReply::Error {
                    code: -32603,
                    message: "Mode session runtime preferences listener is unavailable"
                        .to_owned(),
                });
            }
        }
        RequestReply::Deferred
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
