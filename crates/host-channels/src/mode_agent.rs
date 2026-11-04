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
//! - 已接：getStorageStartupState/getWorkspaceRuntimeIdentity/syncAppRuntimePreferences/
//!   closeSession/grantWorkspaceHookTrust/resolveSuggestedPluginReference/
//!   getSkillReferenceCatalog/readWorkspacePresentation/disposeAll；
//! - 未接：collectLocalRuntimeChildProcesses（宿主进程面）、cancelPluginOperation
//!   （无 workspace，需 operationId 路由）、hasActiveCuaOperationTurn（CUA 跟踪属
//!   TS 服务层）、getAppUsageStats/getTaskTokenUsage（agent 数据库统计）、automation
//!   十方法（按决策 4 由 Renderer 分别调用两个 owner）与多数事件源（进程采样 /
//!   v4 帧 / runtime 偏好请求 / CUA 观察）；
//! - workflows 六方法已接（global 载体简化为「首个本地活跃 runtime → 管理面
//!   workspace」，TS 的 reusable/stale 清理与 cancelPluginOperation（无 workspace、
//!   需按 operationId 路由到持有 runtime）未接）；
//! - plugins 首批只覆盖 list/overview/describe/setEnabled/install/uninstall/
//!   update/configure/validate/referenceCatalog；restoreBuiltin/resetConfig/
//!   cancelOperation/marketplace 增删改与 suggested 引用解析未接线；
//! - `sendPrompt` 不做 TS 的 `unrecognized_keys` 兼容重试，且缺少
//!   browserAmbientContext 的 executor 收集（TS 缺省时主动采集）；
//! - v4 conversation 帧已接：subscribeConversationV4/unsubscribeConversationV4 +
//!   onDynamicConversationFrame 事件（帧按 workspaceKey fan-out，归属键替换旧订阅；
//!   连接身份按宿主内部直调默认 rust-inprocess/desktop-continuous）；sessions-index /
//!   workspace-config / resync / rowsRange 未接线；
//! - 事件订阅已桥 `session.event`/`snapshot` 与 onDynamicStorageStartupState
//!   （generation 取订阅时快照，进程换代后 watch 结束即停止推送）；：TS 聚合的 `state.updated` /
//!   `permission.request` / `userInput.*` 来自通知与反向请求，尚未接线；订阅建立失败
//!   不做 TS 的指数退避重试；`eventCoalescing`（后台摘要合并）未实现。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_process::{
    AgentControlClient, AgentWorkspaceTarget, AppRuntimePreferences, ControlError,
    GrantTrustResult, ListSessionsOptions, RuntimeIdentity, RuntimePolicy, StorageStartupSnapshot,
};
use agent_protocol::mcp::{McpListMode, McpListResult};
use agent_protocol::message::{MessageModel, MessageWithParts};
use agent_protocol::plugin::{
    PluginDeliveryKind, PluginScope, PluginsConfigureResult, PluginsDescribeResult,
    PluginsInstallResult, PluginsListResult, PluginsMarketplaceMutationResult,
    PluginsOverviewResult, PluginsReferenceCatalogResult, PluginsResolveSuggestedReferenceResult,
    PluginsRestoreBuiltinResult, PluginsSetEnabledResult, PluginsUninstallResult,
    PluginsValidateResult,
};
use agent_protocol::session::{SessionSendParams, SessionSendResult, WorkspaceRef};
use agent_protocol::session_create::McpServer;
use agent_protocol::session_debug::SessionDebugSnapshot;
use agent_protocol::session_event::SessionEvent;
use agent_protocol::session_read::{SessionInfo, SessionMode, SessionSubagentsResult};
use agent_protocol::session_send::{
    BotDeliveryTarget, BrowserAmbientContext, ModelExecution, OffPeakRunType,
};
use agent_protocol::session_write::{
    SessionCompactResult, SessionGoalAction, SessionGoalResult, SessionPersistence,
    SessionResumeParams, SessionSetModeParams, SessionSetModelParams, SessionSetThoughtLevelParams,
};
use agent_protocol::skills::SkillsReferenceCatalogResult;
use agent_protocol::snapshot::{DeliveryKind, SessionStateSnapshot};
use agent_protocol::v4_transport::{SubscribeBase, V4ClientMode};
use agent_protocol::workflow::{
    SavedWorkflowMeta, SavedWorkflowScope, WorkflowsDeleteResult, WorkflowsGetResult,
    WorkflowsListResult, WorkflowsMoveResult, WorkflowsRunsResult, WorkflowsUpdateMetaResult,
};
use agent_protocol::workspace::WorkspacePresentation;
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

