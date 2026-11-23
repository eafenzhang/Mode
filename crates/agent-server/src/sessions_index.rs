//! sessions-index live 增量最小闭环：store 行变化 → 广播 → 订阅任务下发 delta。
//!
//! 当前 Runtime 的 sessions-index 快照仍是 store 记录的轻量投影（phase 等字段为
//! 缺省值）；本模块先把「新增/改名/删除」这些显式列表变化接成 live upsert/remove，
//! 会话内 phase/预览/后台工作的投影在后续波次补。协议形状见
//! `agent_protocol::v4_sessions_index`。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_protocol::v4_sessions_index::{SessionSummary, SessionsIndexDelta};
use serde_json::Number;
use session_store::SessionRecord;
use tokio::sync::broadcast;

const SESSIONS_INDEX_BROADCAST_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct SessionsIndexEvent {
    pub workspace_id: String,
    pub delta: SessionsIndexDelta,
}

pub struct SessionsIndexHub {
    sender: broadcast::Sender<SessionsIndexEvent>,
    /// workspaceId → sessionId → summary（conflated 最新态）。
    workspaces: Mutex<HashMap<String, HashMap<String, SessionSummary>>>,
}

impl Default for SessionsIndexHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionsIndexHub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(SESSIONS_INDEX_BROADCAST_CAPACITY);
        Self {
            sender,
            workspaces: Mutex::new(HashMap::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionsIndexEvent> {
        self.sender.subscribe()
    }

    /// 首次订阅时用 store 行补齐缺失摘要；已有 live 状态不被冷默认值覆盖。
    pub fn ensure_workspace(&self, workspace_id: &str, summaries: &[SessionSummary]) {
        if workspace_id.is_empty() {
            return;
        }
        let mut workspaces = self.lock();
        let sessions = workspaces.entry(workspace_id.to_string()).or_default();
        for summary in summaries {
            sessions
                .entry(summary.session_id.clone())
                .or_insert_with(|| summary.clone());
        }
    }

    /// 订阅落后/重同步：用 store 最新行整体替换该 workspace 的投影（会丢弃已删行）。
    pub fn replace_workspace(&self, workspace_id: &str, summaries: &[SessionSummary]) {
        if workspace_id.is_empty() {
            return;
        }
        let mut workspaces = self.lock();
        workspaces.insert(
            workspace_id.to_string(),
            summaries
                .iter()
                .cloned()
                .map(|summary| (summary.session_id.clone(), summary))
                .collect(),
        );
    }

    pub fn snapshot(&self, workspace_id: &str) -> Vec<SessionSummary> {
        self.lock()
            .get(workspace_id)
            .map(|sessions| sessions.values().cloned().collect())
            .unwrap_or_default()
    }

    /// 从运行时直接 upsert 一条 summary（不需要 store 行）。
    /// 等价摘要不产帧；summary 已包含 phase/goalStatus/title 等真实值。
    pub fn upsert_summary(&self, summary: SessionSummary) {
        let mut workspaces = self.lock();
        let sessions = workspaces.entry(summary.workspace_id.clone()).or_default();
        if sessions.get(&summary.session_id) == Some(&summary) {
            return;
        }
        sessions.insert(summary.session_id.clone(), summary.clone());
        drop(workspaces);
        let _ = self.sender.send(SessionsIndexEvent {
            workspace_id: summary.workspace_id.clone(),
            delta: SessionsIndexDelta::SessionUpserted {
                session: Box::new(summary),
            },
        });
    }

    /// 行变化 → 对每个可能的 workspace 主题 upsert（等价摘要不产帧）。
    ///
    /// **store 侧路径**：store 行不含后台任务状态，所以这里**保留**已有摘要里的
    /// `has_background_work`（新条目按 `false`）。用 `false` 覆盖会把正在跑的
    /// 后台任务在列表里抹掉——store 不知道 ≠ 确认没有。
    ///
    /// 主题 key 既可能是 workspaceIdentity（远程），也可能是 workspacePath（本地）；
    /// `list_sessions` 按 directory 过滤，所以这里两者都当作候选，并补上已经包含
    /// 该会话的既有主题。
    pub fn upsert_from_store(&self, record: &SessionRecord) -> Vec<SessionsIndexEvent> {
        self.write_record(record, None)
    }

    /// 行变化 → upsert，且**明确给出**后台任务事实（运行时路径）。
    pub fn upsert(
        &self,
        record: &SessionRecord,
        has_background_work: bool,
    ) -> Vec<SessionsIndexEvent> {
        self.write_record(record, Some(has_background_work))
    }

    fn write_record(
        &self,
        record: &SessionRecord,
        has_background_work: Option<bool>,
    ) -> Vec<SessionsIndexEvent> {
        let mut workspaces = self.lock();
        let keys = workspace_keys(&workspaces, record);
        let mut events = Vec::new();
        for workspace_id in keys {
            let sessions = workspaces.entry(workspace_id.clone()).or_default();
            // store 路径（None）沿用该会话已有的布尔；新条目按 false 起步。
            let known = has_background_work.unwrap_or_else(|| {
                sessions
                    .get(&record.id)
                    .map(|existing| existing.has_background_work)
                    .unwrap_or(false)
            });
            let mut summary = summary_from_record(record, known);
            summary.workspace_id = workspace_id.clone();
            if sessions.get(&summary.session_id) == Some(&summary) {
                continue;
            }
            sessions.insert(summary.session_id.clone(), summary.clone());
            let event = SessionsIndexEvent {
                workspace_id,
                delta: SessionsIndexDelta::SessionUpserted {
                    session: Box::new(summary),
                },
            };
            let _ = self.sender.send(event.clone());
            events.push(event);
        }
        events
    }

    /// 删除行：从所有包含该会话的主题里移除并广播（返回产出的 delta）。
    pub fn remove_record(&self, record: &SessionRecord) -> Vec<SessionsIndexEvent> {
        let mut workspaces = self.lock();
        let keys: Vec<String> = workspaces
            .iter()
            .filter(|(_, sessions)| sessions.contains_key(&record.id))
            .map(|(workspace_id, _)| workspace_id.clone())
            .collect();
        let mut events = Vec::new();
        for workspace_id in keys {
            let Some(sessions) = workspaces.get_mut(&workspace_id) else {
                continue;
            };
            sessions.remove(&record.id);
            let event = SessionsIndexEvent {
                workspace_id,
                delta: SessionsIndexDelta::SessionRemoved {
                    session_id: record.id.clone(),
                },
            };
            let _ = self.sender.send(event.clone());
            events.push(event);
        }
        events
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, HashMap<String, SessionSummary>>> {
        self.workspaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn workspace_keys(
    workspaces: &HashMap<String, HashMap<String, SessionSummary>>,
    record: &SessionRecord,
) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(identity) = record
        .workspace_id
        .clone()
        .filter(|value| !value.is_empty())
    {
        keys.push(identity);
    }
    if !record.directory.is_empty() && !keys.contains(&record.directory) {
        keys.push(record.directory.clone());
    }
    for (workspace_id, sessions) in workspaces {
        if sessions.contains_key(&record.id) && !keys.contains(workspace_id) {
            keys.push(workspace_id.clone());
        }
    }
    keys
}

pub type SharedSessionsIndexHub = Arc<SessionsIndexHub>;

/// store 行 → 侧栏摘要。与迁移前的本地映射保持一致（phase 缺省 Draft）。
///
/// `has_background_work` 由调用方给出：store 行**不含**后台任务状态，它是运行时
/// 事实（`TaskRegistry`）。传 `false` 是「不知道」，不是「确认没有」——所以
/// 有运行时的调用方必须自己查一次再传。
pub fn summary_from_record(record: &SessionRecord, has_background_work: bool) -> SessionSummary {
    SessionSummary {
        session_id: record.id.clone(),
        workspace_id: record.workspace_id.clone().unwrap_or_default(),
        parent_session_id: record.parent_id.clone(),
        title: record.title.clone(),
        title_source: None,
        phase: agent_protocol::v4_snapshot::SessionPhase::Draft,
        session_ended: record.time_archived.is_some(),
        has_background_work,
        workflow_activity: None,
        pending_interaction: None,
        pending_interaction_summary: None,
        goal_status: None,
        last_activity_at: Number::from(record.time_updated.max(0)),
        last_assistant_preview: None,
        created_at: Number::from(record.time_created.max(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, title: &str, workspace: Option<&str>) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            project_id: "project".to_string(),
            workspace_id: workspace.map(str::to_string),
            parent_id: None,
            slug: id.to_string(),
            directory: "/ws".to_string(),
            path: Some("/ws".to_string()),
            title: title.to_string(),
            version: "0.0.0".to_string(),
            share_url: None,
            summary_additions: None,
            summary_deletions: None,
            summary_files: None,
            summary_diffs: None,
            revert: None,
            permission: None,
            time_created: 1,
            time_updated: 2,
            time_compacting: None,
            time_archived: None,
            task_type: "interactive".to_string(),
            title_source: "custom".to_string(),
            title_message_id: None,
            time_title_updated: None,
            trace_id: None,
        }
    }

    #[test]
    fn store_upsert_keeps_the_existing_background_work_flag() {
        // store 行不含后台任务状态：upsert_from_store 必须保留已有摘要里的布尔，
        // 否则一次改名就会把正在跑的后台任务从侧栏抹掉。
        let hub = SessionsIndexHub::new();
        let mut summary = summary_from_record(&record("a", "标题", Some("ws")), true);
        summary.workspace_id = "ws".to_string();
        hub.upsert_summary(summary);

        let events = hub.upsert_from_store(&record("a", "新标题", Some("ws")));
        assert!(!events.is_empty(), "改名要产帧");
        let stored = &hub.snapshot("ws")[0];
        assert_eq!(stored.title, "新标题");
        assert!(
            stored.has_background_work,
            "store 路径不得把已有 true 覆盖成 false"
        );
    }

    #[test]
    fn runtime_upsert_can_clear_the_background_work_flag() {
        // 运行时路径**明确给出**事实：任务跑完后必须能把 true 落回 false。
        let hub = SessionsIndexHub::new();
        let mut summary = summary_from_record(&record("a", "标题", Some("ws")), true);
        summary.workspace_id = "ws".to_string();
        hub.upsert_summary(summary);

        let events = hub.upsert(&record("a", "标题", Some("ws")), false);
        assert!(!events.is_empty(), "布尔变化要产帧");
        assert!(!hub.snapshot("ws")[0].has_background_work);
    }

    #[test]
    fn upsert_is_conflated_and_remove_emits_once() {
        let hub = SessionsIndexHub::new();
        let first = hub.upsert_from_store(&record("a", "标题", Some("ws")));
        assert_eq!(first.len(), 2, "identity + directory 两个主题");
        assert!(matches!(
            first[0].delta,
            SessionsIndexDelta::SessionUpserted { .. }
        ));
        assert!(hub
            .upsert_from_store(&record("a", "标题", Some("ws")))
            .is_empty());
        assert_eq!(hub.snapshot("ws").len(), 1);
        let removed = hub.remove_record(&record("a", "标题", Some("ws")));
        assert_eq!(removed.len(), 2);
        assert!(matches!(
            removed[0].delta,
            SessionsIndexDelta::SessionRemoved { .. }
        ));
        assert!(hub
            .remove_record(&record("a", "标题", Some("ws")))
            .is_empty());
        assert!(hub.snapshot("ws").is_empty());
    }

    #[test]
    fn directory_is_used_as_workspace_key_when_identity_missing() {
        let hub = SessionsIndexHub::new();
        let events = hub.upsert_from_store(&record("a", "标题", None));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].workspace_id, "/ws");
        assert!(hub
            .upsert_from_store(&record("a", "标题", Some("")))
            .is_empty());
    }

    #[test]
    fn rows_without_identity_or_directory_are_skipped() {
        let hub = SessionsIndexHub::new();
        let mut row = record("a", "标题", None);
        row.directory.clear();
        assert!(hub.upsert_from_store(&row).is_empty());
    }

    #[test]
    fn ensure_workspace_does_not_overwrite_live_state() {
        let hub = SessionsIndexHub::new();
        hub.upsert_from_store(&record("a", "live", Some("ws")));
        let stale = summary_from_record(&record("a", "stale", Some("ws")), false);
        hub.ensure_workspace("ws", &[stale]);
        assert_eq!(hub.snapshot("ws")[0].title, "live");
    }
}
