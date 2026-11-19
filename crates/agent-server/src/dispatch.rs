//! 方法分派与有状态服务面（LT-1 C1 Task 7）。
//!
//! [`AgentServer`] 持有：
//! - [`ProviderConfigHub`]：provider/updateAccountConfig 与模型端点解析（Task 4b）；
//! - `ServerConfig`（`.mode` 数据根 → `{root}/cli/db/db.sqlite`）与启动存储门
//!   （`startup/storageState` 通知，形状见 `docs/specs/agent-process.md` §存储启动门
//!   与 `docs/specs/agent-c2-contract.md` §5）；
//! - 会话注册表 `HashMap<sessionId, Arc<SessionRuntime>>` 与共享 [`SessionStore`]
//!   （单写者：crate 文档的锁纪律）。
//!
//! session 方法归 `crate::methods`；未知/未实现方法显式 `-32601`，不返回空结果。
//! stdio 循环与帧契约归 [`crate::server`]，本模块不复制。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use agent_protocol::message::MessageModel;
use agent_protocol::provider_runtime_headers::ProviderRuntimeHeadersRequestParams;
use agent_protocol::session::WorkspaceRef;
use agent_protocol::NotificationFrame;

use crate::mcp::McpHub;
use crate::provider_config::{
    ApiFormat, ProviderConfigError, ProviderConfigHub, ResolvedProvider,
    PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD,
};
use crate::reverse::ReverseHub;
use crate::runtime::{ProviderRequestContext, ProviderSource, SessionRuntime};
use crate::server::ServerError;
use agent_protocol::v4::CommandAck;
use provider_http::{
    AnthropicMessagesProvider, OpenAiChatConfig, OpenAiChatProvider, OpenAiResponsesProvider,
};

use agent_core::ModelCapabilities;
use tokio::io::AsyncWrite;

/// `.mode` 数据根与会话库路径（Task 7 控制器裁决 1）。
///
/// 生产口径：`MODE_DATA_BASE_DIR`（基目录，空串视同未设置）→ 否则家目录；
/// 数据根 = `{base}/.mode`，会话库 = `{root}/cli/db/db.sqlite`（CLI 布局哨兵，
/// 见 `docs/specs/agent-server-c1-contract.md` §3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// `.mode` 数据根。
    pub data_root: std::path::PathBuf,
}

impl ServerConfig {
    /// 从显式基目录构造（测试/嵌入）。
    pub fn from_data_base_dir(base: impl Into<std::path::PathBuf>) -> Self {
        Self {
            data_root: base.into().join(".mode"),
        }
    }

    /// 生产构造：`MODE_DATA_BASE_DIR` 优先，否则家目录。两者都不可得返回 `None`
    /// （没有存储事实源，session 方法显式报错而不是写错位置）。
    pub fn from_env() -> Option<Self> {
        let base = std::env::var_os("MODE_DATA_BASE_DIR")
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
            .or_else(|| std::env::var_os("HOME").filter(|value| !value.is_empty()))?;
        Some(Self::from_data_base_dir(std::path::PathBuf::from(base)))
    }

    /// 会话库路径：`{data_root}/cli/db/db.sqlite`。
    pub fn db_path(&self) -> std::path::PathBuf {
        self.data_root.join("cli").join("db").join("db.sqlite")
    }
}

/// 生产 provider 装配源：hub 三层合并解析 → 按 `api_format` 选适配器
/// （OpenAI Chat Completions / Anthropic Messages / OpenAI Responses，C2 §4.3）。
///
/// 账号型 provider（`zhipu-account` 族）的鉴权材料来自宿主反向请求
/// `interaction/requestProviderRuntimeHeaders`（Task 8 控制器裁决 4）：连接级
/// [`ReverseHub`] 存在且带会话上下文时发起请求；`headersApplied:true` 用
/// `requestAuth.apiKey` 构造配置，`false` / 超时 / 错误应答 / 没有 hub 或上下文
/// 一律落回「缺 `access.apiKey`」的既有显式错误。本结构不做任何网络请求、
/// 不连官方平台；key 只进 Authorization 头路径，不进日志/错误。
pub struct HubProviderSource {
    hub: Arc<ProviderConfigHub>,
    reverse: Arc<RwLock<Option<Arc<ReverseHub>>>>,
}

