//! `agent-server`：Rust 版 `app-server --stdio` 进程（LT-1 C1）。
//!
//! 已交付：LF 分帧（CRLF 容忍、U+2028/U+2029 不切帧、空行忽略、EOF 残留帧处理）、
//! 四类线上帧、方法分派与进程生命周期；Task 4b 接入 provider 配置与凭据解析；
//! Task 6 落地 per-session 会话运行时（[`SessionRuntime`]：CommandInbox 串行准入、
//! 事件总线、session-store 持久化）；Task 7 接入 session 方法表
//! （`session/create|send|subscribe|read|messages|list|close|resume|setModel|
//! setThoughtLevel|setMode`）、启动存储门（`startup/storageState` 通知）与
//! `session/subscribe` 的 live 转发；Task 8 接入反向请求（[`ReverseHub`]：审批
//! `interaction/requestPermission`、偏好 `session/requestRuntimePreferences`、账号
//! 凭据 `interaction/requestProviderRuntimeHeaders`；pending 应答/超时/关闭均
//! fail-closed）。C2 T2 接入 `session/compact`（per-session 单飞受理 + worker 上的
//! 摘要执行）；C2 T3 接入项目记忆召回（[`memory::MemoryContext`]：宿主偏好
//! `memoryEnabled` 开启时按数据根只读注入 `# Memory` 段与首条 user 消息的 agentsMd
//! 索引块，失败 fail-soft）；C3 T1 接入 `session/goal`（目标六 action、续跑 turn、
//! completion verifier、`session_target` 单一事实源与冷恢复）与 `session/stop`。
//! 未支持的方法显式返回 `-32601`。
//!
//! 行为依据：`docs/specs/agent-core-parity.md` §2/§5.4、
//! `docs/specs/mode-protocol-client.md`「帧」、`docs/specs/agent-process.md`
//! 「存储启动门」「Account Provider Config 交付」「反向请求」、
//! `docs/specs/tauri-agent-runtime.md` §1.3、`docs/specs/agent-server-c1-contract.md`。

mod dispatch;
mod history;
mod lifecycle;
mod memory;
mod methods;
mod provider_config;
mod reverse;
mod runtime;
mod scripted;
mod server;
mod state;

pub use dispatch::{AgentServer, HubProviderSource, ServerConfig};
pub use lifecycle::{parse_startup_args, run_main, StartupArgs, APP_SERVER_MODE, STDIO_FLAG};
pub use memory::{load_memory_section, MemoryContext, MemoryLoad};
pub use provider_config::{
    AccountRuntimeHeadersTarget, ApiFormat, ProviderConfigError, ProviderConfigHub, Resolution,
    ResolvedProvider, PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD,
};
pub use reverse::{
    ProtocolPermissionHandler, ReverseHub, ReverseRequestError, RuntimePreferences, RuntimeWiring,
    REVERSE_REPLY_TIMEOUT, RUNTIME_PREFERENCES_METHOD,
};
pub use runtime::{ProviderRequestContext, ProviderSource, SessionRuntime};
pub use scripted::{ScriptedFixtureSource, SCRIPTED_PROVIDER_ENV};
pub use state::{Accepted, RuntimeConfig, RuntimeError, SessionInput};
// 线上帧类型的唯一所有者是 `agent-protocol`；这里只做转出，避免第二套帧定义。
pub use agent_protocol::{
    ErrorObject, ErrorResponseFrame, Frame, NotificationFrame, RequestFrame, RequestId,
    ResponseFrame, ReverseRequestIds,
};
pub use server::{frame_from_value, run_stdio, ServerError, MAX_FRAME_BYTES};
