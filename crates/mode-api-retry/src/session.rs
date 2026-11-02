//! 会话级重试状态跟踪与未提交草稿登记。
//!
//! 真源：`packages/services/src/mode-session/modeSessionApiRetry.ts`、
//! `modeSessionDraftRegistry.ts`。事件/snapshot 以 JSON 形状处理，保持原始字段。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::{
    is_mode_model_retry_recovery_progress_payload,
    mode_api_retry_from_model_network_status_payload, mode_api_retry_from_stream_recovery_payload,
    normalize_mode_api_retry_status, ApiRetryOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTarget {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub session_id: String,
}

fn workspace_key(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    workspace_identity
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .unwrap_or(workspace_path)
        .to_string()
}

fn session_key(target: &SessionTarget) -> String {
    format!(
        "{}\u{0}{}",
        workspace_key(&target.workspace_path, target.workspace_identity.as_deref()),
        target.session_id
    )
}

fn record(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value?.as_object()
}

fn outcome_to_value(outcome: ApiRetryOutcome) -> Option<Value> {
    match outcome {
        ApiRetryOutcome::Undefined => None,
        ApiRetryOutcome::Null => Some(Value::Null),
        ApiRetryOutcome::Status(status) => {
            Some(serde_json::to_value(status).expect("ApiRetryStatus 序列化不会失败"))
        }
    }
}

fn api_retry_from_session_payload(payload: &Map<String, Value>) -> Option<Value> {
    if let Some(value) = payload.get("apiRetry") {
        return outcome_to_value(normalize_mode_api_retry_status(Some(value)));
    }
    if let Some(runtime) = record(payload.get("runtime")) {
        if let Some(value) = runtime.get("apiRetry") {
            let outcome = normalize_mode_api_retry_status(Some(value));
            if outcome != ApiRetryOutcome::Undefined {
                return outcome_to_value(outcome);
            }
        }
    }
    if let Some(meta) = record(payload.get("_meta")) {
        if let Some(mode) = record(meta.get("mode")) {
            if let Some(value) = mode.get("apiRetry") {
                let outcome = normalize_mode_api_retry_status(Some(value));
                if outcome != ApiRetryOutcome::Undefined {
                    return outcome_to_value(outcome);
                }
            }
        }
    }
    // TS 直接把这个 payload 本身传给 stream recovery 归一化（不是 payload.streamRecovery）。
    let stream_payload = Value::Object(payload.clone());
    let stream = mode_api_retry_from_stream_recovery_payload(Some(&stream_payload));
    if stream != ApiRetryOutcome::Undefined {
        return outcome_to_value(stream);
    }
    outcome_to_value(mode_api_retry_from_model_network_status_payload(payload))
}

pub struct SessionApiRetryTracker {
    by_session: Mutex<HashMap<String, Option<Value>>>,
}

impl Default for SessionApiRetryTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionApiRetryTracker {
    pub fn new() -> Self {
        Self {
            by_session: Mutex::new(HashMap::new()),
        }
    }

