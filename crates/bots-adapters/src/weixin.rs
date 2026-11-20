//! 微信 iLink 入站解析（W5 B2b；行为真源 `weixinProvider.ts:232-380,424-560,607-636,881-891`）。
//!
//! 入站载荷两种形态都兼容：轮询 runtime 的紧凑投影
//! `{botId, messages:[{id,text,from,chatId,displayName,context_token,attachments}]}`，
//! 以及 iLink 原始消息（`msg`/`message` 内层 + `item_list`）。附件字段形态按 TS 的
//! 宽容读法（多别名键）。

use bots_core::{AttachmentKind, ChatType, InboundAttachment, InboundMessage, ProviderKind};
use serde_json::{Map, Value};

use crate::util::{first_non_empty, is_record, read_number, read_number_or_string, read_str};

pub fn parse_callback(payload: &Value) -> Vec<InboundMessage> {
    if !is_record(payload) {
        return Vec::new();
    }
    let bot_id = read_str(payload, "botId");
    if bot_id.is_empty() {
        return Vec::new();
    }
    read_messages(payload)
        .iter()
        .filter_map(|message| build_inbound_message(&bot_id, message))
        .collect()
}

/// `readWeixinMessages`（:548-563）：容器取 `data`，数组键 `msgs/messages/updates/items/list`。
fn read_messages(payload: &Value) -> Vec<Value> {
    let container = payload.get("data").unwrap_or(payload);
    let raw = container
        .get("msgs")
        .or_else(|| container.get("messages"))
        .or_else(|| container.get("updates"))
        .or_else(|| container.get("items"))
        .or_else(|| container.get("list"));
    match raw {
        Some(Value::Array(items)) => items
            .iter()
            .filter(|item| is_record(item))
            .cloned()
            .collect(),
        Some(item @ Value::Object(_)) => vec![item.clone()],
        _ => Vec::new(),
    }
}

/// `buildInboundMessage`（:607-636）。
fn build_inbound_message(bot_id: &str, message: &Value) -> Option<InboundMessage> {
    // message_type=2 是机器人自身消息，跳过（TS :611-613）。
    if read_number(message, "message_type") == Some(2.0) {
        return None;
    }
    let text = read_text(message).trim().to_string();
    let attachments = read_attachments(message);
    let user_id = read_user_id(message).trim().to_string();
    if (text.is_empty() && attachments.is_empty()) || user_id.is_empty() {
        return None;
    }
    let chat_id = read_chat_id(message);
    let chat_type = if chat_id.is_some() {
        ChatType::Group
    } else {
        ChatType::Private
    };
    Some(InboundMessage {
        provider: ProviderKind::Weixin,
        bot_id: bot_id.to_string(),
        message_id: read_message_id(message).unwrap_or_default(),
        text,
        chat_type,
        chat_id,
        provider_user_id: user_id,
        display_name: read_display_name(message),
        // 微信通道不输出 @ 语义（TS :625-634 不设置 isMention）。
        is_mention: false,
        quoted_text: None,
        context_token: read_context_token(message),
        attachments,
        raw: message.clone(),
    })
}

fn inner(message: &Value) -> Option<&Value> {
    message
        .get("msg")
        .filter(|value| is_record(value))
        .or_else(|| message.get("message").filter(|value| is_record(value)))
}

/// `readWeixinText`（:424-446）：顶层文本键优先；否则 item_list 文本项拼接。
fn read_text(message: &Value) -> String {
    let direct = first_non_empty([
        read_str(message, "text"),
        read_str(message, "content"),
        read_str(message, "message"),
    ]);
    if !direct.is_empty() {
        return direct;
    }
    let inner = inner(message);
    let item_list = item_list_of(message);
    let joined = item_list
        .iter()
        .map(read_text_item)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !joined.is_empty() {
        return joined;
    }
    inner
        .map(|inner| first_non_empty([read_str(inner, "text"), read_str(inner, "content")]))
        .unwrap_or_default()
}

/// `readWeixinTextItem`（:237-243）。
fn read_text_item(item: &Value) -> String {
    let text_item = item.get("text_item").filter(|value| is_record(value));
    first_non_empty([
        text_item
            .map(|text| read_str(text, "text"))
            .unwrap_or_default(),
        read_str(item, "text"),
        read_str(item, "content"),
    ])
}

