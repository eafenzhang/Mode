//! 会话运行时的对外状态面（LT-1 C1 Task 6）：输入、回执、配置、错误与最小快照。
//!
//! 这里只放**数据**与纯函数；锁与 worker 的所有权归 [`crate::runtime`]。
//! 快照形状对齐 `crates/agent-process/src/bin/agent-process-fixture.rs` 的
//! `minimal_snapshot()`（协议 `modeSessionStateSnapshotSchema` 的最小合法投影）：
//! `session.messages` 的冷读归 Task 7，运行时快照只反映**活跃状态**
//! （mode / model / status / turnCount / usage），`messages` 恒为空。

use std::fmt;
use std::path::PathBuf;

use agent_core::context::{ContextCacheUsage, ContextUsage};
use agent_core::{Admission, ModelCapabilities};
use agent_protocol::message::MessageModel;
use agent_protocol::session_read::{
    SessionInfo, SessionKind, SessionMode, SessionModel, SessionStatus, SessionWorkspaceRef,
};
use agent_protocol::snapshot::{
    ContextCacheUsage as ProtocolContextCacheUsage, ContextCost as ProtocolContextCost,
    ProtocolInfo, ProtocolName, SessionContextUsage, SessionLastError, SessionModeSettings,
    SessionModelSettings, SessionProjection, SessionRuntimeState, SessionSettings,
    SessionStateSnapshot, ThoughtLevelOption, ThoughtLevelSettings,
};
use serde_json::Number;
use session_store::SessionRecord;

/// 运行时内部的状态档位（快照时投影成协议的 [`SessionStatus`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeStatus {
    /// 没有运行中的 turn。
    Idle,
    /// 有 turn 正在跑（或队列里还有已接受的输入）。
    Running,
    /// 最近一次 turn 以 provider 错误结束；下一条输入仍可提交。
    Error,
}

impl RuntimeStatus {
    fn to_protocol(self) -> SessionStatus {
        match self {
            Self::Idle => SessionStatus::Idle,
            Self::Running => SessionStatus::Running,
            Self::Error => SessionStatus::Error,
        }
    }
}

/// 一次 `session/send` 的运行时输入（Task 7 从协议参数投影；这里只收运行时关心的事实）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInput {
    /// 客户端幂等 id。`None` / 空白时由运行时生成；同 id 重复提交返回首次的回执，
    /// 不再启动第二个 turn。
    pub input_id: Option<String>,
    /// 本轮的用户输入原文。
    pub content: String,
    /// 乐观并发控制：与当前 `stateRevision` 不一致时拒绝（显式错误，不启动 turn）。
    pub expected_revision: Option<u64>,
}

impl SessionInput {
    /// 纯文本输入的便捷构造（幂等 id 由运行时生成）。
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            input_id: None,
            content: content.into(),
            expected_revision: None,
        }
    }
}

/// `submit` 的回执：输入已被接受（立即返回，不等 turn 结束）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// 生效的输入 id（客户端给了就用客户端的，否则运行时生成）。
    pub input_id: String,
    /// 接受时的 `stateRevision`（已含本次接受的自增）。
    pub state_revision: u64,
    /// 准入结果：立即成为 running，或进入队尾（`position` 从 1 起）。
    pub admission: Admission,
}

/// 运行时配置：mode / 模型事实 / 容量。由 Task 7 的 `session/create` 组装；
/// setter（`setModel` / `setMode` / `setThoughtLevel`）经 [`SessionRuntime::set_config`]
/// 原位更新（`Shared` 里的 `RwLock`），worker 在下个 turn 开始前读取。
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// 当前模式（`build` / `plan` / …；进系统提示与快照）。
    pub mode: String,
    /// 已解析的模型事实（进快照与持久化行；不带凭据）。
    /// 两者都为空 = 会话还没有可用模型（`session/send` 的 turn 以
    /// `CONFIGURATION_ERROR` 失败，不用默认模型兜底——控制器裁决 7）。
    pub provider_id: String,
    pub model_id: String,
    /// 模型能力（进系统提示；缺失保持 `None`，快照里缺省为 0 而不是编数字）。
    pub capabilities: ModelCapabilities,
    /// 思考档位（`session/setThoughtLevel` 的事实；进快照 `settings.thoughtLevel.current`）。
    pub thought_level: Option<String>,
    /// `.mode` 数据根（工具 artifact 落点）；`None` = 未接线，artifact 落临时目录。
    pub data_root: Option<PathBuf>,
    /// 内存事件日志上限（超过丢最旧）；订阅回放只服务窗口内的事件。
    pub event_log_capacity: usize,
    /// broadcast 总线容量；消费方 lag 丢失时用 `subscribe(after_seq)` 重放补齐。
    pub broadcast_capacity: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mode: "build".to_string(),
            provider_id: String::new(),
            model_id: String::new(),
            capabilities: ModelCapabilities::default(),
            thought_level: None,
            data_root: None,
            event_log_capacity: 4096,
            broadcast_capacity: 256,
        }
    }
}

