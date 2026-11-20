//! Agent JSON-RPC 通知 → TUI 归约（不建立连接，纯解析）。
//!
//! 只认 `v4/conversation/frame` 的 sessions-index 完整帧；分片重组留给后续
//! （与 Host 消费方同一纪律：分片不在这里重新发明）。

use agent_protocol::v4_sessions_index::{SessionsIndexDelta, SessionsIndexSnapshot};
use serde_json::Value;

use crate::agent_events::{apply_delta, apply_snapshot};
use crate::model::TuiModel;

pub const SESSIONS_INDEX_TOPIC_PREFIX: &str = "sessions-index/";
pub const V4_CONVERSATION_FRAME_METHOD: &str = "v4/conversation/frame";

/// 返回是否属于本模块处理的协议帧（便于调用方决定是否再走别的归约）。
pub fn apply_agent_notification(model: &mut TuiModel, value: &Value, now_ms: u64) -> bool {
    if value.get("method").and_then(Value::as_str) != Some(V4_CONVERSATION_FRAME_METHOD) {
        return false;
    }
    let Some(params) = value.get("params") else {
        return true;
    };
    if params.get("kind").and_then(Value::as_str) != Some("complete") {
        return true;
    }
    let Some(frame) = params.get("frame") else {
        return true;
    };
    let topic = frame
        .get("topic")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !topic.starts_with(SESSIONS_INDEX_TOPIC_PREFIX) {
        return true;
    }
    let Some(payload) = frame.get("payload") else {
        return true;
    };
    match payload.get("kind").and_then(Value::as_str) {
        Some("snapshot") => {
            if let Some(raw) = payload.get("snapshot") {
                if let Ok(snapshot) = serde_json::from_value::<SessionsIndexSnapshot>(raw.clone()) {
                    apply_snapshot(model, &snapshot, now_ms);
                }
            }
        }
        Some("deltas") => {
            if let Some(raw) = payload.get("deltas") {
                if let Ok(deltas) = serde_json::from_value::<Vec<SessionsIndexDelta>>(raw.clone()) {
                    for delta in &deltas {
                        apply_delta(model, delta, now_ms);
                    }
                }
            }
        }
        _ => {}
    }
    true
}

/// 反向请求（id 为字符串）需要宿主应答，否则 agent-server 会等待到超时。
pub fn is_reverse_request(value: &Value) -> bool {
    value.get("id").and_then(Value::as_str).is_some() && value.get("method").is_some()
}

/// TUI 是只读客户端：偏好统一回 `memoryEnabled:false`（与 agent-server 夹具的宿主口径一致）。
pub fn reverse_request_reply(value: &Value) -> Option<Value> {
    if !is_reverse_request(value) {
        return None;
    }
    Some(serde_json::json!({
        "id": value.get("id").cloned().unwrap_or(Value::Null),
        "result": { "memoryEnabled": false },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame_payload(payload: Value) -> Value {
        json!({
            "method": V4_CONVERSATION_FRAME_METHOD,
            "params": {
                "wireVersion": 1,
                "kind": "complete",
                "deliveryKind": "initial",
                "topic": "sessions-index/ws",
                "subscriptionId": "six-1",
                "frame": {
                    "topic": "sessions-index/ws",
                    "subscriptionId": "six-1",
                    "payload": payload,
                },
            },
        })
    }

    fn session(id: &str, title: &str, phase: &str, updated: u64) -> Value {
        json!({
            "sessionId": id,
            "workspaceId": "ws",
            "title": title,
            "phase": phase,
            "sessionEnded": false,
            "hasBackgroundWork": false,
            "lastActivityAt": updated,
            "createdAt": 1,
        })
    }

    #[test]
    fn snapshot_and_delta_update_the_model() {
        let mut model = TuiModel::new("ws");
        let snapshot = frame_payload(json!({
            "kind": "snapshot",
            "snapshot": {
                "protocolVersion": 1,
                "workspaceId": "ws",
                "logEpoch": "e",
                "sessions": [session("a", "任务 A", "running", 10), session("b", "任务 B", "draft", 20)],
            },
        }));
        assert!(apply_agent_notification(&mut model, &snapshot, 30_000));
        assert_eq!(model.sessions.len(), 2);
        assert_eq!(model.sessions[0].id, "b");

        let delta = frame_payload(json!({
            "kind": "deltas",
            "deltas": [
                { "op": "session.upserted", "session": session("a", "任务 A2", "completedSuccess", 40) },
                { "op": "session.removed", "sessionId": "b" },
            ],
        }));
        assert!(apply_agent_notification(&mut model, &delta, 50_000));
        assert_eq!(model.sessions.len(), 1);
        assert_eq!(model.sessions[0].id, "a");
        assert_eq!(model.sessions[0].title, "任务 A2");
        assert_eq!(model.sessions[0].phase, crate::model::SessionPhase::Done);
    }

    #[test]
    fn ignores_other_topics_and_fragment_frames() {
        let mut model = TuiModel::new("ws");
        assert!(!apply_agent_notification(
            &mut model,
            &json!({ "method": "startup/storageState", "params": {} }),
            0
        ));
        assert!(apply_agent_notification(
            &mut model,
            &json!({
                "method": V4_CONVERSATION_FRAME_METHOD,
                "params": { "kind": "complete", "frame": { "topic": "conversation/x", "payload": {} } },
            }),
            0
        ));
        assert!(model.sessions.is_empty());
    }

    #[test]
    fn reverse_requests_get_a_minimal_reply() {
        let request = json!({ "id": "server-1", "method": "interaction/requestPreferences" });
        assert!(is_reverse_request(&request));
        assert_eq!(
            reverse_request_reply(&request).expect("reply"),
            json!({ "id": "server-1", "result": { "memoryEnabled": false } })
        );
        assert!(reverse_request_reply(&json!({ "id": 1, "result": {} })).is_none());
    }
}
