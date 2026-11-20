//! Agent 协议 → TUI 视图模型的纯归约（LT-8 A2 首片）。
//!
//! 输入是 `agent-protocol` 的 sessions-index 快照/增量；输出只更新 `TuiModel`。
//! 不持有连接、不读时钟：`now_ms` 由调用方注入，便于场景测试。

use agent_protocol::v4_sessions_index::{
    SessionSummary, SessionsIndexDelta, SessionsIndexSnapshot,
};
use agent_protocol::v4_snapshot::SessionPhase as WirePhase;

use crate::model::{SessionPhase, TuiModel, TuiSession};

pub fn apply_snapshot(model: &mut TuiModel, snapshot: &SessionsIndexSnapshot, now_ms: u64) {
    let selected_id = model.selected_session().map(|session| session.id.clone());
    let mut sessions: Vec<TuiSession> = snapshot
        .sessions
        .iter()
        .map(|summary| session_from_summary(summary, now_ms))
        .collect();
    sort_sessions(&mut sessions);
    model.sessions = sessions;
    restore_selection(model, selected_id.as_deref());
}

pub fn apply_delta(model: &mut TuiModel, delta: &SessionsIndexDelta, now_ms: u64) {
    match delta {
        SessionsIndexDelta::SessionUpserted { session } => {
            let selected_id = model.selected_session().map(|item| item.id.clone());
            let summary = session_from_summary(session, now_ms);
            match model.sessions.iter_mut().find(|item| item.id == summary.id) {
                Some(existing) => *existing = summary,
                None => model.sessions.push(summary),
            }
            sort_sessions(&mut model.sessions);
            restore_selection(model, selected_id.as_deref());
        }
        SessionsIndexDelta::SessionRemoved { session_id } => {
            let selected_id = model.selected_session().map(|item| item.id.clone());
            model.sessions.retain(|item| &item.id != session_id);
            let fallback = if selected_id.as_deref() == Some(session_id.as_str()) {
                None
            } else {
                selected_id
            };
            restore_selection(model, fallback.as_deref());
        }
    }
}

pub fn session_from_summary(summary: &SessionSummary, now_ms: u64) -> TuiSession {
    let title = summary.title.trim();
    TuiSession {
        id: summary.session_id.clone(),
        title: if title.is_empty() {
            "未命名任务".to_string()
        } else {
            title.to_string()
        },
        phase: phase_from_wire(summary.phase),
        last_activity: format_relative_time(summary.last_activity_at.as_u64().unwrap_or(0), now_ms),
        last_activity_ms: summary.last_activity_at.as_u64().unwrap_or(0),
    }
}

pub fn phase_from_wire(phase: WirePhase) -> SessionPhase {
    match phase {
        WirePhase::Draft => SessionPhase::Draft,
        // prewarming 是启动中的运行态：TUI 也按 running 展示，避免侧栏出现额外状态。
        WirePhase::Prewarming | WirePhase::Running => SessionPhase::Running,
        WirePhase::CompletedSuccess => SessionPhase::Done,
        WirePhase::CompletedInterrupted => SessionPhase::Interrupted,
        WirePhase::Error => SessionPhase::Failed,
    }
}

/// 相对时间：刚刚 / N 分钟 / N 小时 / N 天；未知时间（0）返回空串。
pub fn format_relative_time(value_ms: u64, now_ms: u64) -> String {
    if value_ms == 0 {
        return String::new();
    }
    let delta_ms = now_ms.saturating_sub(value_ms);
    let minutes = delta_ms / 60_000;
    if minutes < 1 {
        return "刚刚".to_string();
    }
    if minutes < 60 {
        return format!("{minutes} 分钟");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} 小时");
    }
    format!("{} 天", hours / 24)
}

