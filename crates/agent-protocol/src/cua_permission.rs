//! `v4/cua/permission-observation` 的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/cuaPermission.ts`。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CuaAccessibilityStatus {
    Granted,
    Stale,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CuaScreenRecordingStatus {
    Granted,
    Denied,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CuaRequestAccessStatus {
    pub schema_version: u8,
    pub platform: CuaPlatform,
    pub grant_owner: String,
    pub accessibility: CuaAccessibilityStatus,
    pub screen_recording: CuaScreenRecordingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CuaPlatform {
    Darwin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CuaPermissionObservation {
    pub schema_version: u8,
    pub event_id: String,
    pub event_seq: u64,
    pub occurred_at: f64,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub tool_call_id: String,
    pub permission_status: CuaRequestAccessStatus,
}

impl CuaPermissionObservation {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("cua permission observation schemaVersion must be 1".to_owned());
        }
        if self.permission_status.schema_version != 1 {
            return Err("cua request access status schemaVersion must be 1".to_owned());
        }
        if self.permission_status.grant_owner.is_empty() {
            return Err("grantOwner must not be empty".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_unknown_fields() {
        let value = serde_json::json!({
            "schemaVersion": 1,
            "eventId": "e1",
            "eventSeq": 2,
            "occurredAt": 1700000000000_i64,
            "sessionId": "s1",
            "toolCallId": "t1",
            "permissionStatus": {
                "schemaVersion": 1,
                "platform": "darwin",
                "grantOwner": "owner",
                "accessibility": "denied",
                "screenRecording": "unknown"
            }
        });
        let observation: CuaPermissionObservation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(observation.validate(), Ok(()));
        let encoded = serde_json::to_value(&observation).unwrap();
        assert_eq!(encoded["permissionStatus"]["accessibility"], "denied");

        let mut invalid = value;
        invalid["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<CuaPermissionObservation>(invalid).is_err());
    }
}
