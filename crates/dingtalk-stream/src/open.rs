//! Stream 注册：`POST /v1.0/gateway/connections/open`（TS
//! `dingtalkChannelRuntime.ts:75-104`；官方：ticket 90s 有效且只能建一条连接）。
//!
//! HTTP 传输抽成 [`StreamOpen`] trait：runtime 泛型注入，测试用假注册器（本地 WS
//! 服务端）离线对拍；[`HttpStreamOpen`] 是 reqwest 生产实现（15s 超时对齐
//! `providerRequest.ts` 的 `BOT_PROVIDER_REQUEST_TIMEOUT_MS` 口径）。

use std::time::Duration;

use serde_json::Value;

pub const DINGTALK_API_BASE: &str = "https://api.dingtalk.com";
pub const OPEN_CONNECTION_PATH: &str = "/v1.0/gateway/connections/open";
/// TS `fetchBotProviderJson` 的默认 15s 超时（providerRequest.ts）。
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub api_base: String,
    pub client_id: String,
    pub client_secret: String,
}

/// 注册体：订阅面与 TS :90-97 逐字段一致（机器人回调 + 全量事件）。
pub fn build_open_request(request: &OpenRequest) -> Value {
    serde_json::json!({
        "clientId": request.client_id,
        "clientSecret": request.client_secret,
        "subscriptions": [
            { "type": "CALLBACK", "topic": "/v1.0/im/bot/messages/get" },
            { "type": "EVENT", "topic": "*" },
        ],
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub endpoint: String,
    pub ticket: String,
}

#[derive(Debug)]
pub enum OpenError {
    Transport(String),
    Rejected(u16),
    InvalidResponse,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(message) => write!(formatter, "transport: {message}"),
            Self::Rejected(status) => write!(formatter, "rejected: status {status}"),
            Self::InvalidResponse => write!(formatter, "invalid response payload"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<reqwest::Error> for OpenError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for OpenError {
    fn from(error: serde_json::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

/// 注册传输抽象：返回 boxed future 以免 trait 上的 async fn 丢 auto-trait 约束。
pub trait StreamOpen: Send + Sync {
    fn open(
        &self,
        request: &OpenRequest,
    ) -> futures_util::future::BoxFuture<'static, Result<Registration, OpenError>>;
}

/// 生产实现。TS :100-104 的判定：HTTP 非 ok 或缺 endpoint/ticket 一律视为注册失败。
#[derive(Clone)]
pub struct HttpStreamOpen {
    client: reqwest::Client,
}

impl HttpStreamOpen {
    pub fn new() -> Result<Self, OpenError> {
        // TLS provider 由本 crate 自装（与 account-api-key-resolver/coding-plan-http 同惯例）。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(OPEN_TIMEOUT)
            .build()
            .map_err(|error| OpenError::Transport(error.to_string()))?;
        Ok(Self { client })
    }
}

impl StreamOpen for HttpStreamOpen {
    fn open(
        &self,
        request: &OpenRequest,
    ) -> futures_util::future::BoxFuture<'static, Result<Registration, OpenError>> {
        let client = self.client.clone();
        let request = request.clone();
        Box::pin(async move {
            let url = format!(
                "{}{OPEN_CONNECTION_PATH}",
                request.api_base.trim_end_matches('/')
            );
            let response = client
                .post(url)
                .header("content-type", "application/json")
                .body(serde_json::to_string(&build_open_request(&request))?)
                .send()
                .await?;
            let status = response.status();
            // workspace 的 reqwest 未开 json feature：text + parse 等价（TS 也是 text 后 JSON.parse）。
            let payload: Value = serde_json::from_str(&response.text().await?)?;
            if !status.is_success() {
                return Err(OpenError::Rejected(status.as_u16()));
            }
            let endpoint = payload.get("endpoint").and_then(Value::as_str);
            let ticket = payload.get("ticket").and_then(Value::as_str);
            match (endpoint, ticket) {
                (Some(endpoint), Some(ticket)) if !endpoint.is_empty() && !ticket.is_empty() => {
                    Ok(Registration {
                        endpoint: endpoint.to_string(),
                        ticket: ticket.to_string(),
                    })
                }
                _ => Err(OpenError::InvalidResponse),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_request_body_matches_ts_subscription_surface() {
        // TS :90-97：CALLBACK 机器人消息 + 全量 EVENT；字段名 clientId/clientSecret。
        let body = build_open_request(&OpenRequest {
            api_base: DINGTALK_API_BASE.into(),
            client_id: "cid".into(),
            client_secret: "secret".into(),
        });
        assert_eq!(body["clientId"], serde_json::json!("cid"));
        assert_eq!(body["clientSecret"], serde_json::json!("secret"));
        assert_eq!(
            body["subscriptions"],
            serde_json::json!([
                { "type": "CALLBACK", "topic": "/v1.0/im/bot/messages/get" },
                { "type": "EVENT", "topic": "*" },
            ])
        );
    }
}
