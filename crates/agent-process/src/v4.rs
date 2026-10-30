//! v4 命令最小闭环：`resolveInteraction`。
//!
//! 行为真源：`packages/services/src/mode-agent/modeV4HostCommand.ts` 的
//! `createHostCommandEnvelope` / `assertV4CommandAckOk`，以及
//! `modeAgentService.sendConversationCommandV4` 的调用形状。
//!
//! 差异（已写入 spec）：
//! - `commandId` / `clientId` 只保证「非空且按提交端稳定」，不是 UUID v7（协议 schema 只要求
//!   字符串；幂等表以 `commandId` 为键，语义不受影响）。
//! - 订阅/信任连接（`helloConversationV4`）尚未迁移；`resolveInteraction` 不依赖它。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_protocol::v4::{
    CommandAck, ResolveInteractionAnswer, ResolveInteractionEnvelope, ResolveInteractionPayload,
    ResolveInteractionType, V4_COMMAND_METHOD,
};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

static COMMAND_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_command_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let sequence = COMMAND_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    format!("{now:x}-{:x}-{sequence:x}", std::process::id())
}

impl AgentControlClient {
    /// 回执一次交互请求（权限或 AskUserQuestion）。`accepted` / `duplicate` / `noop`
    /// 按幂等成功返回；`rejected` / `stale` / `failed` 返回 `CommandRejected`。
    pub async fn resolve_interaction_v4(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        interaction_id: &str,
        answer: ResolveInteractionAnswer,
    ) -> Result<CommandAck, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let envelope = ResolveInteractionEnvelope {
            command_id: next_command_id(),
            client_id: self.v4_client_id().to_string(),
            session_id: session_id.to_string(),
            command_type: ResolveInteractionType::ResolveInteraction,
            payload: ResolveInteractionPayload {
                interaction_id: interaction_id.to_string(),
                answer,
            },
            issued_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
        };
        let params = serde_json::to_value(&envelope).expect("v4/command params 序列化不会失败");
        let response = process
            .client()
            .request(V4_COMMAND_METHOD, Some(params))
            .await?;
        let ack: CommandAck = serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/command result: {error}"))
        })?;
        if !ack.status.is_success() {
            let mut message = format!("v4 command resolveInteraction {}", ack.status.as_str());
            if let Some(reason) = &ack.reason_code {
                message.push_str(&format!(" ({reason})"));
            }
            if let Some(detail) = &ack.message {
                message.push_str(&format!(": {detail}"));
            }
            return Err(ControlError::CommandRejected(message));
        }
        Ok(ack)
    }
}
