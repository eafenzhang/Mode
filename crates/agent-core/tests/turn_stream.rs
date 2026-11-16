//! 流式 turn 循环（M2 Task 5A）：事件顺序与终态统计。
//!
//! 期望顺序来自 `docs/specs/tauri-agent-runtime.md` §3.5 的实测；本用例只比
//! `turn.started` / `model.streaming` / `tool.updated` / `turn.completed`——
//! oracle 的 `session.updated` 与 `streamRecovery.updated` 尚未产出，差异已登记。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, Notify};

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
    ProviderError, ScriptStep, ScriptToolCall, ScriptedProvider,
};
use agent_core::session::Session;
use agent_core::CancelToken;
use agent_core::{DeliveryKind, EventSink, SessionEvent, SessionEventType};
use agent_protocol::session_event::{
    ModelStreamingKind, ModelStreamingPayload, TurnCompletedPayload,
};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mode-turn-stream-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn event_kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

fn completed(events: &[SessionEvent]) -> TurnCompletedPayload {
    events
        .iter()
        .find(|event| event.event_type == SessionEventType::TurnCompleted)
        .and_then(|event| event.payload_as::<TurnCompletedPayload>().expect("能解码"))
        .expect("应有 turn.completed")
}

#[tokio::test]
async fn events_carry_the_requested_delivery_kind() {
    let dir = temp_dir("delivery");
    let provider = ScriptedProvider::new(vec![ScriptStep::Text("hi".to_string())]);
    let mut session = Session::new("sess_stream", dir.to_string_lossy().to_string())
        .with_delivery_kind(DeliveryKind::WebRemoteReplayable);
    let mut sink = Vec::new();

    session
        .run_turn("probe delivery", &provider, &mut sink, &CancelToken::new())
        .await
        .expect("turn 应成功");
    assert!(!sink.is_empty());
    assert!(
        sink.iter()
            .all(|event| event.delivery_kind == Some(DeliveryKind::WebRemoteReplayable)),
        "每个事件都应带请求的 deliveryKind"
    );

    let _ = fs::remove_dir_all(&dir);
}

fn streaming(events: &[SessionEvent]) -> Vec<ModelStreamingPayload> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ModelStreaming)
        .filter_map(|event| event.payload_as::<ModelStreamingPayload>().expect("能解码"))
        .collect()
}

#[tokio::test]
async fn text_turn_streams_the_model_and_completes() {
    let dir = temp_dir("text");
    let provider = ScriptedProvider::new(vec![ScriptStep::Text("hello from mock".to_string())]);
    let mut session = Session::new("sess_stream", dir.to_string_lossy().to_string());
    let mut sink = Vec::new();

    session
        .run_turn("probe text turn", &provider, &mut sink, &CancelToken::new())
        .await
        .expect("turn 应成功");

    assert_eq!(
        event_kinds(&sink),
        vec!["turn.started", "model.streaming", "turn.completed"]
    );
    let streaming = streaming(&sink);
    assert_eq!(streaming.len(), 1);
    assert_eq!(streaming[0].kind, ModelStreamingKind::TextDelta);
    assert_eq!(streaming[0].delta.as_deref(), Some("hello from mock"));

    let payload = completed(&sink);
    assert_eq!(payload.response, "hello from mock");
    assert_eq!(payload.token_count, 22, "10 input + 12 output");
    assert_eq!(payload.tool_call_count, 0);
    assert_eq!(payload.history_round_count, Some(1));
    let usage = payload.usage.expect("应有 usage");
    assert_eq!(usage["inputTokens"], json!(10));
    assert_eq!(usage["outputTokens"], json!(12));
    assert_eq!(usage["modelRequestCount"], json!(1));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn tool_turn_interleaves_model_streaming_and_tool_events() {
    let dir = temp_dir("tool");
    fs::write(dir.join("hello.txt"), "hello\n").expect("写种子文件");
    let path = dir.join("hello.txt");
    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Read".to_string(),
            input: json!({ "file_path": path.to_string_lossy() }),
        }]),
        ScriptStep::Text("tool turn done".to_string()),
    ]);
    let mut session = Session::new("sess_stream", dir.to_string_lossy().to_string());
    let mut sink = Vec::new();

    session
        .run_turn("probe tool turn", &provider, &mut sink, &CancelToken::new())
        .await
        .expect("turn 应成功");

    assert_eq!(
        event_kinds(&sink),
        vec![
            "turn.started",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "model.streaming",
            "turn.completed",
        ]
    );

    let streaming_kinds: Vec<ModelStreamingKind> = streaming(&sink)
        .into_iter()
        .map(|payload| payload.kind)
        .collect();
    assert_eq!(
        streaming_kinds,
        vec![
            ModelStreamingKind::ToolInputStart,
            ModelStreamingKind::ToolInputDelta,
            ModelStreamingKind::ToolInputEnd,
            ModelStreamingKind::ToolCall,
            ModelStreamingKind::TextDelta,
        ]
    );

    let payload = completed(&sink);
    assert_eq!(payload.response, "tool turn done");
    assert_eq!(payload.token_count, 52, "两轮模型请求合计");
    assert_eq!(payload.tool_call_count, 1);
    assert_eq!(payload.history_round_count, Some(2));
    assert_eq!(
        payload.usage.expect("应有 usage")["modelRequestCount"],
        json!(2)
    );

    // 历史：User → Assistant(tool_calls) → ToolResult → Assistant(text)
    assert_eq!(session.messages().len(), 4);

    let _ = fs::remove_dir_all(&dir);
}