impl HubProviderSource {
    pub fn new(hub: Arc<ProviderConfigHub>, reverse: Arc<RwLock<Option<Arc<ReverseHub>>>>) -> Self {
        Self { hub, reverse }
    }

    /// 解析成 OpenAI Chat Completions 配置（不含 provider 实例化；测试断言 key 用）。
    ///
    /// Anthropic / Responses 等其它格式在这里显式报不支持
    /// （`to_openai_chat_config` 的格式校验）。
    /// 阻塞等待宿主应答——调用方（worker）把 resolve 放在 `spawn_blocking` 上。
    pub fn resolve_openai_config(
        &self,
        provider_id: &str,
        model_id: &str,
        context: Option<&ProviderRequestContext>,
    ) -> Result<OpenAiChatConfig, String> {
        self.resolve_provider_facts(provider_id, model_id, context)?
            .to_openai_chat_config()
            .map_err(|error| error.to_string())
    }

    /// 解析 provider 事实（端点 + 凭据 + `api_format`），不实例化适配器。
    ///
    /// 账号型 provider：凭据在宿主手里（OAuth→API Key 兑换）。钩子不可用
    /// （无连接级 hub / 无会话上下文）或宿主拒绝时，落回既有显式缺 key 错误。
    fn resolve_provider_facts(
        &self,
        provider_id: &str,
        model_id: &str,
        context: Option<&ProviderRequestContext>,
    ) -> Result<ResolvedProvider, String> {
        let selection = model_selection::ModelSelection {
            provider_id: provider_id.to_owned(),
            model_id: model_id.to_owned(),
            options: None,
        };
        match self.hub.resolve(&selection) {
            Ok(resolved) => Ok(resolved),
            Err(ProviderConfigError::MissingEndpointField {
                field: "access.apiKey",
                ..
            }) => self.resolve_account_via_host(&selection, context),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 账号型 provider 的宿主钩子路径（`interaction/requestProviderRuntimeHeaders`）。
    fn resolve_account_via_host(
        &self,
        selection: &model_selection::ModelSelection,
        context: Option<&ProviderRequestContext>,
    ) -> Result<ResolvedProvider, String> {
        let missing_key = || {
            crate::provider_config::ProviderConfigError::MissingEndpointField {
                provider_id: selection.provider_id.clone(),
                model_id: selection.model_id.clone(),
                field: "access.apiKey",
            }
            .to_string()
        };
        let reverse = self
            .reverse
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (Some(reverse), Some(context)) = (reverse, context) else {
            return Err(missing_key());
        };
        let target = self
            .hub
            .resolve_account_target(selection)
            .map_err(|error| error.to_string())?;
        let params = ProviderRuntimeHeadersRequestParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: context.session_id.clone(),
            turn_id: None,
            workspace: WorkspaceRef {
                workspace_path: context.workspace_path.clone(),
                workspace_key: context
                    .workspace_identity
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(&context.workspace_path)
                    .to_owned(),
                workspace_identity: context.workspace_identity.clone(),
                remote_session_id: None,
                extra: agent_protocol::ExtraFields::new(),
            },
            model_selection: MessageModel {
                provider_id: selection.provider_id.clone(),
                model_id: selection.model_id.clone(),
                options: None,
            },
            provider_id: selection.provider_id.clone(),
            account_access: Some(target.account_access),
            reason:
                agent_protocol::provider_runtime_headers::ProviderRuntimeHeadersReason::ModelRequest,
        };
        let response = match reverse.request_provider_runtime_headers(params) {
            Ok(response) => response,
            // 宿主拒绝 / 超时 / 连接关闭：不重试，落回显式缺 key（fail-closed）。
            // 宿主的错误文案不并入我们的错误（避免把宿主侧内容当事实转发）。
            Err(_reason) => return Err(missing_key()),
        };
        let api_key = response
            .request_auth
            .as_ref()
            .and_then(|auth| auth.api_key.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let Some(api_key) = api_key else {
            return Err(missing_key());
        };
        self.hub
            .resolve_account_provider(&target, api_key)
            .map_err(|error| error.to_string())
    }
}

impl ProviderSource for HubProviderSource {
    fn resolve(
        &self,
        provider_id: &str,
        model_id: &str,
        context: Option<&ProviderRequestContext>,
    ) -> Result<Arc<dyn agent_core::ModelProvider>, String> {
        let resolved = self.resolve_provider_facts(provider_id, model_id, context)?;
        // C2 §4.3：api_format 决定适配器；未支持格式显式报错，不静默回落。
        match resolved.api_format {
            ApiFormat::OpenAiChatCompletions => {
                let config = resolved
                    .to_openai_chat_config()
                    .map_err(|error| error.to_string())?;
                let provider = OpenAiChatProvider::new(config)
                    .map_err(|error| format!("构建模型适配器失败: {error}"))?;
                Ok(Arc::new(provider))
            }
            ApiFormat::AnthropicMessages => {
                let config = resolved
                    .to_anthropic_messages_config()
                    .map_err(|error| error.to_string())?;
                let provider = AnthropicMessagesProvider::new(config)
                    .map_err(|error| format!("构建模型适配器失败: {error}"))?;
                Ok(Arc::new(provider))
            }
            ApiFormat::OpenAiResponses => {
                let config = resolved
                    .to_openai_responses_config()
                    .map_err(|error| error.to_string())?;
                let provider = OpenAiResponsesProvider::new(config)
                    .map_err(|error| format!("构建模型适配器失败: {error}"))?;
                Ok(Arc::new(provider))
            }
        }
    }

    /// 账号 revision + builtin/personal 文件指纹；personal 配置轮换后下一个 turn
    /// 会重新解析 provider（最终评审 I6）。
    fn cache_key(&self) -> String {
        self.hub.resolve_generation()
    }
}

/// 启动存储门的状态（一次性；`run_storage_gate` 推进）。
///
/// `Ready` 同时保留数据根：工具 artifact 的落点来自这里（最终评审 C5——门跑完后
/// 配置被 `take()` 走会让会话退回临时目录）。
enum StorageState {
    /// 门还没跑（或骨架构造没有存储配置）。
    Pending,
    Ready {
        store: Arc<session_store::SessionStore>,
        data_root: std::path::PathBuf,
    },
    Failed(String),
}

/// 订阅的 live 转发任务（response 写出后由 stdio 循环 spawn）。
pub(crate) struct PendingLiveForward {
    pub(crate) after_seq: Option<u64>,
    pub(crate) receiver: tokio::sync::broadcast::Receiver<agent_core::SessionEvent>,
}

/// Agent Server 实例：provider 配置 hub + 存储门 + 会话注册表。
pub struct AgentServer {
    provider_config: Arc<ProviderConfigHub>,
    provider_source: RwLock<Arc<dyn ProviderSource>>,
    /// 连接级反向请求 hub（Task 8）：`run_stdio` 启动时注入，退出时清除。
    /// 装配源（账号型凭据钩子）与 runtime 接线经此共享。
    reverse: Arc<RwLock<Option<Arc<ReverseHub>>>>,
    /// 门待用的存储配置；gate 跑完取走（一次性）。
    storage_config: Mutex<Option<ServerConfig>>,
    storage: Mutex<StorageState>,
    sessions: Mutex<HashMap<String, Arc<SessionRuntime>>>,
    /// close 后仍在 drain 已接受输入的会话台账（Task 7 复评裁决 2）：
    /// key = sessionId，value = worker 退出（drain 完成）信号。
    /// Arc 包装让 drain 清理任务无需持有整个 server。
    closing: Arc<Mutex<HashMap<String, tokio::sync::watch::Receiver<bool>>>>,
    pending_forwards: Mutex<Vec<PendingLiveForward>>,
    /// C4 T1：MCP 连接的唯一所有者（`mcp/list` 与会话工具目录共用同一张表）。
    mcp: Arc<McpHub>,
    /// C4 T4：已受理 v4 命令的 ACK 台账（`v4/commands/query` 的唯一事实源）。
    v4: V4State,
}

/// `(sessionId, commandId)` → ACK。sessionId `None` 是全局幂等桶（createSession）。
#[derive(Default)]
pub struct V4State {
    commands: Mutex<HashMap<(Option<String>, String), CommandAck>>,
}

impl V4State {
    pub fn record_command(&self, session_id: Option<String>, command_id: String, ack: CommandAck) {
        self.commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert((session_id, command_id), ack);
    }

    pub fn lookup(&self, session_id: Option<&str>, command_id: &str) -> Option<CommandAck> {
        self.commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(session_id.map(str::to_string), command_id.to_string()))
            .cloned()
    }
}

impl std::fmt::Debug for AgentServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentServer")
            .field("provider_config", &self.provider_config)
            .field(
                "sessions",
                &self.sessions.lock().map(|sessions| sessions.len()),
            )
            .finish_non_exhaustive()
    }
}

