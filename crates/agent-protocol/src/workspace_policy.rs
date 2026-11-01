//! workspace 策略写路径：交互偏好、模型 I/O、Off-Peak 工具面、动态工作流灰度。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的四组 params/result schema。
//! 四个方法都是 workspace 级事实，结果回显 workspace 与生效值；没有会话副作用。
//! Off-Peak / 动态工作流两条在旧 CLI 上可能 `method-not-found`，降级由调用方决定，
//! 本镜像只做严格收发。

use serde::{Deserialize, Serialize};

use crate::session::WorkspaceRef;

pub const WORKSPACE_UPDATE_INTERACTION_PREFERENCES_METHOD: &str =
    "workspace/updateInteractionPreferences";
pub const WORKSPACE_UPDATE_MODEL_IO_PREFERENCES_METHOD: &str = "workspace/updateModelIoPreferences";
pub const WORKSPACE_UPDATE_OFF_PEAK_TOOL_POLICY_METHOD: &str = "workspace/updateOffPeakToolPolicy";
pub const WORKSPACE_UPDATE_DYNAMIC_WORKFLOW_POLICY_METHOD: &str =
    "workspace/updateDynamicWorkflowPolicy";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InteractionPreferences {
    pub ask_user_question_auto_resolution_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateInteractionPreferencesParams {
    pub workspace: WorkspaceRef,
    pub preferences: InteractionPreferences,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateInteractionPreferencesResult {
    pub workspace: WorkspaceRef,
    pub ask_user_question_auto_resolution_enabled: bool,
    pub snoozed_interaction_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelIoPreferences {
    pub full_retention_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateModelIoPreferencesParams {
    pub workspace: WorkspaceRef,
    pub preferences: ModelIoPreferences,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateModelIoPreferencesResult {
    pub workspace: WorkspaceRef,
    pub full_retention_enabled: bool,
    pub updated_session_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateOffPeakToolPolicyParams {
    pub workspace: WorkspaceRef,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateOffPeakToolPolicyResult {
    pub workspace: WorkspaceRef,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateDynamicWorkflowPolicyParams {
    pub workspace: WorkspaceRef,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUpdateDynamicWorkflowPolicyResult {
    pub workspace: WorkspaceRef,
    pub enabled: bool,
}
