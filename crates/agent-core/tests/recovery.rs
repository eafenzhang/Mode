//! C3 T2：流恢复与重试（`agent-core` 层）。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §2 与 T0 夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c3/retry-*.json`
//! （只读，不修改）。覆盖：
//!
//! - HTTP 5xx：`model_retry_scheduled`（apiRetry 状态）→ 第二次尝试成功；
//! - 流中断（已提交 delta）：4 条 `streamRecovery.updated`、立即重试、被丢弃
//!   delta 不进入最终消息、事件不重复；
//! - 401：非 retryable，只请求一次，错误携带状态/分类；
//! - 退避期间取消：不再发起第二次尝试，终态 cancelled 恰好一次；
//! - 重试耗尽：按观察到的 `maxRetries:10`（11 次尝试）停止（耗尽形状未观察到，
//!   只断言不产生第二个终态/不重复 delta）。
//!
//! 时间：`#[tokio::test(start_paused = true)]` 让退避自动推进，不用 sleep 作为
//! 正确性证明；provider 是进程内脚本，无真实网络。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
    ProviderError,
};
use agent_core::session::Session;
use agent_core::stream::EventSink;
use agent_core::{CancelToken, RetryPolicy, SessionEvent, SessionEventType};

// ---------------------------------------------------------------------------
// 脚本化 provider
// ---------------------------------------------------------------------------

enum Attempt {
    /// 非 2xx HTTP 应答（无 delta）。
    Http { status: u16, message: &'static str },
    /// 已提交若干 delta 后传输中断（stream recovery 路径）。
    AbortAfter {
        deltas: Vec<ModelDelta>,
        message: &'static str,
    },
    /// 成功应答。
    Success {
        deltas: Vec<ModelDelta>,
        stop_reason: &'static str,
        usage: ModelUsage,
    },
    /// 401 认证失败。
    Auth { status: u16 },
}

struct AttemptProvider {
    attempts: Mutex<VecDeque<Attempt>>,
    calls: AtomicU64,
}

impl AttemptProvider {
    fn new(attempts: Vec<Attempt>) -> Self {
        Self {
            attempts: Mutex::new(attempts.into()),
            calls: AtomicU64::new(0),
        }
    }

    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    fn always_http(status: u16) -> Self {
        Self {
            attempts: Mutex::new(VecDeque::new()),
            calls: AtomicU64::new(0),
        }
        .with_fallback(status)
    }

    fn with_fallback(self, status: u16) -> Self {
        // 队列耗尽时重复 503（耗尽用例需要固定响应）。
        let mut attempts = self.attempts.into_inner().expect("attempts lock");
        for _ in 0..16 {
            attempts.push_back(Attempt::Http {
                status,
                message: "Provider returned a server error.",
            });
        }
        Self {
            attempts: Mutex::new(attempts),
            calls: AtomicU64::new(0),
        }
    }
}

fn text_usage() -> ModelUsage {
    ModelUsage {
        input_tokens: 10,
        output_tokens: 12,
        ..ModelUsage::default()
    }
}

impl ModelProvider for AttemptProvider {
    fn stream(
        &self,
        _request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let attempt = self
            .attempts
            .lock()
            .expect("attempts lock")
            .pop_front()
            .unwrap_or(Attempt::Http {
                status: 503,
                message: "Provider returned a server error.",
            });
        Box::pin(async move {
            match attempt {
                Attempt::Http { status, message } => {
                    Err(ProviderError::http_status(status, message))
                }
                Attempt::AbortAfter {
                    deltas: emitted,
                    message,
                } => {
                    for delta in emitted {
                        let _ = deltas.send(delta);
                    }
                    Err(ProviderError::stream_abort(message))
                }
                Attempt::Success {
                    deltas: emitted,
                    stop_reason,
                    usage,
                } => {
                    for delta in emitted {
                        let _ = deltas.send(delta);
                    }
                    Ok(ModelResponse { usage, stop_reason })
                }
                Attempt::Auth { status } => Err(ProviderError::auth(status, "invalid api key")),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// 事件断言辅助
// ---------------------------------------------------------------------------

fn payloads(events: &[SessionEvent], event_type: SessionEventType) -> Vec<&serde_json::Value> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .filter_map(|event| event.payload.as_ref())
        .collect()
}

fn text_deltas(events: &[SessionEvent]) -> Vec<String> {
    payloads(events, SessionEventType::ModelStreaming)
        .into_iter()
        .filter(|payload| payload["kind"] == serde_json::json!("text_delta"))
        .filter_map(|payload| payload["delta"].as_str().map(str::to_owned))
        .collect()
}

fn session_updated<'a>(events: &'a [SessionEvent], kind: &str) -> Vec<&'a serde_json::Value> {
    payloads(events, SessionEventType::SessionUpdated)
        .into_iter()
        .filter(|payload| payload["type"] == serde_json::json!(kind))
        .collect()
}

fn terminal_count(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                SessionEventType::TurnCompleted | SessionEventType::TurnFailed
            )
        })
        .count()
}

