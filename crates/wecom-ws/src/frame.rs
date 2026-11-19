//! JSON 帧信封与 req_id（spec `docs/specs/bots-im-transport.md` §4d/§6.1；
//! SDK `dist/index.cjs.js` :194-198, :345-369, :371-405 的对齐）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 命令常量（SDK `WsCmd` :66-91）。
pub const CMD_SUBSCRIBE: &str = "aibot_subscribe";
pub const CMD_HEARTBEAT: &str = "ping";
pub const CMD_RESPONSE: &str = "aibot_respond_msg";
pub const CMD_RESPONSE_WELCOME: &str = "aibot_respond_welcome_msg";
pub const CMD_RESPONSE_UPDATE: &str = "aibot_respond_update_msg";
pub const CMD_SEND_MSG: &str = "aibot_send_msg";
pub const CMD_UPLOAD_MEDIA_INIT: &str = "aibot_upload_media_init";
pub const CMD_UPLOAD_MEDIA_CHUNK: &str = "aibot_upload_media_chunk";
pub const CMD_UPLOAD_MEDIA_FINISH: &str = "aibot_upload_media_finish";
pub const CMD_CALLBACK: &str = "aibot_msg_callback";
pub const CMD_EVENT_CALLBACK: &str = "aibot_event_callback";

/// 入站消息类型（SDK `MessageType` :45-60）。
pub const MSG_TEXT: &str = "text";
pub const MSG_IMAGE: &str = "image";
pub const MSG_MIXED: &str = "mixed";
pub const MSG_VOICE: &str = "voice";
pub const MSG_FILE: &str = "file";
pub const MSG_VIDEO: &str = "video";

/// 事件类型（SDK `EventType` :111-122）。
pub const EVENT_ENTER_CHAT: &str = "enter_chat";
pub const EVENT_TEMPLATE_CARD: &str = "template_card_event";
pub const EVENT_FEEDBACK: &str = "feedback_event";
/// 服务端因新连接建立而主动断开旧连接（:120-121, :383-401）。
pub const EVENT_DISCONNECTED: &str = "disconnected_event";

/// 下行帧：回执帧只有 `headers/errcode/errmsg`，推送帧有 `cmd/body`
/// （spec §6.1 第 2 条）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Frame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(default)]
    pub headers: FrameHeaders,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errcode: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errmsg: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FrameHeaders {
    #[serde(default)]
    pub req_id: String,
}

impl Frame {
    pub fn req_id(&self) -> &str {
        &self.headers.req_id
    }
}

/// 生成 req_id：`{prefix}_{timestamp}_{8位hex}`（SDK :194-198）。
pub fn generate_req_id(prefix: &str, now_ms: u128) -> String {
    let random: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("{prefix}_{now_ms}_{random}")
}

/// 上行帧构造：`{cmd, headers:{req_id}, body}`（SDK :547-555）。
pub fn outbound_frame(cmd: &str, req_id: &str, body: Value) -> Frame {
    Frame {
        cmd: Some(cmd.to_string()),
        headers: FrameHeaders {
            req_id: req_id.to_string(),
        },
        body: Some(body),
        errcode: None,
        errmsg: None,
    }
}

/// 入站分类（SDK `WsConnectionManager::handleFrame` :371-445 的对齐）。
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// 消息推送：`aibot_msg_callback`。
    Message { req_id: String, body: Value },
    /// 事件推送：`aibot_event_callback`。
    Event {
        req_id: String,
        body: Value,
        event_type: String,
    },
    /// 服务端主动断开（disconnected_event）：派发后禁止重连。
    ServerDisconnected { req_id: String, body: Value },
    /// 认证回执（req_id 以 `aibot_subscribe` 开头）。
    AuthAck { errcode: i64, errmsg: String },
    /// 心跳回执（req_id 以 `ping` 开头）。
    HeartbeatAck { errcode: i64, errmsg: String },
    /// 回复回执（req_id 命中 pendingAcks 与否由队列层判断）。
    ReplyAck {
        req_id: String,
        errcode: i64,
        errmsg: String,
    },
    /// 未知帧：只 warn，不派发（SDK :443-445）。
    Unknown,
}

