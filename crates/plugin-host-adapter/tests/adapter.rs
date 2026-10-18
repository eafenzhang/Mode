//! sidecar → `PluginToolSource` 的端到端映射（M3 生态域 1 · Task 4 Step 1）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use agent_core::plugin_tools::PluginToolSource;
use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::session::Session;
use agent_core::{SessionEvent, SessionEventType};
use agent_protocol::session_event::ToolUpdatedPayload;
use plugin_host::supervisor::{SidecarSupervisor, SupervisorConfig};
use plugin_host_adapter::PluginHostSource;

const FAKE_SIDECAR: &str = r#"
const rl = require('node:readline').createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const frame = JSON.parse(line);
  if (frame.method === 'plugin/listTools') {
    process.stdout.write(JSON.stringify({ id: frame.id, result: { tools: [{
      name: 'echo', description: 'Echo text',
      inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] }
    }] } }) + '\n');
    return;
  }
  if (frame.method === 'plugin/callTool') {
    const toolName = frame.params && frame.params.toolName;
    if (toolName !== 'echo') {
      process.stdout.write(JSON.stringify({ id: frame.id, error: { code: -32601, message: 'Tool not found: ' + toolName } }) + '\n');
      return;
    }
    const input = (frame.params && frame.params.input) || {};
    process.stdout.write(JSON.stringify({ id: frame.id, result: { content: String(input.text || '') } }) + '\n');
    return;
  }
  process.stdout.write(JSON.stringify({ id: frame.id, result: {} }) + '\n');
});
"#;

fn source() -> PluginHostSource {
    let config = SupervisorConfig {
        program: PathBuf::from("node"),
        args: vec!["-e".to_string(), FAKE_SIDECAR.to_string()],
        request_timeout: Duration::from_secs(2),
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
    };
    let supervisor = Arc::new(SidecarSupervisor::start(config).expect("启动 sidecar"));
    PluginHostSource::new(supervisor)
}

#[test]
fn list_tools_maps_the_sidecar_declarations() {
    let source = source();
    let tools = source.list_tools().expect("listTools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Echo text");
    assert_eq!(tools[0].input_schema["required"][0], "text");
}

fn tool_result(events: &[SessionEvent]) -> String {
    for event in events {
        if event.event_type != SessionEventType::ToolUpdated {
            continue;
        }
        let Some(payload) = event.payload_as::<ToolUpdatedPayload>().expect("能解码") else {
            continue;
        };
        if let ToolUpdatedPayload::Result(result) = payload {
            return result
                .result
                .content
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
        }
    }
    panic!("没有工具结果");
}

#[test]
fn session_runs_a_plugin_tool_end_to_end() {
    let dir = std::env::temp_dir().join(format!("mode-plugin-adapter-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let mut session = Session::new("sess_adapter", dir.to_string_lossy().to_string())
        .with_plugin_tools(Arc::new(source()))
        .expect("listTools");

    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "echo".to_string(),
            input: json!({ "text": "e2e" }),
        }]),
        ScriptStep::Text("done".to_string()),
    ]);
    let mut sink = Vec::new();
    session
        .run_turn("run plugin tool", &provider, &mut sink)
        .expect("turn 应成功");
    assert_eq!(tool_result(&sink), "e2e");
}

#[test]
fn call_tool_returns_the_sidecar_content_and_maps_errors() {
    let source = source();
    let result = source
        .call_tool("echo", &json!({ "text": "hello" }))
        .expect("callTool");
    assert_eq!(result["content"], "hello");

    let error = source
        .call_tool("nope", &json!({}))
        .expect_err("未知工具应失败");
    assert!(error.contains("Tool not found: nope"), "实际：{error}");
}
