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

use crate::frame::{ExtraFields, Frame, RequestFrame, RequestId, ResponseFrame};

/// 审批反向请求的方法名（stdio 实测，见 spec §1.3 / §3.3）。
pub const REQUEST_PERMISSION_METHOD: &str = "interaction/requestPermission";

/// 审批帧映射失败：方法名不符，或 `params` / `result` 解不开。
///
/// 宿主侧与运行时侧共用：解析失败必须显式暴露，不能把「没覆盖」当成「已覆盖」。
#[derive(Debug)]
pub enum PermissionFrameError {
    UnexpectedMethod(String),
    MissingParams,
    InvalidParams(serde_json::Error),
    InvalidResponse(serde_json::Error),
}

impl std::fmt::Display for PermissionFrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedMethod(method) => write!(formatter, "unexpected method {method:?}"),
            Self::MissingParams => write!(formatter, "permission request params are missing"),
            Self::InvalidParams(error) => {
                write!(formatter, "invalid permission request params: {error}")
            }
            Self::InvalidResponse(error) => {
                write!(formatter, "invalid permission response: {error}")
            }
        }
    }
}

impl std::error::Error for PermissionFrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnexpectedMethod(_) | Self::MissingParams => None,
            Self::InvalidParams(error) | Self::InvalidResponse(error) => Some(error),
        }
    }
}

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

impl PermissionRequest {
    /// 构造运行时 → host 的反向请求帧（`interaction/requestPermission`）。
    ///
    /// `id` 由调用方分配并按连接单调递增（反向请求观察到的形状是 `"server-<n>"`）。
    pub fn to_request_frame(&self, id: RequestId) -> Result<Frame, PermissionFrameError> {
        let params = serde_json::to_value(self).map_err(PermissionFrameError::InvalidParams)?;
        Ok(Frame::Request(RequestFrame {
            id,
            method: REQUEST_PERMISSION_METHOD.to_string(),
            params: Some(params),
            trace: None,
        }))
    }

    /// 从反向请求帧解析审批请求（宿主侧使用）；方法名不符或 `params` 解不开即报错。
    pub fn from_request_frame(frame: &RequestFrame) -> Result<Self, PermissionFrameError> {
        if frame.method != REQUEST_PERMISSION_METHOD {
            return Err(PermissionFrameError::UnexpectedMethod(frame.method.clone()));
        }
        let params = frame
            .params
            .clone()
            .ok_or(PermissionFrameError::MissingParams)?;
        serde_json::from_value(params).map_err(PermissionFrameError::InvalidParams)
    }
}

impl PermissionResponse {
    /// 构造 host → 运行时的应答帧（`{"id":…,"result":…}`）。
    pub fn to_response_frame(&self, id: RequestId) -> Result<Frame, PermissionFrameError> {
        let result = serde_json::to_value(self).map_err(PermissionFrameError::InvalidResponse)?;
        Ok(Frame::Response(ResponseFrame { id, result }))
    }

    /// 从应答帧解析审批答复；`id` 关联由调用方负责。
    pub fn from_response_frame(frame: &ResponseFrame) -> Result<Self, PermissionFrameError> {
        serde_json::from_value(frame.result.clone()).map_err(PermissionFrameError::InvalidResponse)
    }
}
