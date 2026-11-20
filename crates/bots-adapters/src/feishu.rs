//! 飞书/Lark 入站解析（W5 B2b；行为真源 `feishuProvider.ts:509-821,2079-2102`）。
//!
//! 只实现消息事件（`im.message.receive_v1`，含 SDK 把 event 摊平到顶层的兼容）
//! 与群聊 @ 判定；**卡片回调/elicitation 表单**属服务层语义（B3），本波不迁入。
//! 通道族：`modeProvider === "lark"` → [`ProviderKind::Lark`]，否则 Feishu。

use bots_core::{AttachmentKind, ChatType, InboundAttachment, InboundMessage, ProviderKind};
use serde_json::{Map, Value};

use crate::util::{first_non_empty, is_record, parse_json_record, read_str};

/// 回调包装：`{botId, modeProvider?, modeFeishuBotOpenId?, event:{message,sender}}`。
pub fn parse_callback(payload: &Value) -> Vec<InboundMessage> {
    if !is_record(payload) {
        return Vec::new();
    }
    let bot_id = read_str(payload, "botId");
    if bot_id.is_empty() {
        return Vec::new();
    }
    let Some(mut message) = read_text_message(&bot_id, payload) else {
        return Vec::new();
    };
    if message.chat_type == ChatType::Group {
        let bot_open_id = read_str(payload, "modeFeishuBotOpenId");
        // 身份未知时保守放行（mention 模式宁可多响应一次，也不能静默丢消息）。
        message.is_mention = if bot_open_id.is_empty() {
            true
        } else {
            mention_open_ids(payload)
                .iter()
                .any(|id| id == &bot_open_id)
        };
    }
    vec![message]
}

fn payload_provider(payload: &Value) -> ProviderKind {
    if read_str(payload, "modeProvider") == "lark" {
        ProviderKind::Lark
    } else {
        ProviderKind::Feishu
    }
}

/// `readFeishuTextMessage`（:712-757）。
fn read_text_message(bot_id: &str, payload: &Value) -> Option<InboundMessage> {
    let provider = payload_provider(payload);
    // SDK 不同版本可能把 event 字段摊平到顶层。
    let event = payload
        .get("event")
        .filter(|event| is_record(event))
        .unwrap_or(payload);
    let message = event.get("message").filter(|message| is_record(message));
    let sender = event.get("sender").filter(|sender| is_record(sender));
    let sender_id = sender
        .and_then(|sender| sender.get("sender_id"))
        .filter(|id| is_record(id));
    let content = message.and_then(|message| parse_json_record(message.get("content")));
    let msg_type = message
        .map(|message| {
            first_non_empty([
                read_str(message, "message_type"),
                read_str(message, "msg_type"),
            ])
        })
        .unwrap_or_default();
    let attachment = content
        .as_ref()
        .and_then(|content| read_attachment(&msg_type, content));
    // 富文本/带链接消息会以 post 推送；此前只读 content.text 会静默丢消息。
    let raw_text = content
        .as_ref()
        .map(|content| read_str(content, "text"))
        .filter(|text| !text.is_empty())
        .or_else(|| content.as_ref().and_then(read_post_text))
        .or_else(|| {
            let value = read_str(event, "text_without_at_bot");
            (!value.is_empty()).then_some(value)
        })
        .or_else(|| {
            let value = read_str(event, "text");
            (!value.is_empty()).then_some(value)
        })
        .unwrap_or_default();
    let text = strip_mentions(&raw_text);
    let user_id = first_non_empty([
        sender_id
            .map(|id| read_str(id, "open_id"))
            .unwrap_or_default(),
        sender_id
            .map(|id| read_str(id, "user_id"))
            .unwrap_or_default(),
        sender_id
            .map(|id| read_str(id, "union_id"))
            .unwrap_or_default(),
        read_str(event, "open_id"),
        read_str(event, "user_id"),
        read_str(event, "union_id"),
    ]);
    if (text.is_empty() && attachment.is_none()) || user_id.is_empty() {
        return None;
    }
    let chat_id = message
        .map(|message| read_str(message, "chat_id"))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| read_str(event, "open_chat_id"));
    let message_id = message
        .map(|message| read_str(message, "message_id"))
        .unwrap_or_default();
    let chat_type = read_chat_type(&first_non_empty([
        message
            .map(|message| read_str(message, "chat_type"))
            .unwrap_or_default(),
        read_str(event, "chat_type"),
    ]));
    let attachments = attachment.into_iter().collect();
    Some(InboundMessage {
        provider,
        bot_id: bot_id.to_string(),
        message_id,
        text,
        chat_type,
        // 群聊才带 chatId（TS :753）；私聊回复目标由 providerUserId 表达。
        chat_id: (chat_type == ChatType::Group && !chat_id.is_empty()).then_some(chat_id),
        provider_user_id: user_id,
        display_name: None,
        is_mention: false,
        quoted_text: None,
        context_token: None,
        attachments,
        raw: payload.clone(),
    })
}

