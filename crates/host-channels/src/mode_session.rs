//! `mode-session` 频道适配器（收尾切片：只实现 renderer 仍使用的 `readSession`）。
//!
//! renderer 其余三个方法已切到 `mode-agent`。本频道补回 Node `mode-session` 在
//! `readSession` 上的两层包装：apiRetry 快照暂存与 Claude 导入历史修复。
//! 规范见 `docs/specs/mode-session-channel.md`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_process::{
    AgentControlClient, AgentWorkspaceTarget, CreateSessionOptions, RuntimePolicy,
};
use agent_protocol::message::MessageInfo;
use agent_protocol::session_create::{
    ClaudeCodeImport, ClaudeCodeSource, ImportHistory, ImportMessage, ImportMessageRole,
};
use agent_protocol::session_read::SessionStatus;
use agent_protocol::session_write::SessionPersistence;
use agent_protocol::snapshot::{ApiRetryStatus, DeliveryKind, SessionStateSnapshot};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value as Json;

pub const MODE_SESSION_CHANNEL: &str = "mode-session";

type LogSink = Arc<dyn Fn(&str) + Send + Sync>;
type RetryState = Mutex<HashMap<String, Option<ApiRetryStatus>>>;

struct ModeSessionInner {
    control: Arc<AgentControlClient>,
    data_root: PathBuf,
    retry_state: RetryState,
    on_log: Option<LogSink>,
}

pub struct ModeSessionChannel {
    inner: Arc<ModeSessionInner>,
}

impl ModeSessionChannel {
    pub fn new(control: Arc<AgentControlClient>, data_root: PathBuf) -> Self {
        Self {
            inner: Arc::new(ModeSessionInner {
                control,
                data_root,
                retry_state: Mutex::new(HashMap::new()),
                on_log: None,
            }),
        }
    }

    pub fn with_log(mut self, on_log: LogSink) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.on_log = Some(on_log);
        }
        self
    }
}

impl ServerChannel for ModeSessionChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            match command.as_str() {
                "readSession" => inner.read_session(arg).await,
                other => Err(ChannelError::error(format!(
                    "mode-session channel: unknown or not-yet-migrated command {other}"
                ))),
            }
        })
    }
}

impl ModeSessionInner {
    fn log(&self, message: impl AsRef<str>) {
        if let Some(on_log) = &self.on_log {
            on_log(message.as_ref());
        }
    }

    async fn read_session(&self, arg: Value) -> Result<Value, ChannelError> {
        let params: ReadSessionParams = parse_params("readSession", arg)?;
        let target = params.workspace.target();
        self.control
            .ensure_provider_config(&target)
            .await
            .map_err(|error| ChannelError::error(error.to_string()))?;
        let mut snapshot = self
            .control
            .read_session_with_policy(
                &target,
                &params.session_id,
                delivery_from(params.delivery_kind.as_deref())?,
                params.message_limit,
                params.after_seq,
                RuntimePolicy::StartIfNeeded,
            )
            .await
            .map_err(|error| ChannelError::error(error.to_string()))?;

        self.apply_retry_state(&mut snapshot);
        let snapshot = self
            .repair_imported_claude_session(&target, &params.session_id, snapshot)
            .await;
        json_value(&snapshot)
    }

    /// `modeSessionApiRetry.withApiRetryRuntime` 的快照驱动子集。
    fn apply_retry_state(&self, snapshot: &mut SessionStateSnapshot) {
        if snapshot.session.session_id.is_empty() {
            return;
        }
        let key = format!(
            "{}\0{}",
            snapshot.session.workspace.workspace_key, snapshot.session.session_id
        );
        let terminal = matches!(
            snapshot.session.status,
            SessionStatus::Completed | SessionStatus::Error
        );
        let mut state = self
            .retry_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let stored = state.get(&key).cloned();
        let (entry, next) = merge_retry_state(stored, &snapshot.runtime.api_retry, terminal);
        if let Some(entry) = entry {
            state.insert(key, entry);
        }
        snapshot.runtime.api_retry = next;
    }

