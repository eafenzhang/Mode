//! Tauri 侧协议类型的 **Rust 真源**（路线图 M1 任务 3）。
//!
//! 规格：`docs/specs/tauri-platform-commands.md`。
//! 取材口径：`packages/shared/src/platform.ts` 的**接口面**（类型声明与字段名）；
//! 按 `docs/specs/clean-room-policy.md` §允许输入，只读签名/类型，不读实现体。
//!
//! 字段名逐一对齐 TS：TS 侧是 camelCase，故所有结构体带 `rename_all = "camelCase"`。
//! 导出方向是单向的 Rust → TS（schemars/typeshare），TS 侧不反向生成 Rust。
use serde::{Deserialize, Serialize};

/// `packages/shared/src/platform.ts` 的 `TaskNotificationPayload`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskNotificationPayload {
    pub task_id: String,
    pub status: TaskNotificationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub title: String,
    pub body: String,
}

/// `TaskNotificationPayload["status"]` 的取值集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskNotificationStatus {
    Completed,
    Failed,
    PermissionRequest,
    ElicitationRequest,
    FeedbackUpdate,
}

/// `packages/shared/src/platform.ts` 的 `ApplicationIconLocator`（`kind` 判别联合）。
///
/// TS 侧三个变体共用一个 `value` 字段，只有 `kind` 不同，故这里用内部标签 + kebab-case
/// 名称还原同一形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ApplicationIconLocator {
    DarwinBundleId { value: String },
    WindowsExecutablePath { value: String },
    WindowsAumid { value: String },
}

/// `packages/shared/src/platform.ts` 的 `ApplicationIconRequest`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationIconRequest {
    pub locators: Vec<ApplicationIconLocator>,
}

/// `packages/shared/src/platform.ts` 的 `ApplicationIconInfo`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationIconInfo {
    pub icon_data_url: String,
}

/// `packages/shared/src/platform.ts` 的 `SaveFileRequest`。
///
/// TS 侧是「`data` 与 `sourceUrl` 二者恰有其一」的联合（各自排除另一个）。
/// JSON 传参载体上无法表达「字段不存在即排除」，故这里的表示法是把两者都放成
/// `Option`，并在 [`SaveFileRequest::validate`] 里把 TS 的不变量显式化 —— 由命令面
/// 在入口调用，违反即按 `validation_failed` 拒绝，而不是静默取其中一个。
///
/// `data` 的载体：TS 侧是 `ArrayBuffer`。Tauri 的 `invoke` 会把 `Uint8Array` 序列化成
/// 数字数组，`Vec<u8>` 正是这个形状；不引入 base64（避免与 TS 侧的 `ArrayBuffer` 语义漂移）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveFileRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub suggested_name: String,
}

/// [`SaveFileRequest`] 的不变量违规。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFileRequestError {
    /// `data` 与 `sourceUrl` 同时缺失。
    MissingPayload,
    /// `data` 与 `sourceUrl` 同时出现（TS 的联合类型排除了这种组合）。
    ConflictingPayload,
}

impl SaveFileRequest {
    /// 落实 TS 侧「二者恰有其一」的不变量。
    pub fn validate(&self) -> Result<(), SaveFileRequestError> {
        match (self.data.is_some(), self.source_url.is_some()) {
            (false, false) => Err(SaveFileRequestError::MissingPayload),
            (true, true) => Err(SaveFileRequestError::ConflictingPayload),
            _ => Ok(()),
        }
    }
}

/// `packages/shared/src/platform.ts` 的 `SaveFileResult`。
///
/// TS 侧 `success` 是必填、其余可缺省；`canceled` 与 `path` 在成功/取消两种结局下
/// 各自可缺省，这里用 `Option` 并靠 `skip_serializing_if` 保持与 TS 的 JSON 形状一致
/// （不写出 `null`，避免 TS 侧把 `null` 当成显式值）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveFileResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canceled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub success: bool,
}

/// `openInFileManager` 的返回形状（TS 侧是内联对象字面量，这里是它的具名 Rust 对应）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenPathResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
