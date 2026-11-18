//! `session/close`：注销 runtime（不删任何持久化事实）。
//!
//! - 从注册表移除并释放 runtime 句柄；**运行中的 turn 先取消**（唯一终态由
//!   `run_turn` 发出），**已接受的排队输入照常 drain**——它们已经由 CommandInbox
//!   准入（AGENTS.md：已接受的 busy/running 输入由 runtime 串行 admission），
//!   close 不回滚准入，持久化语义按完成边界照常生效。
//! - **drain 窗口**（Task 7 复评裁决 2）：drain 期间 runtime 仍是该会话唯一的
//!   写路径，但已不在注册表里。为防 resume / 同 id create 在窗口内另起第二个
//!   runtime owner（两条写路径、重复 `message.sequence`），close 把会话记入
//!   `closing` 台账（值 = worker 退出信号）；`session/resume` 与同 id
//!   `session/create` 在窗口内显式报 `-32603 会话正在关闭，请稍后重试`（不隐式
//!   重挂 drain 中的 runtime），drain 完成后台账条目移除，此后 resume 走冷恢复。
//! - `expectedPersistence` 是持久化意图标记（immediate/deferred）；C1 的写入路径
//!   只有完成边界一种，两种取值都按 immediate 处理，事实行不丢。
//! - close 幂等：会话不在注册表（冷会话/重复 close）也返回 `closed:true`；
//!   **绝不删除** session/message 行。

use agent_protocol::session_write::{SessionCloseParams, SessionCloseResult};
use serde_json::Value;

use crate::server::ServerError;

use super::parse_params;

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionCloseParams = parse_params(crate::methods::SESSION_CLOSE_METHOD, params)?;
    if let Some(runtime) = server.remove_runtime(&params.session_id) {
        // 运行中的 turn 尽快收口（取消令牌只作用于当前 turn；排队输入照常执行）。
        let _ = runtime.cancel_turn();
        let watcher = runtime.drain_watcher();
        // 释放注册表引用：队列发送端随最后一个 Arc 关闭，worker drain 完后退出。
        drop(runtime);
        // 台账与清理任务各持一个 watcher 克隆（Receiver: Clone）。
        server.insert_closing(&params.session_id, watcher.clone());

        // drain 完成清理：worker 退出信号（watch）到达后从台账移除，此后 resume
        // 走冷恢复。任务只持有台账句柄与 sessionId，不持有 server / runtime。
        let closing = server.closing_map();
        let session_id = params.session_id.clone();
        let mut watcher = watcher;
        tokio::spawn(async move {
            if !*watcher.borrow_and_update() {
                // worker 全部退出也会关闭 watch sender（changed 返回 Err）——同样按
                // drain 完成处理，不留悬挂条目。
                let _ = watcher.changed().await;
            }
            closing
                .lock()
                .expect("closing poisoned")
                .remove(&session_id);
        });
    }
    serde_json::to_value(SessionCloseResult { closed: Some(true) })
        .map_err(|error| ServerError::internal(error.to_string()))
}
