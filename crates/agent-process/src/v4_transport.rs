//! v4 订阅 facade：`v4/conversation/subscribe` + `v4/conversation/frame` 流。
//!
//! 行为真源：`modeAgentService.subscribeConversationV4` / `subscribeSessionsIndexV4`
//! 与 transport/wire schema。规范见 `docs/specs/session-write-group.md`
//! §「v4 会话订阅」与 `docs/specs/v4-sessions-index.md`。
//!
//! 两个 topic 共用同一 RPC 与 notification，只是 payload 类型不同：
//! - `conversation/<sessionId>` → [`ConversationWireFrame`]
//! - `sessions-index/<workspaceId>` → [`SessionsIndexWireFrame`]
//!
//! 差异：
//! - TS 在订阅前同步 Account Provider Config 并读 task-index 的 thought hint；
//!   本 facade 由调用方在 `params` 里直接给出（或省略）。
//! - 信任连接上下文由调用方提供（`connection_id` / `client_mode`），不在本层推断。

use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, mpsc};
use tokio::task::AbortHandle;

use agent_protocol::v4_sessions_index::{parse_sessions_index_topic, SessionsIndexWireFrame};
use agent_protocol::v4_transport::{
    ConversationSubscribeAck, ConversationSubscribeParams, ConversationSubscribeResult,
    ConversationWireFrame, SubscribeBase, SubscribeVisibility, V4ClientMode,
    V4_CONVERSATION_FRAME_NOTIFICATION, V4_CONVERSATION_SUBSCRIBE_METHOD,
};
use agent_protocol::v4_workspace_config::{parse_workspace_config_topic, WorkspaceConfigWireFrame};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

const FRAME_CHANNEL_CAPACITY: usize = 256;

/// 订阅句柄：`ack` + 已按 subscriptionId 过滤的 wire frame 流；`Drop` 中止转发。
pub struct ConversationFrameSubscription {
    pub ack: ConversationSubscribeAck,
    frames: mpsc::Receiver<ConversationWireFrame>,
    abort: AbortHandle,
}

impl ConversationFrameSubscription {
    pub async fn recv(&mut self) -> Option<ConversationWireFrame> {
        self.frames.recv().await
    }
}

