//! 企微智能机器人入站解析（W5 B2b；行为真源 `wecomProvider.ts:92-184,487-525`）。
//!
//! 入站帧 `payload = {botId, frame:{headers, body}}`；body 形状按 SDK 的
//! `BaseMessage`（msgid/msgtype/from.userid/chattype/chatid/quote/各 msgtype 内容）。

use bots_core::{AttachmentKind, ChatType, InboundAttachment, InboundMessage, ProviderKind};
use serde_json::{Map, Value};

use crate::util::{is_record, read_str};

pub fn parse_callback(payload: &Value) -> Vec<InboundMessage> {
    if !is_record(payload) {
        return Vec::new();
    }
    let bot_id = read_str(payload, "botId");
    let body = payload
        .get("frame")
        .and_then(|frame| frame.get("body"))
        .filter(|body| is_record(body));
    let Some(body) = body else {
        return Vec::new();
    };
    let msgid = read_str(body, "msgid");
    let user_id = body
        .get("from")
        .map(|from| read_str(from, "userid"))
        .unwrap_or_default();
    if bot_id.is_empty() || msgid.is_empty() || user_id.is_empty() {
        return Vec::new();
    }
    let chat_type = if read_str(body, "chattype") == "group" {
        ChatType::Group
    } else {
        ChatType::Private
    };
    let text = inbound_text(body);
    let attachments = inbound_attachments(body);
    // 纯文本为空且无附件 → 丢弃（TS :503-505 用 trim 判断）。
    if text.trim().is_empty() && attachments.is_empty() {
        return Vec::new();
    }
    let quoted = quoted_text(body);
    let chat_id = read_str(body, "chatid");
    vec![InboundMessage {
        provider: ProviderKind::Wecom,
        bot_id,
        message_id: msgid,
        text,
        chat_type,
        chat_id: (!chat_id.is_empty()).then_some(chat_id),
        provider_user_id: user_id,
        display_name: None,
        // 企微只在被 @ 时下发群消息回调；群消息按定向消息处理（TS :517-519）。
        is_mention: chat_type == ChatType::Group,
        quoted_text: (!quoted.is_empty()).then_some(quoted),
        context_token: None,
        attachments,
        raw: payload.clone(),
    }]
}

/// `readWeComInboundText`（:92-116）。
fn inbound_text(body: &Value) -> String {
    match read_str(body, "msgtype").as_str() {
        "text" => body
            .get("text")
            .map(|text| read_str(text, "content"))
            .unwrap_or_default(),
        "mixed" => mixed_text(body),
        "voice" => body
            .get("voice")
            .map(|voice| read_str(voice, "content"))
            .unwrap_or_default(),
        "image" => "[图片]".into(),
        "file" => "[文件]".into(),
        "video" => "[视频]".into(),
        _ => String::new(),
    }
}

