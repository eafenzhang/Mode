//! 钉钉入站解析（W5 B2；行为真源 `dingtalkProvider.ts:58-140,381-400`
//! 与 `packages/services/test/bot-dingtalk.test.ts` 的用例口径）。
//!
//! 只做「平台载荷 → provider 中立消息」的投影；发送路径（HTTP/oapi token）
//! 属 B2b，不在此模块。

use bots_core::{ChatType, InboundMessage, ProviderKind};
use serde_json::Value;

/// 群聊 chatId 前缀（TS `DINGTALK_GROUP_CHAT_PREFIX`，dingtalkProvider.ts:32）。
pub const GROUP_CHAT_PREFIX: &str = "group:";
/// 机器人消息拉取 topic（含尾斜杠变体，TS :389-392）。
pub const BOT_MESSAGE_TOPIC: &str = "/v1.0/im/bot/messages/get";

/// 回调包装载荷：`{botId, frame:{headers:{topic}, data:<JSON 字符串>}}`。
/// 非目标 topic、缺 botId、data 非对象或坏 JSON → 空（TS 的宽容语义；
/// 坏 JSON 在 TS 会抛错，Rust 侧降级为空并留待上层日志）。
pub fn parse_callback(payload: &Value) -> Vec<InboundMessage> {
    let Some(bot_id) = payload.get("botId").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(frame) = payload.get("frame") else {
        return Vec::new();
    };
    let topic = frame
        .get("headers")
        .and_then(|headers| headers.get("topic"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if topic != BOT_MESSAGE_TOPIC && topic != format!("{BOT_MESSAGE_TOPIC}/") {
        return Vec::new();
    }
    let Some(data) = frame
        .get("data")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return Vec::new();
    };
    if bot_id.is_empty() || !data.is_object() {
        return Vec::new();
    }
    parse_message(bot_id, &data).into_iter().collect()
}

/// 单条回调 data → InboundMessage（TS `parseDingtalkBotMessage` :84-140）。
pub fn parse_message(bot_id: &str, data: &Value) -> Option<InboundMessage> {
    let message_id = read_string(data, "msgId");
    if message_id.is_empty() {
        return None;
    }
    let msg_type = {
        let value = read_string(data, "msgtype");
        if value.is_empty() {
            "text".to_string()
        } else {
            value
        }
    };
    let text = match msg_type.as_str() {
        "text" => data
            .get("text")
            .map(|text| read_string(text, "content"))
            .unwrap_or_default(),
        "richText" => {
            let items = data
                .get("content")
                .and_then(|content| content.get("richText"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            items
                .iter()
                .map(|item| item.get("text").and_then(Value::as_str).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("")
        }
        other => format!("[不支持的消息类型: {other}]"),
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let sender_staff_id = read_string(data, "senderStaffId");
    let sender_nick = read_string(data, "senderNick");
    let conversation_type = {
        let value = read_string(data, "conversationType");
        if value.is_empty() {
            "1".to_string()
        } else {
            value
        }
    };
    let is_group = conversation_type == "2";
    let conversation_id = read_string(data, "conversationId");
    // isInAtList 可能是 bool 或字符串 "true"（TS :117-120）。
    let is_in_at_list = match data.get("isInAtList") {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => value == "true",
        _ => false,
    };
    let chat_id = if is_group {
        format!("{GROUP_CHAT_PREFIX}{conversation_id}")
    } else {
        sender_staff_id.clone()
    };
    Some(InboundMessage {
        provider: ProviderKind::Dingtalk,
        bot_id: bot_id.to_string(),
        message_id,
        text,
        chat_type: if is_group {
            ChatType::Group
        } else {
            ChatType::Private
        },
        chat_id: (!chat_id.is_empty()).then_some(chat_id),
        provider_user_id: sender_staff_id,
        display_name: (!sender_nick.is_empty()).then_some(sender_nick),
        // 单聊天然"被 @"；群聊按 isInAtList（钉钉在 @ 机器人时才回调）。
        is_mention: if is_group { is_in_at_list } else { true },
        context_token: None,
        attachments: Vec::new(),
        raw: data.clone(),
    })
}

/// 发送目标判定（TS `resolveDingtalkSendTarget` :75-81）：`group:` 前缀 → 群 API，
/// 其余按单聊 staffId。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTargetKind {
    Private,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTarget {
    pub kind: SendTargetKind,
    pub target: String,
}

#[must_use]
pub fn resolve_send_target(chat_id: &str) -> SendTarget {
    match chat_id.strip_prefix(GROUP_CHAT_PREFIX) {
        Some(rest) => SendTarget {
            kind: SendTargetKind::Group,
            target: rest.to_string(),
        },
        None => SendTarget {
            kind: SendTargetKind::Private,
            target: chat_id.to_string(),
        },
    }
}

fn read_string(record: &Value, key: &str) -> String {
    record
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 用例镜像 packages/services/test/bot-dingtalk.test.ts。

    #[test]
    fn private_text_message_uses_staff_id_as_chat() {
        let message = parse_message(
            "bot-1",
            &json!({
                "msgId": "msg-1",
                "msgtype": "text",
                "text": { "content": "  你好  " },
                "senderStaffId": "staff-1",
                "senderNick": "张三",
                "conversationType": "1",
            }),
        )
        .expect("应解析");
        assert_eq!(message.text, "你好");
        assert_eq!(message.provider, ProviderKind::Dingtalk);
        assert_eq!(message.chat_type, ChatType::Private);
        assert_eq!(message.provider_user_id, "staff-1");
        assert_eq!(message.chat_id.as_deref(), Some("staff-1"));
        assert_eq!(message.display_name.as_deref(), Some("张三"));
        assert!(message.is_mention);
        assert_eq!(message.message_id, "msg-1");
    }

    #[test]
    fn group_message_prefixes_chat_id_and_reads_is_in_at_list_both_forms() {
        let group = parse_message(
            "bot-1",
            &json!({
                "msgId": "msg-2",
                "msgtype": "text",
                "text": { "content": "@机器人 跑一下测试" },
                "senderStaffId": "staff-2",
                "conversationType": "2",
                "conversationId": "cid-abc",
                "isInAtList": "true",
            }),
        )
        .expect("应解析");
        assert_eq!(group.chat_type, ChatType::Group);
        assert_eq!(group.chat_id.as_deref(), Some("group:cid-abc"));
        assert_eq!(group.provider_user_id, "staff-2");
        assert!(group.is_mention);

        let not_mentioned = parse_message(
            "bot-1",
            &json!({
                "msgId": "msg-3",
                "msgtype": "text",
                "text": { "content": "没 @ 机器人" },
                "senderStaffId": "staff-2",
                "conversationType": "2",
                "conversationId": "cid-abc",
                "isInAtList": false,
            }),
        )
        .expect("应解析");
        assert!(!not_mentioned.is_mention);
    }

    #[test]
    fn rich_text_joins_items_and_unknown_types_get_placeholder() {
        let rich = parse_message(
            "bot-1",
            &json!({
                "msgId": "msg-4",
                "msgtype": "richText",
                "content": { "richText": [{ "text": "前" }, { "text": "后" }] },
                "senderStaffId": "staff-1",
                "conversationType": "1",
            }),
        )
        .expect("应解析");
        assert_eq!(rich.text, "前后");

        let unknown = parse_message(
            "bot-1",
            &json!({
                "msgId": "msg-5",
                "msgtype": "picture",
                "senderStaffId": "staff-1",
                "conversationType": "1",
            }),
        )
        .expect("应解析");
        assert_eq!(unknown.text, "[不支持的消息类型: picture]");
    }

    #[test]
    fn empty_content_or_missing_ids_are_dropped() {
        assert!(parse_message(
            "bot-1",
            &json!({
                "msgId": "msg-6",
                "msgtype": "text",
                "text": { "content": "   " },
                "senderStaffId": "staff-1",
                "conversationType": "1",
            }),
        )
        .is_none());
        assert!(parse_message("bot-1", &json!({ "msgtype": "text" })).is_none());
    }

    #[test]
    fn callback_wrapper_filters_topic_and_bot_id() {
        let frame = |topic: &str, data: Value| {
            json!({
                "botId": "bot-1",
                "frame": { "headers": { "topic": topic }, "data": data.to_string() },
            })
        };
        let payload = frame(
            "/v1.0/im/bot/messages/get",
            json!({
                "msgId": "m-1",
                "msgtype": "text",
                "text": { "content": "hi" },
                "senderStaffId": "staff-1",
                "conversationType": "1",
            }),
        );
        assert_eq!(parse_callback(&payload).len(), 1);
        // 尾斜杠变体。
        let trailing = frame(
            "/v1.0/im/bot/messages/get/",
            json!({ "msgId": "m-2", "msgtype": "text", "text": { "content": "hi" }, "senderStaffId": "s" }),
        );
        assert_eq!(parse_callback(&trailing).len(), 1);
        // 其它 topic / 缺 botId / 坏 data。
        assert!(parse_callback(&frame(
            "/other",
            json!({ "msgId": "m", "text": { "content": "x" } })
        ))
        .is_empty());
        assert!(parse_callback(&json!({
            "frame": { "headers": { "topic": "/v1.0/im/bot/messages/get" }, "data": "{}" }
        }))
        .is_empty());
        assert!(parse_callback(&json!({
            "botId": "bot-1",
            "frame": { "headers": { "topic": "/v1.0/im/bot/messages/get" }, "data": "not-json" }
        }))
        .is_empty());
    }

    #[test]
    fn send_target_resolves_group_prefix() {
        assert_eq!(
            resolve_send_target("group:cid-abc"),
            SendTarget {
                kind: SendTargetKind::Group,
                target: "cid-abc".into()
            }
        );
        assert_eq!(
            resolve_send_target("staff-1"),
            SendTarget {
                kind: SendTargetKind::Private,
                target: "staff-1".into()
            }
        );
    }
}
