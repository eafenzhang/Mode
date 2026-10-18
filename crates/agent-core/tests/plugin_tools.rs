//! 插件工具注入（M3 生态域 1 · Task 3）：动态工具表 + 调用桥的占位映射。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use agent_core::plugin_tools::{PluginToolDeclaration, PluginToolSource};
use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::session::Session;
use agent_core::{SessionEvent, SessionEventType};
use agent_protocol::session_event::ToolUpdatedPayload;

struct FakeSource;

impl PluginToolSource for FakeSource {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String> {
        Ok(vec![
            PluginToolDeclaration {
                name: "plugin_echo".to_string(),
                description: "Echo text".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"],
                    "additionalProperties": false
                }),
            },
            PluginToolDeclaration {
                name: "plugin_fail".to_string(),
                description: "Always fails".to_string(),
                input_schema: json!({ "type": "object" }),
            },
        ])
    }

    fn call_tool(&self, tool_name: &str, input: &Value) -> Result<Value, String> {
        match tool_name {
            "plugin_echo" => Ok(json!({
                "content": input.get("text").and_then(Value::as_str).unwrap_or("")
            })),
            "plugin_fail" => Err("boom".to_string()),
            other => Err(format!("Tool not found: {other}")),
        }
    }
}

struct FailingSource;

impl PluginToolSource for FailingSource {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String> {
        Err("sidecar unavailable".to_string())
    }
    fn call_tool(&self, _tool_name: &str, _input: &Value) -> Result<Value, String> {
        Err("unused".to_string())
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-plugin-tools-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn provider(calls: Vec<ScriptToolCall>) -> ScriptedProvider {
    ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(calls),
        ScriptStep::Text("done".to_string()),
    ])
}

fn tool_result(events: &[SessionEvent]) -> (bool, String) {
    for event in events {
        if event.event_type != SessionEventType::ToolUpdated {
            continue;
        }
        let Some(payload) = event.payload_as::<ToolUpdatedPayload>().expect("能解码") else {
            continue;
        };
        match payload {
            ToolUpdatedPayload::Result(result) => {
                return (
                    true,
                    result
                        .result
                        .content
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default(),
                );
            }
            ToolUpdatedPayload::Error(error) => return (false, error.error.message),
            _ => {}
        }
    }
    panic!("没有工具结果");
}

#[test]
fn plugin_tools_are_injected_and_invoked() {
    let dir = temp_dir("invoke");
    let mut session = Session::new("sess_plugin", dir.to_string_lossy().to_string())
        .with_plugin_tools(Arc::new(FakeSource))
        .expect("listTools 应成功");

    let tools = session.tools();
    let echo = tools
        .iter()
        .find(|spec| spec.name == "plugin_echo")
        .expect("插件工具应出现在会话工具表里");
    assert_eq!(echo.schema["required"][0], "text", "schema 应逐字保留");

    let provider = provider(vec![ScriptToolCall {
        name: "plugin_echo".to_string(),
        input: json!({ "text": "hello" }),
    }]);
    let mut sink = Vec::new();
    session
        .run_turn("use plugin", &provider, &mut sink)
        .expect("turn 应成功");

    let (ok, content) = tool_result(&sink);
    assert!(ok);
    assert_eq!(content, "hello");
    assert_eq!(
        sink.iter()
            .filter(|event| event.event_type == SessionEventType::PermissionRequested)
            .count(),
        0,
        "占位 family 目前判 Allow；Task 4 观察后再收紧"
    );
}

#[test]
fn plugin_tool_errors_map_to_tool_execution_failed() {
    let dir = temp_dir("fail");
    let mut session = Session::new("sess_plugin", dir.to_string_lossy().to_string())
        .with_plugin_tools(Arc::new(FakeSource))
        .expect("listTools");
    let provider = provider(vec![ScriptToolCall {
        name: "plugin_fail".to_string(),
        input: json!({}),
    }]);
    let mut sink = Vec::new();
    session
        .run_turn("fail", &provider, &mut sink)
        .expect("turn");

    let (ok, message) = tool_result(&sink);
    assert!(!ok);
    assert_eq!(message, "boom");
}

#[test]
fn unknown_plugin_names_stay_tool_not_found() {
    let dir = temp_dir("unknown");
    let mut session = Session::new("sess_plugin", dir.to_string_lossy().to_string())
        .with_plugin_tools(Arc::new(FakeSource))
        .expect("listTools");
    let provider = provider(vec![ScriptToolCall {
        name: "plugin_missing".to_string(),
        input: json!({}),
    }]);
    let mut sink = Vec::new();
    session
        .run_turn("missing", &provider, &mut sink)
        .expect("turn");

    let (ok, message) = tool_result(&sink);
    assert!(!ok);
    assert_eq!(message, "Tool not found: plugin_missing");
}

#[test]
fn list_tools_failure_surfaces_to_the_caller() {
    let dir = temp_dir("list-fail");
    let error = match Session::new("sess_plugin", dir.to_string_lossy().to_string())
        .with_plugin_tools(Arc::new(FailingSource))
    {
        Ok(_) => panic!("listTools 失败应返回错误"),
        Err(error) => error,
    };
    assert_eq!(error, "sidecar unavailable");
}