impl Drop for ConversationFrameSubscription {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// sessions-index 订阅句柄；语义与 conversation 一致，只是帧类型不同。
pub struct SessionsIndexFrameSubscription {
    pub ack: ConversationSubscribeAck,
    frames: mpsc::Receiver<SessionsIndexWireFrame>,
    abort: AbortHandle,
}

impl SessionsIndexFrameSubscription {
    pub async fn recv(&mut self) -> Option<SessionsIndexWireFrame> {
        self.frames.recv().await
    }
}

impl Drop for SessionsIndexFrameSubscription {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// workspace-config 订阅句柄。
pub struct WorkspaceConfigFrameSubscription {
    pub ack: ConversationSubscribeAck,
    frames: mpsc::Receiver<WorkspaceConfigWireFrame>,
    abort: AbortHandle,
}

impl WorkspaceConfigFrameSubscription {
    pub async fn recv(&mut self) -> Option<WorkspaceConfigWireFrame> {
        self.frames.recv().await
    }
}

impl Drop for WorkspaceConfigFrameSubscription {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// 三个 topic 的 wire frame 都需要提供 subscriptionId 供过滤。
trait TopicWireFrame: DeserializeOwned + Send + 'static {
    fn subscription_id(&self) -> &str;
}

impl TopicWireFrame for ConversationWireFrame {
    fn subscription_id(&self) -> &str {
        self.subscription_id()
    }
}

impl TopicWireFrame for SessionsIndexWireFrame {
    fn subscription_id(&self) -> &str {
        self.subscription_id()
    }
}

impl TopicWireFrame for WorkspaceConfigWireFrame {
    fn subscription_id(&self) -> &str {
        self.subscription_id()
    }
}

/// 建立订阅并把匹配 `subscriptionId` 的 `v4/conversation/frame` 投进通道。
async fn subscribe_topic_frames<F: TopicWireFrame>(
    client: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    params: &ConversationSubscribeParams,
) -> Result<(ConversationSubscribeAck, mpsc::Receiver<F>, AbortHandle), ControlError> {
    let process = client
        .client_for(target, RuntimePolicy::StartIfNeeded)
        .await?;
    let mut notifications = process.client().subscribe_notifications();
    let wire =
        serde_json::to_value(params).expect("v4/conversation/subscribe params 序列化不会失败");
    let response = process
        .client()
        .request(V4_CONVERSATION_SUBSCRIBE_METHOD, Some(wire))
        .await?;
    let result: ConversationSubscribeResult =
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/subscribe result: {error}"
            ))
        })?;
    let wanted = result.ack.subscription_id.clone();
    let (tx, rx) = mpsc::channel(FRAME_CHANNEL_CAPACITY);
    let task = tokio::spawn(async move {
        loop {
            match notifications.recv().await {
                Ok(value) => {
                    if value.get("method").and_then(serde_json::Value::as_str)
                        != Some(V4_CONVERSATION_FRAME_NOTIFICATION)
                    {
                        continue;
                    }
                    let Some(params) = value.get("params") else {
                        continue;
                    };
                    let Ok(frame) = serde_json::from_value::<F>(params.clone()) else {
                        continue;
                    };
                    if frame.subscription_id() != wanted {
                        continue;
                    }
                    if tx.send(frame).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Ok((result.ack, rx, task.abort_handle()))
}

impl AgentControlClient {
    /// 建立会话订阅；首帧（snapshot 或 resume）随后作为 `v4/conversation/frame` 到达。
    pub async fn subscribe_conversation_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationSubscribeParams,
    ) -> Result<ConversationFrameSubscription, ControlError> {
        params
            .validate()
            .map_err(|message| ControlError::InvalidParams(message.to_string()))?;
        let (ack, frames, abort) = subscribe_topic_frames(self, target, &params).await?;
        Ok(ConversationFrameSubscription { ack, frames, abort })
    }

    /// 建立 sessions-index 订阅：与 conversation 共用 RPC，topic 必须是
    /// `sessions-index/<workspaceId>`。
    pub async fn subscribe_sessions_index_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationSubscribeParams,
    ) -> Result<SessionsIndexFrameSubscription, ControlError> {
        params
            .validate()
            .map_err(|message| ControlError::InvalidParams(message.to_string()))?;
        if parse_sessions_index_topic(&params.topic).is_none() {
            return Err(ControlError::InvalidParams(
                "invalid sessions-index topic".to_string(),
            ));
        }
        let (ack, frames, abort) = subscribe_topic_frames(self, target, &params).await?;
        Ok(SessionsIndexFrameSubscription { ack, frames, abort })
    }

    /// 建立 workspace-config 订阅：topic 必须是 `workspace-config/<workspaceId>`。
    pub async fn subscribe_workspace_config_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationSubscribeParams,
    ) -> Result<WorkspaceConfigFrameSubscription, ControlError> {
        params
            .validate()
            .map_err(|message| ControlError::InvalidParams(message.to_string()))?;
        if parse_workspace_config_topic(&params.topic).is_none() {
            return Err(ControlError::InvalidParams(
                "invalid workspace-config topic".to_string(),
            ));
        }
        let (ack, frames, abort) = subscribe_topic_frames(self, target, &params).await?;
        Ok(WorkspaceConfigFrameSubscription { ack, frames, abort })
    }
}

/// `v4/conversation/unsubscribe` 方法名（与 TS `V4_METHODS.conversationUnsubscribe` 对齐）。
pub const V4_CONVERSATION_UNSUBSCRIBE_METHOD: &str = "v4/conversation/unsubscribe";

impl AgentControlClient {
    /// 按 sessionId 建立会话帧订阅（topic=`conversation/<sessionId>`）。
    ///
    /// 连接身份（connectionId / clientMode）由调用方注入；宿主内部直调默认
    /// `desktop-continuous`（与 TS `resolveV4Connection` 的 fallback 同口径）。
    pub async fn subscribe_conversation_frames(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        connection_id: &str,
        client_mode: V4ClientMode,
        base: Option<SubscribeBase>,
        visibility: Option<SubscribeVisibility>,
    ) -> Result<ConversationFrameSubscription, ControlError> {
        let params = ConversationSubscribeParams {
            topic: format!("conversation/{session_id}"),
            base,
            visibility,
            connection_id: connection_id.to_string(),
            client_mode,
            workspace: Some(session_workspace_ref(target)),
            legacy_task_ids: None,
            resume_thought_level: None,
            workflow_run_deltas: None,
        };
        self.subscribe_conversation_v4(target, params).await
    }

    /// 退订：按 route 的 topic/subscriptionId/connectionId 发协议退订请求。
    pub async fn unsubscribe_conversation_frames(
        &self,
        target: &AgentWorkspaceTarget,
        topic: &str,
        subscription_id: &str,
        connection_id: &str,
    ) -> Result<(), ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = serde_json::json!({
            "topic": topic,
            "subscriptionId": subscription_id,
            "connectionId": connection_id,
        });
        process
            .client()
            .request(V4_CONVERSATION_UNSUBSCRIBE_METHOD, Some(params))
            .await?;
        Ok(())
    }
}
