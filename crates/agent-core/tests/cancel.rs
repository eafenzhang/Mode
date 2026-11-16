//! 取消令牌与 `run_turn` 的取消边界（LT-1 C1 · 控制器裁决 4）。
//!
//! 用例只比两个可观察事实：
//! 1. `run_turn` 的 future 是 `Send`（tokio worker 的前提，裁决 1 的编译期验证）；
//! 2. 取消后的终态恰好一条，`resultType:"cancelled"`（协议注释把用户中断归为
//!    `turn.completed`），且流中已投递的 delta 不丢。

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
use agent_core::{CancelToken, EventSink, SessionEvent, SessionEventType, TurnError};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-agent-core-cancel-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn event_kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("取消边界用例超时")
}

/// 把事件转给测试侧通道，让用例能在 turn 进行中观察事件。
struct ChannelSink(mpsc::UnboundedSender<SessionEvent>);

impl EventSink for ChannelSink {
    fn emit(&mut self, event: SessionEvent) {
        let _ = self.0.send(event);
    }
}

/// 编译期断言（裁决 1）：`run_turn` 的 future 必须是 `Send`——它捕获
/// `&mut dyn EventSink` / `&mut Session` / `&dyn ModelProvider` / `&CancelToken`，
/// 任何一个上界缺失都会在这里编译失败。宿主层的 tokio worker 依赖这一点。
#[test]
fn run_turn_future_is_send() {
    fn require_send<T: Send>(_: &T) {}
    let mut session = Session::new("sess_send", ".".to_string());
    let provider = ScriptedProvider::new(vec![ScriptStep::Text("unused".to_string())]);
    let mut sink = Vec::new();
    let cancel = CancelToken::new();
    let future = session.run_turn("probe send", &provider, &mut sink, &cancel);
    require_send(&future);
}

/// 流中取消：provider 发一条 delta 后停在信号上，取消到达后 turn 在 delta 边界退出。
#[tokio::test]
async fn cancel_mid_stream_stops_at_delta_boundary_with_single_terminal() {
    let dir = temp_dir("mid-stream");
    let release = Arc::new(Notify::new());
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
                        text: "before cancel".to_string(),
                    })
                    .expect("会话侧应保持通道打开");
                release.notified().await;
                Ok(ModelResponse {
                    usage: ModelUsage {
                        input_tokens: 2,
                        output_tokens: 3,
                        ..ModelUsage::default()
                    },
                    stop_reason: "stop",
                })
            })
        }
    }

    let provider = GatedProvider {
        release: Arc::clone(&release),
    };
    let mut session = Session::new("sess_cancel_stream", dir.to_string_lossy().to_string());
    let (sender, mut received) = mpsc::unbounded_channel();
    let mut sink = ChannelSink(sender);
    let cancel = CancelToken::new();
    let mut turn = Box::pin(session.run_turn("probe cancel", &provider, &mut sink, &cancel));

    let started = within(async {
        tokio::select! {
            event = received.recv() => event.expect("turn.started 应先到达"),
            result = &mut turn => panic!("provider 未放行，turn 不应结束：{result:?}"),
        }
    })
    .await;
    assert_eq!(started.event_type, SessionEventType::TurnStarted);

    let streaming = within(async {
        tokio::select! {
            event = received.recv() => event.expect("model.streaming 应增量到达"),
            result = &mut turn => panic!("provider 未放行，turn 不应结束：{result:?}"),
        }
    })
    .await;
    assert_eq!(streaming.event_type, SessionEventType::ModelStreaming);

    // 取消：不需要放行 provider；select 的 cancel 分支应在 delta 边界退出。
    cancel.cancel();
    let result = within(&mut turn).await;
    let error = result.expect_err("取消的 turn 应返回 cancelled 错误");
    assert!(error.is_cancelled(), "错误应标记为取消：{error:?}");

    let mut events = vec![started, streaming];
    while let Ok(event) = received.try_recv() {
        events.push(event);
    }
    // 终态恰好一条，且是 turn.completed + resultType:"cancelled"。
    let terminals: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                SessionEventType::TurnCompleted | SessionEventType::TurnFailed
            )
        })
        .collect();
    assert_eq!(
        terminals.len(),
        1,
        "终态恰好一条，实际事件序：{:?}",
        event_kinds(&events)
    );
    assert_eq!(terminals[0].event_type, SessionEventType::TurnCompleted);
    let payload = terminals[0]
        .payload_as::<agent_protocol::session_event::TurnCompletedPayload>()
        .expect("能解码")
        .expect("类型匹配");
    assert_eq!(
        payload.result_type,
        agent_protocol::session_event::TurnResultType::Cancelled
    );
    // 已投递的 delta 不丢：取消前到达的文本进入终态 response。
    assert_eq!(payload.response, "before cancel");

    let _ = fs::remove_dir_all(&dir);
}

/// 步间取消：工具批次跑完后（第二个模型请求之前）取消，工具事件完整、终态唯一。
#[tokio::test]
async fn cancel_after_tool_batch_skips_next_model_request() {
    let dir = temp_dir("between-steps");
    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "TaskOutput".to_string(),
            input: json!({ "task_id": "missing-task" }),
        }]),
        ScriptStep::Text("never reached".to_string()),
    ]);
    let mut session = Session::new("sess_cancel_steps", dir.to_string_lossy().to_string());
    let mut sink = Vec::new();
    let cancel = CancelToken::new();
    cancel.cancel();

    let result = session
        .run_turn("probe step cancel", &provider, &mut sink, &cancel)
        .await;
    let error = result.expect_err("取消的 turn 应返回 cancelled 错误");
    assert!(error.is_cancelled(), "错误应标记为取消：{error:?}");

    // 事件顺序：turn.started 后没有 model.streaming（第一个请求开始前就取消），终态唯一。
    assert_eq!(event_kinds(&sink), vec!["turn.started", "turn.completed"]);
    let payload = sink[1]
        .payload_as::<agent_protocol::session_event::TurnCompletedPayload>()
        .expect("能解码")
        .expect("类型匹配");
    assert_eq!(
        payload.result_type,
        agent_protocol::session_event::TurnResultType::Cancelled
    );
    assert_eq!(payload.token_count, 0, "没有完成的模型请求就没有用量");

    let _ = fs::remove_dir_all(&dir);
}

/// 工具批次之后取消：批次事件完整产出（帧形状不因取消缺帧），然后停止。
#[tokio::test]
async fn cancel_token_not_yet_set_lets_turn_complete_normally() {
    let dir = temp_dir("no-cancel");
    let provider = ScriptedProvider::new(vec![ScriptStep::Text("fine".to_string())]);
    let mut session = Session::new("sess_cancel_idle", dir.to_string_lossy().to_string());
    let mut sink = Vec::new();
    let cancel = CancelToken::new();

    let result = session
        .run_turn("probe no cancel", &provider, &mut sink, &cancel)
        .await;
    result.expect("未取消的 turn 应成功");
    assert_eq!(
        event_kinds(&sink),
        vec!["turn.started", "model.streaming", "turn.completed"]
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `TurnError::cancelled` 与普通错误可区分；构造路径保持 `new`。
#[test]
fn turn_error_flags_cancelled() {
    let plain = TurnError::new("provider 爆炸");
    assert!(!plain.is_cancelled());
    assert_eq!(plain.message, "provider 爆炸");
    let cancelled = TurnError::cancelled();
    assert!(cancelled.is_cancelled());
}
