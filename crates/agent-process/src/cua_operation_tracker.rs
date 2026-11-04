//! CUA operation turn 跟踪：从 `computer-use/operation-event` sideband 维护
//! 「是否仍有 Computer Use 工具在执行」。
//!
//! 真源：`packages/services/src/mode-agent/cuaOperationTurnTracker.ts`。
//! 只保留判定状态机；Windows 顶部提示条 / macOS PiP reporter 属桌面投影，不在本层。

use std::collections::{HashMap, HashSet, VecDeque};

use agent_protocol::cua_operation_event::CuaOperationEvent;

use crate::WorkspaceTarget;

const MAX_RETIRED_TURN_KEYS: usize = 2_048;
const MAX_SEEN_OPERATION_EVENT_IDS: usize = 8_192;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveTurnRecord {
    workspace_key: String,
    session_key: String,
    turn_key: String,
    session_id: String,
    turn_id: String,
}

#[derive(Default)]
pub struct CuaOperationTurnTracker {
    current_turn_by_session: HashMap<String, String>,
    last_seq_by_session: HashMap<String, u64>,
    computer_use_scheduled_calls: HashSet<String>,
    active_turns: HashMap<String, ActiveTurnRecord>,
    retired_turn_keys: HashSet<String>,
    retired_turn_key_order: VecDeque<String>,
    seen_event_ids: HashSet<String>,
    seen_event_id_order: VecDeque<String>,
}

fn session_key_for(workspace_key: &str, session_id: &str) -> String {
    format!("{workspace_key}\u{0}{session_id}")
}

fn turn_key_for(session_key: &str, turn_id: &str) -> String {
    format!("{session_key}\u{0}{turn_id}")
}

fn tool_key_for(turn_key: &str, tool_call_id: &str) -> String {
    format!("{turn_key}\u{0}{tool_call_id}")
}

