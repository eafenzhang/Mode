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

/// `exportLogs` 的返回形状（TS 侧 `packages/shared/src/platform.ts:879` 是内联对象
/// 字面量 `{ success: boolean; path?: string; error?: string }`）。
///
/// 取消（用户关掉保存对话框）是 `success:false` 且**无 `error`**——取消不是失败；
/// `error` 只在真正出错时给出稳定码（`no_logs` / `write_failed` / `home_unavailable`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportLogsResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `openInFileManager` 的返回形状（TS 侧是内联对象字面量，这里是它的具名 Rust 对应）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenPathResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `packages/shared/src/platform.ts` 的 `CreateTempTextAttachmentRequest`。
///
/// 长粘贴文本在桌面端落成临时文件后再作为附件提交（Web 端没有本地文件系统，返回不支持）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTempTextAttachmentRequest {
    pub text: String,
    /// 期望文件名；缺省时由壳生成 `attachment.txt`。壳会去掉路径分量与非法字符。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// `CreateTempTextAttachmentResult["mimeType"]` 的取值；当前只允许 `text/plain`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum TempTextMimeType {
    #[serde(rename = "text/plain")]
    TextPlain,
}

/// `packages/shared/src/platform.ts` 的 `CreateTempTextAttachmentResult`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTempTextAttachmentResult {
    pub filename: String,
    pub local_path: String,
    pub mime_type: TempTextMimeType,
    pub size_bytes: u64,
}

/// 渲染层订阅的深链事件名。
///
/// 真源在 Rust 侧（`crates/deeplink` 用它 emit，渲染层用它 `listen`）。事件名写错是
/// **静默失效**——订阅装上了但永远收不到，所以它和协议字段一样由生成器导出到 TS，
/// 由 CI 的漂移断言兜住，不靠人手抄（见 `emit-schema.rs` 的 `$constants` 段）。
pub const DEEP_LINK_EVENT: &str = "mode:deep-link";

/// `DeepLinkEvent["kind"]` 的取值集合。
///
/// 每个变体**显式写 `rename`**，不用 `rename_all`：serde 的 kebab-case 会把
/// `OAuthCallback` 切成 `o-auth-callback`（连续大写被当成词边界）。线上取值必须与 TS
/// 侧逐字一致，所以写死而不是靠命名转换规则推导。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum DeepLinkEventKind {
    #[serde(rename = "oauth-callback")]
    OAuthCallback,
    #[serde(rename = "payment-callback")]
    PaymentCallback,
    #[serde(rename = "workspace-open")]
    WorkspaceOpen,
    #[serde(rename = "share-import")]
    ShareImport,
    #[serde(rename = "unrecognized")]
    Unrecognized,
}

/// 深链事件载荷（宿主 → 渲染层）。
///
/// M1 只接线 `oauth-callback` / `payment-callback`（spec §2 的「只落一个示范」是下限，
/// 支付走同一条链路所以一并接）；另外三种 `kind` 是**可观察性**：渲染层按 `kind` 过滤，
/// 未认领的链接不会静默消失。
///
/// 用**单个**事件名 + `kind` 判别，而不是每种意图一个事件名：M3 补齐各订阅语义时
/// 不需要新增事件名，渲染层也不必为每种意图装一次监听。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeepLinkEvent {
    pub kind: DeepLinkEventKind,
    /// 链接原文。`workspace-open` / `share-import` 也带上，便于按原文定位问题
    /// （它们只保了解析后的字段，原文不在别处）。
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}
