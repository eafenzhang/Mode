//! `mode-agent` 频道适配器（S2 第一批：会话只读组 + MCP 状态）。
//!
//! spec：`docs/specs/host-ownership-switch.md` §5 S2 与
//! `docs/specs/agent-method-coverage.json`。频道默认**不注册**：只有
//! `MODE_HOST_INPROCESS_CHANNELS` 显式包含 `mode-agent` 时才由壳接管。
//! 未迁移的方法一律返回 `unsupported_method`（决策 2 契约），不静默降级。
//!
//! 已知保真缺口（S2 硬门禁前必须补齐，见 spec §5 与覆盖矩阵）：
//! - `readSession` / `createSession` 的 Account Provider Config 预同步与 TS 的
//!   passive-read runtimePolicy 由服务层编排，本层硬编码 `StartIfNeeded`；
//! - `createSession` 的 `sessionTraceId` 缓存与 `ModelSelection`→wire 的转换属服务层；
//! - `resumeSession` 的 `model` 不落 wire（协议 `SessionResumeParams` 无该字段，TS 同）；
//! - `disposeWorkspace` 的等待中 provider-ready continuation / v4 订阅路由 /
//!   CUA 轮次跟踪清理属服务层；
//! - `listMcpServerStatuses` 不做 TS 的 `unrecognized_keys` 兼容重试；
//! - 事件订阅首批只桥 `session.event` 与 `snapshot`：TS 聚合的 `state.updated` /
//!   `permission.request` / `userInput.*` 来自通知与反向请求，尚未接线；订阅建立失败
//!   不做 TS 的指数退避重试；`eventCoalescing`（后台摘要合并）未实现。

use std::sync::Arc;

use agent_process::{
    AgentControlClient, AgentWorkspaceTarget, ControlError, ListSessionsOptions, RuntimePolicy,
};
use agent_protocol::mcp::{McpListMode, McpListResult};
use agent_protocol::message::{MessageModel, MessageWithParts};
use agent_protocol::session::WorkspaceRef;
use agent_protocol::session_create::McpServer;
use agent_protocol::session_debug::SessionDebugSnapshot;
use agent_protocol::session_event::SessionEvent;
use agent_protocol::session_read::{SessionInfo, SessionMode, SessionSubagentsResult};
use agent_protocol::session_write::{
    SessionCompactResult, SessionGoalAction, SessionGoalResult, SessionResumeParams,
    SessionSetModeParams, SessionSetModelParams, SessionSetThoughtLevelParams,
};
use agent_protocol::snapshot::{DeliveryKind, SessionStateSnapshot};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;

use agent_process::CreateSessionOptions;

use crate::params::first_arg_object;

pub const MODE_AGENT_CHANNEL: &str = "mode-agent";

/// 决策 2 契约：Rust 侧未实现的方法返回该错误码（host-ownership-switch §6 决策 2）。
pub const UNSUPPORTED_METHOD_CODE: &str = "unsupported_method";

/// 事件名（TS IModeAgentService.onDynamicSessionEvent）。
pub const SESSION_EVENT_LISTEN: &str = "onDynamicSessionEvent";

pub struct ModeAgentChannel {
    control: Arc<AgentControlClient>,
}

