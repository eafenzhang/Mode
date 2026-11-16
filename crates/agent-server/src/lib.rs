//! `agent-server`：Rust 版 `app-server --stdio` 进程（LT-1 C1）。
//!
//! 本任务只交付 stdio 骨架：LF 分帧（CRLF 容忍、U+2028/U+2029 不切帧、空行忽略、
//! EOF 残留帧处理）、四类线上帧、方法分派空壳与进程生命周期。会话存储、provider
//! 与 session 方法由后续任务接入；未支持的方法显式返回 `-32601`。
//!
//! 行为依据：`docs/specs/agent-core-parity.md` §2/§5.4、
//! `docs/specs/mode-protocol-client.md`「帧」。

mod dispatch;
mod lifecycle;
mod server;

pub use dispatch::AgentServer;
pub use lifecycle::{parse_startup_args, run_main, StartupArgs, APP_SERVER_MODE, STDIO_FLAG};
// 线上帧类型的唯一所有者是 `agent-protocol`；这里只做转出，避免第二套帧定义。
pub use agent_protocol::{
    ErrorObject, ErrorResponseFrame, Frame, NotificationFrame, RequestFrame, RequestId,
    ResponseFrame, ReverseRequestIds,
};
pub use server::{frame_from_value, run_stdio, ServerError, MAX_FRAME_BYTES};
