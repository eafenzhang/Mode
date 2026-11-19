//! 连接注册（HTTP 网关）与重连参数（spec §5.1；SDK :85191-85366, :85165-85175, :85260-85267）。

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

/// domain 映射（SDK :216-222, :140-142）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeishuDomain {
    #[default]
    Feishu,
    Lark,
}

impl FeishuDomain {
    pub fn base_url(self) -> &'static str {
        match self {
            Self::Feishu => "https://open.feishu.cn",
            Self::Lark => "https://open.larksuite.com",
        }
    }
}

/// 应用凭据（SDK :85309-85320）。
#[derive(Debug, Clone)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub domain: FeishuDomain,
}

/// 会话参数：注册响应展开（SDK `ClientConfig` + URL query，:85348-85356），
/// 缺省值对齐 WSConfig 构造（:85165-85175）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionParams {
    /// 注册返回的 WS 连接串（原样，含 ticket）。
    pub connect_url: String,
    pub device_id: String,
    pub service_id: String,
    pub ping_interval: Duration,
    /// -1 = 无限重连（WSConfig 缺省）。
    pub reconnect_count: i64,
    pub reconnect_interval: Duration,
    pub reconnect_nonce: Duration,
}

impl Default for SessionParams {
    fn default() -> Self {
        Self {
            connect_url: String::new(),
            device_id: String::new(),
            service_id: String::new(),
            ping_interval: Duration::from_secs(120),
            reconnect_count: -1,
            reconnect_interval: Duration::from_secs(120),
            reconnect_nonce: Duration::from_secs(30),
        }
    }
}

/// ErrorCode（SDK :85260-85267）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayErrorCode {
    SystemBusy,
    Forbidden,
    AuthFailed,
    InternalError,
    ExceedConnLimit,
    Other(i64),
}

impl GatewayErrorCode {
    pub fn from_code(code: i64) -> Self {
        match code {
            1 => Self::SystemBusy,
            403 => Self::Forbidden,
            514 => Self::AuthFailed,
            1000040343 => Self::InternalError,
            1000040350 => Self::ExceedConnLimit,
            other => Self::Other(other),
        }
    }
}

#[derive(Debug)]
pub enum GatewayError {
    /// HTTP/网络层失败。
    Transport(String),
    /// 响应 code != 0（SDK :85342-85347 → 注册失败进重试）。
    Rejected {
        code: GatewayErrorCode,
        message: String,
    },
    /// 2xx 但缺 URL/ClientConfig（SDK 走 qs.parse 失败 → catch → false）。
    InvalidResponse,
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(message) => write!(formatter, "gateway transport: {message}"),
            Self::Rejected { code, message } => {
                write!(formatter, "gateway rejected: {code:?} {message}")
            }
            Self::InvalidResponse => write!(formatter, "gateway response missing URL"),
        }
    }
}

pub type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;

/// 注册传输抽象：测试注入本地假网关。
pub trait PullConfig: Send + Sync {
    fn pull(&self, config: &FeishuConfig) -> BoxFuture<Result<SessionParams, GatewayError>>;
}

/// 响应解析纯函数（SDK :85329-85359）：code 必须 = 0；URL query 解析
/// `device_id` / `service_id`；`ClientConfig` 秒 → 毫秒；缺省项回退 WSConfig 默认。
pub fn parse_registration(
    payload: &Value,
    defaults: &SessionParams,
) -> Result<SessionParams, GatewayError> {
    let code = payload
        .get("code")
        .and_then(Value::as_i64)
        .ok_or(GatewayError::InvalidResponse)?;
    if code != 0 {
        return Err(GatewayError::Rejected {
            code: GatewayErrorCode::from_code(code),
            message: payload
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("system busy")
                .to_string(),
        });
    }
    let data = payload.get("data").ok_or(GatewayError::InvalidResponse)?;
    let url = data
        .get("URL")
        .and_then(Value::as_str)
        .ok_or(GatewayError::InvalidResponse)?;
    let parsed = url::Url::parse(url).map_err(|_| GatewayError::InvalidResponse)?;
    let query = |key: &str| {
        parsed
            .query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    let client_config = data.get("ClientConfig").cloned().unwrap_or(Value::Null);
    let seconds = |key: &str, fallback: Duration| -> Duration {
        client_config
            .get(key)
            .and_then(Value::as_f64)
            .map(Duration::from_secs_f64)
            .unwrap_or(fallback)
    };
    let count = client_config
        .get("ReconnectCount")
        .and_then(Value::as_i64)
        .unwrap_or(defaults.reconnect_count);
    Ok(SessionParams {
        connect_url: url.to_string(),
        device_id: query("device_id"),
        service_id: query("service_id"),
        ping_interval: seconds("PingInterval", defaults.ping_interval),
        reconnect_count: count,
        reconnect_interval: seconds("ReconnectInterval", defaults.reconnect_interval),
        reconnect_nonce: seconds("ReconnectNonce", defaults.reconnect_nonce),
    })
}

/// reqwest 生产实现（ring provider 自装，与 workspace 惯例一致）。
#[derive(Clone)]
pub struct ReqwestPullConfig {
    client: Arc<reqwest::Client>,
}

impl Default for ReqwestPullConfig {
    fn default() -> Self {
        Self::new().expect("reqwest client")
    }
}

impl ReqwestPullConfig {
    pub fn new() -> Result<Self, GatewayError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| GatewayError::Transport(error.to_string()))?;
        Ok(Self {
            client: Arc::new(client),
        })
    }
}