/// 运行时错误面。`code()` 给 Task 7 映射 JSON-RPC 用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// `expectedRevision` 与当前 `stateRevision` 不一致；**不启动 turn**。
    RevisionConflict { expected: u64, current: u64 },
    /// 输入不合法。
    InvalidInput(String),
    /// 会话库失败。
    Store(String),
    /// per-session worker 已退出（队列发送失败）；运行时不可再用。
    RuntimeClosed,
}

impl RuntimeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::InvalidInput(_) => "invalid_input",
            Self::Store(_) => "storage_io",
            Self::RuntimeClosed => "runtime_closed",
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RevisionConflict { expected, current } => write!(
                formatter,
                "状态版本冲突：expected {expected}，当前 {current}（turn 未启动）"
            ),
            Self::InvalidInput(message) => write!(formatter, "无效的会话输入：{message}"),
            Self::Store(message) => write!(formatter, "会话库失败：{message}"),
            Self::RuntimeClosed => write!(formatter, "会话运行时已关闭"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<session_store::StoreError> for RuntimeError {
    fn from(error: session_store::StoreError) -> Self {
        Self::Store(error.to_string())
    }
}

/// 运行时活跃事实（快照入参；字段由 [`crate::runtime`] 的锁内状态填充）。
#[derive(Debug, Clone)]
pub(crate) struct LiveView {
    pub status: RuntimeStatus,
    pub revision: u64,
    pub updated_at: i64,
    pub last_error: Option<String>,
    pub turn_count: u64,
    pub total_token_count: u64,
    pub event_seq: u64,
    /// `runtime.contextUsage`；没有任何模型请求时为 `None`（create 快照不带该键）。
    pub context_usage: Option<ContextUsage>,
}

/// `mode` 字符串 → 协议枚举。未登记的取值按 `build` 落（与 DB `permission.mode`
/// 的观察值一致），不为此新造协议取值。
fn parse_mode(mode: &str) -> SessionMode {
    match mode {
        "plan" => SessionMode::Plan,
        "edit" => SessionMode::Edit,
        "yolo" => SessionMode::Yolo,
        "auto" => SessionMode::Auto,
        _ => SessionMode::Build,
    }
}

fn epoch_ms_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// workspace identity key：`workspaceIdentity?.trim() || workspacePath`（AGENTS.md 口径）。
///
/// 热快照、`session/list` 与冷读必须用同一个 key；`directory` 只用于文件操作/展示。
pub(crate) fn workspace_key(record: &SessionRecord) -> String {
    record
        .workspace_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| record.directory.clone())
}

