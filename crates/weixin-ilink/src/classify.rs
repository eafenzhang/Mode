//! 失败分类（TS `weixinProvider.ts:133-159` 的纯函数对齐）与错误类型。
//!
//! 分类决定恢复链路（`bot-outbound-delivery.md` 规则 2/3/4）：
//! session_expired → 停放补投；context_expired → 刷 context_token 重试；
//! retryable → 有界退避重试。2026-10-10 事故：会话过期后 /sendmessage 返回
//! errmsg="prepare failed"，必须显式归类而不是当未知错误丢回复。

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeixinFailureCode {
    /// 登录态丢失：token 缺失 / HTTP 401 / 403。
    SessionExpired,
    /// 会话上下文过期：errmsg = "prepare failed"。
    ContextExpired,
    /// 有界退避可重试：HTTP 429 / ≥500 / 网络与超时错误。
    Retryable,
}

impl WeixinFailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionExpired => "session_expired",
            Self::ContextExpired => "context_expired",
            Self::Retryable => "retryable",
        }
    }
}

/// 纯函数：HTTP 状态与 errmsg 的分类（TS :138-159）。两类都有时 HTTP 优先。
pub fn classify_failure(
    http_status: Option<u16>,
    errmsg: Option<&str>,
) -> Option<WeixinFailureCode> {
    if let Some(status) = http_status {
        if status == 401 || status == 403 {
            return Some(WeixinFailureCode::SessionExpired);
        }
        if status == 429 || status >= 500 {
            return Some(WeixinFailureCode::Retryable);
        }
        return None;
    }
    if errmsg == Some("prepare failed") {
        return Some(WeixinFailureCode::ContextExpired);
    }
    None
}

/// 请求面错误：`BotSendError`（带恢复分类）与未知错误的二分。
#[derive(Debug)]
pub enum WeixinError {
    /// 带恢复分类（走停放/退避/续期链路）。
    Bot(WeixinFailureCode, String),
    /// 未分类错误：按既有恢复链当未知处理。
    Unknown(String),
}

impl WeixinError {
    pub fn unknown(message: impl Into<String>) -> Self {
        Self::Unknown(message.into())
    }
}

impl std::fmt::Display for WeixinError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bot(code, message) => write!(formatter, "{}: {message}", code.as_str()),
            Self::Unknown(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for WeixinError {}

/// 常用超时（测试也用小值加速）。生产值见各调用点：getupdates 90s，其余 15s。
pub fn duration_secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}