fn assert_seq_strict(events: &[SessionEvent]) {
    let seqs: Vec<u64> = events.iter().map(|event| event.seq).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "seq 必须严格连续：{seqs:?}"
    );
}

fn new_session() -> Session {
    Session::new("sess_recovery", ".")
}

// ---------------------------------------------------------------------------
// 1. HTTP 5xx → 退避重试 → 成功
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn http_5xx_retries_with_backoff_then_succeeds() {
    let provider = AttemptProvider::new(vec![
        Attempt::Http {
            status: 503,
            message: "Provider returned a server error.",
        },
        Attempt::Success {
            deltas: vec![ModelDelta::Text {
                text: "answer after one retry".to_string(),
            }],
            stop_reason: "stop",
            usage: text_usage(),
        },
    ]);
    let mut session = new_session().with_retry_policy(RetryPolicy::default());
    let cancel = CancelToken::new();
    let mut sink: Vec<SessionEvent> = Vec::new();
    let result = session
        .run_turn("retry scenario", &provider, &mut sink, &cancel)
        .await;
    result.expect("重试成功后 turn 应完成");

    assert_eq!(provider.calls(), 2, "503 后必须且只能再尝试一次");
    assert_eq!(terminal_count(&sink), 1, "终态恰好一次");
    assert_seq_strict(&sink);

    let scheduled = session_updated(&sink, "model_retry_scheduled");
    assert_eq!(scheduled.len(), 1, "一次重试只发一条 model_retry_scheduled");
    let scheduled = scheduled[0];
    assert_eq!(scheduled["attempt"], serde_json::json!(1));
    assert_eq!(scheduled["nextAttempt"], serde_json::json!(2));
    assert_eq!(scheduled["maxAttempts"], serde_json::json!(11));
    assert_eq!(scheduled["statusCode"], serde_json::json!(503));
    assert_eq!(scheduled["reason"], serde_json::json!("server_error"));
    let delay = scheduled["delayMs"].as_u64().expect("delayMs 是整数");
    assert!(delay > 0, "HTTP 5xx 必须走退避（不是 0 延迟）");
    let api_retry = &scheduled["_meta"]["mode"]["apiRetry"];
    assert_eq!(api_retry["kind"], serde_json::json!("api_retry"));
    assert_eq!(api_retry["attempt"], serde_json::json!(1));
    assert_eq!(api_retry["maxRetries"], serde_json::json!(10));
    assert_eq!(api_retry["retryDelayMs"], serde_json::json!(delay));
    assert_eq!(api_retry["errorStatus"], serde_json::json!(503));
    assert_eq!(
        api_retry["error"],
        serde_json::json!("Provider returned a server error.")
    );

    let completed = session_updated(&sink, "model_request_completed");
    assert_eq!(completed.len(), 1, "成功尝试收口一条 completed");
    assert_eq!(
        completed[0]["_meta"]["mode"]["apiRetry"],
        serde_json::Value::Null,
        "成功后 apiRetry 必须清空"
    );

    assert_eq!(text_deltas(&sink), vec!["answer after one retry"]);
    let terminal = sink
        .iter()
        .find(|event| event.event_type == SessionEventType::TurnCompleted)
        .expect("turn.completed");
    let terminal = terminal.payload.as_ref().expect("payload");
    assert_eq!(
        terminal["response"],
        serde_json::json!("answer after one retry")
    );
    // 事件层只计成功完成的模型请求；HTTP 重试不增加这个计数。
    assert_eq!(terminal["usage"]["modelRequestCount"], serde_json::json!(1));
    // DB/轮次口径：HTTP 重试仍是一个 round。
    assert_eq!(terminal["historyRoundCount"], serde_json::json!(1));
}