impl ModeAgentChannel {
    pub fn new(control: Arc<AgentControlClient>) -> Self {
        Self { control }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
    #[serde(default)]
    remote_session_id: Option<String>,
}

impl WorkspaceParams {
    fn target(&self) -> AgentWorkspaceTarget {
        let mut target = AgentWorkspaceTarget::new(self.workspace_path.clone());
        if let Some(identity) = &self.workspace_identity {
            target = target.with_identity(identity.clone());
        }
        if let Some(remote_session) = &self.remote_session_id {
            target = target.with_remote_session(remote_session.clone());
        }
        target
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSessionsParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    session_ids: Option<Vec<String>>,
    #[serde(default)]
    runtime_policy: Option<String>,
    #[serde(default)]
    include_archived: Option<bool>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    delivery_kind: Option<String>,
    #[serde(default)]
    message_limit: Option<u64>,
    #[serde(default)]
    after_seq: Option<u64>,
    // runtimePolicy 暂不接受：agent-process 的 read_session 硬编码 StartIfNeeded（见模块头
    // 已知保真缺口）。保留键会被 serde 忽略，不制造假接线。
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionMessagesParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    after_message_id: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionEventsParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    after_seq: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSessionSubagentsParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    ended_cursor: Option<String>,
    #[serde(default)]
    ended_limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionTargetParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
}

fn parse_params<T: serde::de::DeserializeOwned>(
    command: &str,
    arg: Value,
) -> Result<T, ChannelError> {
    let object = first_arg_object(&arg)?;
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| ChannelError::error(format!("invalid {command} request: {error}")))
}

fn policy_from(value: Option<&str>) -> Result<RuntimePolicy, ChannelError> {
    match value {
        None | Some("start-if-needed") => Ok(RuntimePolicy::StartIfNeeded),
        Some("existing-only") => Ok(RuntimePolicy::ExistingOnly),
        Some(other) => Err(ChannelError::error(format!(
            "invalid runtimePolicy: {other}"
        ))),
    }
}

fn delivery_from(value: Option<&str>) -> Result<Option<DeliveryKind>, ChannelError> {
    match value {
        None => Ok(None),
        Some("desktop-continuous") => Ok(Some(DeliveryKind::DesktopContinuous)),
        Some("web-remote-replayable") => Ok(Some(DeliveryKind::WebRemoteReplayable)),
        Some(other) => Err(ChannelError::error(format!(
            "invalid deliveryKind: {other}"
        ))),
    }
}

fn control_error(error: ControlError) -> ChannelError {
    ChannelError::error(error.to_string())
}

fn json_text(json: serde_json::Value) -> Value {
    Value::Json(json.to_string())
}

fn json_value<T: serde::Serialize>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_value(value)
        .map(|json| Value::Json(json.to_string()))
        .map_err(|error| ChannelError::error(format!("serialize result failed: {error}")))
}

/// 未迁移方法：显式 `unsupported_method`（带回滚指引），不做静默降级。
fn unsupported(command: &str) -> ChannelError {
    ChannelError::error(format!(
        "mode-agent method {command} 依赖真实模型/网络或尚未迁移；当前频道由 Rust 所有，\
         如需使用请把它从 MODE_HOST_INPROCESS_CHANNELS 移除以回滚到 Node Host"
    ))
    .with_extra("code", Value::Str(UNSUPPORTED_METHOD_CODE.into()))
}

