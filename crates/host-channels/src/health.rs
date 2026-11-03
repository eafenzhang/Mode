//! `agent-health` 只读频道（W4 所有权切换决策 3：健康状态面一等暴露）。
//!
//! 只读、不拉起、不改状态：健康快照由 `AgentControlClient` 从
//! `AgentRuntimeRegistry` 事件驱动计算（见 docs/specs/host-ownership-switch.md
//! §4.2），不做定时 ping。壳接线（`inprocess.rs` 注册）随 S2 的 S0 翻面一起接，
//! 频道名在接管清单里先登记为已移植。

use std::sync::Arc;

use agent_process::{AgentControlClient, AgentWorkspaceTarget};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

use crate::params::first_arg_object;

pub const AGENT_HEALTH_CHANNEL: &str = "agent-health";
pub const LIST: &str = "list";
pub const DESCRIBE: &str = "describe";

pub struct AgentHealthChannel {
    control: Arc<AgentControlClient>,
}

impl AgentHealthChannel {
    pub fn new(control: Arc<AgentControlClient>) -> Self {
        Self { control }
    }
}

impl ServerChannel for AgentHealthChannel {
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
                LIST => {
                    let items: Vec<serde_json::Value> = control
                        .active_workspace_targets()
                        .iter()
                        .map(|target| health_entry(&control, target))
                        .collect();
                    Ok(json_value(&items))
                }
                DESCRIBE => {
                    let params = first_arg_object(&arg)?;
                    let request: serde_json::Value = serde_json::Value::Object(params);
                    let Some(workspace_path) =
                        request.get("workspacePath").and_then(|v| v.as_str())
                    else {
                        return Err(ChannelError::error(
                            "agent-health describe 需要 workspacePath",
                        ));
                    };
                    let mut target = AgentWorkspaceTarget::new(workspace_path);
                    if let Some(identity) =
                        request.get("workspaceIdentity").and_then(|v| v.as_str())
                    {
                        target = target.with_identity(identity);
                    }
                    Ok(control
                        .runtime_health(&target)
                        .and_then(|health| serde_json::to_value(&health).ok())
                        // wire 层没有 Null：JSON null 以 Json 文本承载（spec「Object 分支」）。
                        .map(|health| json_value(&health))
                        .unwrap_or(Value::Json("null".to_string())))
                }
                other => Err(ChannelError::error(format!(
                    "agent-health channel: unknown command {other}"
                ))),
            }
        })
    }
}

fn health_entry(control: &AgentControlClient, target: &AgentWorkspaceTarget) -> serde_json::Value {
    let health = control
        .runtime_health(target)
        .and_then(|health| serde_json::to_value(&health).ok());
    serde_json::json!({
        "workspacePath": target.workspace_path,
        "workspaceIdentity": target.workspace_identity,
        "remoteSessionId": target.remote_session_id,
        "health": health,
    })
}

fn json_value<T: serde::Serialize>(value: &T) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}
