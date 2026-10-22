//! stdio 上的帧形状：JSON-RPC 风格的**四类**帧。
//!
//! 实测（`docs/specs/tauri-agent-runtime.md` §1.2，两种独立观察一致）：
//!
//! | 线上形状 | 含义 |
//! | --- | --- |
//! | `{"method":"…","params":{…}}` | 通知（无 id） |
//! | `{"id":…,"method":"…","params":{…}}` | 反向请求：运行时向 host 要东西，host 必须应答 |
//! | `{"id":…,"result":{…}}` | 应答 |
//! | `{"id":…,"error":{"code":…,"message":…}}` | 错误应答 |
//!
//! **不是** `{type, payload}`，也不是主题帧——那是 host 面（`mode-protocol-v4`）的形状。
//!
//! 严格度：帧形状是封闭契约，四个变体都带 `deny_unknown_fields`（多一个键就整体拒绝），
//! 与 TS 侧 `modeProtocolMessageSchema` 各成员的 `.strict()` 同口径。
//! `params` / `result` 内部**不在这里**建模：它们的类型由会话事件与 host 命令各自的模块负责；
//! 未建模的部分按原样保留，不因为一处未建模就整帧解析失败（见 crate 文档的严格度规则）。
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 请求 id：线上**两种都出现过**（实测）。
///
/// - host 自己发请求时用数字（`session/create` 的 `id:1`）；
/// - 运行时发反向请求时用字符串（`session/requestRuntimePreferences` 的 `id:"server-1"`）。
///
/// 两端共用同一个 id 空间，所以应答与请求靠 id 关联时不能假定是数字。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

/// 同一条 stdio 连接上反向请求的 id 分配器（`server-1`、`server-2` …，实测形状）。
///
/// **必须按连接共享**：多个会话、多个反向请求发射点各自从 1 开始会让 id 碰撞，
/// 应答会被错误的请求认领。用原子计数是为了让共享实例可以安全地被多个处理器并发持有。
#[derive(Debug, Default)]
pub struct ReverseRequestIds {
    next: AtomicU64,
}

impl ReverseRequestIds {
    /// 取下一个 id。
    pub fn next(&self) -> RequestId {
        let value = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        RequestId::String(format!("server-{value}"))
    }
}

/// 反向请求 / 请求（`{"id":…,"method":…,"params":…}`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestFrame {
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// 追踪上下文：M2 的捕获里从未出现，未建模（原样保留，见 crate 文档）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
}

/// 通知（`{"method":…,"params":…}`，无 id）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationFrame {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// 同 [`RequestFrame::trace`]：未建模。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
}

/// 应答（`{"id":…,"result":…}`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseFrame {
    pub id: RequestId,
    pub result: Value,
}

/// 错误对象（`{"code":…,"message":…}`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 错误应答（`{"id":…,"error":{…}}`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorResponseFrame {
    pub id: RequestId,
    pub error: ErrorObject,
}

/// 一条线上帧。四类形状靠键集互斥（id/result/error/method 的在场组合），
/// 所以可以直接按顺序试解，不需要先看别的字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Frame {
    /// 错误应答：`{id, error}`。
    ErrorResponse(ErrorResponseFrame),
    /// 应答：`{id, result}`。
    Response(ResponseFrame),
    /// 请求：`{id, method, params?}`。
    Request(RequestFrame),
    /// 通知：`{method, params?}`。
    Notification(NotificationFrame),
}

/// 额外字段的落脚类型（见 crate 文档的严格度规则）。
pub type ExtraFields = BTreeMap<String, Value>;