fn mixed_text(body: &Value) -> String {
    let items = body
        .get("mixed")
        .and_then(|mixed| mixed.get("msg_item"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .map(|item| {
            if read_str(item, "msgtype") == "text" {
                item.get("text")
                    .map(|text| read_str(text, "content"))
                    .unwrap_or_default()
            } else {
                "[图片]".to_string()
            }
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `readWeComQuotedText`（:122-144）：引用内容只进 prompt，不进命令解析。
fn quoted_text(body: &Value) -> String {
    let Some(quote) = body.get("quote").filter(|quote| is_record(quote)) else {
        return String::new();
    };
    match read_str(quote, "msgtype").as_str() {
        "text" => quote
            .get("text")
            .map(|text| read_str(text, "content").trim().to_string())
            .unwrap_or_default(),
        "voice" => quote
            .get("voice")
            .map(|voice| read_str(voice, "content").trim().to_string())
            .unwrap_or_default(),
        "mixed" => {
            let items = quote
                .get("mixed")
                .and_then(|mixed| mixed.get("msg_item"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            items
                .iter()
                .map(|item| {
                    if read_str(item, "msgtype") == "text" {
                        item.get("text")
                            .map(|text| read_str(text, "content"))
                            .unwrap_or_default()
                    } else {
                        "[图片]".to_string()
                    }
                })
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        }
        "image" => "[图片]".into(),
        "file" => "[文件]".into(),
        _ => String::new(),
    }
}

/// `readWeComInboundAttachments`（:146-184）：媒体必须有 url。
fn inbound_attachments(body: &Value) -> Vec<InboundAttachment> {
    let mut attachments = Vec::new();
    let mut push_media =
        |kind: AttachmentKind, content: Option<&Value>, filename: &str, mime: &str| {
            let Some(content) = content else {
                return;
            };
            let url = read_str(content, "url");
            if url.is_empty() {
                return;
            }
            let mut metadata = Map::new();
            let aeskey = read_str(content, "aeskey");
            if !aeskey.is_empty() {
                metadata.insert("aeskey".into(), Value::String(aeskey.clone()));
            }
            let id = format!("{kind:?}-{}", tail_chars(&url, 24)).to_lowercase();
            attachments.push(InboundAttachment {
                kind,
                provider_id: Some(id),
                download_url: Some(url),
                aes_key: (!aeskey.is_empty()).then_some(aeskey),
                filename: Some(filename.to_string()),
                mime_type: Some(mime.to_string()),
                metadata: Value::Object(metadata),
            });
        };
    match read_str(body, "msgtype").as_str() {
        "image" => push_media(
            AttachmentKind::Image,
            body.get("image"),
            "wecom-image.jpg",
            "image/jpeg",
        ),
        "file" => push_media(
            AttachmentKind::File,
            body.get("file"),
            "wecom-file",
            "application/octet-stream",
        ),
        "video" => push_media(
            AttachmentKind::Video,
            body.get("video"),
            "wecom-video.mp4",
            "video/mp4",
        ),
        "mixed" => {
            let items = body
                .get("mixed")
                .and_then(|mixed| mixed.get("msg_item"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (index, item) in items.iter().enumerate() {
                if read_str(item, "msgtype") == "image" {
                    push_media(
                        AttachmentKind::Image,
                        item.get("image"),
                        &format!("wecom-image-{}.jpg", index + 1),
                        "image/jpeg",
                    );
                }
            }
        }
        _ => {}
    }
    attachments
}

/// 取字符串最后 `n` 个字符（TS `url.slice(-24)`；按字符而非字节，避免切坏 UTF-8）。
fn tail_chars(value: &str, n: usize) -> String {
    let count = value.chars().count();
    value.chars().skip(count.saturating_sub(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(body: Value) -> Value {
        json!({ "botId": "bot-1", "frame": { "headers": { "req_id": "r" }, "body": body } })
    }

    #[test]
    fn parses_private_text_and_requires_identity() {
        let messages = parse_callback(&payload(json!({
            "msgid": "m-1",
            "msgtype": "text",
            "text": { "content": "  你好  " },
            "from": { "userid": "u-1" },
            "chattype": "single",
        })));
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.provider, ProviderKind::Wecom);
        assert_eq!(message.message_id, "m-1");
        assert_eq!(message.text, "  你好  ", "企微不做 trim");
        assert_eq!(message.chat_type, ChatType::Private);
        assert_eq!(message.provider_user_id, "u-1");
        assert!(!message.is_mention);

        assert!(parse_callback(&payload(json!({
            "msgid": "m-2", "msgtype": "text", "text": { "content": "x" }, "from": {}
        })))
        .is_empty());
    }

    #[test]
    fn group_messages_are_mentions_with_chat_id() {
        let messages = parse_callback(&payload(json!({
            "msgid": "m-3", "msgtype": "text", "text": { "content": "hi" },
            "from": { "userid": "u-2" }, "chattype": "group", "chatid": "chat-9",
        })));
        assert_eq!(messages[0].chat_type, ChatType::Group);
        assert_eq!(messages[0].chat_id.as_deref(), Some("chat-9"));
        assert!(messages[0].is_mention);
    }

    #[test]
    fn mixed_voice_and_quoted_text() {
        let mixed = parse_callback(&payload(json!({
            "msgid": "m-4", "msgtype": "mixed",
            "mixed": { "msg_item": [
                { "msgtype": "text", "text": { "content": "看这个" } },
                { "msgtype": "image", "image": { "url": "https://cdn/i.png" } }
            ] },
            "from": { "userid": "u-3" }, "chattype": "single",
        })));
        assert_eq!(mixed[0].text, "看这个\n[图片]");

        let voice = parse_callback(&payload(json!({
            "msgid": "m-5", "msgtype": "voice", "voice": { "content": "转写文本" },
            "from": { "userid": "u-3" }, "chattype": "single",
            "quote": { "msgtype": "text", "text": { "content": " 被引用 " } },
        })));
        assert_eq!(voice[0].text, "转写文本");
        assert_eq!(voice[0].quoted_text.as_deref(), Some("被引用"));

        // 纯图片消息：text 占位 + 附件。
        let image = parse_callback(&payload(json!({
            "msgid": "m-6", "msgtype": "image",
            "image": { "url": "https://cdn/img.png", "aeskey": "k1" },
            "from": { "userid": "u-3" }, "chattype": "single",
        })));
        assert_eq!(image[0].text, "[图片]");
        assert_eq!(image[0].attachments.len(), 1);
        let attachment = &image[0].attachments[0];
        assert_eq!(attachment.kind, AttachmentKind::Image);
        assert_eq!(
            attachment.download_url.as_deref(),
            Some("https://cdn/img.png")
        );
        assert_eq!(attachment.aes_key.as_deref(), Some("k1"));
        assert_eq!(attachment.filename.as_deref(), Some("wecom-image.jpg"));
    }

    #[test]
    fn empty_content_without_attachments_is_dropped() {
        assert!(parse_callback(&payload(json!({
            "msgid": "m-7", "msgtype": "text", "text": { "content": "   " },
            "from": { "userid": "u-4" }, "chattype": "single",
        })))
        .is_empty());
    }
}