/// 事件名（TS `IModeAgentService.onDynamicStorageStartupState`）。
pub const STORAGE_STARTUP_LISTEN: &str = "onDynamicStorageStartupState";

pub struct ModeAgentChannel {
    control: Arc<AgentControlClient>,
    /// 工作流 global scope 的管理面载体（dataRoot/.mode/plugin-workspace，宿主注入）。
    management_workspace: std::path::PathBuf,
    /// v4 conversation 帧监听者（workspaceKey -> 回调表）。
    frame_listeners: Arc<Mutex<HashMap<String, Vec<rpc_host::EventCallback>>>>,
    /// 活跃订阅（subscriptionId -> route）与归属键索引。
    routes: Arc<Mutex<HashMap<String, V4Route>>>,
    route_owners: Arc<Mutex<HashMap<String, String>>>,
}

impl ModeAgentChannel {
    pub fn new(control: Arc<AgentControlClient>, management_workspace: std::path::PathBuf) -> Self {
        Self {
            control,
            management_workspace,
            frame_listeners: Arc::new(Mutex::new(HashMap::new())),
            routes: Arc::new(Mutex::new(HashMap::new())),
            route_owners: Arc::new(Mutex::new(HashMap::new())),
        }
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
        let management_workspace = self.management_workspace.clone();
        let frame_listeners_handle = Arc::clone(&self.frame_listeners);
        let routes_handle = Arc::clone(&self.routes);
        let route_owners_handle = Arc::clone(&self.route_owners);
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
                "sendPrompt" => {
                    let params: SendPromptParams = parse_params("sendPrompt", arg)?;
                    let target = params.workspace.target();
                    let result: SessionSendResult = control
                        .send_prompt(&target, params.into_wire())
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
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
                "listPlugins" => {
                    let params: PluginViewParams = parse_params("listPlugins", arg)?;
                    let result: PluginsListResult = control
                        .list_plugins(&params.workspace.target(), params.config_scope)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "getPluginsOverview" => {
                    let params: PluginViewParams = parse_params("getPluginsOverview", arg)?;
                    let result: PluginsOverviewResult = control
                        .get_plugins_overview(&params.workspace.target(), params.config_scope)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "describePlugin" => {
                    let params: DescribePluginParams = parse_params("describePlugin", arg)?;
                    let result: PluginsDescribeResult = control
                        .describe_plugin(
                            &params.workspace.target(),
                            &params.marketplace,
                            &params.plugin_name,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "setPluginEnabled" => {
                    let params: SetPluginEnabledParams = parse_params("setPluginEnabled", arg)?;
                    let result: PluginsSetEnabledResult = control
                        .set_plugin_enabled(
                            &params.workspace.target(),
                            &params.plugin_id,
                            params.enabled,
                            params.operation_id.as_deref(),
                            params.scope,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "installPlugin" => {
                    let params: InstallPluginParams = parse_params("installPlugin", arg)?;
                    let result: PluginsInstallResult = control
                        .install_plugin(
                            &params.workspace.target(),
                            &params.plugin_name,
                            &params.marketplace,
                            params.scope,
                            params.dry_run,
                            params.operation_id.as_deref(),
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "uninstallPlugin" => {
                    let params: UninstallPluginParams = parse_params("uninstallPlugin", arg)?;
                    let result: PluginsUninstallResult = control
                        .uninstall_plugin(
                            &params.workspace.target(),
                            params.plugin_id.as_deref(),
                            params.plugin_name.as_deref(),
                            params.marketplace.as_deref(),
                            params.remove_cache,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "updatePlugin" => {
                    let params: UpdatePluginParams = parse_params("updatePlugin", arg)?;
                    let result: PluginsInstallResult = control
                        .update_plugin(
                            &params.workspace.target(),
                            params.plugin_id.as_deref(),
                            params.marketplace.as_deref(),
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "configurePlugin" => {
                    let params: ConfigurePluginParams = parse_params("configurePlugin", arg)?;
                    let result: PluginsConfigureResult = control
                        .configure_plugin(
                            &params.workspace.target(),
                            &params.plugin_id,
                            params.options,
                            params.clear_option_keys,
                            params.scope,
                            params.dry_run,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "validatePlugin" => {
                    let params: ValidatePluginParams = parse_params("validatePlugin", arg)?;
                    let result: PluginsValidateResult = control
                        .validate_plugin(
                            &params.workspace.target(),
                            params.plugin_name.as_deref(),
                            params.marketplace.as_deref(),
                            params.source.as_deref(),
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "getPluginReferenceCatalog" => {
                    let params: PluginReferenceCatalogParams =
                        parse_params("getPluginReferenceCatalog", arg)?;
                    let result: PluginsReferenceCatalogResult = control
                        .plugin_reference_catalog(
                            &params.workspace.target(),
                            params.session_id.as_deref(),
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "addPluginMarketplace" => {
                    let params: AddMarketplaceParams = parse_params("addPluginMarketplace", arg)?;
                    let result: PluginsMarketplaceMutationResult = control
                        .add_plugin_marketplace(
                            &params.workspace.target(),
                            &params.source,
                            params.dry_run,
                            params.operation_id.as_deref(),
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "removePluginMarketplace" => {
                    let params: RemoveMarketplaceParams =
                        parse_params("removePluginMarketplace", arg)?;
                    let result: PluginsMarketplaceMutationResult = control
                        .remove_plugin_marketplace(&params.workspace.target(), &params.marketplace)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "updatePluginMarketplace" => {
                    let params: UpdateMarketplaceParams =
                        parse_params("updatePluginMarketplace", arg)?;
                    let result: PluginsMarketplaceMutationResult = control
                        .update_plugin_marketplace(
                            &params.workspace.target(),
                            params.marketplace.as_deref(),
                            params.operation_id.as_deref(),
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "restoreBuiltinPlugin" => {
                    let params: RestoreBuiltinParams = parse_params("restoreBuiltinPlugin", arg)?;
                    let result: PluginsRestoreBuiltinResult = control
                        .restore_builtin_plugin(&params.workspace.target(), &params.plugin_id)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "resetPluginConfig" => {
                    let params: ResetPluginConfigParams = parse_params("resetPluginConfig", arg)?;
                    let result: PluginsConfigureResult = control
                        .reset_plugin_config(
                            &params.workspace.target(),
                            &params.plugin_id,
                            params.scope,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "listSavedWorkflows" => {
                    let params: WorkflowTargetParams = parse_params("listSavedWorkflows", arg)?;
                    let target = self_carrier_target(&control, &management_workspace, &params);
                    let result: WorkflowsListResult = control
                        .list_saved_workflows(&target, params.scope)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "getSavedWorkflow" => {
                    let params: WorkflowNameParams = parse_params("getSavedWorkflow", arg)?;
                    let target =
                        self_carrier_target(&control, &management_workspace, &params.target);
                    let result: WorkflowsGetResult = control
                        .get_saved_workflow(&target, &params.name, params.target.scope)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "listSavedWorkflowRuns" => {
                    let params: WorkflowRunsParams = parse_params("listSavedWorkflowRuns", arg)?;
                    let target =
                        self_carrier_target(&control, &management_workspace, &params.target);
                    let result: WorkflowsRunsResult = control
                        .list_saved_workflow_runs(
                            &target,
                            params.name.as_deref(),
                            params.limit,
                            params.target.scope,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "updateSavedWorkflowMeta" => {
                    let params: WorkflowUpdateMetaParams =
                        parse_params("updateSavedWorkflowMeta", arg)?;
                    let target =
                        self_carrier_target(&control, &management_workspace, &params.target);
                    let result: WorkflowsUpdateMetaResult = control
                        .update_saved_workflow_meta(
                            &target,
                            &params.name,
                            params.meta,
                            params.target.scope,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "deleteSavedWorkflow" => {
                    let params: WorkflowNameParams = parse_params("deleteSavedWorkflow", arg)?;
                    let target =
                        self_carrier_target(&control, &management_workspace, &params.target);
                    let result: WorkflowsDeleteResult = control
                        .delete_saved_workflow(&target, &params.name, params.target.scope)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "moveSavedWorkflow" => {
                    let params: MoveWorkflowParams = parse_params("moveSavedWorkflow", arg)?;
                    let result: WorkflowsMoveResult = control
                        .move_saved_workflow(&params.workspace.target(), &params.name)
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "getStorageStartupState" => {
                    let params: WorkspaceParams = parse_params("getStorageStartupState", arg)?;
                    let snapshot: Option<StorageStartupSnapshot> =
                        control.storage_startup_state(&params.target());
                    match snapshot {
                        Some(snapshot) => json_value(&snapshot),
                        None => Ok(Value::Json("null".to_string())),
                    }
                }
                "getWorkspaceRuntimeIdentity" => {
                    let params: WorkspaceParams = parse_params("getWorkspaceRuntimeIdentity", arg)?;
                    let identity: Option<RuntimeIdentity> =
                        control.runtime_identity(&params.target());
                    match identity {
                        Some(identity) => json_value(&identity),
                        // TS 在无 runtime 时抛同文案。
                        None => Err(ChannelError::error(
                            "Mode agent runtime identity is unavailable.",
                        )),
                    }
                }
                "syncAppRuntimePreferences" => {
                    let params: AppPreferencesParams =
                        parse_params("syncAppRuntimePreferences", arg)?;
                    control
                        .sync_app_runtime_preferences(AppRuntimePreferences {
                            ask_user_question_auto_resolution_enabled: params
                                .ask_user_question_auto_resolution_enabled,
                            model_io_full_retention_enabled: params.model_io_full_retention_enabled,
                        })
                        .await
                        .map_err(control_error)?;
                    Ok(Value::Undefined)
                }
                "closeSession" => {
                    let params: CloseSessionParams = parse_params("closeSession", arg)?;
                    let closed = control
                        .close_session(
                            &params.workspace.target(),
                            &params.session_id,
                            params.expected_persistence,
                        )
                        .await
                        .map_err(control_error)?;
                    Ok(json_text(serde_json::json!(closed)))
                }
                "grantWorkspaceHookTrust" => {
                    let params: GrantHookTrustParams =
                        parse_params("grantWorkspaceHookTrust", arg)?;
                    let result: GrantTrustResult = control
                        .grant_workspace_hook_trust(
                            &params.workspace.target(),
                            &params.bundle_digest,
                            &params.hook_declaration_digest,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "resolveSuggestedPluginReference" => {
                    let params: ResolveSuggestedParams =
                        parse_params("resolveSuggestedPluginReference", arg)?;
                    let result: PluginsResolveSuggestedReferenceResult = control
                        .resolve_suggested_plugin_reference(
                            &params.workspace.target(),
                            &params.stable_id,
                            &params.operation_id,
                            params.client_mode,
                            params.delivery_kind,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "getSkillReferenceCatalog" => {
                    let params: PluginReferenceCatalogParams =
                        parse_params("getSkillReferenceCatalog", arg)?;
                    let result: SkillsReferenceCatalogResult = control
                        .skill_reference_catalog(
                            &params.workspace.target(),
                            params.session_id.as_deref(),
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "readWorkspacePresentation" => {
                    let params: WorkspaceParams = parse_params("readWorkspacePresentation", arg)?;
                    let result: WorkspacePresentation = control
                        .read_workspace_presentation(&params.target())
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "disposeAll" => {
                    control.dispose_all(DISPOSE_GRACE).await;
                    Ok(Value::Undefined)
                }
                "subscribeConversationV4" => {
                    let params: SubscribeV4Params = parse_params("subscribeConversationV4", arg)?;
                    let target = params.workspace.target();
                    let workspace_key = target.workspace_key();
                    let session_id = params.session_id;
                    let topic = format!("conversation/{session_id}");
                    let base = params.base.map(|base| SubscribeBase {
                        log_epoch: base.log_epoch,
                        seq: base.seq,
                    });
                    let visibility = visibility_from(params.visibility.as_deref())?;
                    // 归属键相同的旧订阅先停掉（TS 的 ownership 替换语义）。
                    let owner_key =
                        format!("{workspace_key}\u{0}{topic}\u{0}{V4_LOCAL_CONNECTION_ID}");
                    if let Some(old_id) = route_owners_handle.lock().unwrap().remove(&owner_key) {
                        if let Some(old) = routes_handle.lock().unwrap().remove(&old_id) {
                            old.stop.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    let listeners = Arc::clone(&frame_listeners_handle);
                    let routes = Arc::clone(&routes_handle);
                    let route_owners = Arc::clone(&route_owners_handle);
                    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let control2 = Arc::clone(&control);
                    let stop_flag = Arc::clone(&stop);
                    let owner_for_pump = owner_key.clone();
                    let workspace_for_pump = workspace_key.clone();
                    let topic_for_pump = topic.clone();
                    let (ack_tx, ack_rx) =
                        tokio::sync::oneshot::channel::<Result<serde_json::Value, String>>();
                    // 订阅与帧泵自带 current-thread runtime：ChannelServer 的调用上下文
                    // 不保证已有 tokio 运行时。
                    std::thread::spawn(move || {
                        let runtime = match tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                        {
                            Ok(runtime) => runtime,
                            Err(error) => {
                                let _ = ack_tx.send(Err(error.to_string()));
                                return;
                            }
                        };
                        runtime.block_on(async move {
                            let mut subscription = match control2
                                .subscribe_conversation_frames(
                                    &target,
                                    &session_id,
                                    V4_LOCAL_CONNECTION_ID,
                                    V4ClientMode::DesktopContinuous,
                                    base,
                                    visibility,
                                )
                                .await
                            {
                                Ok(subscription) => subscription,
                                Err(error) => {
                                    let _ = ack_tx.send(Err(error.to_string()));
                                    return;
                                }
                            };
                            let subscription_id = subscription.ack.subscription_id.clone();
                            routes.lock().unwrap().insert(
                                subscription_id.clone(),
                                V4Route {
                                    workspace_key: workspace_for_pump.clone(),
                                    topic: topic_for_pump.clone(),
                                    subscription_id: subscription_id.clone(),
                                    connection_id: V4_LOCAL_CONNECTION_ID.to_string(),
                                    stop: Arc::clone(&stop_flag),
                                },
                            );
                            route_owners
                                .lock()
                                .unwrap()
                                .insert(owner_for_pump.clone(), subscription_id.clone());
                            let ack_json =
                                serde_json::to_value(&subscription.ack).unwrap_or_default();
                            let _ = ack_tx.send(Ok(ack_json));
                            loop {
                                tokio::select! {
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                                        if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                                            break;
                                        }
                                    }
                                    frame = subscription.recv() => match frame {
                                        Some(frame) => {
                                            let Ok(text) = serde_json::to_string(&frame) else {
                                                continue;
                                            };
                                            let callbacks = listeners
                                                .lock()
                                                .unwrap()
                                                .get(&workspace_for_pump)
                                                .cloned()
                                                .unwrap_or_default();
                                            for callback in callbacks {
                                                callback(Value::Json(text.clone()));
                                            }
                                        }
                                        None => break,
                                    },
                                }
                            }
                            routes.lock().unwrap().remove(&subscription_id);
                            route_owners.lock().unwrap().remove(&owner_for_pump);
                        });
                    });
                    match ack_rx.await {
                        Ok(Ok(ack)) => Ok(json_text(ack)),
                        Ok(Err(message)) => Err(ChannelError::error(message)),
                        Err(_) => Err(ChannelError::error("subscribeConversationV4 aborted")),
                    }
                }
                "unsubscribeConversationV4" => {
                    let params: UnsubscribeV4Params =
                        parse_params("unsubscribeConversationV4", arg)?;
                    let target = params.workspace.target();
                    let workspace_key = target.workspace_key();
                    let route = routes_handle
                        .lock()
                        .unwrap()
                        .remove(&params.subscription_id)
                        .filter(|route| route.workspace_key == workspace_key);
                    if let Some(route) = route {
                        route.stop.store(true, std::sync::atomic::Ordering::SeqCst);
                        route_owners_handle.lock().unwrap().remove(&format!(
                            "{}\u{0}{}\u{0}{}",
                            route.workspace_key, route.topic, route.connection_id
                        ));
                        control
                            .unsubscribe_conversation_frames(
                                &target,
                                &route.topic,
                                &route.subscription_id,
                                &route.connection_id,
                            )
                            .await
                            .map_err(control_error)?;
                    }
                    Ok(Value::Undefined)
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
        if event == CONVERSATION_FRAME_LISTEN {
            let params: WorkspaceParams = parse_params("onDynamicConversationFrame", arg)?;
            let workspace_key = params.target().workspace_key();
            let listeners = Arc::clone(&self.frame_listeners);
            let callback: rpc_host::EventCallback = Arc::new(move |value| emit(value));
            listeners
                .lock()
                .unwrap()
                .entry(workspace_key.clone())
                .or_default()
                .push(Arc::clone(&callback));
            let listeners_for_dispose = Arc::clone(&listeners);
            let key_for_dispose = workspace_key.clone();
            return Ok(Box::new(move || {
                let mut map = listeners_for_dispose.lock().unwrap();
                if let Some(callbacks) = map.get_mut(&key_for_dispose) {
                    callbacks.retain(|candidate| !Arc::ptr_eq(candidate, &callback));
                    if callbacks.is_empty() {
                        map.remove(&key_for_dispose);
                    }
                }
            }));
        }
        if event == STORAGE_STARTUP_LISTEN {
            let params: WorkspaceParams = parse_params("onDynamicStorageStartupState", arg)?;
            let control = Arc::clone(&self.control);
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_flag = Arc::clone(&stop);
            std::thread::spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => return,
                };
                runtime.block_on(async move {
                    let target = params.target();
                    // 没有活进程时无事件可推（与 TS 的静默语义一致）。
                    let Some(mut receiver) = control.subscribe_storage_startup(&target) else {
                        return;
                    };
                    let generation = control
                        .storage_startup_state(&target)
                        .map(|snapshot| snapshot.generation)
                        .unwrap_or(0);
                    let emit_snapshot = |state: &Option<agent_process::StorageStartupState>| {
                        let json = serde_json::json!({
                            "generation": generation,
                            "state": state,
                        });
                        if let Ok(text) = serde_json::to_string(&json) {
                            emit(Value::Json(text));
                        }
                    };
                    loop {
                        if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                            return;
                        }
                        match receiver.changed().await {
                            Ok(()) => {
                                let state = receiver.borrow().clone();
                                emit_snapshot(&state);
                            }
                            Err(_) => return,
                        }
                    }
                });
            });
            return Ok(Box::new(move || {
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        }
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

/// `sendPrompt` 的频道参数：按 TS `buildSessionSendParams` 的落 wire 字段集映射；
/// messageId/clientMode/sessionTraceId/runtimeProviderHeaders 只用于服务层记账，不落 wire。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendPromptParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    model_selection: Option<MessageModel>,
    #[serde(default)]
    model_execution: Option<ModelExecution>,
    #[serde(default)]
    input_id: Option<String>,
    #[serde(default)]
    query_id: Option<String>,
    content: String,
    #[serde(default)]
    attachments: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
    #[serde(default)]
    browser_ambient_context: Option<BrowserAmbientContext>,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    expected_provider_revision: Option<String>,
    #[serde(default)]
    automation_id: Option<String>,
    #[serde(default)]
    off_peak_task_id: Option<String>,
    #[serde(default)]
    off_peak_run_type: Option<OffPeakRunType>,
    #[serde(default)]
    bot_delivery_target: Option<BotDeliveryTarget>,
    #[serde(default)]
    tool_denylist: Option<Vec<String>>,
}

impl SendPromptParams {
    fn into_wire(self) -> SessionSendParams {
        SessionSendParams {
            session_id: self.session_id,
            model_selection: self.model_selection,
            model_execution: self.model_execution,
            input_id: self.input_id,
            query_id: self.query_id,
            content: self.content,
            attachments: self.attachments,
            browser_ambient_context: self.browser_ambient_context,
            expected_revision: self.expected_revision,
            expected_provider_revision: self.expected_provider_revision,
            automation_id: self.automation_id,
            off_peak_task_id: self.off_peak_task_id,
            off_peak_run_type: self.off_peak_run_type,
            bot_delivery_target: self.bot_delivery_target,
            tool_denylist: self.tool_denylist,
        }
    }
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginViewParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    config_scope: Option<PluginScope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescribePluginParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    marketplace: String,
    plugin_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetPluginEnabledParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    plugin_id: String,
    enabled: bool,
    #[serde(default)]
    operation_id: Option<String>,
    #[serde(default)]
    scope: Option<PluginScope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallPluginParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    dry_run: Option<bool>,
    marketplace: String,
    #[serde(default)]
    operation_id: Option<String>,
    plugin_name: String,
    #[serde(default)]
    scope: Option<PluginScope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UninstallPluginParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    marketplace: Option<String>,
    #[serde(default)]
    plugin_id: Option<String>,
    #[serde(default)]
    plugin_name: Option<String>,
    #[serde(default)]
    remove_cache: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePluginParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    plugin_id: Option<String>,
    #[serde(default)]
    marketplace: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigurePluginParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    clear_option_keys: Option<Vec<String>>,
    #[serde(default)]
    dry_run: Option<bool>,
    options: std::collections::BTreeMap<String, serde_json::Value>,
    plugin_id: String,
    #[serde(default)]
    scope: Option<PluginScope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidatePluginParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    marketplace: Option<String>,
    #[serde(default)]
    plugin_name: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginReferenceCatalogParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMarketplaceParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    dry_run: Option<bool>,
    #[serde(default)]
    operation_id: Option<String>,
    source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveMarketplaceParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    marketplace: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMarketplaceParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    marketplace: Option<String>,
    #[serde(default)]
    operation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreBuiltinParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    plugin_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetPluginConfigParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    plugin_id: String,
    #[serde(default)]
    scope: Option<PluginScope>,
}

/// 工作流频道参数：workspace 可省（仅 `scope:"global"` 时），与 TS union 同口径。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowTargetParams {
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    workspace_identity: Option<String>,
    #[serde(default)]
    remote_session_id: Option<String>,
    #[serde(default)]
    scope: Option<SavedWorkflowScope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowNameParams {
    #[serde(flatten)]
    target: WorkflowTargetParams,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowUpdateMetaParams {
    #[serde(flatten)]
    target: WorkflowTargetParams,
    name: String,
    meta: SavedWorkflowMeta,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRunsParams {
    #[serde(flatten)]
    target: WorkflowTargetParams,
    #[serde(default)]
    name: Option<String>,
    limit: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveWorkflowParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    name: String,
}

/// 载体选择（TS `resolveSavedWorkflowCarrier`）：带 workspace 直通；只给 global 时
/// 先选本机活跃 runtime，否则落到管理面 workspace（`<dataRoot>/.mode/plugin-workspace`，
/// 与 TS `ensurePluginManagementWorkspacePath` 同路径）。
fn self_carrier_target(
    control: &AgentControlClient,
    management_workspace: &std::path::Path,
    params: &WorkflowTargetParams,
) -> AgentWorkspaceTarget {
    if let Some(path) = params
        .workspace_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut target = AgentWorkspaceTarget::new(path);
        if let Some(identity) = &params.workspace_identity {
            target = target.with_identity(identity.clone());
        }
        if let Some(remote_session) = &params.remote_session_id {
            target = target.with_remote_session(remote_session.clone());
        }
        return target;
    }
    for active in control.active_workspace_targets() {
        if active.remote_session_id.is_none() && active.workspace_identity.is_none() {
            return active;
        }
    }
    let _ = std::fs::create_dir_all(management_workspace);
    AgentWorkspaceTarget::new(management_workspace.to_string_lossy().into_owned())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppPreferencesParams {
    ask_user_question_auto_resolution_enabled: bool,
    #[serde(default)]
    model_io_full_retention_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseSessionParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    expected_persistence: Option<SessionPersistence>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantHookTrustParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    bundle_digest: String,
    hook_declaration_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveSuggestedParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    stable_id: String,
    operation_id: String,
    client_mode: PluginDeliveryKind,
    delivery_kind: PluginDeliveryKind,
}

// ---- v4 conversation 帧路由（S2：subscribe/unsubscribe + 事件） ----

pub const CONVERSATION_FRAME_LISTEN: &str = "onDynamicConversationFrame";

/// 订阅参数（TS ModeAgentConversationSubscribeParams）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeV4Params {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    base: Option<SubscribeBaseParams>,
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeBaseParams {
    log_epoch: String,
    seq: serde_json::Number,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnsubscribeV4Params {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    subscription_id: String,
}

/// 活跃订阅的宿主侧记录（归属键 = workspaceKey+topic+connectionId）。
struct V4Route {
    workspace_key: String,
    topic: String,
    subscription_id: String,
    connection_id: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

const V4_LOCAL_CONNECTION_ID: &str = "rust-inprocess";

fn visibility_from(
    value: Option<&str>,
) -> Result<Option<agent_protocol::v4_transport::SubscribeVisibility>, ChannelError> {
    match value {
        None => Ok(None),
        Some("foreground") => Ok(Some(
            agent_protocol::v4_transport::SubscribeVisibility::Foreground,
        )),
        Some("background") => Ok(Some(
            agent_protocol::v4_transport::SubscribeVisibility::Background,
        )),
        Some(other) => Err(ChannelError::error(format!("invalid visibility: {other}"))),
    }
}