/// 把事件转给测试侧的 sink，让用例能在 `run_turn` future 尚未完成时观察事件。
struct ChannelSink(mpsc::UnboundedSender<SessionEvent>);

impl EventSink for ChannelSink {
    fn emit(&mut self, event: SessionEvent) {
        let _ = self.0.send(event);
    }
}

/// 发一条 delta 后卡在 `release` 上，直到测试确认已经看到 `model.streaming`。
struct GatedProvider {
    release: Arc<Notify>,
}

impl ModelProvider for GatedProvider {
    fn stream(
        &self,
        _request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            deltas
                .send(ModelDelta::Text {
                    text: "incremental".to_string(),
                })
                .expect("会话应在 provider 返回前保持 delta 通道打开");
            release.notified().await;
            Ok(ModelResponse {
                usage: ModelUsage {
                    input_tokens: 3,
                    output_tokens: 5,
                    ..ModelUsage::default()
                },
                stop_reason: "stop",
            })
        })
    }
}

/// 门控用例的失败上限：事件不按增量到达时用例应失败，而不是挂住测试进程。
/// 它不是正确性证明——证明仍是 `release` 只在观察到事件之后才放行。
async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("门控超时：model.streaming 没有在 provider 返回前到达")
}

/// delta 到达即发事件：provider future 还卡在信号上时，sink 已经能看到
/// `model.streaming`（不是收齐后再补发）。
#[tokio::test]
async fn model_streaming_is_emitted_before_the_provider_returns() {
    let dir = temp_dir("incremental");
    let release = Arc::new(Notify::new());
    let provider = GatedProvider {
        release: Arc::clone(&release),
    };
    let mut session = Session::new("sess_stream", dir.to_string_lossy().to_string());
    let (events, mut received) = mpsc::unbounded_channel();
    let mut sink = ChannelSink(events);
    let cancel = CancelToken::new();
    let mut turn = Box::pin(session.run_turn("probe incremental", &provider, &mut sink, &cancel));

    let started = within(async {
        tokio::select! {
            event = received.recv() => event.expect("turn.started 应先到达"),
            result = &mut turn => panic!("turn 在首个事件前结束：{result:?}"),
        }
    })
    .await;
    assert_eq!(started.event_type, SessionEventType::TurnStarted);

    let streaming = within(async {
        tokio::select! {
            event = received.recv() => event.expect("model.streaming 应增量到达"),
            result = &mut turn => panic!("provider 尚未放行，turn 不应结束：{result:?}"),
        }
    })
    .await;
    assert_eq!(streaming.event_type, SessionEventType::ModelStreaming);
    let payload = streaming
        .payload_as::<ModelStreamingPayload>()
        .expect("能解码")
        .expect("类型匹配");
    assert_eq!(payload.kind, ModelStreamingKind::TextDelta);
    assert_eq!(payload.delta.as_deref(), Some("incremental"));

    // 只有已经观察到 model.streaming 之后才放行 provider。
    release.notify_one();
    within(&mut turn).await.expect("turn 应成功");

    let mut events = vec![started, streaming];
    while let Ok(event) = received.try_recv() {
        events.push(event);
    }
    assert_eq!(
        event_kinds(&events),
        vec!["turn.started", "model.streaming", "turn.completed"]
    );
    let payload = completed(&events);
    assert_eq!(payload.response, "incremental");
    assert_eq!(payload.token_count, 8);
    assert_eq!(payload.history_round_count, Some(1));

    let _ = fs::remove_dir_all(&dir);
}