// ---------------------------------------------------------------------------
// 2. 流中断（已提交 delta）→ stream recovery
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn stream_abort_emits_recovery_events_and_discards_partial() {
    let provider = AttemptProvider::new(vec![
        Attempt::AbortAfter {
            deltas: vec![ModelDelta::Text {
                text: "partial text ".to_string(),
            }],
            message: "connection reset by peer",
        },
        Attempt::Success {
            deltas: vec![ModelDelta::Text {
                text: "complete answer after recovery".to_string(),
            }],
            stop_reason: "stop",
            usage: text_usage(),
        },
    ]);
    let mut session = new_session();
    let cancel = CancelToken::new();
    let mut sink: Vec<SessionEvent> = Vec::new();
    session
        .run_turn("retry scenario", &provider, &mut sink, &cancel)
        .await
        .expect("恢复后 turn 应完成");

    assert_eq!(provider.calls(), 2);
    assert_eq!(terminal_count(&sink), 1);
    assert_seq_strict(&sink);

    // 事件里保留被丢弃的 partial delta，但最终消息只有恢复后的文本。
    assert_eq!(
        text_deltas(&sink),
        vec!["partial text ", "complete answer after recovery"]
    );
    let recovery = payloads(&sink, SessionEventType::StreamRecoveryUpdated);
    assert_eq!(recovery.len(), 4, "流恢复固定四条事件（夹具 §2.2）");
    let streamed = payloads(&sink, SessionEventType::ModelStreaming);
    assert_eq!(
        recovery[0]["assistantMessageId"], streamed[0]["assistantMessageId"],
        "恢复事件必须回指被丢弃尝试的 assistant 消息"
    );
    assert_eq!(
        recovery[0]["failureKind"],
        serde_json::json!("provider_network_error")
    );
    assert_eq!(
        recovery[0]["message"],
        serde_json::json!("Network connection failed for the provider request.")
    );
    assert_eq!(recovery[0]["retryNumber"], serde_json::json!(1));
    assert_eq!(recovery[0]["maxRetries"], serde_json::json!(10));
    assert!(recovery[0]["failedRequestId"].as_str().is_some());
    assert_eq!(
        recovery[0]["_meta"]["mode"]["apiRetry"]["retryDelayMs"],
        serde_json::json!(0),
        "流恢复是立即重试（retryDelay 0）"
    );
    assert_eq!(
        recovery[0]["_meta"]["mode"]["apiRetry"]["error"],
        serde_json::json!("Network connection failed for the provider request.")
    );
    assert_eq!(
        recovery[1]["reason"],
        serde_json::json!("no_tool_committed")
    );
    assert_eq!(recovery[1]["committedToolCallIds"], serde_json::json!([]));
    assert_eq!(recovery[2]["discardedTextBytes"], serde_json::json!(13));
    assert_eq!(recovery[2]["discardedReasoningBytes"], serde_json::json!(0));
    assert_eq!(recovery[2]["discardedToolCallIds"], serde_json::json!([]));
    assert_eq!(recovery[3]["retryNumber"], serde_json::json!(1));
    assert_eq!(recovery[3]["maxRetries"], serde_json::json!(10));
    assert_eq!(recovery[3]["streamMode"], serde_json::json!("sse"));
    assert_eq!(
        recovery[3]["_meta"]["mode"]["apiRetry"]["error"],
        serde_json::json!("Model stream recovery retry started")
    );
    // 同一恢复周期的 attemptId/anchorId 共享，failedRequestId 回指失败请求。
    let attempt_id = recovery[0]["attemptId"].as_str().expect("attemptId");
    for payload in &recovery {
        assert_eq!(payload["attemptId"], serde_json::json!(attempt_id));
    }
    let anchor_id = recovery[1]["anchorId"].as_str().expect("anchorId");
    for payload in &recovery[1..] {
        assert_eq!(payload["anchorId"], serde_json::json!(anchor_id));
    }
    assert_eq!(
        recovery[3]["failedRequestId"],
        recovery[0]["failedRequestId"]
    );

    let terminal = sink
        .iter()
        .find(|event| event.event_type == SessionEventType::TurnCompleted)
        .expect("turn.completed");
    let terminal = terminal.payload.as_ref().expect("payload");
    assert_eq!(
        terminal["response"],
        serde_json::json!("complete answer after recovery")
    );
    // 事件层只计成功请求；DB/round 口径在 agent-server 层断言（夹具 1/2 差异）。
    assert_eq!(terminal["usage"]["modelRequestCount"], serde_json::json!(1));
    assert_eq!(terminal["historyRoundCount"], serde_json::json!(2));
}