impl PullConfig for ReqwestPullConfig {
    fn pull(&self, config: &FeishuConfig) -> BoxFuture<Result<SessionParams, GatewayError>> {
        let client = Arc::clone(&self.client);
        let config = config.clone();
        Box::pin(async move {
            let url = format!(
                "{}/callback/ws/endpoint",
                config.domain.base_url().trim_end_matches('/')
            );
            let response = client
                .post(url)
                .timeout(Duration::from_secs(15))
                .header("locale", "zh")
                .header("content-type", "application/json")
                .body(
                    serde_json::to_string(&serde_json::json!({
                        "AppID": config.app_id,
                        "AppSecret": config.app_secret,
                    }))
                    .unwrap_or_default(),
                )
                .send()
                .await
                .map_err(|error| GatewayError::Transport(error.to_string()))?;
            let text = response.text().await.unwrap_or_default();
            let payload: Value = serde_json::from_str(&text)
                .map_err(|error| GatewayError::Transport(error.to_string()))?;
            let defaults = SessionParams::default();
            parse_registration(&payload, &defaults)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_registration_url_and_client_config() {
        let payload = json!({
            "code": 0,
            "data": {
                "URL": "wss://gw.example/connect?device_id=d-1&service_id=42&ticket=tk",
                "ClientConfig": {
                    "PingInterval": 30,
                    "ReconnectCount": 5,
                    "ReconnectInterval": 10,
                    "ReconnectNonce": 3
                }
            }
        });
        let params = parse_registration(&payload, &SessionParams::default()).expect("解析成功");
        assert_eq!(
            params.connect_url,
            "wss://gw.example/connect?device_id=d-1&service_id=42&ticket=tk"
        );
        assert_eq!(params.device_id, "d-1");
        assert_eq!(params.service_id, "42");
        assert_eq!(params.ping_interval, Duration::from_secs(30));
        assert_eq!(params.reconnect_count, 5);
        assert_eq!(params.reconnect_interval, Duration::from_secs(10));
        assert_eq!(params.reconnect_nonce, Duration::from_secs(3));
    }

    #[test]
    fn missing_client_config_falls_back_to_defaults() {
        let payload = json!({
            "code": 0,
            "data": { "URL": "wss://gw.example/connect?service_id=7" }
        });
        let defaults = SessionParams::default();
        let params = parse_registration(&payload, &defaults).expect("解析成功");
        assert_eq!(params.ping_interval, defaults.ping_interval);
        assert_eq!(params.reconnect_count, -1);
        assert_eq!(params.service_id, "7");
    }

    #[test]
    fn error_codes_map_to_sdk_enum() {
        for (code, expected) in [
            (1, GatewayErrorCode::SystemBusy),
            (403, GatewayErrorCode::Forbidden),
            (514, GatewayErrorCode::AuthFailed),
            (1000040343, GatewayErrorCode::InternalError),
            (1000040350, GatewayErrorCode::ExceedConnLimit),
            (999, GatewayErrorCode::Other(999)),
        ] {
            assert_eq!(GatewayErrorCode::from_code(code), expected);
        }
        let payload = json!({ "code": 514, "msg": "auth failed" });
        assert!(matches!(
            parse_registration(&payload, &SessionParams::default()),
            Err(GatewayError::Rejected {
                code: GatewayErrorCode::AuthFailed,
                ..
            })
        ));
    }

    #[test]
    fn missing_url_is_invalid_response() {
        let payload = json!({ "code": 0, "data": {} });
        assert!(matches!(
            parse_registration(&payload, &SessionParams::default()),
            Err(GatewayError::InvalidResponse)
        ));
    }
}