fn read_chat_type(value: &str) -> ChatType {
    if value == "group" || value == "group_chat" {
        ChatType::Group
    } else {
        ChatType::Private
    }
}

/// 群聊 mention 的 open_id 列表（`event.message.mentions[].id.open_id`，:584-592）。
fn mention_open_ids(payload: &Value) -> Vec<String> {
    let event = payload
        .get("event")
        .filter(|event| is_record(event))
        .unwrap_or(payload);
    let mentions = event
        .get("message")
        .and_then(|message| message.get("mentions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    mentions
        .iter()
        .filter(|mention| is_record(mention))
        .map(|mention| {
            mention
                .get("id")
                .filter(|id| is_record(id))
                .map(|id| read_str(id, "open_id"))
                .unwrap_or_default()
        })
        .filter(|open_id| !open_id.is_empty())
        .collect()
}

/// `stripFeishuMentions`（:594-599）：`<at …>…</at>` 与 `@xxx` 剥除后 trim。
fn strip_mentions(text: &str) -> String {
    let at_tag = regex::Regex::new(r"(?i)<at\b[^>]*>.*?</at>").expect("合法正则");
    let at_word = regex::Regex::new(r"@\S+").expect("合法正则");
    at_word
        .replace_all(&at_tag.replace_all(text, ""), "")
        .trim()
        .to_string()
}

/// `readFeishuAttachment`（:677-710）+ kind 推断（:660-675）。
fn read_attachment(msg_type: &str, content: &Value) -> Option<InboundAttachment> {
    let provider_file_id = first_non_empty([
        read_str(content, "image_key"),
        read_str(content, "file_key"),
        read_str(content, "media_key"),
        read_str(content, "audio_key"),
        read_str(content, "key"),
    ]);
    if provider_file_id.is_empty() {
        return None;
    }
    let kind = infer_attachment_kind(msg_type);
    let filename = first_non_empty([
        read_str(content, "file_name"),
        read_str(content, "filename"),
        format!(
            "{msg_type}-{}",
            &provider_file_id[..provider_file_id.len().min(8)]
        ),
    ]);
    let mime_type = first_non_empty([
        read_str(content, "mime_type"),
        read_str(content, "mimeType"),
        default_mime_type(kind).to_string(),
    ]);
    let mut metadata = Map::new();
    if let Some(size) = content.get("size").and_then(Value::as_f64) {
        metadata.insert("size_bytes".into(), serde_json::json!(size));
    }
    Some(InboundAttachment {
        kind,
        provider_id: Some(provider_file_id),
        download_url: None,
        aes_key: None,
        filename: Some(filename),
        mime_type: Some(mime_type),
        metadata: Value::Object(metadata),
    })
}

fn infer_attachment_kind(msg_type: &str) -> AttachmentKind {
    match msg_type {
        "image" => AttachmentKind::Image,
        "audio" => AttachmentKind::Audio,
        "media" | "video" => AttachmentKind::Video,
        _ => AttachmentKind::File,
    }
}

fn default_mime_type(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "image/jpeg",
        AttachmentKind::Audio => "audio/mpeg",
        AttachmentKind::Video => "video/mp4",
        AttachmentKind::File => "application/octet-stream",
    }
}

/// `readFeishuPostText`（:601-658）：locale 内容 + 标题；token 递归格式化。
fn read_post_text(content: &Value) -> Option<String> {
    let post = content.get("post").filter(|post| is_record(post));
    let zh_cn = content
        .get("zh_cn")
        .filter(|value| is_record(value))
        .or_else(|| {
            post.and_then(|post| post.get("zh_cn"))
                .filter(|value| is_record(value))
        });
    let en_us = content
        .get("en_us")
        .filter(|value| is_record(value))
        .or_else(|| {
            post.and_then(|post| post.get("en_us"))
                .filter(|value| is_record(value))
        });
    let post_content = content
        .get("content")
        .or_else(|| zh_cn.and_then(|zh| zh.get("content")))
        .or_else(|| en_us.and_then(|en| en.get("content")));
    let lines = post_content
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .map(|line| format_post_token(line).trim().to_string())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let title = first_non_empty([
        read_str(content, "title"),
        zh_cn.map(|zh| read_str(zh, "title")).unwrap_or_default(),
        en_us.map(|en| read_str(en, "title")).unwrap_or_default(),
    ])
    .trim()
    .to_string();
    let body = lines.join("\n").trim().to_string();
    let text = match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title}\n{body}"),
        (false, true) => title,
        (true, false) => body,
        (true, true) => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// `formatFeishuPostToken`（:615-643）。
fn format_post_token(token: &Value) -> String {
    if let Some(text) = token.as_str() {
        return text.to_string();
    }
    if let Some(items) = token.as_array() {
        return items
            .iter()
            .map(format_post_token)
            .collect::<Vec<_>>()
            .join("");
    }
    let Some(record) = token.as_object() else {
        return String::new();
    };
    let tag = read_str(token, "tag");
    if tag == "at" {
        return String::new();
    }
    let nested = record
        .get("content")
        .or_else(|| record.get("children"))
        .or_else(|| record.get("elements"));
    let nested_text = nested
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(format_post_token)
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let text = first_non_empty([
        read_str(token, "text"),
        read_str(token, "un_escape_text"),
        read_str(token, "name"),
        nested_text,
    ]);
    if tag == "a" {
        let href = read_str(token, "href");
        if !href.is_empty() && href != text {
            return if text.is_empty() {
                href
            } else {
                format!("{text} {href}")
            };
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn private_payload() -> Value {
        json!({
            "botId": "bot-1",
            "event": {
                "message": {
                    "message_id": "om-1",
                    "chat_id": "oc-1",
                    "chat_type": "p2p",
                    "message_type": "text",
                    "content": "{\"text\":\"<at user_id=\\\"bot\\\">@机器人</at> 你好\"}"
                },
                "sender": { "sender_id": { "open_id": "ou-1" } }
            }
        })
    }

    #[test]
    fn parses_private_text_and_strips_mentions() {
        let messages = parse_callback(&private_payload());
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.provider, ProviderKind::Feishu);
        assert_eq!(message.bot_id, "bot-1");
        assert_eq!(message.message_id, "om-1");
        assert_eq!(message.text, "你好");
        assert_eq!(message.chat_type, ChatType::Private);
        assert_eq!(message.provider_user_id, "ou-1");
        assert_eq!(message.chat_id, None, "私聊不带 chatId");
        assert!(message.attachments.is_empty());
    }

    #[test]
    fn honors_flattened_event_and_lark_provider() {
        // SDK 把 event 摊平到顶层 + modeProvider=lark。
        let payload = json!({
            "botId": "bot-1",
            "modeProvider": "lark",
            "message": { "message_id": "om-2", "chat_type": "p2p", "message_type": "text",
                         "content": {"text": "hi"} },
            "sender": { "sender_id": { "open_id": "ou-2" } }
        });
        let messages = parse_callback(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].provider, ProviderKind::Lark);
        assert_eq!(messages[0].text, "hi");
    }

    #[test]
    fn group_mention_gating_uses_bot_open_id() {
        let payload = json!({
            "botId": "bot-1",
            "modeFeishuBotOpenId": "ou-bot",
            "event": {
                "message": {
                    "message_id": "om-3", "chat_id": "oc-2", "chat_type": "group",
                    "message_type": "text", "content": {"text": "hello"},
                    "mentions": [
                        { "id": { "open_id": "ou-bot" } },
                        { "id": { "open_id": "ou-other" } }
                    ]
                },
                "sender": { "sender_id": { "open_id": "ou-3" } }
            }
        });
        let messages = parse_callback(&payload);
        assert_eq!(messages[0].chat_type, ChatType::Group);
        assert_eq!(messages[0].chat_id.as_deref(), Some("oc-2"));
        assert!(messages[0].is_mention);

        // bot open_id 未知 → 保守放行。
        let unknown = json!({
            "botId": "bot-1",
            "event": {
                "message": { "message_id": "om-4", "chat_id": "oc-2", "chat_type": "group",
                             "message_type": "text", "content": {"text": "hello"} },
                "sender": { "sender_id": { "open_id": "ou-3" } }
            }
        });
        assert!(parse_callback(&unknown)[0].is_mention);
    }

    #[test]
    fn post_message_text_and_attachment_kinds() {
        let post = json!({
            "botId": "bot-1",
            "event": {
                "message": {
                    "message_id": "om-5", "chat_type": "p2p", "message_type": "post",
                    "content": { "zh_cn": { "title": "标题", "content": [
                        [{ "tag": "text", "text": "第一行" }],
                        [{ "tag": "a", "text": "链接", "href": "https://x" }]
                    ] } }
                },
                "sender": { "sender_id": { "open_id": "ou-5" } }
            }
        });
        let messages = parse_callback(&post);
        assert_eq!(messages[0].text, "标题\n第一行\n链接 https://x");

        let image = json!({
            "botId": "bot-1",
            "event": {
                "message": { "message_id": "om-6", "chat_type": "p2p", "message_type": "image",
                             "content": { "image_key": "img_key_123456789" } },
                "sender": { "sender_id": { "open_id": "ou-6" } }
            }
        });
        let messages = parse_callback(&image);
        assert_eq!(messages[0].attachments.len(), 1);
        let attachment = &messages[0].attachments[0];
        assert_eq!(attachment.kind, AttachmentKind::Image);
        assert_eq!(attachment.provider_id.as_deref(), Some("img_key_123456789"));
        assert_eq!(attachment.filename.as_deref(), Some("image-img_key_"));
        assert_eq!(attachment.mime_type.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn empty_or_identityless_payloads_are_dropped() {
        // 无 text 且无附件。
        let empty = json!({
            "botId": "bot-1",
            "event": {
                "message": { "message_id": "om-7", "chat_type": "p2p", "message_type": "text",
                             "content": {"text": "   "} },
                "sender": { "sender_id": { "open_id": "ou-7" } }
            }
        });
        assert!(parse_callback(&empty).is_empty());
        // 缺用户身份。
        let no_user = json!({
            "botId": "bot-1",
            "event": { "message": { "message_id": "om-8", "chat_type": "p2p",
                                    "message_type": "text", "content": {"text": "hi"} } }
        });
        assert!(parse_callback(&no_user).is_empty());
        // 缺 botId。
        assert!(parse_callback(&json!({ "event": {} })).is_empty());
    }
}