// ---------------------------------------------------------------------------
// 3. 401：非 retryable、立即失败
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_401_is_not_retried() {
    let provider = AttemptProvider::new(vec![Attempt::Auth { status: 401 }]);
    let mut session = new_session();
    let cancel = CancelToken::new();
    let mut sink: Vec<SessionEvent> = Vec::new();
    let error = session
        .run_turn("retry scenario", &provider, &mut sink, &cancel)
        .await
        .expect_err("401 必须失败");

    assert_eq!(provider.calls(), 1, "401 不能重试");
    assert!(error.provider_error.is_some(), "错误必须携带 provider 分类");
    let provider_error = error.provider_error.as_ref().expect("分类");
    assert_eq!(provider_error.status, Some(401));
    assert!(!provider_error.retryable);
    assert!(provider_error.is_auth());
    assert_eq!(error.progress.model_requests, 1);
    assert!(session_updated(&sink, "model_retry_scheduled").is_empty());
    assert!(payloads(&sink, SessionEventType::StreamRecoveryUpdated).is_empty());
    assert_eq!(
        terminal_count(&sink),
        0,
        "agent-core 不发终态；turn.failed 由宿主恰好发一次"
    );
}

// ---------------------------------------------------------------------------
// 4. 退避期间取消：取消优先于重试
// ---------------------------------------------------------------------------

struct CancellingSink {
    events: Vec<SessionEvent>,
    cancel: CancelToken,
}

impl EventSink for CancellingSink {
    fn emit(&mut self, event: SessionEvent) {
        if event.event_type == SessionEventType::SessionUpdated
            && event.payload.as_ref().is_some_and(|payload| {
                payload["type"] == serde_json::json!("model_retry_scheduled")
            })
        {
            self.cancel.cancel();
        }
        self.events.push(event);
    }
}

#[tokio::test(start_paused = true)]
async fn cancel_during_backoff_wins_over_retry() {
    let provider = AttemptProvider::new(vec![Attempt::Http {
        status: 503,
        message: "Provider returned a server error.",
    }]);
    let mut session = new_session().with_retry_policy(RetryPolicy::default());
    let cancel = CancelToken::new();
    let mut sink = CancellingSink {
        events: Vec::new(),
        cancel: cancel.clone(),
    };
    let error = session
        .run_turn("cancel during retry", &provider, &mut sink, &cancel)
        .await
        .expect_err("取消以 TurnError::cancelled 返回");
    assert!(error.is_cancelled());

    assert_eq!(provider.calls(), 1, "取消后不能再发起第二次尝试");
    let events = sink.events;
    assert_eq!(terminal_count(&events), 1, "取消终态恰好一次");
    let terminal = events
        .iter()
        .find(|event| event.event_type == SessionEventType::TurnCompleted)
        .expect("turn.completed(cancelled)");
    assert_eq!(
        terminal.payload.as_ref().expect("payload")["resultType"],
        serde_json::json!("cancelled")
    );
    assert_eq!(session_updated(&events, "model_retry_scheduled").len(), 1);
    let cancelled = session_updated(&events, "model_request_failed");
    assert_eq!(cancelled.len(), 1, "退避取消必须收口失败事件");
    assert_eq!(cancelled[0]["reason"], serde_json::json!("cancelled"));
    assert_eq!(
        cancelled[0]["message"],
        serde_json::json!("Model request was cancelled.")
    );
    assert_eq!(cancelled[0]["retryable"], serde_json::json!(false));
    assert_eq!(
        cancelled[0]["_meta"]["mode"]["apiRetry"],
        serde_json::Value::Null
    );
    assert!(payloads(&events, SessionEventType::StreamRecoveryUpdated).is_empty());
}

