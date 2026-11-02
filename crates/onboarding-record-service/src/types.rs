//! Onboarding 记录的领域类型（`packages/shared/src/onboardingRecord.ts` 的 Rust 投影）。

use serde::{Deserialize, Deserializer, Serialize};

use crate::parse::parse_entry_input;

pub const RECORD_VERSION: u8 = 2;

/// `onboardingInterfaceModeSchema`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceMode {
    Coding,
    Office,
}

/// `appSettingsOccupationEnum`；同步回 settings 时用它做窄化。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Occupation {
    Office,
    Developer,
    Independent,
    Infrastructure,
    Product,
    Design,
    Student,
    Creator,
    Operations,
    Marketing,
    Finance,
    Accounting,
    Legal,
    Other,
}

impl Occupation {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "office" => Some(Self::Office),
            "developer" => Some(Self::Developer),
            "independent" => Some(Self::Independent),
            "infrastructure" => Some(Self::Infrastructure),
            "product" => Some(Self::Product),
            "design" => Some(Self::Design),
            "student" => Some(Self::Student),
            "creator" => Some(Self::Creator),
            "operations" => Some(Self::Operations),
            "marketing" => Some(Self::Marketing),
            "finance" => Some(Self::Finance),
            "accounting" => Some(Self::Accounting),
            "legal" => Some(Self::Legal),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// 当前只有 `pending`；读到时出现其它值等价于文件损坏。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadState {
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Dismissed,
    ExistingLocalUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    UserClosed,
    ExistingLocalTask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingDecision {
    pub user_id: Option<String>,
    pub status: DecisionStatus,
    pub reason: DecisionReason,
    pub decided_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingRecordEntry {
    pub user_id: Option<String>,
    pub occupation: Option<String>,
    pub interface_mode: Option<InterfaceMode>,
    pub memory_enabled: Option<bool>,
    pub proactive_suggestions_enabled: Option<bool>,
    pub completed_at: String,
    pub upload_state: UploadState,
}

/// `appendRecord` 入参：userId / uploadState 由服务补全。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingRecordEntryInput {
    pub occupation: Option<String>,
    pub interface_mode: Option<InterfaceMode>,
    pub memory_enabled: Option<bool>,
    pub proactive_suggestions_enabled: Option<bool>,
    pub completed_at: String,
}

impl<'de> Deserialize<'de> for OnboardingRecordEntryInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("entry input must be an object"))?;
        parse_entry_input(object).map_err(serde::de::Error::custom)
    }
}

impl OnboardingRecordEntryInput {
    pub(crate) fn into_entry(self, user_id: Option<String>) -> OnboardingRecordEntry {
        OnboardingRecordEntry {
            user_id,
            occupation: self.occupation,
            interface_mode: self.interface_mode,
            memory_enabled: self.memory_enabled,
            proactive_suggestions_enabled: self.proactive_suggestions_enabled,
            completed_at: self.completed_at,
            upload_state: UploadState::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingRecordFile {
    pub version: u8,
    pub device_mid: String,
    pub entries: Vec<OnboardingRecordEntry>,
    pub decisions: Vec<OnboardingDecision>,
}

impl OnboardingRecordFile {
    pub fn new(device_mid: &str) -> Self {
        Self {
            version: RECORD_VERSION,
            device_mid: device_mid.to_string(),
            entries: Vec::new(),
            decisions: Vec::new(),
        }
    }

    pub fn has_identity_record(&self, user_id: Option<&str>) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.user_id.as_deref() == user_id)
            || self
                .decisions
                .iter()
                .any(|decision| decision.user_id.as_deref() == user_id)
    }

    pub fn latest_entry(&self, user_id: Option<&str>) -> Option<&OnboardingRecordEntry> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.user_id.as_deref() == user_id)
    }
}

/// `syncSettingsFromRecord` 的回填补丁；settings 仍是运行时唯一事实源。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingSettingsSyncPatch {
    pub onboarding_occupation: Occupation,
    pub proactive_suggestions_enabled: bool,
    pub memory_enabled: bool,
}

/// `updateRecordPreferences` 的补丁：`None` = 不修改，`Some(None)` = 显式置 null。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingPreferencesPatch {
    #[serde(default, deserialize_with = "double_option")]
    pub memory_enabled: Option<Option<bool>>,
    #[serde(default, deserialize_with = "double_option")]
    pub proactive_suggestions_enabled: Option<Option<bool>>,
}

/// 区分「键缺失」与「显式 null」。
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
