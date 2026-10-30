//! 存储启动状态门（对应 `modeStorageStartupGate.ts`）。
//!
//! 每条 Agent 连接（= 一个进程代次）一个 gate：只有合法的 `startup/storageState`
//! 控制帧能推进状态；`ready` 放行，`failed` 以 `SQLite startup failed: <code>` 失败；
//! 首个状态超过超时时间未到也按 `startup_status_timeout` 失败。
//!
//! 与 TS 的差异：状态解析不启用 `deny_unknown_fields`（忽略未来新增的错误详情字段），
//! 其余必填字段、phase/databaseKind 枚举、`failed` 必须带 errorCode、sequence 单调
//! 等规则一致。合成的失败快照用时间戳生成 attemptId/databaseId，不追求 UUID 形态。

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

pub const DEFAULT_FIRST_STATUS_TIMEOUT: Duration = Duration::from_secs(30);
pub const STORAGE_STATE_NOTIFICATION: &str = "startup/storageState";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageDatabaseKind {
    Session,
    TasksIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageStartupPhase {
    Checking,
    WaitingForLock,
    Migrating,
    Committing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStartupState {
    pub schema_version: u8,
    pub attempt_id: String,
    pub sequence: i64,
    pub database_id: String,
    pub database_kind: StorageDatabaseKind,
    pub phase: StorageStartupPhase,
    #[serde(default)]
    pub migration: Option<Value>,
    pub elapsed_ms: f64,
    #[serde(default)]
    pub completed: Option<i64>,
    #[serde(default)]
    pub total: Option<i64>,
    #[serde(default)]
    pub error_code: Option<String>,
}

impl StorageStartupState {
    fn is_terminal(&self) -> bool {
        matches!(
            self.phase,
            StorageStartupPhase::Ready | StorageStartupPhase::Failed
        )
    }
}

fn parse_state(value: &Value) -> Option<StorageStartupState> {
    let state: StorageStartupState = serde_json::from_value(value.clone()).ok()?;
    if state.schema_version != 1 {
        return None;
    }
    if state.attempt_id.is_empty() || state.database_id.is_empty() {
        return None;
    }
    if state.sequence <= 0 || !state.elapsed_ms.is_finite() || state.elapsed_ms < 0.0 {
        return None;
    }
    if state.phase == StorageStartupPhase::Failed && state.error_code.is_none() {
        return None;
    }
    Some(state)
}

struct GateState {
    current: Option<StorageStartupState>,
    terminal_error: Option<String>,
}

/// 单连接（单进程代次）的存储启动门。
pub struct StorageStartupGate {
    required: bool,
    first_status_timeout: Duration,
    state: Mutex<GateState>,
    sender: watch::Sender<Option<StorageStartupState>>,
}

impl StorageStartupGate {
    pub fn new(required: bool, first_status_timeout: Duration) -> Self {
        let (sender, _) = watch::channel(None);
        Self {
            required,
            first_status_timeout,
            state: Mutex::new(GateState {
                current: None,
                terminal_error: None,
            }),
            sender,
        }
    }

    pub fn snapshot(&self) -> Option<StorageStartupState> {
        self.state
            .lock()
            .expect("storage gate poisoned")
            .current
            .clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<StorageStartupState>> {
        self.sender.subscribe()
    }

    pub fn is_required(&self) -> bool {
        self.required
    }

    /// 接受一条状态：校验 + attemptId/databaseId 不变 + sequence 严格递增 + 终态不可再变。
    pub fn accept(&self, value: &Value) -> bool {
        let Some(next) = parse_state(value) else {
            return false;
        };
        let mut guard = self.state.lock().expect("storage gate poisoned");
        if guard.terminal_error.is_some() {
            return false;
        }
        if let Some(current) = &guard.current {
            if next.attempt_id != current.attempt_id
                || next.database_id != current.database_id
                || next.sequence <= current.sequence
                || current.is_terminal()
            {
                return false;
            }
        }
        if next.phase == StorageStartupPhase::Failed {
            let code = next.error_code.clone().unwrap_or_else(|| "sql_failed".into());
            guard.terminal_error = Some(format!("SQLite startup failed: {code}"));
            guard.current = Some(next.clone());
            drop(guard);
            let _ = self.sender.send(Some(next));
            return true;
        }
        guard.current = Some(next.clone());
        drop(guard);
        let _ = self.sender.send(Some(next));
        true
    }

    /// 等待 `ready`；失败/首帧超时返回错误。非 required 连接立即返回。
    pub async fn wait_ready(&self) -> Result<(), String> {
        if !self.required {
            return Ok(());
        }
        {
            let guard = self.state.lock().expect("storage gate poisoned");
            if let Some(error) = &guard.terminal_error {
                return Err(error.clone());
            }
            if matches!(
                guard.current.as_ref().map(|state| state.phase),
                Some(StorageStartupPhase::Ready)
            ) {
                return Ok(());
            }
        }

        let mut receiver = self.sender.subscribe();
        // 首帧超时：30s 内必须看到任何合法状态。
        let mut first_seen = self.sender.borrow().is_some();
        if !first_seen {
            match tokio::time::timeout(self.first_status_timeout, receiver.changed()).await {
                Err(_) => {
                    let message = "SQLite startup failed: startup_status_timeout".to_string();
                    self.fail_with(message.clone());
                    return Err(message);
                }
                Ok(Err(_)) => return Err("storage gate closed".into()),
                Ok(Ok(())) => first_seen = true,
            }
        }
        let _ = first_seen;

        loop {
            {
                let guard = self.state.lock().expect("storage gate poisoned");
                if let Some(error) = &guard.terminal_error {
                    return Err(error.clone());
                }
                if matches!(
                    guard.current.as_ref().map(|state| state.phase),
                    Some(StorageStartupPhase::Ready)
                ) {
                    return Ok(());
                }
            }
            match receiver.changed().await {
                Ok(()) => {}
                Err(_) => return Err("storage gate closed".into()),
            }
        }
    }

    fn fail_with(&self, message: String) {
        let mut guard = self.state.lock().expect("storage gate poisoned");
        if guard.terminal_error.is_none() {
            guard.terminal_error = Some(message);
            let synthetic = StorageStartupState {
                schema_version: 1,
                attempt_id: format!("rust-{}", now_nanos()),
                sequence: guard.current.as_ref().map(|state| state.sequence + 1).unwrap_or(1),
                database_id: "unresolved".into(),
                database_kind: StorageDatabaseKind::Session,
                phase: StorageStartupPhase::Failed,
                migration: None,
                elapsed_ms: 0.0,
                completed: None,
                total: None,
                error_code: Some("startup_status_timeout".into()),
            };
            guard.current = Some(synthetic.clone());
            drop(guard);
            let _ = self.sender.send(Some(synthetic));
        }
    }
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(sequence: i64, phase: &str, attempt: &str) -> Value {
        json!({
            "schemaVersion": 1,
            "attemptId": attempt,
            "sequence": sequence,
            "databaseId": "db-1",
            "databaseKind": "session",
            "phase": phase,
            "elapsedMs": 1.0
        })
    }

    #[test]
    fn accepts_monotonic_states_and_rejects_lower_or_changed_identity() {
        let gate = StorageStartupGate::new(true, Duration::from_secs(1));
        assert!(gate.accept(&state(1, "checking", "a")));
        assert!(!gate.accept(&state(1, "migrating", "a")), "sequence 必须递增");
        assert!(gate.accept(&state(2, "ready", "a")));
        assert!(!gate.accept(&state(3, "failed", "a")), "ready 后不可再变");
        assert!(!gate.accept(&state(2, "ready", "b")), "attemptId 不可变");
    }

    #[test]
    fn failed_requires_error_code_and_is_terminal() {
        let gate = StorageStartupGate::new(true, Duration::from_secs(1));
        let mut failed = state(1, "failed", "a");
        failed["errorCode"] = json!("sql_failed");
        assert!(gate.accept(&failed));
        assert_eq!(
            gate.state.lock().unwrap().terminal_error.as_deref(),
            Some("SQLite startup failed: sql_failed")
        );
        let mut missing = state(1, "failed", "b");
        missing.as_object_mut().unwrap().remove("errorCode");
        assert!(!gate.accept(&missing), "failed 缺 errorCode 必须拒绝");
    }

    #[tokio::test]
    async fn wait_ready_resolves_on_ready_and_immediately_when_not_required() {
        let gate = std::sync::Arc::new(StorageStartupGate::new(true, Duration::from_secs(5)));
        let waiter = tokio::spawn({
            let gate = std::sync::Arc::clone(&gate);
            async move { gate.wait_ready().await }
        });
        gate.accept(&state(1, "checking", "a"));
        gate.accept(&state(2, "ready", "a"));
        assert_eq!(waiter.await.unwrap(), Ok(()));

        let optional = StorageStartupGate::new(false, Duration::from_secs(1));
        assert_eq!(optional.wait_ready().await, Ok(()));
    }

    #[tokio::test]
    async fn wait_ready_times_out_without_first_status() {
        let gate = StorageStartupGate::new(true, Duration::from_millis(50));
        let error = gate.wait_ready().await.unwrap_err();
        assert_eq!(error, "SQLite startup failed: startup_status_timeout");
    }

    #[test]
    fn rejects_unknown_schema_version_and_negative_sequence() {
        let gate = StorageStartupGate::new(true, Duration::from_secs(1));
        let mut wrong_version = state(1, "ready", "a");
        wrong_version["schemaVersion"] = json!(2);
        assert!(!gate.accept(&wrong_version));
        assert!(!gate.accept(&state(0, "ready", "a")));
    }
}