    pub fn track_api_retry_from_session_event(
        &self,
        target: &SessionTarget,
        event: &Value,
    ) -> Value {
        let Some(event_object) = event.as_object() else {
            return event.clone();
        };
        let event_type = event_object.get("type").and_then(Value::as_str);
        if event_type == Some("session.event") {
            let Some(inner) = record(event_object.get("event")) else {
                return event.clone();
            };
            let inner_type = inner.get("type").and_then(Value::as_str);
            if matches!(
                inner_type,
                Some("session.updated" | "streamRecovery.updated")
            ) {
                let payload = record(inner.get("payload")).cloned().unwrap_or_default();
                let api_retry = api_retry_from_session_payload(&payload);
                let key = session_key(&SessionTarget {
                    workspace_path: target.workspace_path.clone(),
                    workspace_identity: target.workspace_identity.clone(),
                    session_id: inner
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
                let mut by_session = self.by_session.lock().expect("api retry lock");
                if let Some(value) = api_retry {
                    by_session.insert(key, Some(value));
                } else if by_session.get(&key).is_some_and(|value| value.is_some())
                    && is_mode_model_retry_recovery_progress_payload(&payload)
                {
                    by_session.insert(key, None);
                }
            }
            return event.clone();
        }
        if event_type == Some("snapshot") {
            let mut next = event_object.clone();
            let snapshot = event_object.get("snapshot").cloned().unwrap_or(Value::Null);
            next.insert(
                "snapshot".to_string(),
                self.with_api_retry_runtime(&snapshot),
            );
            return Value::Object(next);
        }
        event.clone()
    }

    pub fn with_api_retry_runtime(&self, snapshot: &Value) -> Value {
        let Some(snapshot_object) = snapshot.as_object() else {
            return snapshot.clone();
        };
        let Some(session) = record(snapshot_object.get("session")) else {
            return snapshot.clone();
        };
        let session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(workspace) = record(session.get("workspace")) else {
            return snapshot.clone();
        };
        let workspace_path = workspace
            .get("workspacePath")
            .and_then(Value::as_str)
            .unwrap_or("");
        if session_id.is_empty() || workspace_path.is_empty() {
            return snapshot.clone();
        }
        let key = session_key(&SessionTarget {
            workspace_path: workspace_path.to_string(),
            workspace_identity: workspace
                .get("workspaceIdentity")
                .and_then(Value::as_str)
                .map(str::to_string),
            session_id: session_id.to_string(),
        });
        if let Some(runtime) = record(snapshot_object.get("runtime")) {
            if let Some(existing) = runtime.get("apiRetry") {
                self.by_session
                    .lock()
                    .expect("api retry lock")
                    .insert(key, Some(existing.clone()));
                return snapshot.clone();
            }
        }
        let status = session.get("status").and_then(Value::as_str);
        if matches!(status, Some("completed" | "error")) {
            self.by_session
                .lock()
                .expect("api retry lock")
                .insert(key, None);
            let mut next = snapshot_object.clone();
            let mut runtime = record(next.get("runtime")).cloned().unwrap_or_default();
            runtime.insert("apiRetry".to_string(), Value::Null);
            next.insert("runtime".to_string(), Value::Object(runtime));
            return Value::Object(next);
        }
        let stored = {
            let by_session = self.by_session.lock().expect("api retry lock");
            match by_session.get(&key) {
                None => return snapshot.clone(),
                Some(value) => value.clone(),
            }
        };
        let mut next = snapshot_object.clone();
        let mut runtime = record(next.get("runtime")).cloned().unwrap_or_default();
        runtime.insert("apiRetry".to_string(), stored.unwrap_or(Value::Null));
        next.insert("runtime".to_string(), Value::Object(runtime));
        Value::Object(next)
    }
}

pub struct DeferredDraftRegistry {
    session_keys: Mutex<HashSet<String>>,
}

impl Default for DeferredDraftRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DeferredDraftRegistry {
    pub fn new() -> Self {
        Self {
            session_keys: Mutex::new(HashSet::new()),
        }
    }

    pub fn remember(&self, params: &SessionTarget, snapshot: &Value) {
        let Some(session) = record(snapshot.get("session")) else {
            return;
        };
        let Some(workspace) = record(session.get("workspace")) else {
            return;
        };
        let workspace_path = workspace
            .get("workspacePath")
            .and_then(Value::as_str)
            .unwrap_or("");
        let workspace_identity = workspace
            .get("workspaceIdentity")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| params.workspace_identity.clone());
        let session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        self.session_keys
            .lock()
            .expect("draft registry lock")
            .insert(session_key(&SessionTarget {
                workspace_path: workspace_path.to_string(),
                workspace_identity,
                session_id: session_id.to_string(),
            }));
    }

    pub fn has(&self, target: &SessionTarget) -> bool {
        self.session_keys
            .lock()
            .expect("draft registry lock")
            .contains(&session_key(target))
    }

    pub fn forget(&self, target: &SessionTarget) {
        self.session_keys
            .lock()
            .expect("draft registry lock")
            .remove(&session_key(target));
    }
}
