//! 账号层输入/输出类型。
//!
//! 真源：`packages/services/src/model-provider/accountProviderConnectionResolver.ts`
//! （设置与连接结果）、`packages/provider/src/account-provider-resolution.ts`
//! （连接结果联合）、`packages/services/src/model-provider/codingPlanProviderAvailability.ts`
//! （可用性结果）、`packages/shared/src/provider-family-connection-selection.ts`。
//!
//! 字段顺序即 TS 对象字面量的序列化顺序；`skip_serializing_if` 对应可选字段。

use std::fmt;

use provider_config::{
    AccountProviderUnavailableReason, ProviderConfigMap, ZhipuAccountMode, ZhipuAccountType,
};
use serde::{Deserialize, Serialize};

/// Provider family：`"zai" | "bigmodel"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderFamilyDomain {
    Zai,
    Bigmodel,
}

impl ProviderFamilyDomain {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderFamilyDomain::Zai => "zai",
            ProviderFamilyDomain::Bigmodel => "bigmodel",
        }
    }

    pub fn from_account_type(account_type: ZhipuAccountType) -> Self {
        match account_type {
            ZhipuAccountType::Zai => ProviderFamilyDomain::Zai,
            ZhipuAccountType::Bigmodel => ProviderFamilyDomain::Bigmodel,
        }
    }

    pub fn as_account_type(self) -> ZhipuAccountType {
        match self {
            ProviderFamilyDomain::Zai => ZhipuAccountType::Zai,
            ProviderFamilyDomain::Bigmodel => ZhipuAccountType::Bigmodel,
        }
    }
}

/// 用户对一个 family 的连接选择意图。
///
/// 序列化顺序必须与 `providerFamilyConnectionSelectionSchema` 的对象字面量一致：
/// team 是 `kind, productId, organizationId, projectId`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProviderFamilyConnectionSelection {
    StartPlan,
    IndividualCodingPlan,
    TeamCodingPlan {
        #[serde(rename = "productId")]
        product_id: String,
        #[serde(rename = "organizationId")]
        organization_id: String,
        #[serde(rename = "projectId")]
        project_id: String,
    },
}

impl ProviderFamilyConnectionSelection {
    /// `kind` 对应的 plan mode；off-peak 不是选择 kind。
    pub fn plan_mode(&self) -> ZhipuAccountMode {
        match self {
            ProviderFamilyConnectionSelection::StartPlan => ZhipuAccountMode::StartPlan,
            ProviderFamilyConnectionSelection::IndividualCodingPlan => {
                ZhipuAccountMode::IndividualCodingPlan
            }
            ProviderFamilyConnectionSelection::TeamCodingPlan { .. } => {
                ZhipuAccountMode::TeamCodingPlan
            }
        }
    }

