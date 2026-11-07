//! Agent 反向请求：`interaction/requestOfficialMcpAuthHeaders`。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的
//! `modeOfficialMcpAuthHeadersRequestParamsSchema` / Response 判别联合与
//! `packages/shared/src/official-mcp-auth.ts` 的失败分类。
//! Agent 请求官方 Server MCP 身份头；Host 是身份权威边界，自动应答零 UI。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::WorkspaceRef;

pub const OFFICIAL_MCP_AUTH_HEADERS_METHOD: &str =
    "interaction/requestOfficialMcpAuthHeaders";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfficialMcpAuthPortFailureReason {
    OfficialAuthUnavailable,
    OfficialAuthPlanRequired,
    OfficialMcpOriginUntrusted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficialMcpAuthHeadersRequestParams {
    pub request_id: String,
    pub workspace: WorkspaceRef,
    pub plugin_id: String,
    pub mcp_key: String,
    pub target_origin: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficialMcpAuthHeadersSuccess {
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficialMcpAuthHeadersFailure {
    pub reason: OfficialMcpAuthPortFailureReason,
}

impl OfficialMcpAuthPortFailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OfficialAuthUnavailable => "official_auth_unavailable",
            Self::OfficialAuthPlanRequired => "official_auth_plan_required",
            Self::OfficialMcpOriginUntrusted => "official_mcp_origin_untrusted",
        }
    }
}

/// 判别联合：`{ok:true, headers}` / `{ok:false, reason}`。
/// serde 内部标签无法产出布尔字面量 `ok:true`，这里手工实现序列化。
#[derive(Debug, Clone, PartialEq)]
pub enum OfficialMcpAuthHeadersResponse {
    Ok { headers: BTreeMap<String, String> },
    Failed { reason: OfficialMcpAuthPortFailureReason },
}

impl Serialize for OfficialMcpAuthHeadersResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = match self {
            Self::Ok { headers } => serde_json::json!({ "ok": true, "headers": headers }),
            Self::Failed { reason } => {
                serde_json::json!({ "ok": false, "reason": reason.as_str() })
            }
        };
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OfficialMcpAuthHeadersResponse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let object = value.as_object().ok_or_else(|| {
            serde::de::Error::custom("official MCP auth headers response must be an object")
        })?;
        match object.get("ok").and_then(Value::as_bool) {
            Some(true) => {
                let Some(headers) = object.get("headers").and_then(Value::as_object) else {
                    return Err(serde::de::Error::custom("missing headers"));
                };
                let mut parsed = BTreeMap::new();
                for (name, header_value) in headers {
                    let header = header_value.as_str().ok_or_else(|| {
                        serde::de::Error::custom("header value must be a string")
                    })?;
                    parsed.insert(name.clone(), header.to_string());
                }
                Ok(Self::Ok { headers: parsed })
            }
            Some(false) => {
                let reason = match object
                    .get("reason")
                    .and_then(Value::as_str)
                {
                    Some("official_auth_unavailable") => {
                        OfficialMcpAuthPortFailureReason::OfficialAuthUnavailable
                    }
                    Some("official_auth_plan_required") => {
                        OfficialMcpAuthPortFailureReason::OfficialAuthPlanRequired
                    }
                    Some("official_mcp_origin_untrusted") => {
                        OfficialMcpAuthPortFailureReason::OfficialMcpOriginUntrusted
                    }
                    _ => {
                        return Err(serde::de::Error::custom("unknown failure reason"));
                    }
                };
                Ok(Self::Failed { reason })
            }
            _ => Err(serde::de::Error::custom("ok must be a boolean")),
        }
    }
}