fn item_list_of(message: &Value) -> Vec<Value> {
    let lists = message
        .get("item_list")
        .and_then(Value::as_array)
        .or_else(|| {
            inner(message)
                .and_then(|inner| inner.get("item_list"))
                .and_then(Value::as_array)
        });
    lists
        .map(|items| {
            items
                .iter()
                .filter(|item| is_record(item))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// `readWeixinUserId`（:473-489）。
fn read_user_id(message: &Value) -> String {
    let from = message.get("from").filter(|value| is_record(value));
    let sender = message.get("sender").filter(|value| is_record(value));
    first_non_empty([
        read_str(message, "from_user_id"),
        read_str(message, "from"),
        read_str(message, "from_user"),
        read_str(message, "fromUser"),
        read_str(message, "user"),
        read_str(message, "user_id"),
        read_str(message, "userId"),
        from.map(|value| read_str(value, "id")).unwrap_or_default(),
        from.map(|value| read_str(value, "wxid"))
            .unwrap_or_default(),
        sender
            .map(|value| read_str(value, "id"))
            .unwrap_or_default(),
        sender
            .map(|value| read_str(value, "wxid"))
            .unwrap_or_default(),
    ])
}

/// `readWeixinChatId`（:491-501）。
fn read_chat_id(message: &Value) -> Option<String> {
    let chat_id = first_non_empty([
        read_str(message, "room"),
        read_str(message, "room_id"),
        read_str(message, "roomId"),
        read_str(message, "chat"),
        read_str(message, "chat_id"),
        read_str(message, "chatId"),
    ]);
    (!chat_id.is_empty()).then_some(chat_id)
}

/// `readWeixinDisplayName`（:503-522）。
fn read_display_name(message: &Value) -> Option<String> {
    let from = message.get("from").filter(|value| is_record(value));
    let sender = message.get("sender").filter(|value| is_record(value));
    let inner = inner(message);
    let name = first_non_empty([
        read_str(message, "name"),
        read_str(message, "displayName"),
        read_str(message, "nickname"),
        from.map(|value| read_str(value, "name"))
            .unwrap_or_default(),
        from.map(|value| read_str(value, "nickname"))
            .unwrap_or_default(),
        sender
            .map(|value| read_str(value, "name"))
            .unwrap_or_default(),
        sender
            .map(|value| read_str(value, "nickname"))
            .unwrap_or_default(),
        inner
            .map(|value| read_str(value, "sender_name"))
            .unwrap_or_default(),
    ]);
    (!name.is_empty()).then_some(name)
}

/// `readWeixinMessageId`（:524-548）：字符串键优先，数值键转十进制字符串。
fn read_message_id(message: &Value) -> Option<String> {
    let inner = inner(message);
    let direct = first_non_empty([
        read_str(message, "id"),
        read_str(message, "msgid"),
        read_str(message, "msgId"),
        read_number_or_string(message, "message_id"),
        inner.map(|value| read_str(value, "id")).unwrap_or_default(),
        inner
            .map(|value| read_str(value, "msgid"))
            .unwrap_or_default(),
        inner
            .map(|value| read_str(value, "msgId"))
            .unwrap_or_default(),
        inner
            .map(|value| read_number_or_string(value, "message_id"))
            .unwrap_or_default(),
    ]);
    if !direct.is_empty() {
        return Some(direct);
    }
    let numeric = read_number(message, "id")
        .or_else(|| read_number(message, "msgid"))
        .or_else(|| read_number(message, "msgId"))
        .or_else(|| inner.and_then(|value| read_number(value, "id")))
        .or_else(|| inner.and_then(|value| read_number(value, "msgid")))
        .or_else(|| inner.and_then(|value| read_number(value, "msgId")))?;
    Some(format_number(numeric))
}

/// `readWeixinContextToken`（:579-594）。
fn read_context_token(message: &Value) -> Option<String> {
    let inner = inner(message);
    let token = first_non_empty([
        read_str(message, "context_token"),
        read_str(message, "contextToken"),
        read_str(message, "context"),
        inner
            .map(|value| read_str(value, "context_token"))
            .unwrap_or_default(),
    ]);
    (!token.is_empty()).then_some(token)
}

/// `readWeixinAttachments`（:447-471）：item_list 媒体项 + 直接附件列表。
fn read_attachments(message: &Value) -> Vec<InboundAttachment> {
    let mut attachments: Vec<InboundAttachment> = item_list_of(message)
        .iter()
        .enumerate()
        .filter_map(|(index, item)| read_attachment_item(item, index))
        .collect();
    let direct = message
        .get("attachments")
        .and_then(Value::as_array)
        .or_else(|| {
            inner(message)
                .and_then(|inner| inner.get("attachments"))
                .and_then(Value::as_array)
        });
    if let Some(items) = direct {
        attachments.extend(
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| read_direct_attachment(item, index)),
        );
    }
    attachments
}

/// `readWeixinAttachmentItem`（:275-350）。
fn read_attachment_item(item: &Value, index: usize) -> Option<InboundAttachment> {
    if !read_text_item(item).is_empty() {
        return None;
    }
    let media = [
        "image_item",
        "file_item",
        "video_item",
        "audio_item",
        "media_item",
    ]
    .iter()
    .find_map(|key| item.get(*key).filter(|value| is_record(value)))
    .unwrap_or(item);
    // mediaSource = { ...media, ...media.media }（后者覆盖前者，TS :296-298）。
    let media_source = {
        let mut merged: Map<String, Value> = media.as_object().cloned().unwrap_or_default();
        if let Some(payload) = media.get("media").filter(|value| is_record(value)) {
            for (key, value) in payload.as_object().into_iter().flatten() {
                merged.insert(key.clone(), value.clone());
            }
        }
        Value::Object(merged)
    };
    let provider_file_id = first_non_empty([
        read_number_or_string(&media_source, "file_id"),
        read_number_or_string(&media_source, "fileId"),
        read_number_or_string(&media_source, "media_id"),
        read_number_or_string(&media_source, "mediaId"),
        read_number_or_string(&media_source, "id"),
        read_number_or_string(&media_source, "encrypt_query_param"),
        read_number_or_string(&media_source, "encryptQueryParam"),
        read_number_or_string(&media_source, "media"),
        read_number_or_string(&media_source, "md5"),
    ]);
    let download_url = first_non_empty([
        read_str(&media_source, "url"),
        read_str(&media_source, "download_url"),
        read_str(&media_source, "downloadUrl"),
        // 微信 image_item.media.full_url 是实际下载地址（TS :315-318）。
        read_str(&media_source, "full_url"),
        read_str(&media_source, "fullUrl"),
    ]);
    let data_base64 = first_non_empty([
        read_str(&media_source, "data_base64"),
        read_str(&media_source, "dataBase64"),
        read_str(&media_source, "base64"),
    ]);
    if provider_file_id.is_empty() && download_url.is_empty() && data_base64.is_empty() {
        return None;
    }
    let kind = infer_attachment_kind(item, &media_source);
    let filename = first_non_empty([
        read_str(&media_source, "filename"),
        read_str(&media_source, "file_name"),
        read_str(&media_source, "name"),
        if kind == AttachmentKind::Image {
            format!("weixin-image-{}.jpg", index + 1)
        } else {
            format!("weixin-attachment-{}", index + 1)
        },
    ]);
    let mime_type = first_non_empty([
        read_str(&media_source, "mime_type"),
        read_str(&media_source, "mimeType"),
        infer_mime_from_filename(&filename),
        default_mime_for(kind).to_string(),
    ]);
    let mut metadata = Map::new();
    let size = read_number(&media_source, "size")
        .or_else(|| read_number(&media_source, "sizeBytes"))
        .or_else(|| read_number(&media_source, "file_size"))
        .or_else(|| read_number(&media_source, "len"))
        .or_else(|| read_number(&media_source, "mid_size"));
    if let Some(size) = size {
        metadata.insert("size_bytes".into(), serde_json::json!(size));
    }
    if !data_base64.is_empty() {
        metadata.insert("data_base64".into(), Value::String(data_base64));
    }
    let aes_key = first_non_empty([
        read_str(&media_source, "aes_key"),
        read_str(&media_source, "aesKey"),
        read_str(&media_source, "aeskey"),
    ]);
    Some(InboundAttachment {
        kind,
        provider_id: (!provider_file_id.is_empty()).then_some(provider_file_id),
        download_url: (!download_url.is_empty()).then_some(download_url),
        aes_key: (!aes_key.is_empty()).then_some(aes_key),
        filename: Some(filename),
        mime_type: Some(mime_type),
        metadata: Value::Object(metadata),
    })
}

/// `readWeixinDirectAttachment`（:380-421）。
fn read_direct_attachment(item: &Value, index: usize) -> Option<InboundAttachment> {
    if !is_record(item) {
        return None;
    }
    let kind = match read_str(item, "kind").as_str() {
        "image" => AttachmentKind::Image,
        "audio" => AttachmentKind::Audio,
        "video" => AttachmentKind::Video,
        "file" => AttachmentKind::File,
        _ => return None,
    };
    let id = first_non_empty([read_str(item, "id"), format!("weixin-{}", index + 1)]);
    let filename = first_non_empty([
        read_str(item, "filename"),
        format!("{id}.{}", kind_str(kind)),
    ]);
    let mime_type = first_non_empty([
        read_str(item, "mimeType"),
        read_str(item, "mime_type"),
        default_mime_for(kind).to_string(),
    ]);
    let provider_file_id =
        first_non_empty([read_str(item, "providerFileId"), read_str(item, "file_id")]);
    let download_url = first_non_empty([
        read_str(item, "downloadUrl"),
        read_str(item, "download_url"),
    ]);
    let data_base64 =
        first_non_empty([read_str(item, "dataBase64"), read_str(item, "data_base64")]);
    let mut metadata = Map::new();
    if let Some(size) = read_number(item, "sizeBytes").or_else(|| read_number(item, "size")) {
        metadata.insert("size_bytes".into(), serde_json::json!(size));
    }
    if let Some(provider_metadata) = item
        .get("providerMetadata")
        .filter(|value| is_record(value))
    {
        for (key, value) in provider_metadata.as_object().into_iter().flatten() {
            if let Some(text) = value.as_str() {
                metadata.insert(key.clone(), Value::String(text.to_string()));
            }
        }
    }
    if !data_base64.is_empty() {
        metadata.insert("data_base64".into(), Value::String(data_base64));
    }
    Some(InboundAttachment {
        kind,
        provider_id: Some(first_non_empty([provider_file_id, id])),
        download_url: (!download_url.is_empty()).then_some(download_url),
        aes_key: None,
        filename: Some(filename),
        mime_type: Some(mime_type),
        metadata: Value::Object(metadata),
    })
}

/// `inferWeixinAttachmentKind`（:245-273）。
fn infer_attachment_kind(item: &Value, media_source: &Value) -> AttachmentKind {
    if item.get("image_item").is_some() {
        return AttachmentKind::Image;
    }
    let explicit = first_non_empty([
        read_str(item, "kind"),
        read_str(item, "media_type"),
        read_str(item, "mediaType"),
        read_str(item, "type_name"),
    ]);
    let mime_type = first_non_empty([read_str(item, "mime_type"), read_str(item, "mimeType")]);
    let filename = first_non_empty([
        read_str(item, "filename"),
        read_str(item, "file_name"),
        read_str(item, "name"),
    ]);
    let normalized = format!("{explicit} {mime_type} {filename}").to_lowercase();
    if normalized.contains("image")
        || normalized.contains("photo")
        || normalized.contains("picture")
        || has_image_extension(&filename)
    {
        return AttachmentKind::Image;
    }
    if normalized.contains("audio") || normalized.contains("voice") {
        return AttachmentKind::Audio;
    }
    if normalized.contains("video") {
        return AttachmentKind::Video;
    }
    // media_source 的平台字段名（例如 image_item 已在上方短路）也可能带类型线索。
    let _ = media_source;
    AttachmentKind::File
}

fn has_image_extension(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    [
        ".svg", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic", ".bmp",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension))
}

fn infer_mime_from_filename(filename: &str) -> String {
    let lower = filename.to_lowercase();
    if lower.ends_with(".svg") {
        "image/svg+xml".into()
    } else if lower.ends_with(".png") {
        "image/png".into()
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".into()
    } else if lower.ends_with(".gif") {
        "image/gif".into()
    } else if lower.ends_with(".webp") {
        "image/webp".into()
    } else {
        String::new()
    }
}

fn default_mime_for(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "image/jpeg",
        AttachmentKind::Audio => "audio/mpeg",
        AttachmentKind::Video => "video/mp4",
        AttachmentKind::File => "application/octet-stream",
    }
}

fn kind_str(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "image",
        AttachmentKind::Audio => "audio",
        AttachmentKind::Video => "video",
        AttachmentKind::File => "file",
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_compact_polling_projection() {
        let payload = json!({
            "botId": "bot-1",
            "messages": [{
                "id": "m-1",
                "text": "你好",
                "from": "wxid-1",
                "chatId": "room-1",
                "displayName": "张三",
                "context_token": "ctx-1",
            }],
            "buf": "cursor",
        });
        let messages = parse_callback(&payload);
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.provider, ProviderKind::Weixin);
        assert_eq!(message.message_id, "m-1");
        assert_eq!(message.text, "你好");
        assert_eq!(message.chat_type, ChatType::Group);
        assert_eq!(message.chat_id.as_deref(), Some("room-1"));
        assert_eq!(message.provider_user_id, "wxid-1");
        assert_eq!(message.display_name.as_deref(), Some("张三"));
        assert_eq!(message.context_token.as_deref(), Some("ctx-1"));
        assert!(!message.is_mention);
    }

    #[test]
    fn parses_raw_item_list_text_and_image_with_aes_key() {
        let payload = json!({
            "botId": "bot-1",
            "msgs": [{
                "id": 42,
                "from_user_id": "wxid-2",
                "item_list": [
                    { "type": 1, "text_item": { "text": "看图" } },
                    { "type": 3, "image_item": { "media": {
                        "file_id": "img-1", "full_url": "https://cdn/x.jpg", "aes_key": "k1" } } }
                ],
            }],
        });
        let messages = parse_callback(&payload);
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.message_id, "42");
        assert_eq!(message.text, "看图");
        assert_eq!(message.chat_type, ChatType::Private);
        assert_eq!(message.attachments.len(), 1);
        let attachment = &message.attachments[0];
        assert_eq!(attachment.kind, AttachmentKind::Image);
        assert_eq!(attachment.provider_id.as_deref(), Some("img-1"));
        assert_eq!(
            attachment.download_url.as_deref(),
            Some("https://cdn/x.jpg")
        );
        assert_eq!(attachment.aes_key.as_deref(), Some("k1"));
        assert_eq!(attachment.filename.as_deref(), Some("weixin-image-2.jpg"));
        assert_eq!(attachment.mime_type.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn direct_attachments_and_bot_echo_are_handled() {
        let payload = json!({
            "botId": "bot-1",
            "messages": [
                { "id": "m-2", "message_type": 2, "text": "echo", "from": "bot" },
                { "id": "m-3", "from": "wxid-3",
                  "attachments": [{ "kind": "file", "filename": "a.pdf", "providerFileId": "file-1",
                                    "downloadUrl": "https://cdn/a.pdf", "sizeBytes": 12 }] },
            ],
        });
        let messages = parse_callback(&payload);
        assert_eq!(messages.len(), 1, "机器人自身消息应跳过");
        let attachment = &messages[0].attachments[0];
        assert_eq!(attachment.kind, AttachmentKind::File);
        assert_eq!(attachment.provider_id.as_deref(), Some("file-1"));
        assert_eq!(attachment.filename.as_deref(), Some("a.pdf"));
        assert_eq!(
            attachment.mime_type.as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(attachment.metadata["size_bytes"], json!(12.0));
    }

    #[test]
    fn empty_text_and_no_attachments_are_dropped() {
        let payload = json!({
            "botId": "bot-1",
            "messages": [
                { "id": "m-4", "text": "  ", "from": "u" },
                { "id": "m-5", "text": "hi", "from": "u" },
            ],
        });
        let messages = parse_callback(&payload);
        assert_eq!(messages.len(), 1, "空文本与缺用户的消息被丢弃");
        assert_eq!(messages[0].message_id, "m-5");
    }

    #[test]
    fn unknown_attachment_kind_in_direct_list_is_skipped() {
        let payload = json!({
            "botId": "bot-1",
            "messages": [{ "id": "m-6", "from": "u",
                           "attachments": [{ "kind": "sticker", "id": "x" }],
                           "text": "fallback" }],
        });
        let messages = parse_callback(&payload);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].attachments.is_empty());
    }
}
