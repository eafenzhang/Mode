//! iLink HTTP 请求面：传输 trait + 响应解析纯函数 + reqwest 生产实现。
//!
//! TS `requestWeixinJson`（weixinProvider.ts:160-220）的对齐：
//! - 头/体构造见 [`crate::headers`]；
//! - HTTP 非 ok → 分类（Bot）或未知错误，**先于**业务码检查；
//! - 业务码：`ret !== 0 || errcode != 0` 即失败，errmsg/message 取首个；
//! - 空 body 视为成功（TS :104-118 的宽容语义）。

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::classify::{classify_failure, WeixinError};

/// 一次 POST 的全部输入（传输实现负责拼 URL/头并发出）。
#[derive(Debug, Clone)]
pub struct WeixinRequest {
    /// 完整 URL：`{base}{/ilink/bot}{path}`。
    pub url: String,
    pub token: String,
    pub body: Value,
    /// 单请求超时（getupdates 90s，其余 15s）。
    pub timeout: Duration,
}

pub type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;

/// 传输抽象：测试注入本地假 HTTP 服务端或直接注入响应。
pub trait WeixinHttp: Send + Sync {
    fn post_json(&self, request: WeixinRequest) -> BoxFuture<Result<Value, WeixinError>>;
}

/// 响应解析纯函数（TS :195-219）。`path` 只进错误文案。
pub fn parse_weixin_response(path: &str, status: u16, text: &str) -> Result<Value, WeixinError> {
    if !(200..300).contains(&status) {
        let message = format!("Weixin iLink {path} failed: HTTP {status}");
        return match classify_failure(Some(status), None) {
            Some(code) => Err(WeixinError::Bot(code, message)),
            None => Err(WeixinError::Unknown(message)),
        };
    }
    if text.is_empty() {
        // TS :104-118：空 body → payload undefined → ret/errcode 缺省 → 成功。
        return Ok(Value::Null);
    }
    let payload: Value = match serde_json::from_str(text) {
        Ok(payload) => payload,
        Err(error) => return Err(WeixinError::unknown(error.to_string())),
    };
    let ret = payload.get("ret").and_then(Value::as_i64);
    let errcode = payload.get("errcode").and_then(Value::as_i64);
    // TS 的 `ret !== null && ret !== 0`：字段缺失（null）不算失败，只有显式非 0 才失败。
    if ret.is_some_and(|value| value != 0) || errcode.is_some_and(|value| value != 0) {
        let message = payload
            .get("errmsg")
            .and_then(Value::as_str)
            .or_else(|| payload.get("message").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "ret={} errcode={}",
                    ret.map(|value| value.to_string()).unwrap_or_default(),
                    errcode.map(|value| value.to_string()).unwrap_or_default()
                )
                .trim()
                .to_string()
            });
        return match classify_failure(None, Some(&message)) {
            Some(code) => Err(WeixinError::Bot(
                code,
                format!("Weixin iLink {path} failed: {message}"),
            )),
            None => Err(WeixinError::unknown(format!(
                "Weixin iLink {path} failed: {message} (ret={ret:?} errcode={errcode:?})"
            ))),
        };
    }
    Ok(payload)
}

/// reqwest 生产实现（ring provider 自装，与 workspace 惯例一致）。
#[derive(Clone)]
pub struct ReqwestWeixinHttp {
    client: Arc<reqwest::Client>,
}

impl ReqwestWeixinHttp {
    pub fn new() -> Result<Self, WeixinError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| WeixinError::unknown(error.to_string()))?;
        Ok(Self {
            client: Arc::new(client),
        })
    }
}

impl WeixinHttp for ReqwestWeixinHttp {
    fn post_json(&self, request: WeixinRequest) -> BoxFuture<Result<Value, WeixinError>> {
        let client = Arc::clone(&self.client);
        Box::pin(async move {
            // 头四件套（TS :114-121）；UIN 随机源 = uuid v4 前 4 字节。
            let uin = crate::headers::build_uin_header_value(uuid_bytes_u32());
            let response = client
                .post(&request.url)
                .timeout(request.timeout)
                .header("content-type", "application/json")
                .header("AuthorizationType", "ilink_bot_token")
                .header("Authorization", format!("Bearer {}", request.token))
                .header("X-WECHAT-UIN", uin)
                .body(request.body.to_string())
                .send()
                .await
                .map_err(|error| {
                    // TS :186-194：网络/超时归 retryable，走有界退避。
                    WeixinError::Bot(
                        crate::classify::WeixinFailureCode::Retryable,
                        error.to_string(),
                    )
                })?;
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            parse_weixin_response(&path_of(&request.url), status, &text)
        })
    }
}

fn path_of(url: &str) -> String {
    url.rsplit('/').next().unwrap_or(url).to_string()
}

/// 从 uuid v4 前 4 字节取随机 u32（TS randomInt(0, 2^32) 的等价随机源）。
fn uuid_bytes_u32() -> u32 {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::classify::WeixinFailureCode;

    #[test]
    fn http_status_classification_matches_ts() {
        // 401/403 → session_expired；429/5xx → retryable；其余非 2xx → 未知。
        assert!(matches!(
            parse_weixin_response("sendmessage", 401, ""),
            Err(WeixinError::Bot(WeixinFailureCode::SessionExpired, _))
        ));
        assert!(matches!(
            parse_weixin_response("sendmessage", 429, "{}"),
            Err(WeixinError::Bot(WeixinFailureCode::Retryable, _))
        ));
        assert!(matches!(
            parse_weixin_response("sendmessage", 400, "{}"),
            Err(WeixinError::Unknown(_))
        ));
    }

    #[test]
    fn business_code_and_prepare_failed_classification() {
        // ret/errcode 非 0 + "prepare failed" → context_expired（2026-10-10 事故）。
        let payload = serde_json::to_string(&json!({
            "ret": 1,
            "errmsg": "prepare failed",
        }))
        .unwrap();
        assert!(matches!(
            parse_weixin_response("sendmessage", 200, &payload),
            Err(WeixinError::Bot(WeixinFailureCode::ContextExpired, _))
        ));

        // ret=0 → 成功透传 payload。
        let ok = serde_json::to_string(&json!({ "ret": 0, "data": { "x": 1 } })).unwrap();
        let value = parse_weixin_response("getupdates", 200, &ok).unwrap();
        assert_eq!(value["data"]["x"], json!(1));
    }

    #[test]
    fn empty_body_is_success_and_errcode_zero_passes() {
        assert_eq!(
            parse_weixin_response("getupdates", 200, "").unwrap(),
            Value::Null
        );
        let payload = serde_json::to_string(&json!({ "errcode": 0 })).unwrap();
        assert!(parse_weixin_response("getupdates", 200, &payload).is_ok());
    }
}
