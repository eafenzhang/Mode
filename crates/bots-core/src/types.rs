//! IM bots 域 provider 中立类型（W5 B1 seam；spec `docs/specs/bots-domain-migration.md` §3）。
//!
//! 形状对齐自家 TS 契约（`packages/shared/src/bots.ts`、
//! `packages/services/src/bots/providers/types.ts`）的可移植子集；
//! 平台专有字段塞进 `raw`/`metadata`，不做建模。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 四个 IM 通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Feishu,
    Wecom,
    Dingtalk,
    Weixin,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feishu => "feishu",
            Self::Wecom => "wecom",
            Self::Dingtalk => "dingtalk",
            Self::Weixin => "weixin",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatType {
    Private,
    Group,
}

/// 入站附件：kind 由平台字段推断（TS `infer*AttachmentKind` 的口径），平台专有信息留在
/// `metadata`。`download_url`/`aes_key` 供各 transport crate 的下载路径使用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundAttachment {
    pub kind: AttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Image,
    Audio,
    Video,
    File,
}

/// 入站消息（TS `BotInboundMessage` 的投影：actor 字段摊平）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundMessage {
    pub provider: ProviderKind,
    pub bot_id: String,
    pub message_id: String,
    pub text: String,
    pub chat_type: ChatType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    pub provider_user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 是否 @ 机器人（TS `actor.isMention`）：单聊恒 true；群聊按平台字段。
    #[serde(default)]
    pub is_mention: bool,
    /// 会话上下文令牌（微信 `context_token`；其它通道可为空）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<InboundAttachment>,
    /// 原始载荷：诊断与后续语义扩展用，不参与业务投影。
    #[serde(default)]
    pub raw: Value,
}

/// 出站文本消息（TS `BotOutboundMessage` 的最小子集）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub provider_user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    pub text: String,
}

/// 出站媒体（TS `BotOutboundMedia`；服务层完成路径校验/大小上限后才交 provider）。
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundMedia {
    pub kind: MediaKind,
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub caption: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    File,
}

/// typing 目标（TS `BotTypingTarget`）。
#[derive(Debug, Clone, PartialEq)]
pub struct TypingTarget {
    pub provider_user_id: String,
    pub provider_message_id: Option<String>,
    pub context_token: Option<String>,
}

/// 流式回复卡片状态（TS `BotStreamingReplyCardState` 的块子集）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamingCardState {
    pub provider_user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    pub blocks: Vec<StreamingCardBlock>,
    pub status: StreamingCardStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StreamingCardBlock {
    Message {
        text: String,
    },
    Tools {
        summaries: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expanded: Option<bool>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamingCardStatus {
    Running,
    Sealed,
    Completed,
    Error,
}

/// 卡片句柄（TS `BotStreamingReplyCardHandle`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamingCardHandle {
    pub provider_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// 该 provider 未实现此可选能力（TS 的可选方法缺省语义）。
    Unsupported(&'static str),
    /// 平台/网络失败（原始文案由调用方分类处理）。
    Platform(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(method) => write!(formatter, "unsupported provider method: {method}"),
            Self::Platform(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_serializes_to_protocol_strings() {
        for (kind, expected) in [
            (ProviderKind::Feishu, "\"feishu\""),
            (ProviderKind::Wecom, "\"wecom\""),
            (ProviderKind::Dingtalk, "\"dingtalk\""),
            (ProviderKind::Weixin, "\"weixin\""),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), expected);
            assert_eq!(format!("\"{}\"", kind.as_str()), expected);
        }
    }

    #[test]
    fn inbound_message_round_trips_with_optional_fields_omitted() {
        let message = InboundMessage {
            provider: ProviderKind::Dingtalk,
            bot_id: "bot-1".into(),
            message_id: "m-1".into(),
            text: "hi".into(),
            chat_type: ChatType::Group,
            chat_id: Some("chat-1".into()),
            provider_user_id: "u-1".into(),
            display_name: None,
            is_mention: false,
            context_token: None,
            attachments: Vec::new(),
            raw: serde_json::json!({"msgId":"m-1"}),
        };
        let text = serde_json::to_string(&message).unwrap();
        assert!(!text.contains("display_name"), "None 字段不落空值");
        assert!(!text.contains("attachments"), "空附件不落数组");
        let back: InboundMessage = serde_json::from_str(&text).unwrap();
        assert_eq!(back, message);
    }

    #[test]
    fn attachment_shapes_keep_platform_metadata() {
        let attachment = InboundAttachment {
            kind: AttachmentKind::Image,
            provider_id: Some("img-1".into()),
            download_url: Some("https://cdn/x".into()),
            aes_key: Some("key".into()),
            filename: None,
            mime_type: Some("image/png".into()),
            metadata: serde_json::json!({"width": 2}),
        };
        let text = serde_json::to_string(&attachment).unwrap();
        assert!(text.contains("\"kind\":\"image\""));
        assert!(text.contains("\"width\":2"));
    }
}
