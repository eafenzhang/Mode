//! 与 TS 会话级 tracker / draft registry 的对拍：
//! `pnpm mode-api-retry-session:fixtures`。

use std::path::Path;

use mode_api_retry::{DeferredDraftRegistry, SessionApiRetryTracker, SessionTarget};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Fixture {
    tracker: Vec<TrackerCase>,
    draft: Vec<DraftCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetInput {
    workspace_path: String,
    workspace_identity: Option<String>,
    session_id: String,
}

impl TargetInput {
    fn to_target(&self) -> SessionTarget {
        SessionTarget {
            workspace_path: self.workspace_path.clone(),
            workspace_identity: self.workspace_identity.clone(),
            session_id: self.session_id.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TrackerCase {
    name: String,
    target: TargetInput,
    steps: Vec<TrackerStep>,
}

#[derive(Debug, Deserialize)]
struct TrackerStep {
    kind: String,
    #[serde(default)]
    event: Option<Value>,
    #[serde(default)]
    snapshot: Option<Value>,
    expected: Value,
}

#[derive(Debug, Deserialize)]
struct DraftCase {
    name: String,
    steps: Vec<DraftStep>,
}

#[derive(Debug, Deserialize)]
struct DraftStep {
    kind: String,
    #[serde(default)]
    params: Option<TargetInput>,
    #[serde(default)]
    snapshot: Option<Value>,
    #[serde(default)]
    target: Option<TargetInput>,
    expected: Value,
}

#[test]
fn matches_ts_session_api_retry_tracker() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("mode-api-retry-session.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.tracker {
        let tracker = SessionApiRetryTracker::new();
        let target = case.target.to_target();
        for (index, step) in case.steps.iter().enumerate() {
            let actual = match step.kind.as_str() {
                "event" => tracker.track_api_retry_from_session_event(
                    &target,
                    step.event.as_ref().expect("event"),
                ),
                "snapshot" => {
                    tracker.with_api_retry_runtime(step.snapshot.as_ref().expect("snapshot"))
                }
                other => panic!("{} step {index}: 未知 kind {other}", case.name),
            };
            assert_eq!(actual, step.expected, "{} step {index}", case.name);
        }
    }
}

#[test]
fn matches_ts_deferred_draft_registry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("mode-api-retry-session.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.draft {
        let registry = DeferredDraftRegistry::new();
        for (index, step) in case.steps.iter().enumerate() {
            match step.kind.as_str() {
                "remember" => {
                    registry.remember(
                        &step.params.as_ref().expect("params").to_target(),
                        step.snapshot.as_ref().expect("snapshot"),
                    );
                    assert_eq!(step.expected, Value::Null, "{} step {index}", case.name);
                }
                "forget" => {
                    registry.forget(&step.target.as_ref().expect("target").to_target());
                    assert_eq!(step.expected, Value::Null, "{} step {index}", case.name);
                }
                "has" => {
                    let actual = registry.has(&step.target.as_ref().expect("target").to_target());
                    assert_eq!(
                        Value::Bool(actual),
                        step.expected,
                        "{} step {index}",
                        case.name
                    );
                }
                other => panic!("{} step {index}: 未知 kind {other}", case.name),
            }
        }
    }
}

#[test]
fn model_streaming_progress_clears_retry_state() {
    let tracker = SessionApiRetryTracker::new();
    let target = SessionTarget {
        workspace_path: "/ws".to_string(),
        workspace_identity: None,
        session_id: "sess".to_string(),
    };
    let scheduled = serde_json::json!({
        "type": "session.event",
        "event": {
            "type": "session.updated",
            "sessionId": "sess",
            "payload": {
                "type": "model_retry_scheduled",
                "attempt": 1,
                "_meta": {
                    "mode": {
                        "apiRetry": {
                            "kind": "api_retry",
                            "attempt": 1,
                            "maxRetries": 10,
                            "retryDelayMs": 10,
                            "errorStatus": 503,
                            "error": "Provider returned a server error."
                        }
                    }
                }
            }
        }
    });
    tracker.track_api_retry_from_session_event(&target, &scheduled);
    let snapshot = serde_json::json!({
        "session": {
            "sessionId": "sess",
            "workspace": { "workspacePath": "/ws" },
            "status": "running"
        },
        "runtime": {}
    });
    let projected = tracker.with_api_retry_runtime(&snapshot);
    assert_eq!(projected["runtime"]["apiRetry"]["attempt"], 1);

    // 空 delta 不是恢复进展：状态保持。
    tracker.track_model_streaming_progress(
        &target,
        &serde_json::json!({ "kind": "text_delta", "delta": "" }),
    );
    let projected = tracker.with_api_retry_runtime(&snapshot);
    assert_eq!(projected["runtime"]["apiRetry"]["attempt"], 1);

    // 首个恢复文本 delta → 清空（恢复进度规则与事件分支同源）。
    tracker.track_model_streaming_progress(
        &target,
        &serde_json::json!({ "kind": "text_delta", "delta": "recovered" }),
    );
    let projected = tracker.with_api_retry_runtime(&snapshot);
    assert!(
        projected["runtime"]["apiRetry"].is_null(),
        "恢复进展必须清除 apiRetry：{projected}"
    );
}
