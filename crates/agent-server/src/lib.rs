//! `agent-server`：Rust 版 `app-server --stdio` 进程（LT-1 C1）。
//!
//! 已交付：LF 分帧（CRLF 容忍、U+2028/U+2029 不切帧、空行忽略、EOF 残留帧处理）、
//! 四类线上帧、方法分派与进程生命周期；Task 4b 接入 provider 配置与凭据解析；
//! Task 6 落地 per-session 会话运行时（[`SessionRuntime`]：CommandInbox 串行准入、
//! 事件总线、session-store 持久化）。session 方法分派归 Task 7；未支持的方法
//! 显式返回 `-32601`。
//!
//! 行为依据：`docs/specs/agent-core-parity.md` §2/§5.4、
//! `docs/specs/mode-protocol-client.md`「帧」、`docs/specs/agent-process.md`
//! 「Account Provider Config 交付」、`docs/specs/agent-server-c1-contract.md`。

mod dispatch;
mod lifecycle;
mod provider_config;
mod runtime;
mod server;
mod state;

pub use dispatch::AgentServer;
pub use lifecycle::{parse_startup_args, run_main, StartupArgs, APP_SERVER_MODE, STDIO_FLAG};
pub use provider_config::{
    ApiFormat, ProviderConfigError, ProviderConfigHub, ResolvedProvider,
    PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD,
};
pub use runtime::SessionRuntime;
pub use state::{Accepted, RuntimeConfig, RuntimeError, SessionInput};
// 线上帧类型的唯一所有者是 `agent-protocol`；这里只做转出，避免第二套帧定义。
pub use agent_protocol::{
    ErrorObject, ErrorResponseFrame, Frame, NotificationFrame, RequestFrame, RequestId,
    ResponseFrame, ReverseRequestIds,
};
pub use server::{frame_from_value, run_stdio, ServerError, MAX_FRAME_BYTES};