impl CuaOperationTurnTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_active_turn(&self) -> bool {
        !self.active_turns.is_empty()
    }

    pub fn accept(&mut self, workspace: &WorkspaceTarget, event: &CuaOperationEvent) {
        let workspace_key = workspace.key();
        let base = event.base();
        let session_key = session_key_for(&workspace_key, &base.session_id);
        let event_key = format!("{workspace_key}\u{0}{}", base.event_id);
        if !self.seen_event_ids.insert(event_key.clone()) {
            return;
        }
        self.seen_event_id_order.push_back(event_key);
        while self.seen_event_id_order.len() > MAX_SEEN_OPERATION_EVENT_IDS {
            if let Some(removed) = self.seen_event_id_order.pop_front() {
                self.seen_event_ids.remove(&removed);
            }
        }

        let last_seq = self
            .last_seq_by_session
            .get(&session_key)
            .copied()
            .unwrap_or(0);
        if base.sequence_number < last_seq {
            return;
        }
        self.last_seq_by_session
            .insert(session_key.clone(), last_seq.max(base.sequence_number));

        match event {
            CuaOperationEvent::TurnStarted { turn_id, .. } => {
                let next_turn_key = turn_key_for(&session_key, turn_id);
                if self.retired_turn_keys.contains(&next_turn_key) {
                    return;
                }
                let previous_turn_id = self.current_turn_by_session.get(&session_key).cloned();
                if let Some(previous) = previous_turn_id {
                    if &previous != turn_id {
                        self.retire_turn(&turn_key_for(&session_key, &previous));
                        self.clear_session(&session_key);
                    }
                }
                self.current_turn_by_session
                    .insert(session_key, turn_id.clone());
            }
            CuaOperationEvent::SessionClosed { .. } => {
                if let Some(current) = self.current_turn_by_session.get(&session_key).cloned() {
                    self.retire_turn(&turn_key_for(&session_key, &current));
                }
                self.clear_session(&session_key);
                self.current_turn_by_session.remove(&session_key);
            }
            CuaOperationEvent::TurnCompleted { turn_id, .. }
            | CuaOperationEvent::TurnFailed { turn_id, .. } => {
                let turn_key = turn_key_for(&session_key, turn_id);
                self.retire_turn(&turn_key);
                self.clear_turn(&turn_key);
                if self.current_turn_by_session.get(&session_key) == Some(turn_id) {
                    self.current_turn_by_session.remove(&session_key);
                }
            }
            CuaOperationEvent::ToolScheduled {
                turn_id,
                tool_call_id,
                computer_use,
                ..
            } => {
                let Some(turn_id) = self.current_turn_id(&session_key, Some(turn_id)) else {
                    return;
                };
                let turn_key = turn_key_for(&session_key, &turn_id);
                if self.retired_turn_keys.contains(&turn_key) {
                    return;
                }
                if computer_use == &Some(true) {
                    self.computer_use_scheduled_calls
                        .insert(tool_key_for(&turn_key, tool_call_id));
                }
            }
            CuaOperationEvent::ToolStarted {
                turn_id,
                tool_call_id,
                ..
            } => {
                let Some(turn_id) = self.current_turn_id(&session_key, turn_id.as_deref()) else {
                    return;
                };
                let turn_key = turn_key_for(&session_key, &turn_id);
                if self.retired_turn_keys.contains(&turn_key) {
                    return;
                }
                let tool_key = tool_key_for(&turn_key, tool_call_id);
                if !self.computer_use_scheduled_calls.remove(&tool_key) {
                    return;
                }
                if self.active_turns.contains_key(&turn_key) {
                    // 同一 turn 内新 CUA cell：只刷新桌面浮层截止时间，不重复建窗口。
                    return;
                }
                self.active_turns.insert(
                    turn_key.clone(),
                    ActiveTurnRecord {
                        workspace_key,
                        session_key,
                        turn_key,
                        session_id: base.session_id.clone(),
                        turn_id,
                    },
                );
            }
        }
    }

    fn current_turn_id(&mut self, session_key: &str, event_turn_id: Option<&str>) -> Option<String> {
        let current = self.current_turn_by_session.get(session_key).cloned();
        let turn_id = event_turn_id.map(str::to_owned).or_else(|| current.clone())?;
        if let Some(current) = current {
            if current != turn_id {
                // 新 turn 已替换旧 turn 后，迟到的旧 tool event 不能复活已终止的提示。
                return None;
            }
        } else {
            self.current_turn_by_session
                .insert(session_key.to_owned(), turn_id.clone());
        }
        Some(turn_id)
    }

    fn retire_turn(&mut self, turn_key: &str) {
        if !self.retired_turn_keys.insert(turn_key.to_owned()) {
            return;
        }
        self.retired_turn_key_order.push_back(turn_key.to_owned());
        while self.retired_turn_key_order.len() > MAX_RETIRED_TURN_KEYS {
            if let Some(removed) = self.retired_turn_key_order.pop_front() {
                self.retired_turn_keys.remove(&removed);
            }
        }
    }

    fn clear_turn(&mut self, turn_key: &str) {
        self.active_turns.remove(turn_key);
        let tool_prefix = format!("{turn_key}\u{0}");
        self.computer_use_scheduled_calls
            .retain(|key| !key.starts_with(&tool_prefix));
    }

    fn clear_session(&mut self, session_key: &str) {
        let turn_keys: Vec<String> = self
            .active_turns
            .values()
            .filter(|record| record.session_key == session_key)
            .map(|record| record.turn_key.clone())
            .collect();
        for turn_key in turn_keys {
            self.clear_turn(&turn_key);
        }
        let session_prefix = format!("{session_key}\u{0}");
        self.computer_use_scheduled_calls
            .retain(|key| !key.starts_with(&session_prefix));
    }

    pub fn clear_workspace_key(&mut self, workspace_key: &str) {
        let turn_keys: Vec<String> = self
            .active_turns
            .values()
            .filter(|record| record.workspace_key == workspace_key)
            .map(|record| record.turn_key.clone())
            .collect();
        for turn_key in turn_keys {
            self.clear_turn(&turn_key);
        }
        let workspace_prefix = format!("{workspace_key}\u{0}");
        self.current_turn_by_session
            .retain(|key, _| !key.starts_with(&workspace_prefix));
        self.last_seq_by_session
            .retain(|key, _| !key.starts_with(&workspace_prefix));
        self.computer_use_scheduled_calls
            .retain(|key| !key.starts_with(&workspace_prefix));
        self.retired_turn_key_order
            .retain(|key| !key.starts_with(&workspace_prefix));
        self.retired_turn_keys
            .retain(|key| !key.starts_with(&workspace_prefix));
        self.seen_event_id_order
            .retain(|key| !key.starts_with(&workspace_prefix));
        self.seen_event_ids
            .retain(|key| !key.starts_with(&workspace_prefix));
    }

    pub fn clear_all(&mut self) {
        self.active_turns.clear();
        self.current_turn_by_session.clear();
        self.last_seq_by_session.clear();
        self.computer_use_scheduled_calls.clear();
        self.retired_turn_keys.clear();
        self.retired_turn_key_order.clear();
        self.seen_event_ids.clear();
        self.seen_event_id_order.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspace() -> WorkspaceTarget {
        WorkspaceTarget::new("D:/ws/cua")
    }

    fn event(value: serde_json::Value) -> CuaOperationEvent {
        serde_json::from_value(value).unwrap()
    }

    fn base(kind: serde_json::Value) -> serde_json::Value {
        let mut value = json!({
            "eventId": kind["eventId"],
            "sequenceNumber": kind["sequenceNumber"],
            "sessionId": "s1",
            "timestamp": 1700000000000_i64
        });
        let map = value.as_object_mut().unwrap();
        for (key, field) in kind.as_object().unwrap() {
            map.insert(key.clone(), field.clone());
        }
        value
    }

    #[test]
    fn tool_started_activates_only_when_scheduled_with_computer_use() {
        let mut tracker = CuaOperationTurnTracker::new();
        let target = workspace();
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "turn-started",
                "eventId": "e1",
                "sequenceNumber": 1,
                "turnId": "t1"
            }))),
        );
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "tool-scheduled",
                "eventId": "e2",
                "sequenceNumber": 2,
                "turnId": "t1",
                "toolCallId": "c1",
                "toolName": "computer"
            }))),
        );
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "tool-started",
                "eventId": "e3",
                "sequenceNumber": 3,
                "turnId": "t1",
                "toolCallId": "c1"
            }))),
        );
        assert!(!tracker.has_active_turn(), "未标记 computerUse 不激活");

        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "tool-scheduled",
                "eventId": "e4",
                "sequenceNumber": 4,
                "turnId": "t1",
                "toolCallId": "c2",
                "toolName": "computer",
                "computerUse": true
            }))),
        );
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "tool-started",
                "eventId": "e5",
                "sequenceNumber": 5,
                "turnId": "t1",
                "toolCallId": "c2"
            }))),
        );
        assert!(tracker.has_active_turn());
    }

    #[test]
    fn completed_clears_active_and_stale_sequences_are_ignored() {
        let mut tracker = CuaOperationTurnTracker::new();
        let target = workspace();
        for (id, seq, kind, extra) in [
            ("e1", 1, "turn-started", json!({"turnId": "t1"})),
            (
                "e2",
                2,
                "tool-scheduled",
                json!({"turnId": "t1", "toolCallId": "c1", "toolName": "computer", "computerUse": true}),
            ),
            (
                "e3",
                3,
                "tool-started",
                json!({"turnId": "t1", "toolCallId": "c1"}),
            ),
        ] {
            let mut value = json!({"kind": kind, "eventId": id, "sequenceNumber": seq});
            for (key, field) in extra.as_object().unwrap() {
                value[key] = field.clone();
            }
            tracker.accept(&target, &event(base(value)));
        }
        assert!(tracker.has_active_turn());

        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "turn-completed",
                "eventId": "e4",
                "sequenceNumber": 4,
                "turnId": "t1"
            }))),
        );
        assert!(!tracker.has_active_turn());

        // 迟到的旧序号 tool-started 不会复活。
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "tool-started",
                "eventId": "e5",
                "sequenceNumber": 2,
                "turnId": "t1",
                "toolCallId": "c1"
            }))),
        );
        assert!(!tracker.has_active_turn());
    }

    #[test]
    fn clear_workspace_removes_state() {
        let mut tracker = CuaOperationTurnTracker::new();
        let target = workspace();
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "turn-started",
                "eventId": "e1",
                "sequenceNumber": 1,
                "turnId": "t1"
            }))),
        );
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "tool-scheduled",
                "eventId": "e2",
                "sequenceNumber": 2,
                "turnId": "t1",
                "toolCallId": "c1",
                "toolName": "computer",
                "computerUse": true
            }))),
        );
        tracker.accept(
            &target,
            &event(base(json!({
                "kind": "tool-started",
                "eventId": "e3",
                "sequenceNumber": 3,
                "turnId": "t1",
                "toolCallId": "c1"
            }))),
        );
        assert!(tracker.has_active_turn());
        tracker.clear_workspace_key(&target.key());
        assert!(!tracker.has_active_turn());
    }
}
