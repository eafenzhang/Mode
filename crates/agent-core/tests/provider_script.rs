//! 脚本化 `ModelProvider` 的确定性（M2 Task 5A）。
//!
//! 口径（`docs/plans/2026-10-18-m2-session-stream.md` DR-T5-2）：下标 = 请求里
//! `Assistant` 消息的条数；同一请求两次运行必须逐字相同。

use serde_json::json;

use agent_core::provider::{
    ModelDelta, ModelMessage, ModelProvider, ModelRequest, ScriptStep, ScriptToolCall,
    ScriptedProvider,
};

fn request(assistant_messages: usize) -> ModelRequest {
    let mut messages = vec![ModelMessage::User("prompt".to_string())];
    for index in 0..assistant_messages {
        messages.push(ModelMessage::Assistant {
            text: Some(format!("answer {index}")),
            tool_calls: Vec::new(),
        });
    }
    ModelRequest {
        messages,
        tools: vec!["Read".to_string(), "Bash".to_string()],
    }
}

fn provider() -> ScriptedProvider {
    ScriptedProvider::new(vec![
        ScriptStep::Text("hello from mock".to_string()),
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Read".to_string(),
            input: json!({ "file_path": "hello.txt" }),
        }]),
    ])
}

#[test]
fn selects_step_by_assistant_message_count() {
    let provider = provider();
    let first = provider.stream(&request(0)).expect("第一步");
    assert_eq!(first.stop_reason, "stop");
    assert_eq!(
        first.deltas,
        vec![ModelDelta::Text {
            text: "hello from mock".to_string()
        }]
    );

    let second = provider.stream(&request(1)).expect("第二步");
    assert_eq!(second.stop_reason, "tool-calls");
    assert_eq!(second.deltas.len(), 4);
}

#[test]
fn tool_call_deltas_follow_the_observed_order() {
    let response = provider().stream(&request(1)).expect("工具步");
    let kinds: Vec<&str> = response
        .deltas
        .iter()
        .map(|delta| match delta {
            ModelDelta::Text { .. } => "text",
            ModelDelta::ToolInputStart { .. } => "tool_input_start",
            ModelDelta::ToolInputDelta { .. } => "tool_input_delta",
            ModelDelta::ToolInputEnd { .. } => "tool_input_end",
            ModelDelta::ToolCall { .. } => "tool_call",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "tool_input_start",
            "tool_input_delta",
            "tool_input_end",
            "tool_call"
        ]
    );

    let ModelDelta::ToolCall {
        tool_call_id,
        tool_name,
        input,
    } = &response.deltas[3]
    else {
        panic!("最后一步应是 tool_call");
    };
    assert_eq!(tool_call_id, "toolu_mock_1_0");
    assert_eq!(tool_name, "Read");
    assert_eq!(input, &json!({ "file_path": "hello.txt" }));
}

#[test]
fn the_same_request_is_deterministic() {
    let provider = provider();
    let first = provider.stream(&request(1)).expect("第一次");
    let second = provider.stream(&request(1)).expect("第二次");
    assert_eq!(first, second);
}

#[test]
fn empty_script_is_an_error_not_an_empty_response() {
    let provider = ScriptedProvider::new(Vec::new());
    let error = provider.stream(&request(0)).expect_err("空剧本必须失败");
    assert!(error.message.contains("剧本为空"));
}
