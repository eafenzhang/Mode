//! Agent 反向请求：`interaction/requestProviderRuntimeHeaders`。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的
//! `modeProviderRuntimeHeadersRequestParamsSchema` / Response 判别联合。
//! Agent 请求模型调用所需的账号鉴权材料；Host 自动应答，零 UI。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::message::MessageModel;
use crate::session::WorkspaceRef;

pub const PROVIDER_RUNTIME_HEADERS_METHOD: &str = "interaction/requestProviderRuntimeHeaders";
pub const PROVIDER_RUNTIME_HEADERS_CANCELLED_METHOD: &str =
    "interaction/providerRuntimeHeadersCancelled";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderRuntimeAccessType {
    #[serde(rename = "zhipu-account")]
    ZhipuAccount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRuntimeAccountType {
    Zai,
    Bigmodel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderRuntimeAccountMode {
    StartPlan,
    IndividualCodingPlan,
    TeamCodingPlan,
    OffPeak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderAccountAccess {
    #[serde(rename = "type")]
    pub access_type: ProviderRuntimeAccessType,
    pub account_type: ProviderRuntimeAccountType,
    pub mode: ProviderRuntimeAccountMode,
    pub entitled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderRuntimeHeadersReason {
    ModelRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRuntimeHeadersRequestParams {
    pub request_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub workspace: WorkspaceRef,
    pub model_selection: MessageModel,
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_access: Option<ProviderAccountAccess>,
    pub reason: ProviderRuntimeHeadersReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequestAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRuntimeHeadersResponse {
    pub headers_applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_auth: Option<ProviderRequestAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl ProviderRuntimeHeadersResponse {
    pub fn applied(api_key: String) -> Self {
        Self {
            headers_applied: true,
            request_auth: Some(ProviderRequestAuth {
                api_key: Some(api_key),
                headers: None,
            }),
            error_message: None,
        }
    }

    pub fn not_applied(message: impl Into<String>) -> Self {
        Self {
            headers_applied: false,
            request_auth: None,
            error_message: Some(message.into()),
        }
    }
}
