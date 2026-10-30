//! v4 面：`resolveInteraction` 命令闭环与只读查询（`rowsRange` / `plans`）。
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
use agent_protocol::v4_transport::{
    ClientHello, Compression, DeliveryProfile, HelloAuth, HelloKind, HelloMessage,
    HostCapabilities, V4ClientMode, V4_WIRE_PROTOCOL_VERSION,
};
use agent_protocol::v4_queries::{
    PlansParams, PlansResult, RowsRangeParams, RowsRangeResult, V4_CONVERSATION_PLANS_METHOD,
    V4_CONVERSATION_ROWS_RANGE_METHOD,
};
use serde_json::Value;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

static COMMAND_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// host 的 hello（与 TS `helloConversationV4` 的能力集一致）。
pub fn build_host_hello(connection_id: &str) -> HelloMessage {
    HelloMessage {
        kind: HelloKind::Hello,
        protocol_version: V4_WIRE_PROTOCOL_VERSION,
        connection_id: connection_id.to_string(),
        client_mode: V4ClientMode::DesktopContinuous,
        delivery_profile: DeliveryProfile::Continuous,
        server_time: serde_json::Number::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
        ),
        capabilities: HostCapabilities {
            native_dialogs: true,
            local_terminal: true,
            binary_frames: false,
            compression: Compression::None,
            workspace_hook_review: Some(true),
            independent_plan_state: Some(true),
            workflow_run_deltas: Some(true),
        },
        auth: HelloAuth { user_id: None },
    }
}

/// `initializeConversationV4`：严格解析客户端 hello（未知键拒绝）。
pub fn validate_client_hello(value: &Value) -> Result<ClientHello, ControlError> {
    serde_json::from_value(value.clone())
        .map_err(|error| ControlError::InvalidResult(format!("invalid clientHello: {error}")))
}

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

    /// `v4/conversation/rowsRange`：游标制行分页（loadOlder）。只读、无状态，
    /// 陈旧读防护由调用方按 `atLogEpoch` 判断。
    pub async fn conversation_rows_range_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: RowsRangeParams,
        policy: RuntimePolicy,
    ) -> Result<RowsRangeResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("rowsRange params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_ROWS_RANGE_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/conversation/rowsRange result: {error}"))
        })
    }

    /// `v4/conversation/plans`：计划目录来自 CLI 完整 projection，不用 renderer 的有界窗口
    /// 推导。
    pub async fn conversation_plans_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: PlansParams,
        policy: RuntimePolicy,
    ) -> Result<PlansResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("plans params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_PLANS_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/conversation/plans result: {error}"))
        })
    }
}
