//! v4 会话订阅 facade：`v4/conversation/subscribe` + `v4/conversation/frame` 流。
//!
//! 行为真源：`modeAgentService.subscribeConversationV4` 与 transport/wire schema。
//! 规范见 `docs/specs/session-write-group.md` §「v4 会话订阅」。
//!
//! 差异：
//! - TS 在订阅前同步 Account Provider Config 并读 task-index 的 thought hint；
//!   本 facade 由调用方在 `params` 里直接给出（或省略）。
//! - 信任连接上下文由调用方提供（`connection_id` / `client_mode`），不在本层推断。

use tokio::sync::{broadcast, mpsc};
use tokio::task::AbortHandle;

use agent_protocol::v4_transport::{
    ConversationSubscribeAck, ConversationSubscribeParams, ConversationSubscribeResult,
    ConversationWireFrame, V4_CONVERSATION_FRAME_NOTIFICATION, V4_CONVERSATION_SUBSCRIBE_METHOD,
};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

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
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let mut notifications = process.client().subscribe_notifications();
        let wire = serde_json::to_value(&params)
            .expect("v4/conversation/subscribe params 序列化不会失败");
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
                        let Ok(frame) =
                            serde_json::from_value::<ConversationWireFrame>(params.clone())
                        else {
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
        Ok(ConversationFrameSubscription {
            ack: result.ack,
            frames: rx,
            abort: task.abort_handle(),
        })
    }
}