    /// `[organizationId, projectId, productId]` 的 scope 三元组。
    pub fn team_scope(&self) -> Option<(&str, &str, &str)> {
        match self {
            ProviderFamilyConnectionSelection::TeamCodingPlan {
                product_id,
                organization_id,
                project_id,
            } => Some((organization_id, project_id, product_id)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFamilyConnectionSelectionSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zai: Option<ProviderFamilyConnectionSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bigmodel: Option<ProviderFamilyConnectionSelection>,
}

impl ProviderFamilyConnectionSelectionSettings {
    pub fn get(&self, family: ProviderFamilyDomain) -> Option<&ProviderFamilyConnectionSelection> {
        match family {
            ProviderFamilyDomain::Zai => self.zai.as_ref(),
            ProviderFamilyDomain::Bigmodel => self.bigmodel.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProviderConnectionSettings {
    pub provider_family_domain: Option<ProviderFamilyDomain>,
    pub selections: ProviderFamilyConnectionSelectionSettings,
    /// Host 旧连接导入尚不能确定身份；仅运行时事实，不写入配置或协议。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_families: Option<Vec<ProviderFamilyDomain>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanKind {
    StartPlan,
    IndividualCodingPlan,
    TeamCodingPlan,
}

impl PlanKind {
    pub fn from_mode(mode: ZhipuAccountMode) -> Option<Self> {
        match mode {
            ZhipuAccountMode::StartPlan => Some(PlanKind::StartPlan),
            ZhipuAccountMode::IndividualCodingPlan => Some(PlanKind::IndividualCodingPlan),
            ZhipuAccountMode::TeamCodingPlan => Some(PlanKind::TeamCodingPlan),
            ZhipuAccountMode::OffPeak => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanUnavailableReason {
    CodingPlanNotAuthenticated,
    CodingPlanNotConnected,
    CodingPlanAuthFailed,
    CodingPlanNotEntitled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CodingPlanAvailabilityResult {
    Available {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        models: Option<Vec<String>>,
    },
    Pending {
        #[serde(rename = "effectiveAt")]
        effective_at: i64,
        models: Vec<String>,
    },
    Unavailable {
        reason: CodingPlanUnavailableReason,
    },
    Unknown,
}

impl CodingPlanAvailabilityResult {
    pub fn kind(&self) -> AccountProviderConnectionStatus {
        match self {
            CodingPlanAvailabilityResult::Available { .. } => {
                AccountProviderConnectionStatus::Available
            }
            CodingPlanAvailabilityResult::Pending { .. } => {
                AccountProviderConnectionStatus::Pending
            }
            CodingPlanAvailabilityResult::Unavailable { .. } => {
                AccountProviderConnectionStatus::Unavailable
            }
            CodingPlanAvailabilityResult::Unknown => AccountProviderConnectionStatus::Unknown,
        }
    }

    pub fn models(&self) -> Option<&[String]> {
        match self {
            CodingPlanAvailabilityResult::Available { models } => models.as_deref(),
            CodingPlanAvailabilityResult::Pending { models, .. } => Some(models),
            _ => None,
        }
    }

    pub fn effective_at(&self) -> Option<i64> {
        match self {
            CodingPlanAvailabilityResult::Pending { effective_at, .. } => Some(*effective_at),
            _ => None,
        }
    }

    pub fn reason(&self) -> Option<CodingPlanUnavailableReason> {
        match self {
            CodingPlanAvailabilityResult::Unavailable { reason } => Some(*reason),
            _ => None,
        }
    }
}

/// `AccountProviderFamilyAvailabilityInput.providers` 的元素。
///
/// `apiKey` 恒有键（team 与读取失败都是 `null`），因此序列化时不省略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingPlanAvailabilityProvider {
    pub provider_id: String,
    pub family: ProviderFamilyDomain,
    pub plan_kind: PlanKind,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProviderFamilyAvailabilityInput {
    pub family: ProviderFamilyDomain,
    pub providers: Vec<CodingPlanAvailabilityProvider>,
    pub selections: ProviderFamilyConnectionSelectionSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountProviderConnectionStatus {
    Available,
    Pending,
    Unavailable,
    Unknown,
}

impl AccountProviderConnectionStatus {
    pub fn availability(self) -> provider_config::AccountProviderAvailability {
        match self {
            AccountProviderConnectionStatus::Available => {
                provider_config::AccountProviderAvailability::Available
            }
            AccountProviderConnectionStatus::Pending => {
                provider_config::AccountProviderAvailability::Pending
            }
            AccountProviderConnectionStatus::Unavailable => {
                provider_config::AccountProviderAvailability::Unavailable
            }
            AccountProviderConnectionStatus::Unknown => {
                provider_config::AccountProviderAvailability::Unknown
            }
        }
    }
}

/// 单 Provider 的账号连接结果。
///
/// 序列化字段顺序与 TS `connections.push({...})` 字面量一致：
/// `providerId, status, unavailableReason?, current?, connectionKey?, models?,
/// effectiveAt?, resetPrevious?`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProviderConnectionResult {
    pub provider_id: String,
    pub status: AccountProviderConnectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<AccountProviderUnavailableReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_at: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset_previous: bool,
}

impl AccountProviderConnectionResult {
    pub fn simple(provider_id: impl Into<String>, status: AccountProviderConnectionStatus) -> Self {
        Self {
            provider_id: provider_id.into(),
            status,
            unavailable_reason: None,
            current: None,
            connection_key: None,
            models: None,
            effective_at: None,
            reset_previous: false,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// 解析/构造错误。消息原文与 TS 抛出的 `Error.message` 一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProviderOverlayError {
    message: String,
}

impl AccountProviderOverlayError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AccountProviderOverlayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AccountProviderOverlayError {}

/// `createAccountProviderConfigResolver` 的输入。
#[derive(Debug, Clone, Copy)]
pub struct AccountProviderResolveInput<'a> {
    pub config_revision: &'a str,
    pub configured_providers: &'a ProviderConfigMap,
    pub previous_providers: &'a ProviderConfigMap,
    pub previous_states: Option<&'a provider_config::AccountProviderStates>,
    pub reasons: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProviderResolveOutput {
    pub providers: ProviderConfigMap,
    pub states: provider_config::AccountProviderStates,
}
