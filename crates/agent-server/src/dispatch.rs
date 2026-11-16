//! 方法分派与有状态服务面（LT-1 C1 Task 7）。
//!
//! [`AgentServer`] 持有：
//! - [`ProviderConfigHub`]：provider/updateAccountConfig 与模型端点解析（Task 4b）；
//! - `ServerConfig`（`.mode` 数据根 → `{root}/cli/db/db.sqlite`）与启动存储门
//!   （`startup/storageState` 通知，形状见 `docs/specs/agent-process.md` §存储启动门）；
//! - 会话注册表 `HashMap<sessionId, Arc<SessionRuntime>>` 与共享 [`SessionStore`]
//!   （单写者：crate 文档的锁纪律）。
//!
//! session 方法归 `crate::methods`；未知/未实现方法（含 `session/compact`、
//! `session/goal`）显式 `-32601`，不返回空结果。stdio 循环与帧契约归
//! [`crate::server`]，本模块不复制。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use agent_protocol::NotificationFrame;

use crate::provider_config::{ProviderConfigHub, PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD};
use crate::runtime::{ProviderSource, SessionRuntime};
use crate::server::ServerError;
use provider_http::OpenAiChatProvider;

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

/// 生产 provider 装配源：hub 三层合并解析 → OpenAI Chat Completions 适配器。
///
/// Anthropic / Responses 属 C2（`ResolvedProvider::to_openai_chat_config` 显式报不支持）。
struct HubProviderSource {
    hub: Arc<ProviderConfigHub>,
}

impl ProviderSource for HubProviderSource {
    fn resolve(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Arc<dyn agent_core::ModelProvider>, String> {
        let selection = model_selection::ModelSelection {
            provider_id: provider_id.to_owned(),
            model_id: model_id.to_owned(),
            options: None,
        };
        let resolved = self
            .hub
            .resolve(&selection)
            .map_err(|error| error.to_string())?;
        let config = resolved
            .to_openai_chat_config()
            .map_err(|error| error.to_string())?;
        let provider = OpenAiChatProvider::new(config)
            .map_err(|error| format!("构建模型适配器失败: {error}"))?;
        Ok(Arc::new(provider))
    }
}

/// 启动存储门的状态（一次性；`run_storage_gate` 推进）。
enum StorageState {
    /// 门还没跑（或骨架构造没有存储配置）。
    Pending,
    Ready(Arc<session_store::SessionStore>),
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
    /// 门待用的存储配置；gate 跑完取走（一次性）。
    storage_config: Mutex<Option<ServerConfig>>,
    storage: Mutex<StorageState>,
    sessions: Mutex<HashMap<String, Arc<SessionRuntime>>>,
    /// close 后仍在 drain 已接受输入的会话台账（Task 7 复评裁决 2）：
    /// key = sessionId，value = worker 退出（drain 完成）信号。
    /// Arc 包装让 drain 清理任务无需持有整个 server。
    closing: Arc<Mutex<HashMap<String, tokio::sync::watch::Receiver<bool>>>>,
    pending_forwards: Mutex<Vec<PendingLiveForward>>,
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
    pub fn new() -> Self {
        let hub = Arc::new(ProviderConfigHub::from_env());
        Self {
            provider_source: RwLock::new(Arc::new(HubProviderSource {
                hub: Arc::clone(&hub),
            })),
            provider_config: hub,
            storage_config: Mutex::new(None),
            storage: Mutex::new(StorageState::Pending),
            sessions: Mutex::new(HashMap::new()),
            closing: Arc::new(Mutex::new(HashMap::new())),
            pending_forwards: Mutex::new(Vec::new()),
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
            storage_config: Mutex::new(Some(config)),
            storage: Mutex::new(StorageState::Pending),
            sessions: Mutex::new(HashMap::new()),
            closing: Arc::new(Mutex::new(HashMap::new())),
            pending_forwards: Mutex::new(Vec::new()),
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
            _ => Err(ServerError::method_not_found(method)),
        }
    }

    /// provider 配置 hub（模型端点解析入口；测试断言用）。
    pub fn provider_config(&self) -> &ProviderConfigHub {
        &self.provider_config
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

    /// `.mode` 数据根（工具 artifact 落点）；门没跑/未配置时 `None`。
    pub(crate) fn data_root(&self) -> Option<std::path::PathBuf> {
        self.storage_config
            .lock()
            .expect("storage config poisoned")
            .as_ref()
            .map(|config| config.data_root.clone())
    }

    /// 已就绪的会话库；门失败/未初始化时显式报错（fail-closed，不静默降级）。
    pub(crate) fn store(&self) -> Result<Arc<session_store::SessionStore>, ServerError> {
        match &*self.storage.lock().expect("storage poisoned") {
            StorageState::Ready(store) => Ok(Arc::clone(store)),
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

    /// 取活跃 runtime（无则 `None`，不报错）。
    pub(crate) fn find_runtime(&self, session_id: &str) -> Option<Arc<SessionRuntime>> {
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

    /// 启动存储门（`agent-process.md` §存储启动门）。
    ///
    /// 一次性：`checking` → 打开 [`session_store::SessionStore`]（fail-closed 的
    /// schema 门，不建库不迁移）→ `ready` / `failed`。通知帧**直接写出**（保证
    /// 它们是本连接的第一批帧，宿主 `prepare_storage` 靠它们放行）。骨架构造
    /// （无存储配置）不发通知、不碰盘。`databaseKind` 恒为 `session`（本进程只有
    /// 会话库一个门）；`failed` 带稳定 `errorCode`。
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
        self.notify_storage(writer, "checking", None, started)
            .await?;

        match session_store::SessionStore::open(&config.db_path()) {
            Ok(store) => {
                self.notify_storage(writer, "ready", None, started).await?;
                *self.storage.lock().expect("storage poisoned") =
                    StorageState::Ready(Arc::new(store));
                Ok(())
            }
            Err(error) => {
                // 稳定错误码来自 `session_store::StoreError::code()`（`agent-core-parity.md`
                // §6 的事实源），这里不另造取值。
                let error_code = error.code();
                self.notify_storage(writer, "failed", Some(error_code.to_owned()), started)
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
        error_code: Option<String>,
        started: std::time::Instant,
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
        if let Some(code) = error_code {
            params["errorCode"] = serde_json::json!(code);
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
