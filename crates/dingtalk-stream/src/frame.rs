//! 下行帧解析与单一处理核。
//!
//! [`handle_frame`] 是 TS `handleTextFrame`（dingtalkChannelRuntime.ts:148-230）的
//! 逐条对齐：SYSTEM/ping 原样回 pong；SYSTEM/dispatch 关闭；CALLBACK/EVENT 先 ACK
//! （EVENT 的 data 必须带 `{"status":"SUCCESS","message":"success"}`）；仅 CALLBACK 且
//! topic 为机器人消息拉取（含尾斜杠变体）时解析 `msgId` 去重后派发；data 解析失败忽略
//! （ACK 已发）。socket IO 不在本函数——只产出决策，由 runtime 执行。

use serde::Serialize;
use serde_json::{Map, Value};

use crate::dedupe::DedupeSet;

/// 机器人消息拉取的 topic（含官方文档与线上观察到的尾斜杠变体）。
pub const BOT_MESSAGE_TOPIC: &str = "/v1.0/im/bot/messages/get";

/// 上行帧：字段形状与顺序对齐 TS 的 `JSON.stringify({code, headers, message, data})`。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpFrame {
    pub code: u32,
    pub headers: Value,
    pub message: &'static str,
    pub data: Value,
}

impl UpFrame {
    pub fn to_text(&self) -> String {
        serde_json::to_string(self).expect("UpFrame 序列化不会失败")
    }
}

/// 下行帧：`raw`/`headers` 保留原始 JSON（pong 透传与派发载荷都要原样）。
#[derive(Debug, Clone)]
pub struct DownFrame {
    pub raw: Value,
    pub frame_type: String,
    pub headers: Value,
    pub message_id: String,
    pub topic: String,
    /// `data` 的字符串形态（协议里 data 是 JSON 字符串；非字符串为 None）。
    pub data: Option<String>,
}

impl DownFrame {
    /// TS 的宽容解析：非对象/坏 JSON 直接忽略（返回 None）；`type`/`messageId`/`topic`
    /// 非字符串时按空串继续处理（必然不命中任何分支，等价 TS 的 `""` 回退）。
    pub fn parse(text: &str) -> Option<DownFrame> {
        let raw: Value = serde_json::from_str(text).ok()?;
        if !raw.is_object() {
            return None;
        }
        let frame_type = raw
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let headers = raw
            .get("headers")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Map::new()));
        let message_id = headers
            .get("messageId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let topic = headers
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let data = raw.get("data").and_then(Value::as_str).map(str::to_string);
        Some(DownFrame {
            raw,
            frame_type,
            headers,
            message_id,
            topic,
            data,
        })
    }
}

/// 派发载荷：`processProviderCallback("dingtalk", { botId, frame })` 的 frame 半边
/// （botId 由 runtime 层补上，dingtalkChannelRuntime.ts:220-222）。
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchedCallback {
    pub msg_id: String,
    pub raw: Value,
}

/// 处理决策：`reply` 立即回写 socket；`dispatch` 交给调用方（fire-and-forget）；
/// `close` 关闭连接交给重连循环。三个字段可同时出现（ACK + 派发）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FrameOutcome {
    pub reply: Option<UpFrame>,
    pub close: bool,
    pub dispatch: Option<DispatchedCallback>,
}