impl Default for AgentServer {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentServer {
    /// 骨架构造：provider 配置可用，**不配置存储**（session 方法显式报错）。
    /// 现有分派/分帧测试继续可用；生产入口用 [`Self::from_env`]。
    ///
    /// provider 装配源（Task 10）：`MODE_AGENT_SERVER_SCRIPTED_PROVIDER` 指向 oracle
    /// 行为夹具时改用脚本化装配（**仅测试可达**，无默认路径）；否则生产恒为
    /// [`HubProviderSource`]（三层合并 → 按 `api_format` 选适配器）。
    pub fn new() -> Self {
        let hub = Arc::new(ProviderConfigHub::from_env());
        let reverse = Arc::new(RwLock::new(None));
        let source: Arc<dyn ProviderSource> =
            match crate::scripted::ScriptedFixtureSource::from_env() {
                Some(scripted) => Arc::new(scripted),
                None => Arc::new(HubProviderSource::new(
                    Arc::clone(&hub),
                    Arc::clone(&reverse),
                )),
            };
        Self {
            provider_source: RwLock::new(source),
            provider_config: hub,
            reverse,
            storage_config: Mutex::new(None),
            storage: Mutex::new(StorageState::Pending),
            sessions: Mutex::new(HashMap::new()),
            closing: Arc::new(Mutex::new(HashMap::new())),
            pending_forwards: Mutex::new(Vec::new()),
            mcp: Arc::new(McpHub::new()),
            v4: V4State::default(),
        }
    }

