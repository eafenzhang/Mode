//! Account Provider 实时事实的纯形状（不参与配置 Overlay 的 revision 生成）。
//!
//! 真源：`packages/provider/src/account-provider-state.ts` 与
//! `packages/shared/src/account-provider-state.ts`。字段可空/可选直接映射为 `Option`。

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountProviderUnavailableReason {
    NotAuthenticated,
    NotConnected,
    CredentialFailed,
    NotEntitled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountProviderAvailability {
    Available,
    Pending,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProviderState {
    pub availability: AccountProviderAvailability,
    pub entitled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<AccountProviderUnavailableReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_key: Option<String>,
    /// Unix 秒；与 billing `effective_at` 单位一致。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_at: Option<i64>,
}

pub type AccountProviderStates = IndexMap<String, AccountProviderState>;