struct CancelOnRecoverySink {
    events: Vec<SessionEvent>,
    cancel: CancelToken,
}

impl EventSink for CancelOnRecoverySink {
    fn emit(&mut self, event: SessionEvent) {
        if event.event_type == SessionEventType::StreamRecoveryUpdated
            && event
                .payload
                .as_ref()
                .is_some_and(|payload| payload["streamMode"] == serde_json::json!("sse"))
        {
            self.cancel.cancel();
        }
        self.events.push(event);
    }
}

#[tokio::test]
async fn cancel_after_stream_recovery_events_clears_retry_state() {
    let provider = AttemptProvider::new(vec![Attempt::AbortAfter {
        deltas: vec![ModelDelta::Text {
            text: "partial text ".to_string(),
        }],
        message: "connection reset by peer",
    }]);
    let mut session = new_session();
    let cancel = CancelToken::new();
    let mut sink = CancelOnRecoverySink {
        events: Vec::new(),
        cancel: cancel.clone(),
    };
    let error = session
        .run_turn("retry scenario", &provider, &mut sink, &cancel)
        .await
        .expect_err("取消以 TurnError::cancelled 返回");
    assert!(error.is_cancelled());
    assert_eq!(provider.calls(), 1, "取消后不能再发起恢复重试");

    let events = sink.events;
    assert_eq!(terminal_count(&events), 1);
    assert_eq!(
        payloads(&events, SessionEventType::StreamRecoveryUpdated).len(),
        4
    );
    let cancelled = session_updated(&events, "model_request_failed");
    assert_eq!(
        cancelled.len(),
        1,
        "恢复期间取消必须收口失败事件（清空 apiRetry）"
    );
    assert_eq!(cancelled[0]["reason"], serde_json::json!("cancelled"));
    assert_eq!(
        cancelled[0]["_meta"]["mode"]["apiRetry"],
        serde_json::Value::Null
    );
}

// ---------------------------------------------------------------------------
// 5. 重试耗尽：按观察到的 maxRetries=10（11 次尝试），不产生重复终态
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn retry_exhaustion_stops_after_observed_attempt_count() {
    // 耗尽形状**未观察到**（契约 §2.7）：这里只断言尝试上限来自夹具的
    // `maxRetries:10`，且 agent-core 不自己发第二个终态。
    let provider = AttemptProvider::always_http(503);
    let mut session = new_session();
    let cancel = CancelToken::new();
    let mut sink: Vec<SessionEvent> = Vec::new();
    let error = session
        .run_turn("retry scenario", &provider, &mut sink, &cancel)
        .await
        .expect_err("耗尽必须失败");
    assert!(!error.is_cancelled());
    assert_eq!(provider.calls(), 11, "1 次初始 + maxRetries 10");
    assert_eq!(session_updated(&sink, "model_retry_scheduled").len(), 10);
    assert_eq!(terminal_count(&sink), 0);
    assert_eq!(error.progress.model_requests, 1);
    let provider_error = error.provider_error.as_ref().expect("分类");
    assert_eq!(provider_error.status, Some(503));
    assert!(provider_error.retryable);
}
