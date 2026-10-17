//! 审批面：`interaction/requestPermission` 的请求与应答，以及 `permissionUpdates` 的规则形状。
//!
//! 观察出处：`docs/specs/tauri-agent-runtime.md` §3.3（`probe-write.mts perm`，2026-10-17）。
//! 逐字形状（运行时 → host）：
//!
//! ```text
//! {"input":{…},"reason":"Tool has side effects and requires approval",
//!  "requestId":"perm_<uuid>","riskLevel":"medium","sessionId":"sess_<uuid>",
//!  "options":[…],"toolCallId":"toolu_…","toolName":"Write","turnId":"turn_<uuid>"}
//! ```
//!
//! 严格度（crate 文档的规则）：审批请求的**内部**是会话面里会长的那部分，所以已知字段声明为
//! 类型、其余落进 `extra`；判别键（`kind` / `decision` / `type` / `behavior`）取值为字面量
//! 白名单，未知取值报错——静默接受未知选项会把「没覆盖」伪装成「已覆盖」。
//!
//! `riskLevel` 是开放标量而不是判别键：本次只观察到 `"medium"`，把它写成单值枚举会把没观察过的
//! 档位当成协议错误。
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::frame::ExtraFields;

/// 审批请求（`interaction/requestPermission` 的 `params`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    /// 被审批的工具入参（`Write` 是 `{file_path, content}`）。
    pub input: Value,
    /// 为什么需要审批；`Write` 的观察值是 `Tool has side effects and requires approval`。
    pub reason: String,
    pub request_id: String,
    /// 观察值 `"medium"`；见模块注释，按开放标量处理。
    pub risk_level: String,
    pub session_id: String,
    /// 三条可选答复（`allow_once` / `allow_project` / `deny`）。
    pub options: Vec<PermissionOption>,
    pub tool_call_id: String,
    pub tool_name: String,
    pub turn_id: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 一条可选答复，含 host 直接回传给运行时的 `response`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub kind: PermissionOptionKind,
    pub name: String,
    /// 只有 `allow_project` 观察到该字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub response: PermissionResponse,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 选项的 `kind` 标签；**不是**决策本身（`allow_always` 是 `allow_project` 的标签，
/// 没有独立的全局一直允许）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionOptionKind {
    #[serde(rename = "allow_once")]
    AllowOnce,
    #[serde(rename = "allow_always")]
    AllowAlways,
    #[serde(rename = "deny")]
    Deny,
}

/// 回给运行时的决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionDecision {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
    Deny,
}

/// 审批应答（`option.response`，也是 host → 运行时的答复体）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResponse {
    pub decision: PermissionDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 允许修改入参后放行的通道；本次观察里三条选项都没有该字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_input: Option<Value>,
    /// `allow_project` 才带；按观察**恰好一条** `addRules`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_updates: Option<Vec<PermissionUpdate>>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 一条持久化更新。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionUpdate {
    #[serde(rename = "type")]
    pub update_type: PermissionUpdateType,
    pub behavior: RuleBehavior,
    pub rules: Vec<PermissionRuleValue>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `PermissionUpdate.type` 的判别键；本次只观察到 `addRules`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionUpdateType {
    #[serde(rename = "addRules")]
    AddRules,
}

/// 规则行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuleBehavior {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
    Deny,
}

/// 单条规则：`{toolName, ruleContent}`。`ruleContent` 是**原始大小写**的路径
/// （与 `scope_id` 的小写折算不同，见 spec §3.3 的落盘表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRuleValue {
    pub tool_name: String,
    pub rule_content: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}
