//! M2 Task 5D：desktop-continuous 事件流的投影对拍。
//!
//! 夹具 `packages/desktop/tests/agent-runtime-oracle/fixtures/session-stream.json` 由
//! `record-session-stream.mts` 从 oracle 录制，只含 Rust 侧已产出的四类事件：
//! `turn.started` / `model.streaming` / `tool.updated` / `turn.completed`。
//! `session.updated` 与 `streamRecovery.updated` 不在投影内（差异已登记）。
//!
//! 投影规则与录制器逐条对应；路径统一换成 `${cwd}`（含 JSON 文本里被转义一次的形态）。

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::session::Session;
use agent_core::{SessionEvent, SessionEventType};
use agent_protocol::session_event::{
    ModelStreamingKind, ModelStreamingPayload, ToolUpdatedPayload, TurnCompletedPayload,
    TurnStartedPayload,
};
use agent_tools::test_support::fixture_dir;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mode-stream-parity-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn replace_cwd(text: &str, cwd: &str) -> String {
    let escaped = serde_json::to_string(cwd).expect("cwd 可序列化");
    let escaped = &escaped[1..escaped.len() - 1];
    text.replace(cwd, "${cwd}").replace(escaped, "${cwd}")
}

fn substitute(value: &Value, cwd: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("${cwd}", cwd)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| substitute(item, cwd)).collect())
        }
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, inner)| (key.clone(), substitute(inner, cwd)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn normalize(value: &Value, cwd: &str) -> Value {
    match value {
        Value::String(text) => Value::String(replace_cwd(text, cwd)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| normalize(item, cwd)).collect())
        }
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, inner)| (key.clone(), normalize(inner, cwd)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn payload<T: serde::de::DeserializeOwned>(event: &SessionEvent) -> Option<T> {
    event.payload_as::<T>().expect("载荷能解码")
}

fn tool_name(payload: &ToolUpdatedPayload) -> Option<String> {
    match payload {
        ToolUpdatedPayload::Scheduled(scheduled) => Some(scheduled.tool_name.clone()),
        ToolUpdatedPayload::Started(started) => started.tool_name.clone(),
        ToolUpdatedPayload::Progress(progress) => Some(progress.tool_name.clone()),
        ToolUpdatedPayload::Result(_)
        | ToolUpdatedPayload::Error(_)
        | ToolUpdatedPayload::Batch(_) => None,
    }
}

fn project(events: &[SessionEvent], cwd: &str) -> Vec<Value> {
    let mut projected = Vec::new();
    for event in events {
        let value = match event.event_type {
            SessionEventType::TurnStarted => {
                let Some(started) = payload::<TurnStartedPayload>(event) else {
                    continue;
                };
                json!({
                    "type": "turn.started",
                    "turnNumber": started.turn_number,
                    "input": started.input,
                })
            }
            SessionEventType::ModelStreaming => {
                let Some(streaming) = payload::<ModelStreamingPayload>(event) else {
                    continue;
                };
                let mut object = Map::new();
                object.insert("type".to_string(), json!("model.streaming"));
                object.insert(
                    "kind".to_string(),
                    serde_json::to_value(streaming.kind).expect("kind 可序列化"),
                );
                if matches!(
                    streaming.kind,
                    ModelStreamingKind::TextDelta | ModelStreamingKind::ToolInputDelta
                ) {
                    object.insert("delta".to_string(), json!(streaming.delta));
                }
                if streaming.kind == ModelStreamingKind::ToolCall {
                    object.insert("input".to_string(), json!(streaming.input));
                }
                if let Some(tool_call_id) = &streaming.tool_call_id {
                    object.insert("toolCallId".to_string(), json!(tool_call_id));
                }
                if let Some(tool_name) = &streaming.tool_name {
                    object.insert("toolName".to_string(), json!(tool_name));
                }
                normalize(&Value::Object(object), cwd)
            }
            SessionEventType::ToolUpdated => {
                let Some(updated) = payload::<ToolUpdatedPayload>(event) else {
                    continue;
                };
                let mut object = Map::new();
                object.insert("type".to_string(), json!("tool.updated"));
                object.insert(
                    "kind".to_string(),
                    serde_json::to_value(&updated).expect("载荷可序列化")["kind"].clone(),
                );
                if let Some(name) = tool_name(&updated) {
                    object.insert("toolName".to_string(), json!(name));
                }
                Value::Object(object)
            }
            SessionEventType::TurnCompleted => {
                let Some(completed) = payload::<TurnCompletedPayload>(event) else {
                    continue;
                };
                let usage = completed.usage.unwrap_or_else(|| json!({}));
                json!({
                    "type": "turn.completed",
                    "resultType": serde_json::to_value(completed.result_type).expect("可序列化"),
                    "tokenCount": completed.token_count,
                    "usage": {
                        "inputTokens": usage["inputTokens"],
                        "outputTokens": usage["outputTokens"],
                        "modelRequestCount": usage["modelRequestCount"],
                    },
                })
            }
            _ => continue,
        };
        projected.push(value);
    }
    projected
}

#[test]
fn desktop_continuous_stream_matches_the_recorded_projection() {
    let path = fixture_dir().join("session-stream.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "读不到夹具 {}：{error}（先跑 record-session-stream.mts）",
            path.display()
        )
    });
    let fixture: Value = serde_json::from_str(&raw).expect("夹具是合法 JSON");
    let cases = fixture["cases"].as_array().expect("cases 是数组");
    assert!(!cases.is_empty());

    for case in cases {
        let name = case["name"].as_str().expect("case 有 name");
        let dir = temp_dir(name);
        let cwd = dir.to_string_lossy().to_string();
        if let Some(seed) = case["seed"].as_object() {
            for (relative, content) in seed {
                let path = dir.join(relative);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).expect("建种子目录");
                }
                fs::write(&path, content.as_str().expect("种子是文本")).expect("写种子");
            }
        }

        let steps = case["steps"]
            .as_array()
            .expect("steps 是数组")
            .iter()
            .map(|step| {
                if let Some(text) = step["text"].as_str() {
                    return ScriptStep::Text(text.to_string());
                }
                let calls = step["toolCalls"]
                    .as_array()
                    .expect("step 要么是 text，要么是 toolCalls")
                    .iter()
                    .map(|call| ScriptToolCall {
                        name: call["name"].as_str().expect("工具有名字").to_string(),
                        input: substitute(&call["input"], &cwd),
                    })
                    .collect();
                ScriptStep::ToolCalls(calls)
            })
            .collect();
        let provider = ScriptedProvider::new(steps);
        let mut session = Session::new(format!("sess_stream_{name}"), cwd.clone());
        let mut sink = Vec::new();
        session
            .run_turn(
                case["prompt"].as_str().expect("case 有 prompt"),
                &provider,
                &mut sink,
            )
            .expect("turn 应成功");

        let actual = project(&sink, &cwd);
        assert_eq!(
            Value::Array(actual),
            case["expect"],
            "{name}：desktop-continuous 事件流与 oracle 投影不一致"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