    /// 生产构造：数据根来自 env，provider 装配走 hub。
    pub fn from_env() -> Self {
        let server = Self::new();
        *server
            .storage_config
            .lock()
            .expect("storage config poisoned") = ServerConfig::from_env();
        server
    }

    /// 测试/嵌入构造：显式数据根、provider hub 与 provider 装配源（脚本化 fake）。
    pub fn with_parts(
        config: ServerConfig,
        hub: ProviderConfigHub,
        source: Arc<dyn ProviderSource>,
    ) -> Self {
        let hub = Arc::new(hub);
        Self {
            provider_source: RwLock::new(source),
            provider_config: hub,
            reverse: Arc::new(RwLock::new(None)),
            storage_config: Mutex::new(Some(config)),
            storage: Mutex::new(StorageState::Pending),
            sessions: Mutex::new(HashMap::new()),
            closing: Arc::new(Mutex::new(HashMap::new())),
            pending_forwards: Mutex::new(Vec::new()),
            mcp: Arc::new(McpHub::new()),
            v4: V4State::default(),
        }
    }

    /// 按方法名分派一次请求。
    ///
    /// 已接入：`provider/updateAccountConfig`、session 方法表（Task 7）。
    /// 未知/未实现方法统一 `-32601 Method not found`。
    pub fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ServerError> {
        use crate::methods;
        match method {
            PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD => self
                .provider_config
                .handle_account_config(params)
                .map_err(ServerError::from),
            crate::methods::SESSION_CREATE_METHOD => methods::create::handle(self, params),
            crate::methods::SESSION_SEND_METHOD => methods::send::handle(self, params),
            crate::methods::SESSION_SUBSCRIBE_METHOD => methods::subscribe::handle(self, params),
            crate::methods::SESSION_READ_METHOD => methods::read::handle(self, params),
            crate::methods::SESSION_MESSAGES_METHOD => methods::messages::handle(self, params),
            crate::methods::SESSION_LIST_METHOD => methods::list::handle(self, params),
            crate::methods::SESSION_CLOSE_METHOD => methods::close::handle(self, params),
            crate::methods::SESSION_RESUME_METHOD => methods::resume::handle(self, params),
            crate::methods::SESSION_SET_MODEL_METHOD => {
                methods::setters::handle_set_model(self, params)
            }
            crate::methods::SESSION_SET_THOUGHT_LEVEL_METHOD => {
                methods::setters::handle_set_thought_level(self, params)
            }
            crate::methods::SESSION_SET_MODE_METHOD => {
                methods::setters::handle_set_mode(self, params)
            }
            crate::methods::SESSION_COMPACT_METHOD => methods::compact::handle(self, params),
            crate::methods::SESSION_GOAL_METHOD => methods::goal::handle(self, params),
            crate::methods::SESSION_STOP_METHOD => methods::goal::handle_stop(self, params),
            crate::methods::SESSION_SUBAGENTS_METHOD => methods::subagents::handle(self, params),
            crate::methods::MCP_LIST_METHOD => methods::mcp::handle(self, params),
            crate::methods::SKILLS_REFERENCE_CATALOG_METHOD => {
                methods::skills::handle(self, params)
            }
            crate::methods::PLUGINS_LIST_METHOD => methods::plugins::handle_list(self, params),
            crate::methods::PLUGINS_DESCRIBE_METHOD => {
                methods::plugins::handle_describe(self, params)
            }
            crate::methods::PLUGINS_OVERVIEW_METHOD => {
                methods::plugins::handle_overview(self, params)
            }
            crate::methods::PLUGINS_SET_ENABLED_METHOD => {
                methods::plugins::handle_set_enabled(self, params)
            }
            crate::methods::PLUGINS_REFERENCE_CATALOG_METHOD => {
                methods::plugins::handle_reference_catalog(self, params)
            }
            crate::methods::PLUGINS_REFERENCE_CATALOG_WITH_CATEGORY_METHOD => {
                methods::plugins::handle_reference_catalog_with_category(self, params)
            }
            crate::methods::PLUGINS_RESOLVE_SUGGESTED_REFERENCE_METHOD => {
                methods::plugins::handle_resolve_suggested_reference(self, params)
            }
            crate::methods::PLUGINS_MARKETPLACE_ADD_METHOD => {
                methods::plugins::handle_marketplace_add(self, params)
            }
            crate::methods::PLUGINS_MARKETPLACE_REMOVE_METHOD => {
                methods::plugins::handle_marketplace_remove(self, params)
            }
            crate::methods::PLUGINS_MARKETPLACE_UPDATE_METHOD => {
                methods::plugins::handle_marketplace_update(self, params)
            }
            crate::methods::PLUGINS_INSTALL_METHOD => {
                methods::plugins::handle_install(self, params)
            }
            crate::methods::PLUGINS_UNINSTALL_METHOD => {
                methods::plugins::handle_uninstall(self, params)
            }
            crate::methods::PLUGINS_UPDATE_METHOD => methods::plugins::handle_update(self, params),
            crate::methods::PLUGINS_CANCEL_OPERATION_METHOD => {
                methods::plugins::handle_cancel_operation(self, params)
            }
            crate::methods::PROCESS_CHILD_PROCESSES_METHOD => {
                methods::process::handle(self, params)
            }
            crate::methods::WORKFLOWS_LIST_METHOD => methods::workflows::handle_list(self, params),
            crate::methods::WORKFLOWS_GET_METHOD => methods::workflows::handle_get(self, params),
            crate::methods::WORKFLOWS_UPDATE_META_METHOD => {
                methods::workflows::handle_update_meta(self, params)
            }
            crate::methods::WORKFLOWS_DELETE_METHOD => {
                methods::workflows::handle_delete(self, params)
            }
            crate::methods::WORKFLOWS_MOVE_METHOD => methods::workflows::handle_move(self, params),
            crate::methods::WORKFLOWS_RUNS_METHOD => methods::workflows::handle_runs(self, params),
            crate::methods::V4_COMMAND_METHOD => methods::v4::handle_command(self, params),
            crate::methods::V4_COMMANDS_QUERY_METHOD => {
                methods::v4::handle_commands_query(self, params)
            }
            _ => Err(ServerError::method_not_found(method)),
        }
    }