fn sort_sessions(sessions: &mut [TuiSession]) {
    sessions.sort_by(|left, right| {
        right
            .last_activity_ms
            .cmp(&left.last_activity_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn restore_selection(model: &mut TuiModel, selected_id: Option<&str>) {
    if model.sessions.is_empty() {
        model.selected = 0;
        return;
    }
    model.selected = selected_id
        .and_then(|id| model.sessions.iter().position(|item| item.id == id))
        .unwrap_or(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::v4_sessions_index::{
        SessionSummary, SessionsIndexDelta, SessionsIndexSnapshot,
    };
    use serde_json::Number;

    fn summary(id: &str, title: &str, phase: WirePhase, updated: u64) -> SessionSummary {
        SessionSummary {
            session_id: id.to_string(),
            workspace_id: "ws".to_string(),
            parent_session_id: None,
            title: title.to_string(),
            title_source: None,
            phase,
            session_ended: false,
            has_background_work: false,
            workflow_activity: None,
            pending_interaction: None,
            pending_interaction_summary: None,
            goal_status: None,
            last_activity_at: Number::from(updated),
            last_assistant_preview: None,
            created_at: Number::from(1),
        }
    }

    const NOW: u64 = 1_000_000_000;

    #[test]
    fn snapshot_maps_titles_phases_and_sorts_by_activity() {
        let mut model = TuiModel::new("ws");
        let snapshot = SessionsIndexSnapshot {
            protocol_version: 1,
            workspace_id: "ws".to_string(),
            log_epoch: "e".to_string(),
            sessions: vec![
                summary(
                    "old",
                    "旧任务",
                    WirePhase::CompletedSuccess,
                    NOW - 3 * 86_400_000,
                ),
                summary("new", "  ", WirePhase::Running, NOW - 30_000),
                summary(
                    "mid",
                    "中断",
                    WirePhase::CompletedInterrupted,
                    NOW - 2 * 3_600_000,
                ),
            ],
        };
        apply_snapshot(&mut model, &snapshot, NOW);
        assert_eq!(
            model
                .sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "mid", "old"]
        );
        assert_eq!(model.sessions[0].title, "未命名任务");
        assert_eq!(model.sessions[0].phase, SessionPhase::Running);
        assert_eq!(model.sessions[0].last_activity, "刚刚");
        assert_eq!(model.sessions[1].phase, SessionPhase::Interrupted);
        assert_eq!(model.sessions[1].last_activity, "2 小时");
        assert_eq!(model.sessions[2].last_activity, "3 天");
    }

    #[test]
    fn delta_upserts_updates_inserts_and_preserves_selection_by_id() {
        let mut model = TuiModel::with_sessions(
            "ws",
            vec![
                TuiSession::new("a", "A", SessionPhase::Done, "5 分钟")
                    .with_last_activity_ms(NOW - 300_000),
                TuiSession::new("b", "B", SessionPhase::Draft, "2 分钟")
                    .with_last_activity_ms(NOW - 120_000),
            ],
        );
        model.selected = 1; // b
        apply_delta(
            &mut model,
            &SessionsIndexDelta::SessionUpserted {
                session: Box::new(summary("a", "A2", WirePhase::Running, NOW - 60_000)),
            },
            NOW,
        );
        assert_eq!(model.selected, 1, "选中项按 id 保持");
        let a = model
            .sessions
            .iter()
            .find(|session| session.id == "a")
            .expect("a");
        assert_eq!(a.title, "A2");
        assert_eq!(a.phase, SessionPhase::Running);
        assert_eq!(a.last_activity, "1 分钟");

        apply_delta(
            &mut model,
            &SessionsIndexDelta::SessionUpserted {
                session: Box::new(summary("c", "C", WirePhase::Draft, NOW)),
            },
            NOW,
        );
        assert_eq!(model.sessions[0].id, "c");
        assert_eq!(model.selected, 2, "新插入不影响原选中 id");
    }

    #[test]
    fn remove_clamps_selection_and_unknown_time_is_blank() {
        let mut model = TuiModel::with_sessions(
            "ws",
            vec![
                TuiSession::new("a", "A", SessionPhase::Done, ""),
                TuiSession::new("b", "B", SessionPhase::Done, ""),
            ],
        );
        model.selected = 1;
        apply_delta(
            &mut model,
            &SessionsIndexDelta::SessionRemoved {
                session_id: "b".to_string(),
            },
            NOW,
        );
        assert_eq!(model.sessions.len(), 1);
        assert_eq!(model.selected, 0);
        assert_eq!(format_relative_time(0, NOW), "");
    }
}