    async fn repair_imported_claude_session(
        &self,
        target: &AgentWorkspaceTarget,
        task_id: &str,
        snapshot: SessionStateSnapshot,
    ) -> SessionStateSnapshot {
        if !should_repair_snapshot(&snapshot) {
            return snapshot;
        }
        let data_root = self.data_root.clone();
        let workspace_path = target.workspace_path.clone();
        let workspace_identity = target.workspace_identity.clone();
        let task = task_id.to_string();
        let history = match tokio::task::spawn_blocking(move || {
            resolve_history(
                &data_root,
                &workspace_path,
                workspace_identity.as_deref(),
                &task,
            )
        })
        .await
        {
            Ok(Ok(Some(history))) => history,
            Ok(Ok(None)) => return snapshot,
            Ok(Err(error)) => {
                self.log(format!(
                    "mode-session repair resolve failed task={task_id}: {error}"
                ));
                return snapshot;
            }
            Err(error) => {
                self.log(format!(
                    "mode-session repair task failed task={task_id}: {error}"
                ));
                return snapshot;
            }
        };
        let options = CreateSessionOptions {
            session_id: Some(task_id.to_string()),
            parent_session_id: None,
            mode: Some(snapshot.session.mode),
            model: snapshot.settings.model.current.clone(),
            persistence: Some(SessionPersistence::Immediate),
            thought_level: snapshot.settings.thought_level.current.clone(),
            title_generation_enabled: Some(true),
            mcp_servers: None,
            tool_allowlist: None,
            tool_denylist: None,
            imported_history: Some(ImportHistory::ClaudeCode(ClaudeCodeImport {
                source: ClaudeCodeSource::ClaudeCode,
                title: history.title.clone(),
                created_at: history.created_at.map(|value| value.max(0) as u64),
                updated_at: history.updated_at.map(|value| value.max(0) as u64),
                messages: history.messages.clone(),
            })),
            off_peak_tool_enabled: false,
            dynamic_workflow_enabled: false,
        };
        match self.control.create_session(target, options).await {
            Ok(repaired) => repaired,
            Err(error) => {
                self.log(format!(
                    "mode-session repair createSession failed task={task_id}: {error}"
                ));
                snapshot
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
    #[serde(default)]
    remote_session_id: Option<String>,
}

impl WorkspaceParams {
    fn target(&self) -> AgentWorkspaceTarget {
        AgentWorkspaceTarget {
            workspace_path: self.workspace_path.clone(),
            workspace_identity: self.workspace_identity.clone(),
            remote_session_id: self.remote_session_id.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    delivery_kind: Option<String>,
    #[serde(default)]
    message_limit: Option<u64>,
    #[serde(default)]
    after_seq: Option<u64>,
}

fn parse_params<T: DeserializeOwned>(command: &str, arg: Value) -> Result<T, ChannelError> {
    serde_json::from_value(arg.to_json())
        .map_err(|error| ChannelError::error(format!("invalid {command} params: {error}")))
}

fn delivery_from(value: Option<&str>) -> Result<Option<DeliveryKind>, ChannelError> {
    match value {
        None => Ok(None),
        Some("desktop-continuous") => Ok(Some(DeliveryKind::DesktopContinuous)),
        Some("web-remote-replayable") => Ok(Some(DeliveryKind::WebRemoteReplayable)),
        Some(other) => Err(ChannelError::error(format!(
            "invalid deliveryKind: {other}"
        ))),
    }
}

fn json_value<T: serde::Serialize>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_string(value)
        .map(Value::Json)
        .map_err(|error| ChannelError::error(error.to_string()))
}

/// 返回 `(entry 更新, 快照值)`；`entry == None` 表示无需写入暂存。
fn merge_retry_state(
    stored: Option<Option<ApiRetryStatus>>,
    snapshot_value: &Option<Option<ApiRetryStatus>>,
    terminal: bool,
) -> (
    Option<Option<ApiRetryStatus>>,
    Option<Option<ApiRetryStatus>>,
) {
    if snapshot_value.is_some() {
        return (snapshot_value.clone(), snapshot_value.clone());
    }
    if terminal {
        return (Some(None), Some(None));
    }
    match stored {
        Some(value) => (None, Some(value)),
        None => (None, None),
    }
}

fn should_repair_snapshot(snapshot: &SessionStateSnapshot) -> bool {
    let messages: Vec<(String, bool)> = snapshot
        .messages
        .iter()
        .map(|message| {
            (
                message_id(&message.info).to_string(),
                is_assistant(&message.info),
            )
        })
        .collect();
    should_repair_parts(
        &snapshot.session.session_id,
        snapshot.session.status,
        snapshot.runtime.active_turn_id.is_some(),
        &messages,
    )
}

fn should_repair_parts(
    session_id: &str,
    status: SessionStatus,
    active_turn: bool,
    messages: &[(String, bool)],
) -> bool {
    if status == SessionStatus::Running || active_turn {
        return false;
    }
    let has_legacy_ids = messages
        .iter()
        .any(|(message_id, _)| is_legacy_import_message_id(message_id));
    if !session_id.starts_with("claude-import-") && !has_legacy_ids {
        return false;
    }
    let has_assistant = messages.iter().any(|(_, assistant)| *assistant);
    if !has_assistant {
        return true;
    }
    if messages.first().is_some_and(|(_, assistant)| *assistant) {
        return true;
    }
    has_legacy_ids
}

fn is_legacy_import_message_id(message_id: &str) -> bool {
    let Some(suffix) = message_id.strip_prefix("msg_import_") else {
        return false;
    };
    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

fn message_id(info: &MessageInfo) -> &str {
    match info {
        MessageInfo::User(info) => &info.message_id,
        MessageInfo::Assistant(info) => &info.message_id,
    }
}

fn is_assistant(info: &MessageInfo) -> bool {
    matches!(info, MessageInfo::Assistant(_))
}

#[derive(Debug, Clone)]
struct RepairHistory {
    title: Option<String>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
    messages: Vec<ImportMessage>,
    assistant_count: usize,
}

fn workspace_key(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    workspace_identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(workspace_path)
        .to_string()
}

fn workspace_hash(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(workspace_key(workspace_path, workspace_identity).as_bytes());
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn legacy_snapshot_paths(
    data_root: &Path,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_id: &str,
) -> Vec<PathBuf> {
    let dir = data_root
        .join("v2")
        .join("sessions")
        .join(workspace_hash(workspace_path, workspace_identity));
    vec![
        dir.join(format!("{task_id}.json")),
        dir.join(format!("{task_id}.deleted.json")),
    ]
}

fn resolve_history(
    data_root: &Path,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_id: &str,
) -> Result<Option<RepairHistory>, String> {
    let legacy = read_legacy_history(data_root, workspace_path, workspace_identity, task_id)?;
    let native = read_native_history(workspace_path, task_id)?;
    Ok(pick_history(legacy, native))
}

fn pick_history(
    legacy: Option<RepairHistory>,
    native: Option<RepairHistory>,
) -> Option<RepairHistory> {
    match (legacy, native) {
        (None, native) => native,
        (legacy, None) => legacy,
        (Some(legacy), Some(native)) => {
            // 旧版可能已把 user-only 的 legacy 备份写坏：assistant 更多者胜。
            if native.assistant_count >= legacy.assistant_count {
                Some(native)
            } else {
                Some(legacy)
            }
        }
    }
}

fn read_legacy_history(
    data_root: &Path,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_id: &str,
) -> Result<Option<RepairHistory>, String> {
    let mut raw = None;
    for path in legacy_snapshot_paths(data_root, workspace_path, workspace_identity, task_id) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                raw = Some(text);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Ok(file) = serde_json::from_str::<Json>(&raw) else {
        return Ok(None);
    };
    if file.pointer("/meta/migrationSource").and_then(Json::as_str) != Some("claudeCode") {
        return Ok(None);
    }
    let messages = file
        .get("messages")
        .and_then(Json::as_array)
        .map(|messages| import_from_legacy(messages.iter().filter_map(legacy_message)))
        .unwrap_or_default();
    if messages.is_empty() {
        return Ok(None);
    }
    let meta = file.get("meta");
    Ok(Some(RepairHistory {
        title: meta
            .and_then(|meta| meta.get("title"))
            .and_then(Json::as_str)
            .map(str::to_string),
        created_at: meta
            .and_then(|meta| meta.get("createdAt"))
            .and_then(Json::as_i64),
        updated_at: meta
            .and_then(|meta| meta.get("updatedAt"))
            .and_then(Json::as_i64),
        assistant_count: count_assistant(&messages),
        messages,
    }))
}

fn legacy_message(value: &Json) -> Option<(String, String, Option<i64>)> {
    let role = value.get("role")?.as_str()?;
    let content = value.get("content")?.as_str()?.to_string();
    let timestamp = value.get("timestamp").and_then(Json::as_i64);
    Some((role.to_string(), content, timestamp))
}

fn import_from_legacy(
    messages: impl Iterator<Item = (String, String, Option<i64>)>,
) -> Vec<ImportMessage> {
    messages
        .filter_map(|(role, content, timestamp)| {
            let role = match role.as_str() {
                "user" => ImportMessageRole::User,
                "assistant" => ImportMessageRole::Assistant,
                _ => return None,
            };
            Some(ImportMessage {
                role,
                content,
                timestamp: timestamp.map(|value| value.max(0) as u64),
            })
        })
        .collect()
}

fn read_native_history(
    workspace_path: &str,
    task_id: &str,
) -> Result<Option<RepairHistory>, String> {
    let candidates =
        crate::claude_import::scan_importable_sessions(&crate::claude_import::ScanParams {
            workspace_path: Some(workspace_path.to_string()),
            modified_since: None,
            limit: None,
        });
    let candidate = candidates.iter().find(|candidate| {
        let (Some(workspace), Some(session_id)) = (
            candidate.get("workspacePath").and_then(Json::as_str),
            candidate.get("sessionId").and_then(Json::as_str),
        ) else {
            return false;
        };
        crate::claude_import_persist::build_imported_task_id(workspace, session_id) == task_id
    });
    let Some(candidate) = candidate else {
        return Ok(None);
    };
    let Some(source_path) = candidate.get("sourcePath").and_then(Json::as_str) else {
        return Ok(None);
    };
    let source_workspace = candidate
        .get("workspacePath")
        .and_then(Json::as_str)
        .unwrap_or(workspace_path);
    let session_id = candidate
        .get("sessionId")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let source = crate::claude_import_persist::parse_session_file(
        Path::new(source_path),
        source_workspace,
        session_id,
        None,
        candidate.get("createdAt").and_then(Json::as_i64),
        candidate.get("updatedAt").and_then(Json::as_i64),
    )?;
    let messages: Vec<ImportMessage> = import_from_legacy(source.messages.iter().map(|message| {
        (
            message.role.clone(),
            message.content.clone(),
            Some(message.timestamp),
        )
    }));
    if messages.is_empty() {
        return Ok(None);
    }
    Ok(Some(RepairHistory {
        title: source.title.clone(),
        created_at: Some(source.created_at),
        updated_at: Some(source.updated_at),
        assistant_count: count_assistant(&messages),
        messages,
    }))
}

fn count_assistant(messages: &[ImportMessage]) -> usize {
    messages
        .iter()
        .filter(|message| message.role == ImportMessageRole::Assistant)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry(attempt: u64) -> ApiRetryStatus {
        ApiRetryStatus {
            kind: agent_protocol::snapshot::ApiRetryKind::ApiRetry,
            attempt,
            max_retries: 3,
            retry_delay_ms: 1000,
            error_status: Some(503),
            error: "busy".to_string(),
        }
    }

    #[test]
    fn retry_state_merges_snapshot_values() {
        let status = retry(1);
        // 快照自带值：写入暂存并原样返回。
        let (entry, next) = merge_retry_state(None, &Some(Some(status.clone())), false);
        assert_eq!(entry, Some(Some(status.clone())));
        assert_eq!(next, Some(Some(status.clone())));
        // 快照缺字段、暂存有值：补回。
        let (entry, next) = merge_retry_state(Some(Some(status.clone())), &None, false);
        assert_eq!(entry, None);
        assert_eq!(next, Some(Some(status)));
        // 终态：清空暂存并补 null。
        let (entry, next) = merge_retry_state(Some(Some(retry(2))), &None, true);
        assert_eq!(entry, Some(None));
        assert_eq!(next, Some(None));
        // 无暂存：保持缺省。
        let (entry, next) = merge_retry_state(None, &None, false);
        assert_eq!(entry, None);
        assert_eq!(next, None);
    }

    #[test]
    fn repair_conditions_match_ts() {
        let msg = |id: &str, assistant: bool| (id.to_string(), assistant);
        assert!(!should_repair_parts(
            "task-1",
            SessionStatus::Idle,
            false,
            &[msg("msg_1", false), msg("msg_2", true)]
        ));
        assert!(!should_repair_parts(
            "claude-import-x",
            SessionStatus::Running,
            false,
            &[msg("msg_1", false)]
        ));
        assert!(should_repair_parts(
            "claude-import-x",
            SessionStatus::Idle,
            false,
            &[msg("msg_1", false)]
        ));
        assert!(should_repair_parts(
            "claude-import-x",
            SessionStatus::Idle,
            false,
            &[msg("msg_1", true)]
        ));
        assert!(should_repair_parts(
            "task-1",
            SessionStatus::Idle,
            false,
            &[msg("msg_import_0", false), msg("msg_import_1", true)]
        ));
        assert!(!should_repair_parts(
            "task-1",
            SessionStatus::Idle,
            false,
            &[msg("msg_1", false), msg("msg_2", true)]
        ));
    }

    #[test]
    fn legacy_history_reads_migration_source() {
        let dir = std::env::temp_dir().join(format!(
            "mode-session-legacy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        let path = legacy_snapshot_paths(&dir, "/work/a", None, "task-1")
            .into_iter()
            .next()
            .expect("path");
        std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        std::fs::write(
            &path,
            serde_json::json!({
                "meta": {
                    "migrationSource": "claudeCode",
                    "title": "旧会话",
                    "createdAt": 10,
                    "updatedAt": 20
                },
                "messages": [
                    {"role": "user", "content": "hi", "timestamp": 11},
                    {"role": "assistant", "content": "hello", "timestamp": 12}
                ]
            })
            .to_string(),
        )
        .expect("write");

        let history = read_legacy_history(&dir, "/work/a", None, "task-1")
            .expect("read")
            .expect("history");
        assert_eq!(history.title.as_deref(), Some("旧会话"));
        assert_eq!(history.messages.len(), 2);
        assert_eq!(history.assistant_count, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_pick_prefers_more_assistant() {
        let history = |count: usize| RepairHistory {
            title: None,
            created_at: None,
            updated_at: None,
            messages: Vec::new(),
            assistant_count: count,
        };
        assert!(pick_history(Some(history(0)), None).is_some());
        assert_eq!(
            pick_history(Some(history(0)), Some(history(1)))
                .unwrap()
                .assistant_count,
            1
        );
        assert_eq!(
            pick_history(Some(history(2)), Some(history(1)))
                .unwrap()
                .assistant_count,
            2
        );
    }
}
