//! host 命令面：M2 的 host 真正会发出/接收的那几条请求与应答。
//!
//! 只镜像**对拍与最小会话循环要用到的**命令（YAGNI）：建会话、提交输入、以及运行时反向
//! 请求的偏好面。协议里还有几十条方法（`session/read`、`session/list`、`mcp/list`…），
//! 一次翻译整套 v1 方法表既没有用例支撑，也接近 clean-room 禁止的「按上游思路复现」；
//! 后续任务按需补，补的时候同时补一条录制的夹具。
//!
//! 严格度：参数/结果的**内部**是会话面里会长的那部分，所以已知字段声明为类型、其余落进
//! `extra`（`#[serde(flatten)]`），不会因为运行时多了一个可选字段就整条命令解析失败；
//! 而字段名逐一对齐线上（`rename_all = "camelCase"`），类型不符仍然会立刻报错。
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::frame::ExtraFields;

/// 工作目录引用（`modeWorkspaceRefSchema` 的镜像）。
///
/// `workspaceIdentity` / `remoteSessionId` 是远程链路的身份字段：M2 的本地对拍用不到，
/// 但它们在协议里是**同一个对象**的字段，漏掉会让带身份的远端请求解不进来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRef {
    pub workspace_path: String,
    pub workspace_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_session_id: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 模型选择的推理档位（`modelSelectionSchema.options` 的镜像）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 模型选择（`modelSelectionSchema` 的镜像）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ModelSelectionOptions>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `session/create` 的 params（`modeSessionCreateParamsSchema`，`.strict()`）。
///
/// 字段顺序与 TS `buildSessionCreateParams` 的字面量一致；缺省字段省略。
/// `mcpServers` / `importedHistory` 的嵌套类型在 [`crate::session_create`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub workspace: crate::session_read::SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<crate::session_read::SessionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::message::MessageModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence: Option<crate::session_write::SessionPersistence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_level: Option<String>,
    /// 标题生成是一次额外模型调用，到达时刻不确定；对拍一律关掉（spec §1.5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_generation_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<crate::session_create::McpServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_allowlist: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_denylist: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_history: Option<crate::session_create::ImportHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_peak_tool_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_workflow_enabled: Option<bool>,
}

/// `session/send` 的 params（`modeSessionSendParamsSchema`，`.strict()`）。
///
/// 字段顺序与 TS `buildSessionSendParams` 的字面量一致；缺省字段省略。
/// `superRefine` 的三条组合约束由 [`SessionSendParams::validate`] 表达。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSendParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<crate::message::MessageModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_execution: Option<crate::session_send::ModelExecution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_ambient_context: Option<crate::session_send::BrowserAmbientContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_provider_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_peak_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_peak_run_type: Option<crate::session_send::OffPeakRunType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_delivery_target: Option<crate::session_send::BotDeliveryTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_denylist: Option<Vec<String>>,
}

impl SessionSendParams {
    /// 协议 `superRefine` 的三条组合约束；返回第一条违规消息。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.automation_id.is_some() && self.off_peak_task_id.is_some() {
            return Err("automationId and offPeakTaskId are mutually exclusive");
        }
        if self.off_peak_run_type.is_some() && self.off_peak_task_id.is_none() {
            return Err("offPeakRunType requires offPeakTaskId");
        }
        if self.model_execution.is_some() && self.model_selection.is_none() {
            return Err("modelExecution requires modelSelection");
        }
        Ok(())
    }
}

/// `session/send` 的 result，也就是「输入已被受理」的确认。
///
/// 协议里 `accepted` 是 `z.literal(true)`；镜像保留 `bool`，由调用方按 schema 语义
/// 拒绝 `false`（facade 已做）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSendResult {
    pub accepted: bool,
    pub session_id: String,
    pub state_revision: u64,
}

/// `session/requestRuntimePreferences` 的 params.scope。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimePreferencesScope {
    /// 线上取值逐条写死，不用 `rename_all` 推导（值里有连字符，正是 M1 踩过的坑）。
    #[serde(rename = "runtime-materialization")]
    RuntimeMaterialization,
    #[serde(rename = "user-execution")]
    UserExecution,
}

/// `session/requestRuntimePreferences` 的 params（运行时反向请求，`.strict()`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePreferencesRequestParams {
    pub session_id: String,
    pub scope: RuntimePreferencesScope,
}

/// `modelContextBudgetStrategy` 的取值。
///
/// `Default` 就是协议声明的默认值（`preflight-v1`）：字段本身建成 `Option` 以保留「省略」，
/// 需要协议的默认值时空缺由消费方用 `unwrap_or_default()` 补。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelContextBudgetStrategy {
    #[serde(rename = "legacy")]
    Legacy,
    #[serde(rename = "preflight-v1")]
    #[default]
    PreflightV1,
}

/// host 对 `session/requestRuntimePreferences` 的应答（**镜像方向**：Rust host 将来要发出它）。
///
/// `memoryEnabled` / `askUserQuestionAutoResolutionEnabled` / `modelContextBudgetStrategy`
/// 在协议里带 `.default(...)`——即旧 host 可以**省略**它们，运行时按默认走。
///
/// 镜像把这三个字段建模成 `Option`，就是为了**保留省略**：如果解析时替协议补上默认值，
/// 「读一条合法的旧应答、再写回去」会凭空多出字段，round-trip 相等这条断言就比协议更严，
/// 会给出假红（Task 1 复评 Minor 2）。需要协议的默认值时由消费方自己取：
/// `unwrap_or(false)` / `unwrap_or(true)` / `unwrap_or_default()`
/// （`ModelContextBudgetStrategy` 的 `Default` 实现就是协议声明的默认值本身）。
///
/// schema 本身是 `.strict()`：未知键拒绝（与 `modeSessionRuntimePreferencesResultSchema` 一致）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePreferencesResult {
    pub native_search_enhancements_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_user_question_auto_resolution_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_terminal_shell: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_context_budget_strategy: Option<ModelContextBudgetStrategy>,
}
