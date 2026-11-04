//! `computer-use/operation-event` sideband 事件的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的
//! `modeComputerUseOperationEventSchema`。只镜像形状；CUA turn 跟踪的时序状态机在
//! `agent-process::cua_operation_tracker`。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CuaOperationEventBase {
    pub event_id: String,
    pub sequence_number: u64,
    pub session_id: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum CuaOperationEvent {
    #[serde(rename = "turn-started", rename_all = "camelCase")]
    TurnStarted {
        #[serde(flatten)]
        base: CuaOperationEventBase,
        turn_id: String,
    },
    #[serde(rename = "turn-completed", rename_all = "camelCase")]
    TurnCompleted {
        #[serde(flatten)]
        base: CuaOperationEventBase,
        turn_id: String,
    },
    #[serde(rename = "turn-failed", rename_all = "camelCase")]
    TurnFailed {
        #[serde(flatten)]
        base: CuaOperationEventBase,
        turn_id: String,
    },
    #[serde(rename = "tool-scheduled", rename_all = "camelCase")]
    ToolScheduled {
        #[serde(flatten)]
        base: CuaOperationEventBase,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
        /// `z.literal(true).optional()`：Some(false) 视为非法。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        computer_use: Option<bool>,
    },
    #[serde(rename = "tool-started", rename_all = "camelCase")]
    ToolStarted {
        #[serde(flatten)]
        base: CuaOperationEventBase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
    },
    #[serde(rename = "session-closed")]
    SessionClosed {
        #[serde(flatten)]
        base: CuaOperationEventBase,
    },
}

impl CuaOperationEvent {
    pub fn base(&self) -> &CuaOperationEventBase {
        match self {
            Self::TurnStarted { base, .. }
            | Self::TurnCompleted { base, .. }
            | Self::TurnFailed { base, .. }
            | Self::ToolScheduled { base, .. }
            | Self::ToolStarted { base, .. }
            | Self::SessionClosed { base } => base,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Self::ToolScheduled {
            computer_use: Some(false),
            ..
        } = self
        {
            return Err("tool-scheduled computerUse must be true when present".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_scheduled_round_trips_and_rejects_computer_use_false() {
        let value = serde_json::json!({
            "kind": "tool-scheduled",
            "eventId": "e1",
            "sequenceNumber": 3,
            "sessionId": "s1",
            "timestamp": 1700000000000_i64,
            "turnId": "t1",
            "toolCallId": "c1",
            "toolName": "computer",
            "computerUse": true
        });
        let event: CuaOperationEvent = serde_json::from_value(value).unwrap();
        assert_eq!(event.validate(), Ok(()));
        assert_eq!(event.base().sequence_number, 3);

        let invalid = serde_json::json!({
            "kind": "tool-scheduled",
            "eventId": "e2",
            "sequenceNumber": 4,
            "sessionId": "s1",
            "timestamp": 1700000000001_i64,
            "turnId": "t1",
            "toolCallId": "c1",
            "toolName": "computer",
            "computerUse": false
        });
        let event: CuaOperationEvent = serde_json::from_value(invalid).unwrap();
        assert!(event.validate().is_err());
    }
}