/// 组装最小合法快照（`agent-process-fixture::minimal_snapshot` 的形状）。
///
/// 热快照保留 `workspaceIdentity` 并以 `identity?.trim() || path` 作为 `workspaceKey`
/// （最终评审 I5：此前热快照把 identity 丢掉、key 退回 directory）。
pub(crate) fn build_snapshot(
    record: &SessionRecord,
    config: &RuntimeConfig,
    live: &LiveView,
) -> SessionStateSnapshot {
    let mode = parse_mode(&config.mode);
    let status = live.status.to_protocol();
    // 没有模型事实（`session/create` 未带 model）时省略 model 键，不造空 provider。
    let has_model = !config.provider_id.is_empty() && !config.model_id.is_empty();
    let model = has_model.then(|| SessionModel {
        provider_id: config.provider_id.clone(),
        model_id: config.model_id.clone(),
        options: None,
    });
    let message_model = has_model.then(|| MessageModel {
        provider_id: config.provider_id.clone(),
        model_id: config.model_id.clone(),
        options: None,
    });
    SessionStateSnapshot {
        protocol: ProtocolInfo {
            name: ProtocolName::ModeProtocol,
            version: 1,
        },
        session: SessionInfo {
            session_id: record.id.clone(),
            workspace: SessionWorkspaceRef {
                workspace_path: record.directory.clone(),
                workspace_identity: record.workspace_id.clone(),
                remote_session_id: None,
                workspace_key: workspace_key(record),
            },
            parent_session_id: record.parent_id.clone(),
            trace_id: record.trace_id.clone(),
            session_kind: SessionKind::Interactive,
            title: record.title.clone(),
            title_source: None,
            mode,
            status,
            model,
            target: None,
            created_at: epoch_ms_to_u64(record.time_created),
            updated_at: epoch_ms_to_u64(live.updated_at.max(record.time_updated)),
            archived_at: record.time_archived.map(epoch_ms_to_u64),
        },
        settings: SessionSettings {
            model: SessionModelSettings {
                current: message_model,
                available: Vec::new(),
                last_used: None,
            },
            thought_level: ThoughtLevelSettings {
                enabled: false,
                current: config.thought_level.clone(),
                default_level: None,
                available: Vec::<ThoughtLevelOption>::new(),
            },
            mode: SessionModeSettings { current: mode },
            permission: None,
        },
        projection: SessionProjection {
            session_id: record.id.clone(),
            status,
            mode,
            turn_count: live.turn_count,
            total_token_count: live.total_token_count,
            context_used: live.total_token_count,
            // 上限未由 provider 配置给出时按 0（「未上报」），不编默认窗口。
            context_window: config.capabilities.context_window.unwrap_or(0),
            current_turn_id: None,
            pending_permissions: Vec::new(),
            active_tool_calls: Vec::new(),
            background_jobs: Vec::new(),
            target: None,
            // 活跃事实：最近一次失败（provider 错误 / 落库失败）原样带出。
            last_error: live.last_error.clone().map(|message| SessionLastError {
                error_type: "runtime_error".to_owned(),
                code: None,
                message,
                detail: None,
                attribution: None,
            }),
        },
        runtime: SessionRuntimeState {
            event_seq: live.event_seq,
            state_revision: live.revision,
            delivery_kind: None,
            active_turn_id: None,
            active_turn_kind: None,
            pending_request_ids: Vec::new(),
            api_retry: None,
            context_usage: live.context_usage.as_ref().map(project_context_usage),
            goal_verifications: None,
            goal_verification_timeline: None,
        },
        // 活跃快照不带历史消息；冷读（store → 消息行）归 Task 7 的 session/read。
        messages: Vec::new(),
        goal_stats: None,
        todos: None,
        todo_groups: None,
        slash_commands: None,
    }
}

/// `contextUsage` 的协议投影（T0 观察键：`used` / `size` / `cache` / `cost`）。
///
/// T1 不产出 `breakdown`（T0 只在完整 read 视图出现；chars 分解由 T6 补齐），也没有
/// `percent`（T0 明确不存在）。`cost` 没有 provider 事实时投影为 `null`。
fn project_context_usage(usage: &ContextUsage) -> SessionContextUsage {
    SessionContextUsage {
        used: usage.used,
        size: usage.size,
        cost: Some(usage.cost.as_ref().map(|cost| ProtocolContextCost {
            amount: cost.amount.clone(),
            currency: cost.currency.clone(),
        })),
        cache: usage.cache.as_ref().map(project_cache_usage),
        breakdown: None,
    }
}

fn project_cache_usage(cache: &ContextCacheUsage) -> ProtocolContextCacheUsage {
    ProtocolContextCacheUsage {
        input_tokens: cache.input_tokens,
        cache_read_tokens: cache.cache_read_tokens,
        cache_write_tokens: cache.cache_write_tokens,
        latest_hit_rate: Some(Some(hit_rate_number(
            cache.cache_read_tokens,
            cache.input_tokens + cache.cache_read_tokens,
        ))),
        hit_rate_request_count: Some(cache.hit_rate_request_count),
        total_input_tokens: Some(cache.total_input_tokens),
        total_cache_read_tokens: Some(cache.total_cache_read_tokens),
        total_cache_write_tokens: Some(cache.total_cache_write_tokens),
        hit_rate: Some(hit_rate_number(
            cache.total_cache_read_tokens,
            cache.total_input_tokens + cache.total_cache_read_tokens,
        )),
    }
}

/// 命中率：没有命中/没有请求时是整数 `0`（与 T0 观察的 JSON 形状一致）。
fn hit_rate_number(hits: u64, total: u64) -> Number {
    if hits == 0 || total == 0 {
        return Number::from(0_u64);
    }
    Number::from_f64(hits as f64 / total as f64).unwrap_or_else(|| Number::from(0_u64))
}

/// 进程级唯一 id：`<prefix>_<毫秒>_<计数>`。只要求进程内单调唯一（持久化行的
/// 主键），不模拟线上的 base36+uuid 形状——那是 Task 7 之后再对齐的事。
pub(crate) fn fresh_id(prefix: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    format!("{prefix}_{millis}_{count}")
}