    /// provider 配置 hub（模型端点解析入口；测试断言用）。
    pub fn provider_config(&self) -> &ProviderConfigHub {
        &self.provider_config
    }

    /// C4 T1：MCP 连接 hub（`mcp/list`、会话工具目录与 `process/childProcesses` 共用）。
    pub fn mcp_hub(&self) -> &Arc<McpHub> {
        &self.mcp
    }

    /// C4 T4：v4 命令 ACK 台账（`v4/commands/query` 读面）。
    pub fn v4_state(&self) -> &V4State {
        &self.v4
    }

    /// 替换 provider 装配源（测试注入；生产恒为 hub 适配）。
    pub fn set_provider_source(&self, source: Arc<dyn ProviderSource>) {
        *self
            .provider_source
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = source;
    }

    pub(crate) fn provider_source(&self) -> Arc<dyn ProviderSource> {
        self.provider_source
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 注入连接级反向请求 hub（`run_stdio` 启动时；同一 server 复用给新连接前
    /// 先 [`Self::clear_reverse_hub`]）。此后物化的会话接审批/偏好/凭据钩子。
    pub(crate) fn set_reverse_hub(&self, hub: Arc<ReverseHub>) {
        *self
            .reverse
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hub);
    }

    /// 清除连接级 hub（`run_stdio` 退出时；pending 已由 hub 自身 close 失败）。
    pub(crate) fn clear_reverse_hub(&self) {
        *self
            .reverse
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// 当前连接级 hub（会话物化接线用；`None` = 不在 stdio 连接里，会话退回
    /// `AutoAllow` 默认审批且不发偏好请求）。
    pub(crate) fn reverse_hub(&self) -> Option<Arc<ReverseHub>> {
        self.reverse
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// `.mode` 数据根（工具 artifact 落点）；门没跑/失败/未配置时 `None`。
    pub(crate) fn data_root(&self) -> Option<std::path::PathBuf> {
        match &*self.storage.lock().expect("storage poisoned") {
            StorageState::Ready { data_root, .. } => Some(data_root.clone()),
            StorageState::Pending | StorageState::Failed(_) => None,
        }
    }

    /// 已就绪的会话库；门失败/未初始化时显式报错（fail-closed，不静默降级）。
    pub(crate) fn store(&self) -> Result<Arc<session_store::SessionStore>, ServerError> {
        match &*self.storage.lock().expect("storage poisoned") {
            StorageState::Ready { store, .. } => Ok(Arc::clone(store)),
            StorageState::Failed(reason) => {
                Err(ServerError::internal(format!("会话库不可用：{reason}")))
            }
            StorageState::Pending => Err(ServerError::internal("会话库尚未初始化（存储门未运行）")),
        }
    }

    /// 取活跃 runtime；会话不在注册表时报 `-32602`（冷会话走 read/resume 的 store 路径）。
    pub(crate) fn runtime(&self, session_id: &str) -> Result<Arc<SessionRuntime>, ServerError> {
        self.find_runtime(session_id)
            .ok_or_else(|| ServerError::invalid_params(format!("会话不存在或未运行: {session_id}")))
    }

    /// 取活跃 runtime（无则 `None`，不报错）。公开给测试与嵌入方读取事件
    /// （`session/subscribe` 之外的进程内订阅面，如连接关闭后的终态断言）。
    pub fn find_runtime(&self, session_id: &str) -> Option<Arc<SessionRuntime>> {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .get(session_id)
            .cloned()
    }

    pub(crate) fn insert_runtime(&self, runtime: Arc<SessionRuntime>) {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .insert(runtime.record().id.clone(), runtime);
    }

    pub(crate) fn remove_runtime(&self, session_id: &str) -> Option<Arc<SessionRuntime>> {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .remove(session_id)
    }

    /// 登记「close 后 drain 中」的会话（`session/resume` / 同 id `session/create`
    /// 据此显式拒绝，不另起第二个 runtime owner）。
    pub(crate) fn insert_closing(
        &self,
        session_id: &str,
        watcher: tokio::sync::watch::Receiver<bool>,
    ) {
        self.closing
            .lock()
            .expect("closing poisoned")
            .insert(session_id.to_string(), watcher);
    }

    /// 取 drain 中会话的退出信号（`None` = 不在 closing 台账）。
    pub(crate) fn find_closing(
        &self,
        session_id: &str,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.closing
            .lock()
            .expect("closing poisoned")
            .get(session_id)
            .cloned()
    }

    /// 从 closing 台账移除（drain 完成后调用；此后 resume 走冷恢复）。
    pub(crate) fn remove_closing(&self, session_id: &str) {
        self.closing
            .lock()
            .expect("closing poisoned")
            .remove(session_id);
    }

    /// closing 台账句柄（drain 清理任务用；任务只依赖台账，不持有 server）。
    pub(crate) fn closing_map(
        &self,
    ) -> Arc<Mutex<HashMap<String, tokio::sync::watch::Receiver<bool>>>> {
        Arc::clone(&self.closing)
    }

    pub(crate) fn enqueue_live_forward(&self, forward: PendingLiveForward) {
        self.pending_forwards
            .lock()
            .expect("pending forwards poisoned")
            .push(forward);
    }

    pub(crate) fn take_pending_forwards(&self) -> Vec<PendingLiveForward> {
        std::mem::take(
            &mut self
                .pending_forwards
                .lock()
                .expect("pending forwards poisoned"),
        )
    }

    /// 按当前 hub 解析模型能力（setter 用；失败显式暴露）。
    pub(crate) fn resolve_capabilities(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<ModelCapabilities, ServerError> {
        let selection = model_selection::ModelSelection {
            provider_id: provider_id.to_owned(),
            model_id: model_id.to_owned(),
            options: None,
        };
        let resolved = self
            .provider_config
            .resolve(&selection)
            .map_err(ServerError::from)?;
        Ok(ModelCapabilities {
            context_window: resolved.context_window,
            max_output_tokens: resolved.max_output_tokens,
        })
    }

    /// 启动存储门（`agent-process.md` §存储启动门；C2 帧契约见
    /// `agent-c2-contract.md` §5.1/§5.5）。
    ///
    /// 一次性：`checking ×2`（无 migration 事实）→ [`session_store::SessionStore`]
    /// 打开（缺库/空 schema 走 fresh-install 迁移）→ 第 3 个 `checking`（带预检
    /// 事实）→ `migrating ×N`（每缺失步一帧，执行前发出）→ `committing` →
    /// `ready`。全迁移完成时（第二次启动）没有 `migrating` 帧，共 5 帧；fresh 共
    /// 27 帧。打开是同步 SQLite 操作，迁移事件先收集、按序写出（帧序不变）。
    /// 骨架构造（无存储配置）不发通知、不碰盘；`failed` 带稳定 `errorCode`。
    pub(crate) async fn run_storage_gate<W>(&self, writer: &mut W) -> Result<(), ServerError>
    where
        W: AsyncWrite + Unpin,
    {
        let Some(config) = self
            .storage_config
            .lock()
            .expect("storage config poisoned")
            .take()
        else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        self.notify_storage(writer, "checking", started, serde_json::json!({}))
            .await?;
        self.notify_storage(writer, "checking", started, serde_json::json!({}))
            .await?;

        let mut events = Vec::new();
        let open_result =
            session_store::SessionStore::open_with_progress(&config.db_path(), &mut |event| {
                events.push(event)
            });

        match open_result {
            Ok(store) => {
                let mut committing_facts = None;
                for event in &events {
                    match event {
                        session_store::MigrationEvent::Inspected { facts } => {
                            // 第 3 个 checking 帧：预检事实（锁内基线尚未取得）。
                            self.notify_storage(
                                writer,
                                "checking",
                                started,
                                serde_json::json!({
                                    "migration": migration_facts_json(facts, false),
                                }),
                            )
                            .await?;
                        }
                        session_store::MigrationEvent::Migrating {
                            migration_id,
                            facts,
                        } => {
                            self.notify_storage(
                                writer,
                                "migrating",
                                started,
                                serde_json::json!({
                                    "migration": migration_facts_json(facts, true),
                                    "completed": facts.executed_count,
                                    "total": session_store::migration_definitions().len(),
                                    "migrationId": migration_id,
                                }),
                            )
                            .await?;
                        }
                        session_store::MigrationEvent::Committing { facts } => {
                            self.notify_storage(
                                writer,
                                "committing",
                                started,
                                serde_json::json!({
                                    "migration": migration_facts_json(facts, true),
                                }),
                            )
                            .await?;
                            committing_facts = Some(facts.clone());
                        }
                    }
                }
                // ready 帧：`committedCount` 追平 `executedCount`（C1 观察的 ready 形状；
                // 全部已应用时 executed/committed 恒 0）。
                let mut ready_facts =
                    committing_facts.unwrap_or(session_store::MigrationFacts::default());
                ready_facts.committed_count = ready_facts.executed_count;
                self.notify_storage(
                    writer,
                    "ready",
                    started,
                    serde_json::json!({
                        "migration": migration_facts_json(&ready_facts, true),
                    }),
                )
                .await?;
                // 数据根随 store 一起保留：会话物化时用它接线工具 artifact 落点。
                *self.storage.lock().expect("storage poisoned") = StorageState::Ready {
                    store: Arc::new(store),
                    data_root: config.data_root,
                };
                Ok(())
            }
            Err(error) => {
                // 稳定错误码来自 `session_store::StoreError::code()`（`agent-core-parity.md`
                // §6 的事实源），这里不另造取值。
                let error_code = error.code();
                self.notify_storage(
                    writer,
                    "failed",
                    started,
                    serde_json::json!({ "errorCode": error_code }),
                )
                .await?;
                *self.storage.lock().expect("storage poisoned") =
                    StorageState::Failed(error.to_string());
                Ok(())
            }
        }
    }

    async fn notify_storage<W>(
        &self,
        writer: &mut W,
        phase: &str,
        started: std::time::Instant,
        extra: serde_json::Value,
    ) -> Result<(), ServerError>
    where
        W: AsyncWrite + Unpin,
    {
        static GATE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = GATE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let mut params = serde_json::json!({
            "schemaVersion": 1,
            "attemptId": "attempt-1",
            "sequence": sequence,
            "databaseId": "db-1",
            "databaseKind": "session",
            "phase": phase,
            "elapsedMs": started.elapsed().as_secs_f64() * 1000.0,
        });
        if let Some(object) = extra.as_object() {
            for (key, value) in object {
                params[key.as_str()] = value.clone();
            }
        }
        crate::server::write_frame(
            writer,
            &agent_protocol::Frame::Notification(NotificationFrame {
                method: "startup/storageState".to_string(),
                params: Some(params),
                trace: None,
            }),
        )
        .await
    }
}

/// 迁移事实 → 帧 `migration` 字段。
///
/// `include_last_applied=false` 用于预检帧（oracle 第 3 个 checking 帧没有
/// `lastAppliedMigrationId`）；其余帧带该键（空账本为 `null`）。
fn migration_facts_json(
    facts: &session_store::MigrationFacts,
    include_last_applied: bool,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "kind": facts.kind.as_str(),
        "executedCount": facts.executed_count,
        "committedCount": facts.committed_count,
    });
    if include_last_applied {
        value["lastAppliedMigrationId"] = serde_json::json!(facts.last_applied_migration_id);
    }
    value
}
