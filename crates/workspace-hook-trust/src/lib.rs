//! Workspace Hook Trust store（`workspace-hook-trust-v1.json`）解析。
//!
//! 真源：`packages/shared/src/workspace-hook-trust-store-file.ts`。这是权限边界：
//! JSON 语法错误与 schema 不合法同等视为不可信，必须整体 fail-closed，不返回部分结果。
//! 校验对齐 zod `.strict()`：未知字段拒绝、`schemaVersion=1`、`digestAlgorithm=sha256`、
//! `decision=trusted`、摘要为 64 位小写十六进制、`(identity, digest)` 唯一。

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustRecord {
    pub workspace_identity: String,
    pub hook_declaration_digest: String,
    pub digest_algorithm: String,
    pub decision: String,
    pub granted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_digest_at_grant: Option<String>,
    pub event_at_grant: String,
    pub display_command_at_grant: String,
    pub source_path_at_grant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_discovery_order_at_grant: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher_at_grant: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher_index_at_grant: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_index_at_grant: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version_at_grant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustStore {
    pub schema_version: u64,
    pub records: Vec<TrustRecord>,
}

const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
];

/// 任何不可信都返回 `None`（消费方据此 fail-closed）。
pub fn parse_trust_store(content: &str) -> Option<TrustStore> {
    let store: TrustStore = serde_json::from_str(content).ok()?;
    validate(&store).then_some(store)
}

fn validate(store: &TrustStore) -> bool {
    if store.schema_version != SCHEMA_VERSION {
        return false;
    }
    let mut keys = std::collections::HashSet::new();
    for record in &store.records {
        if record.workspace_identity.trim().is_empty()
            || !is_sha256(&record.hook_declaration_digest)
            || record.digest_algorithm != "sha256"
            || record.decision != "trusted"
            || !HOOK_EVENTS.contains(&record.event_at_grant.as_str())
            || record.display_command_at_grant.trim().is_empty()
            || record.source_path_at_grant.trim().is_empty()
            || !is_iso_datetime(&record.granted_at)
            || record.last_used_at.as_deref().is_some_and(|v| !is_iso_datetime(v))
            || record.bundle_digest_at_grant.as_deref().is_some_and(|v| !is_sha256(v))
        {
            return false;
        }
        if !keys.insert((
            record.workspace_identity.trim().to_string(),
            record.hook_declaration_digest.clone(),
        )) {
            return false;
        }
    }
    true
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// ISO-8601：`YYYY-MM-DDTHH:MM[:SS[.sss]]` + `Z` 或 `±HH:MM`；只校验形状。
fn is_iso_datetime(value: &str) -> bool {
    let Some((date, rest)) = value.split_once('T') else {
        return false;
    };
    let date_ok = {
        let parts: Vec<&str> = date.split('-').collect();
        parts.len() == 3
            && parts[0].len() == 4
            && parts[1].len() == 2
            && parts[2].len() == 2
            && parts
                .iter()
                .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    if !date_ok {
        return false;
    }
    let (time, zone) = if let Some(time) = rest.strip_suffix('Z') {
        (time, "Z")
    } else if let Some(index) = rest.rfind(['+', '-']) {
        (&rest[..index], &rest[index..])
    } else {
        return false;
    };
    let zone_ok = zone == "Z"
        || (zone.len() == 6
            && zone.as_bytes()[3] == b':'
            && zone[1..3].bytes().all(|b| b.is_ascii_digit())
            && zone[4..6].bytes().all(|b| b.is_ascii_digit()));
    let core = time.split('.').next().unwrap_or("");
    let parts: Vec<&str> = core.split(':').collect();
    let time_ok = (parts.len() == 2 || parts.len() == 3)
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit()));
    zone_ok && time_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn record(identity: &str) -> serde_json::Value {
        serde_json::json!({
            "workspaceIdentity": identity,
            "hookDeclarationDigest": DIGEST,
            "digestAlgorithm": "sha256",
            "decision": "trusted",
            "grantedAt": "2026-10-29T12:00:00Z",
            "eventAtGrant": "PreToolUse",
            "displayCommandAtGrant": "npm test",
            "sourcePathAtGrant": "D:/w/.mode/hooks.json"
        })
    }

    fn file(records: Vec<serde_json::Value>) -> String {
        serde_json::json!({ "schemaVersion": 1, "records": records }).to_string()
    }

    #[test]
    fn accepts_valid_and_rejects_untrusted_shapes() {
        assert!(parse_trust_store(&file(vec![record("D:/w")])).is_some());
        assert!(parse_trust_store(":wq").is_none(), "坏 JSON");
        assert!(
            parse_trust_store(&file(vec![record("D:/w"), record("D:/w")])).is_none(),
            "重复键"
        );
        let mut extra = record("D:/w");
        extra["unexpected"] = serde_json::json!(1);
        assert!(parse_trust_store(&file(vec![extra])).is_none(), "未知字段");
        let mut bad_event = record("D:/w");
        bad_event["eventAtGrant"] = serde_json::json!("Nope");
        assert!(parse_trust_store(&file(vec![bad_event])).is_none());
        assert!(
            parse_trust_store(&serde_json::json!({ "schemaVersion": 2, "records": [] }).to_string())
                .is_none()
        );
    }
}