impl ServerChannel for ModeAgentChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let control = Arc::clone(&self.control);
        Box::pin(async move {
            match command.as_str() {
                "listSessions" => {
                    let params: ListSessionsParams = parse_params("listSessions", arg)?;
                    let policy = policy_from(params.runtime_policy.as_deref())?;
                    let sessions: Vec<SessionInfo> = control
                        .list_sessions(
                            &params.workspace.target(),
                            ListSessionsOptions {
                                include_archived: params.include_archived.unwrap_or(false),
                                limit: params.limit,
                                session_ids: params.session_ids,
                                runtime_policy: policy,
                            },
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&sessions)
                }
                "readSession" => {
                    let params: ReadSessionParams = parse_params("readSession", arg)?;
                    let snapshot: SessionStateSnapshot = control
                        .read_session(
                            &params.workspace.target(),
                            &params.session_id,
                            delivery_from(params.delivery_kind.as_deref())?,
                            params.message_limit,
                            params.after_seq,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "readSessionMessages" => {
                    let params: ReadSessionMessagesParams =
                        parse_params("readSessionMessages", arg)?;
                    let messages: Vec<MessageWithParts> = control
                        .read_session_messages(
                            &params.workspace.target(),
                            &params.session_id,
                            params.after_message_id.as_deref(),
                            params.limit,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&messages)
                }
                "readSessionEvents" => {
                    let params: ReadSessionEventsParams = parse_params("readSessionEvents", arg)?;
                    let events: Vec<SessionEvent> = control
                        .read_session_events(
                            &params.workspace.target(),
                            &params.session_id,
                            params.after_seq,
                            params.limit,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&events)
                }
                "listSessionSubagents" => {
                    let params: ListSessionSubagentsParams =
                        parse_params("listSessionSubagents", arg)?;
                    let result: SessionSubagentsResult = control
                        .list_session_subagents(
                            &params.workspace.target(),
                            &params.session_id,
                            params.ended_cursor.as_deref(),
                            params.ended_limit,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "readSessionDebug" => {
                    let params: SessionTargetParams = parse_params("readSessionDebug", arg)?;
                    let snapshot: SessionDebugSnapshot = control
                        .read_session_debug(&params.workspace.target(), &params.session_id)
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "listMcpServerStatuses" => {
                    let params: ListMcpParams = parse_params("listMcpServerStatuses", arg)?;
                    let result: McpListResult = control
                        .list_mcp_server_statuses(
                            &params.workspace.target(),
                            agent_process::ListMcpServersOptions {
                                mcp_servers: params.mcp_servers,
                                mode: params.mode,
                            },
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "initialize" => {
                    let params: WorkspaceParams = parse_params("initialize", arg)?;
                    let result = control.initialize(&params.target()).await;
                    // AgentInitializeResult 没有 Serialize（内部只读快照）：按 TS
                    // ModeAgentInitializeResult 的 camelCase 形状逐字段输出。
                    Ok(json_text(serde_json::json!({
                        "available": result.available,
                        "workspaceKey": result.workspace_key,
                        "protocolName": result.protocol_name,
                        "protocolVersion": result.protocol_version,
                        "transportKind": result.transport_kind,
                        "reason": result.reason,
                        "reasonCode": result.reason_code,
                    })))
                }
                "prepareStorage" => {
                    let params: WorkspaceParams = parse_params("prepareStorage", arg)?;
                    control
                        .prepare_storage(&params.target())
                        .await
                        .map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                "disposeWorkspace" => {
                    let params: WorkspaceParams = parse_params("disposeWorkspace", arg)?;
                    // TS 还会清理等待中的 provider-ready continuation / v4 订阅路由 /
                    // CUA 轮次跟踪；这些属服务层（spec 已知保真缺口）。
                    control
                        .dispose_workspace(&params.target(), DISPOSE_GRACE)
                        .await;
                    Ok(Value::Undefined)
                }
                "createSession" => {
                    let params: CreateSessionParams = parse_params("createSession", arg)?;
                    let snapshot: SessionStateSnapshot = control
                        .create_session(&params.workspace.target(), params.options())
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "resumeSession" => {
                    let params: ResumeSessionParams = parse_params("resumeSession", arg)?;
                    let target = params.workspace.target();
                    let resume = params.into_wire(&target);
                    let snapshot: SessionStateSnapshot = control
                        .resume_session(&target, resume)
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "setModel" => {
                    let params: SetModelParams = parse_params("setModel", arg)?;
                    let snapshot: SessionStateSnapshot = control
                        .set_session_model(
                            &params.workspace.target(),
                            &params.session_id,
                            &params.model,
                            params.expected_revision,
                            params.persist_as_workspace_last_used,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "setThoughtLevel" => {
                    let params: SetThoughtLevelParams = parse_params("setThoughtLevel", arg)?;
                    let snapshot: SessionStateSnapshot = control
                        .set_session_thought_level(
                            &params.workspace.target(),
                            &params.session_id,
                            params.thought_level.as_deref(),
                            params.expected_revision,
                            params.persist_as_workspace_last_used,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "setMode" => {
                    let params: SetModeParams = parse_params("setMode", arg)?;
                    let snapshot: SessionStateSnapshot = control
                        .set_session_mode(
                            &params.workspace.target(),
                            &params.session_id,
                            params.mode,
                            params.expected_revision,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "compactSession" => {
                    let params: CompactParams = parse_params("compactSession", arg)?;
                    let result: SessionCompactResult = control
                        .compact_session(
                            &params.workspace.target(),
                            agent_protocol::session_write::SessionCompactParams {
                                session_id: params.session_id,
                                input_id: params.input_id,
                                instructions: params.instructions,
                                expected_revision: params.expected_revision,
                            },
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "goalSession" => {
                    let params: GoalParams = parse_params("goalSession", arg)?;
                    let result: SessionGoalResult = control
                        .goal_session(
                            &params.workspace.target(),
                            agent_protocol::session_write::SessionGoalParams {
                                session_id: params.session_id,
                                input_id: params.input_id,
                                action: params.action,
                                objective: params.objective,
                                expected_revision: params.expected_revision,
                            },
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                other => Err(unsupported(other)),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        arg: Value,
        emit: rpc_host::EventCallback,
    ) -> Result<rpc_host::Subscription, ChannelError> {
        if event != SESSION_EVENT_LISTEN {
            return Err(ChannelError::error(format!(
                "mode-agent channel: unknown event {event}"
            )));
        }
        let params: SubscribeParams = parse_params("onDynamicSessionEvent", arg)?;
        let control = Arc::clone(&self.control);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        // 订阅循环可能被同步调用（ChannelServer::listen），这里自带 current-thread
        // runtime，不依赖调用方是否位于 tokio 上下文。
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(async move {
                let target = params.workspace.target();
                let mut subscription = match control
                    .subscribe_session(
                        &target,
                        &params.session_id,
                        params.delivery_kind,
                        params.after_seq,
                        params.include_snapshot.unwrap_or(false),
                    )
                    .await
                {
                    Ok(subscription) => subscription,
                    // TS 对订阅建立失败做指数退避重试（SESSION_SUBSCRIBE_MAX_ATTEMPTS）；
                    // Rust 首批不做重试（已知缺口），失败即结束该订阅。
                    Err(_) => return,
                };
                let emit_json = |json: serde_json::Value| {
                    if let Ok(text) = serde_json::to_string(&json) {
                        emit(Value::Json(text));
                    }
                };
                // TS 顺序：replay（已排序）→ snapshot → 实时事件。
                for event in &subscription.replay {
                    if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    emit_json(serde_json::json!({ "type": "session.event", "event": event }));
                }
                if let Some(snapshot) = &subscription.snapshot {
                    if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    emit_json(serde_json::json!({ "type": "snapshot", "snapshot": snapshot }));
                }
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                            if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                                return;
                            }
                        }
                        event = subscription.recv() => match event {
                            Some(event) => emit_json(
                                serde_json::json!({ "type": "session.event", "event": event }),
                            ),
                            None => return,
                        },
                    }
                }
            });
        });
        Ok(Box::new(move || {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMcpParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    mcp_servers: Option<Vec<McpServer>>,
    #[serde(default)]
    mode: Option<McpListMode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    delivery_kind: DeliveryKind,
    #[serde(default)]
    after_seq: Option<u64>,
    #[serde(default)]
    include_snapshot: Option<bool>,
}

/// `disposeWorkspace` 的优雅关闭宽限（TS 服务层使用进程管理器的默认清理节奏）。
const DISPOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    mode: Option<SessionMode>,
    #[serde(default)]
    model: Option<MessageModel>,
    #[serde(default)]
    persistence: Option<agent_protocol::session_write::SessionPersistence>,
    #[serde(default)]
    thought_level: Option<String>,
    #[serde(default)]
    title_generation_enabled: Option<bool>,
    #[serde(default)]
    mcp_servers: Option<Vec<McpServer>>,
    #[serde(default)]
    tool_allowlist: Option<Vec<String>>,
    #[serde(default)]
    tool_denylist: Option<Vec<String>>,
    #[serde(default)]
    imported_history: Option<agent_protocol::session_create::ImportHistory>,
}

impl CreateSessionParams {
    fn options(self) -> CreateSessionOptions {
        CreateSessionOptions {
            session_id: self.session_id,
            parent_session_id: self.parent_session_id,
            mode: self.mode,
            model: self.model,
            persistence: self.persistence,
            thought_level: self.thought_level,
            title_generation_enabled: self.title_generation_enabled,
            mcp_servers: self.mcp_servers,
            tool_allowlist: self.tool_allowlist,
            tool_denylist: self.tool_denylist,
            imported_history: self.imported_history,
            off_peak_tool_enabled: false,
            dynamic_workflow_enabled: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeSessionParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    thought_level: Option<String>,
    #[serde(default)]
    mcp_servers: Option<Vec<McpServer>>,
    #[serde(default)]
    tool_allowlist: Option<Vec<String>>,
    #[serde(default)]
    tool_denylist: Option<Vec<String>>,
}

impl ResumeSessionParams {
    fn into_wire(self, target: &AgentWorkspaceTarget) -> SessionResumeParams {
        SessionResumeParams {
            session_id: self.session_id,
            workspace: Some(WorkspaceRef {
                workspace_path: target.workspace_path.clone(),
                workspace_key: target.workspace_key(),
                workspace_identity: target.workspace_identity.clone(),
                remote_session_id: target.remote_session_id.clone(),
                extra: Default::default(),
            }),
            thought_level: self.thought_level,
            mcp_servers: self.mcp_servers,
            tool_allowlist: self.tool_allowlist,
            tool_denylist: self.tool_denylist,
            off_peak_tool_enabled: None,
            dynamic_workflow_enabled: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetModelParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    model: MessageModel,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    persist_as_workspace_last_used: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetThoughtLevelParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    thought_level: Option<String>,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    persist_as_workspace_last_used: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetModeParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    mode: SessionMode,
    #[serde(default)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompactParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    input_id: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    input_id: Option<String>,
    action: SessionGoalAction,
    #[serde(default)]
    objective: Option<String>,
    #[serde(default)]
    expected_revision: Option<u64>,
}

/// 写组方法的 setter 参数使用的协议类型别名（避免顶层 import 与只读组重名）。
#[allow(dead_code)]
type _WireSetModel = SessionSetModelParams;
#[allow(dead_code)]
type _WireSetThoughtLevel = SessionSetThoughtLevelParams;
#[allow(dead_code)]
type _WireSetMode = SessionSetModeParams;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_options_maps_protocol_field_set() {
        let params: CreateSessionParams = serde_json::from_value(serde_json::json!({
            "workspacePath": "D:/ws",
            "sessionId": "s-1",
            "mode": "build",
            "titleGenerationEnabled": false,
            "toolAllowlist": ["bash"]
        }))
        .expect("createSession 参数");
        let options = params.options();
        assert_eq!(options.session_id.as_deref(), Some("s-1"));
        assert_eq!(options.title_generation_enabled, Some(false));
        assert_eq!(options.tool_allowlist, Some(vec!["bash".to_string()]));
        // 灰度开关缺省不下发。
        assert!(!options.off_peak_tool_enabled);
        assert!(!options.dynamic_workflow_enabled);
    }

    #[test]
    fn resume_session_fills_workspace_ref_and_drops_unwired_fields() {
        let params: ResumeSessionParams = serde_json::from_value(serde_json::json!({
            "workspacePath": "D:/ws",
            "workspaceIdentity": "remote-1",
            "sessionId": "s-1",
            "thoughtLevel": "high",
            "model": { "providerId": "p", "modelId": "m" }
        }))
        .expect("resumeSession 参数");
        let target = params.workspace.target();
        let wire = params.into_wire(&target);
        let workspace = wire.workspace.expect("workspace ref");
        assert_eq!(workspace.workspace_path, "D:/ws");
        assert_eq!(workspace.workspace_key, "remote-1");
        assert_eq!(wire.thought_level.as_deref(), Some("high"));
        assert_eq!(wire.off_peak_tool_enabled, None);
    }
}