/// 单一处理核。`now_ms` 供去重时间戳使用（TS 用 `Date.now()`，这里由调用方注入）。
pub fn handle_frame(frame: &DownFrame, dedupe: &mut DedupeSet, now_ms: u64) -> FrameOutcome {
    let mut outcome = FrameOutcome::default();

    if frame.frame_type == "SYSTEM" {
        if frame.topic == "ping" {
            // 原样回 pong（headers + data 透传，code 200）——TS :166-172、官方「直接响应」。
            outcome.reply = Some(UpFrame {
                code: 200,
                headers: frame.headers.clone(),
                message: "OK",
                data: frame.raw.get("data").cloned().unwrap_or(Value::Null),
            });
        } else if frame.topic == "disconnect" {
            // 服务端优雅断开通知：不响应，关闭连接交给重连循环——TS :174-183。
            outcome.close = true;
        }
        return outcome;
    }

    if (frame.frame_type == "CALLBACK" || frame.frame_type == "EVENT")
        && !frame.message_id.is_empty()
    {
        // ACK 与派发解耦：先 ACK（不会触发服务端重投），再决定是否派发——TS :187-199。
        let ack_data = if frame.frame_type == "EVENT" {
            Value::String("{\"status\":\"SUCCESS\",\"message\":\"success\"}".to_string())
        } else {
            Value::String("{}".to_string())
        };
        outcome.reply = Some(UpFrame {
            code: 200,
            headers: serde_json::json!({
                "contentType": "application/json",
                "messageId": frame.message_id,
            }),
            message: "OK",
            data: ack_data,
        });
    }

    if frame.frame_type != "CALLBACK" {
        return outcome;
    }
    if frame.topic != BOT_MESSAGE_TOPIC && frame.topic != format!("{BOT_MESSAGE_TOPIC}/") {
        return outcome;
    }
    // data 是 JSON 字符串；解析失败直接忽略（ACK 已发，不会触发服务端重投）——TS :209-216。
    let data: Value = match frame.data.as_deref().map(serde_json::from_str::<Value>) {
        Some(Ok(data)) if data.is_object() => data,
        _ => return outcome,
    };
    let Some(msg_id) = data.get("msgId").and_then(Value::as_str) else {
        return outcome;
    };
    if msg_id.is_empty() || dedupe.is_duplicate(msg_id, now_ms) {
        return outcome;
    }
    outcome.dispatch = Some(DispatchedCallback {
        msg_id: msg_id.to_string(),
        raw: frame.raw.clone(),
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(text: &str) -> DownFrame {
        DownFrame::parse(text).expect("合法帧")
    }

    /// 构造 CALLBACK 帧：data 是「JSON 字符串里再套一层 JSON」的协议形状。
    fn callback_frame(message_id: &str, topic: &str, data: Value) -> DownFrame {
        DownFrame::parse(
            &json!({
                "type": "CALLBACK",
                "headers": { "contentType": "application/json", "messageId": message_id, "topic": topic },
                "data": data.to_string(),
            })
            .to_string(),
        )
        .expect("合法帧")
    }

    #[test]
    fn system_ping_replies_with_passthrough_headers_and_data() {
        // TS :166-172：headers 与 data 原样透传。
        let down = frame(
            r#"{"specVersion":"1.0","type":"SYSTEM","headers":{"contentType":"application/json","messageId":"m1","topic":"ping"},"data":"{\"opaque\":\"abc\"}"}"#,
        );
        let mut dedupe = DedupeSet::default();
        let outcome = handle_frame(&down, &mut dedupe, 0);
        let reply = outcome.reply.expect("应有 pong");
        assert_eq!(reply.code, 200);
        assert_eq!(
            reply.headers,
            json!({"contentType":"application/json","messageId":"m1","topic":"ping"})
        );
        assert_eq!(reply.data, json!("{\"opaque\":\"abc\"}"));
        assert!(!outcome.close);
        assert!(outcome.dispatch.is_none());
    }

    #[test]
    fn system_disconnect_closes_without_reply() {
        let down = frame(
            r#"{"type":"SYSTEM","headers":{"topic":"disconnect"},"data":"{\"reason\":\"connection is expired\"}"}"#,
        );
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let outcome = handle_frame(&down, &mut dedupe, 0);
        assert!(outcome.close);
        assert!(outcome.reply.is_none());
    }

    #[test]
    fn event_acks_with_success_status() {
        // 官方：EVENT 的 data 必须含 status；TS :188-199。
        let down = frame(
            r#"{"type":"EVENT","headers":{"contentType":"application/json","messageId":"e1","topic":"*"},"data":"{}"}"#,
        );
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let outcome = handle_frame(&down, &mut dedupe, 0);
        let reply = outcome.reply.expect("应有 ACK");
        assert_eq!(
            reply.data,
            Value::String("{\"status\":\"SUCCESS\",\"message\":\"success\"}".to_string())
        );
        assert_eq!(
            reply.headers,
            json!({"contentType":"application/json","messageId":"e1"})
        );
        assert!(outcome.dispatch.is_none());
    }

    #[test]
    fn callback_target_topic_acks_and_dispatches_once() {
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let down = callback_frame("c1", BOT_MESSAGE_TOPIC, json!({"msgId":"m-1","text":"hi"}));
        let outcome = handle_frame(&down, &mut dedupe, 0);
        let reply = outcome.reply.expect("应有 ACK");
        assert_eq!(reply.data, Value::String("{}".to_string()));
        let dispatch = outcome.dispatch.expect("应派发");
        assert_eq!(dispatch.msg_id, "m-1");
        assert_eq!(dispatch.raw["headers"]["messageId"], json!("c1"));

        // 重复 msgId：只 ACK，不再派发（服务端重投窗口）。
        let outcome = handle_frame(&down, &mut dedupe, 1);
        assert!(outcome.reply.is_some());
        assert!(outcome.dispatch.is_none());
    }

    #[test]
    fn callback_trailing_slash_topic_is_accepted() {
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let down = callback_frame(
            "c2",
            &format!("{BOT_MESSAGE_TOPIC}/"),
            json!({"msgId":"m-2"}),
        );
        let outcome = handle_frame(&down, &mut dedupe, 0);
        assert!(outcome.dispatch.is_some());
    }

    #[test]
    fn callback_other_topic_acks_but_does_not_dispatch() {
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let down = frame(
            r#"{"type":"CALLBACK","headers":{"messageId":"c3","topic":"/v1.0/im/bot/other"},"data":"{\"msgId\":\"m-3\"}"}"#,
        );
        let outcome = handle_frame(&down, &mut dedupe, 0);
        assert!(outcome.reply.is_some());
        assert!(outcome.dispatch.is_none());
    }

    #[test]
    fn callback_without_message_id_is_not_acked() {
        // TS :187：ACK 前置条件是 messageId 非空。
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let down = frame(
            r#"{"type":"CALLBACK","headers":{"topic":"/v1.0/im/bot/messages/get"},"data":"{\"msgId\":\"m-4\"}"}"#,
        );
        let outcome = handle_frame(&down, &mut dedupe, 0);
        assert!(outcome.reply.is_none());
        assert!(outcome.dispatch.is_some(), "派发不依赖 messageId");
    }

    #[test]
    fn callback_with_broken_data_payload_is_acked_but_ignored() {
        // data 解析失败：ACK 已发、不派发——TS :209-216 的 catch-return。
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let down = frame(
            r#"{"type":"CALLBACK","headers":{"messageId":"c5","topic":"/v1.0/im/bot/messages/get"},"data":"not-json"}"#,
        );
        let outcome = handle_frame(&down, &mut dedupe, 0);
        assert!(outcome.reply.is_some());
        assert!(outcome.dispatch.is_none());

        // msgId 缺失同样忽略。
        let down = frame(
            r#"{"type":"CALLBACK","headers":{"messageId":"c6","topic":"/v1.0/im/bot/messages/get"},"data":"{}"}"#,
        );
        let outcome = handle_frame(&down, &mut dedupe, 0);
        assert!(outcome.dispatch.is_none());
    }

    #[test]
    fn unparseable_text_is_ignored() {
        // 坏 JSON / 非对象：TS :150-158 直接 return。
        assert!(DownFrame::parse("not-json").is_none());
        assert!(DownFrame::parse("[1,2]").is_none());
        let mut dedupe = DedupeSet::with_windows(200, 1);
        let down = frame(r#"{"type":123,"headers":{}}"#);
        let outcome = handle_frame(&down, &mut dedupe, 0);
        assert_eq!(outcome, FrameOutcome::default());
    }
}
