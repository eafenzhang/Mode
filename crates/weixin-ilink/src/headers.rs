//! 常量与请求头构造（TS `weixinProvider.ts:16-21,110-121` 的对齐）。

use base64::Engine;
use serde_json::json;

/// iLink 通道基址（TS :16）。
pub const DEFAULT_WEIXIN_ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
/// API 前缀（TS :17）。
pub const WEIXIN_BOT_API_PREFIX: &str = "/ilink/bot";
/// `base_info.channel_version`（TS :18）。
pub const WEIXIN_CHANNEL_VERSION: &str = "2.0.0";
/// 机器人消息 `message_type`（TS :19）。
pub const WEIXIN_MESSAGE_TYPE_BOT: i64 = 2;
/// 终态 `message_state`（TS :20）。
pub const WEIXIN_MESSAGE_STATE_FINISH: i64 = 2;
/// `/getupdates` 长轮询超时（TS :22）。
pub const WEIXIN_GET_UPDATES_TIMEOUT_MS: u64 = 90_000;

/// 请求头四件套（TS :114-121）。`uin` 由 [`build_uin_header_value`] 产出。
pub fn build_headers(token: &str, uin: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::from_iter([
        ("content-type".to_string(), json!("application/json")),
        ("AuthorizationType".to_string(), json!("ilink_bot_token")),
        (
            "Authorization".to_string(),
            json!(format!("Bearer {token}")),
        ),
        ("X-WECHAT-UIN".to_string(), json!(uin)),
    ])
}

/// `X-WECHAT-UIN` 的值：随机 u32 十进制字符串的 UTF-8 字节做 base64
/// （TS :110-112 `Buffer.from(String(randomInt(0, 2^32)), "utf8").toString("base64")`）。
/// 随机源由调用方注入（uuid v4 字节前 4 位），纯函数便于复现。
pub fn build_uin_header_value(random_u32: u32) -> String {
    base64::engine::general_purpose::STANDARD.encode(random_u32.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_match_ts_shape() {
        let headers = build_headers("tok", "MzI=");
        let get = |key: &str| headers[key].as_str().expect("字符串头");
        assert_eq!(get("content-type"), "application/json");
        assert_eq!(get("AuthorizationType"), "ilink_bot_token");
        assert_eq!(get("Authorization"), "Bearer tok");
        assert_eq!(get("X-WECHAT-UIN"), "MzI=");
    }

    #[test]
    fn uin_value_is_base64_of_decimal_string() {
        // TS：String(randomInt) 的 UTF-8 字节再 base64。
        assert_eq!(build_uin_header_value(12345), "MTIzNDU=");
    }
}
