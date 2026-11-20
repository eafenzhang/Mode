//! `ProviderAdapter` seam（W5 B1；镜像 TS `BotProviderAdapter` 的可移植子集，
//! spec `docs/specs/bots-domain-migration.md` §3.2）。
//!
//! 必选：`provider` / `parse_callback` / `send`；可选能力给安全缺省（对齐 TS 的
//! 可选方法语义与「未实现视为总是可写」口径）。四通道 transport crate 在 B2 起
//! 实现本 trait。

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::types::{
    InboundMessage, OutboundMedia, OutboundMessage, ProviderError, ProviderKind,
    StreamingCardHandle, StreamingCardState, TypingTarget,
};

pub type BoxFutureResult<T> = BoxFuture<'static, Result<T, ProviderError>>;

pub trait ProviderAdapter: Send + Sync {
    fn provider(&self) -> ProviderKind;

    /// 入站解析：把平台回调载荷/推送帧映射为 provider 中立消息（B2 各通道实现）。
    fn parse_callback(&self, payload: &Value) -> Vec<InboundMessage>;

    /// 被动回复文本。
    fn send(&self, message: OutboundMessage) -> BoxFutureResult<()>;

    /// 出站媒体（TS `sendMedia` 可选；未实现视为「该通道不支持发送文件」）。
    fn send_media(&self, _target: TypingTarget, _media: OutboundMedia) -> BoxFutureResult<()> {
        Box::pin(async { Err(ProviderError::Unsupported("sendMedia")) })
    }

    /// 开始 typing（TS `startTyping`/`sendTyping` 可选）。
    fn start_typing(&self, _target: TypingTarget) -> BoxFutureResult<()> {
        Box::pin(async { Err(ProviderError::Unsupported("startTyping")) })
    }

    /// 结束 typing（TS `stopTyping` 可选；微信/飞书有原生或 reaction 路径）。
    fn stop_typing(&self, _target: TypingTarget) -> BoxFutureResult<()> {
        Box::pin(async { Err(ProviderError::Unsupported("stopTyping")) })
    }

    /// context 过期恢复链的主动续期探测（TS `refreshContextToken` 可选）：
    /// `Ok(None)` = 没拿到新 token（或探测失败降级），与 TS `undefined` 同义。
    fn refresh_context_token(&self, _provider_user_id: &str) -> BoxFutureResult<Option<String>> {
        Box::pin(async { Ok(None) })
    }

    /// 该会话此刻是否具备流式写通道（TS `canWriteStreamingReply`；缺省 true）。
    fn can_write_streaming_reply(&self, _provider_user_id: &str) -> bool {
        true
    }

    /// 该 provider 是否提供流式卡片能力（微信不支持 → 服务层走离散气泡）。
    fn supports_streaming_reply(&self) -> bool {
        false
    }

    /// 创建流式卡片；`Ok(None)` = 本回合不可用（回退离散气泡）。
    fn create_streaming_reply_card(
        &self,
        _state: StreamingCardState,
    ) -> BoxFutureResult<Option<StreamingCardHandle>> {
        Box::pin(async { Ok(None) })
    }

    /// 更新流式卡片。
    fn update_streaming_reply_card(
        &self,
        _handle: StreamingCardHandle,
        _state: StreamingCardState,
    ) -> BoxFutureResult<()> {
        Box::pin(async { Err(ProviderError::Unsupported("updateStreamingReplyCard")) })
    }

    /// 卡片状态分段（TS `splitStreamingReplyCardStates`；缺省整段一个状态）。
    fn split_streaming_reply_card_states(
        &self,
        state: StreamingCardState,
    ) -> Vec<StreamingCardState> {
        vec![state]
    }

    /// 回调回执（TS `acknowledgeCallback` 可选）。
    fn acknowledge_callback(&self, _payload: &Value) -> BoxFutureResult<()> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatType, ProviderKind};
    use serde_json::json;

    struct MinimalAdapter;

    impl ProviderAdapter for MinimalAdapter {
        fn provider(&self) -> ProviderKind {
            ProviderKind::Weixin
        }

        fn parse_callback(&self, payload: &Value) -> Vec<InboundMessage> {
            payload
                .get("text")
                .and_then(Value::as_str)
                .map(|text| {
                    vec![InboundMessage {
                        provider: ProviderKind::Weixin,
                        bot_id: "bot".into(),
                        message_id: "m".into(),
                        text: text.to_string(),
                        chat_type: ChatType::Private,
                        chat_id: None,
                        provider_user_id: "u".into(),
                        display_name: None,
                        is_mention: true,
                        quoted_text: None,
                        context_token: None,
                        attachments: Vec::new(),
                        raw: payload.clone(),
                    }]
                })
                .unwrap_or_default()
        }

        fn send(&self, _message: OutboundMessage) -> BoxFutureResult<()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn minimal_adapter_gets_safe_defaults() {
        let adapter = MinimalAdapter;
        assert_eq!(adapter.provider(), ProviderKind::Weixin);
        // 缺省：不可流式、typing/媒体/卡片 Unsupported、续期 Ok(None)、回执 Ok。
        assert!(!adapter.supports_streaming_reply());
        assert!(adapter.can_write_streaming_reply("u"));
        assert!(matches!(
            adapter
                .start_typing(TypingTarget {
                    provider_user_id: "u".into(),
                    provider_message_id: None,
                    context_token: None,
                })
                .await,
            Err(ProviderError::Unsupported("startTyping"))
        ));
        assert!(adapter.refresh_context_token("u").await.unwrap().is_none());
        assert!(adapter.acknowledge_callback(&json!({})).await.is_ok());
        assert!(adapter
            .create_streaming_reply_card(StreamingCardState {
                provider_user_id: "u".into(),
                locale: None,
                blocks: Vec::new(),
                status: crate::types::StreamingCardStatus::Running,
            })
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn parse_callback_projection_is_provider_neutral() {
        let messages = MinimalAdapter.parse_callback(&json!({ "text": "你好" }));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "你好");
        assert_eq!(messages[0].chat_type, ChatType::Private);
        assert!(MinimalAdapter.parse_callback(&json!({})).is_empty());
    }
}
