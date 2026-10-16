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

/// `session/create` 的 params。
///
/// 只声明 M2 会用到的字段（workspace / model / thoughtLevel / 标题生成开关）；协议里还有
/// `mode`、`persistence`、`mcpServers`、`toolAllowlist`… 它们落进 `extra`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreateParams {
    pub workspace: WorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_level: Option<String>,
    /// 标题生成是一次额外模型调用，到达时刻不确定；对拍一律关掉（spec §1.5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_generation_enabled: Option<bool>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `session/send` 的 params：提交一次用户输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSendParams {
    pub session_id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `session/send` 的 result，也就是「输入已被受理」的确认。
///
/// 这是命令面里的 **ack**：host 提交输入后不能假定被受理，必须看到 `accepted`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSendResult {
    pub accepted: bool,
    pub session_id: String,
    pub state_revision: u64,
    #[serde(flatten)]
    pub extra: ExtraFields,
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

/// `session/requestRuntimePreferences` 的 params（运行时反向请求）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePreferencesRequestParams {
    pub session_id: String,
    pub scope: RuntimePreferencesScope,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `modelContextBudgetStrategy` 的取值。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelContextBudgetStrategy {
    #[serde(rename = "legacy")]
    Legacy,
    #[serde(rename = "preflight-v1")]
    #[default]
    PreflightV1,
}

fn enabled() -> bool {
    true
}

/// host 对 `session/requestRuntimePreferences` 的应答（**镜像方向**：Rust host 将来要发出它）。
///
/// `memoryEnabled` / `askUserQuestionAutoResolutionEnabled` / `modelContextBudgetStrategy`
/// 在协议里带默认值——即旧 host 可以不传，运行时按默认走；镜像必须保留这个性质，
/// 否则一个合法的旧应答会被判成解析失败。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePreferencesResult {
    pub native_search_enhancements_enabled: bool,
    #[serde(default)]
    pub memory_enabled: bool,
    #[serde(default = "enabled")]
    pub ask_user_question_auto_resolution_enabled: bool,
    /// 未建模（M2 捕获里未出现）：原样保留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_terminal_shell: Option<Value>,
    #[serde(default)]
    pub model_context_budget_strategy: ModelContextBudgetStrategy,
    #[serde(flatten)]
    pub extra: ExtraFields,
}