/// 纯函数：按 cmd / req_id 前缀分类。`is_pending_reply` 由队列层注入
/// （SDK 用 pendingAcks 是否命中区分回复回执与未知帧）。
pub fn classify(frame: &Frame, is_pending_reply: impl Fn(&str) -> bool) -> Inbound {
    if let Some(cmd) = &frame.cmd {
        if cmd == CMD_CALLBACK {
            return Inbound::Message {
                req_id: frame.req_id().to_string(),
                body: frame.body.clone().unwrap_or(Value::Null),
            };
        }
        if cmd == CMD_EVENT_CALLBACK {
            let body = frame.body.clone().unwrap_or(Value::Null);
            let event_type = body
                .get("event")
                .and_then(|event| event.get("eventtype"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if event_type == EVENT_DISCONNECTED {
                return Inbound::ServerDisconnected {
                    req_id: frame.req_id().to_string(),
                    body,
                };
            }
            return Inbound::Event {
                req_id: frame.req_id().to_string(),
                body,
                event_type,
            };
        }
        return Inbound::Unknown;
    }
    let req_id = frame.req_id();
    if req_id.starts_with(CMD_SUBSCRIBE) {
        return Inbound::AuthAck {
            errcode: frame.errcode.unwrap_or_default(),
            errmsg: frame.errmsg.clone().unwrap_or_default(),
        };
    }
    if req_id.starts_with(CMD_HEARTBEAT) {
        return Inbound::HeartbeatAck {
            errcode: frame.errcode.unwrap_or_default(),
            errmsg: frame.errmsg.clone().unwrap_or_default(),
        };
    }
    if is_pending_reply(req_id) {
        return Inbound::ReplyAck {
            req_id: req_id.to_string(),
            errcode: frame.errcode.unwrap_or_default(),
            errmsg: frame.errmsg.clone().unwrap_or_default(),
        };
    }
    Inbound::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn req_id_format_matches_sdk() {
        let req_id = generate_req_id("aibot_send_msg", 1_700_000_000_000);
        // 前缀本身含下划线：从右侧拆 {prefix}_{ts}_{hex}。
        let mut parts = req_id.rsplitn(3, '_');
        let hex = parts.next().expect("hex");
        let timestamp = parts.next().expect("timestamp");
        let prefix = parts.next().expect("prefix");
        assert_eq!(prefix, "aibot_send_msg");
        assert_eq!(timestamp, "1700000000000");
        assert_eq!(hex.len(), 8);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn serializes_outbound_envelope() {
        let frame = outbound_frame(CMD_RESPONSE, "req-1", json!({ "msgtype": "stream" }));
        let text = serde_json::to_string(&frame).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["cmd"], json!("aibot_respond_msg"));
        assert_eq!(value["headers"]["req_id"], json!("req-1"));
        assert_eq!(value["body"]["msgtype"], json!("stream"));
        assert!(value.get("errcode").is_none(), "上行帧不带 errcode");
    }

    #[test]
    fn classifies_inbound_frames() {
        let message = Frame {
            cmd: Some(CMD_CALLBACK.into()),
            headers: FrameHeaders {
                req_id: "r1".into(),
            },
            body: Some(json!({"msgid":"m1","msgtype":"text"})),
            errcode: None,
            errmsg: None,
        };
        assert!(matches!(
            classify(&message, |_| false),
            Inbound::Message { .. }
        ));

        let event = Frame {
            cmd: Some(CMD_EVENT_CALLBACK.into()),
            headers: FrameHeaders {
                req_id: "r2".into(),
            },
            body: Some(json!({"event":{"eventtype":"enter_chat"}})),
            errcode: None,
            errmsg: None,
        };
        assert!(matches!(classify(&event, |_| false), Inbound::Event { .. }));

        let kicked = Frame {
            cmd: Some(CMD_EVENT_CALLBACK.into()),
            headers: FrameHeaders {
                req_id: "r3".into(),
            },
            body: Some(json!({"event":{"eventtype":"disconnected_event"}})),
            errcode: None,
            errmsg: None,
        };
        assert!(matches!(
            classify(&kicked, |_| false),
            Inbound::ServerDisconnected { .. }
        ));

        let auth = Frame {
            cmd: None,
            headers: FrameHeaders {
                req_id: "aibot_subscribe_1_ab".into(),
            },
            body: None,
            errcode: Some(0),
            errmsg: Some("ok".into()),
        };
        assert!(matches!(
            classify(&auth, |_| false),
            Inbound::AuthAck { errcode: 0, .. }
        ));

        let heartbeat = Frame {
            cmd: None,
            headers: FrameHeaders {
                req_id: "ping_2_cd".into(),
            },
            body: None,
            errcode: Some(0),
            errmsg: None,
        };
        assert!(matches!(
            classify(&heartbeat, |_| false),
            Inbound::HeartbeatAck { errcode: 0, .. }
        ));

        let reply = Frame {
            cmd: None,
            headers: FrameHeaders {
                req_id: "aibot_respond_msg_3_ef".into(),
            },
            body: Some(json!({})),
            errcode: Some(0),
            errmsg: None,
        };
        assert!(matches!(
            classify(&reply, |req_id| req_id.contains("respond_msg")),
            Inbound::ReplyAck { .. }
        ));

        let unknown = Frame {
            cmd: None,
            headers: FrameHeaders { req_id: "x".into() },
            body: None,
            errcode: None,
            errmsg: None,
        };
        assert_eq!(classify(&unknown, |_| false), Inbound::Unknown);
    }
}
