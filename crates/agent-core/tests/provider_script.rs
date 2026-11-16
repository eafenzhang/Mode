//! 脚本化 `ModelProvider` 的确定性（M2 Task 5A；LT-1 Task 3 改为异步流）。
//!
//! 口径（`docs/plans/2026-10-18-m2-session-stream.md` DR-T5-2）：下标 = 请求里
//! `Assistant` 消息的条数；同一请求两次运行必须逐字相同。
//!
//! delta 走 `DeltaSink` 增量送达；`ModelResponse` 只承载终态统计与停止原因。

use serde_json::json;
use tokio::sync::mpsc::unbounded_channel;

use agent_core::provider::{
    ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse, ScriptStep,
    ScriptToolCall, ScriptedProvider,
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

/// 跑一次剧本，按到达顺序收齐 sink 里的 delta。
async fn run(
    provider: &ScriptedProvider,
    request: ModelRequest,
) -> (Vec<ModelDelta>, ModelResponse) {
    let (sender, mut receiver) = unbounded_channel();
    let response = provider.stream(request, sender).await.expect("剧本应成功");
    let mut deltas = Vec::new();
    while let Ok(delta) = receiver.try_recv() {
        deltas.push(delta);
    }
    (deltas, response)
}

#[tokio::test]
async fn selects_step_by_assistant_message_count() {
    let provider = provider();
    let (deltas, first) = run(&provider, request(0)).await;
    assert_eq!(first.stop_reason, "stop");
    assert_eq!(
        deltas,
        vec![ModelDelta::Text {
            text: "hello from mock".to_string()
        }]
    );
    assert_eq!(first.usage.input_tokens, 10);
    assert_eq!(first.usage.output_tokens, 12);

    let (deltas, second) = run(&provider, request(1)).await;
    assert_eq!(second.stop_reason, "tool-calls");
    assert_eq!(deltas.len(), 4);
    assert_eq!(second.usage.input_tokens, 10);
    assert_eq!(second.usage.output_tokens, 20);
}

#[tokio::test]
async fn tool_call_deltas_follow_the_observed_order() {
    let (deltas, _) = run(&provider(), request(1)).await;
    let kinds: Vec<&str> = deltas
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
    } = &deltas[3]
    else {
        panic!("最后一步应是 tool_call");
    };
    assert_eq!(tool_call_id, "toolu_mock_1_0");
    assert_eq!(tool_name, "Read");
    assert_eq!(input, &json!({ "file_path": "hello.txt" }));
}

#[tokio::test]
async fn the_same_request_is_deterministic() {
    let provider = provider();
    let (first_deltas, first) = run(&provider, request(1)).await;
    let (second_deltas, second) = run(&provider, request(1)).await;
    assert_eq!(first_deltas, second_deltas);
    assert_eq!(first, second);
}

#[tokio::test]
async fn empty_script_is_an_error_not_an_empty_response() {
    let provider = ScriptedProvider::new(Vec::new());
    let (sender, mut receiver) = unbounded_channel();
    let error = provider
        .stream(request(0), sender)
        .await
        .expect_err("空剧本必须失败");
    assert!(error.message.contains("剧本为空"));
    assert!(receiver.try_recv().is_err(), "失败时不应发出 delta");
}
