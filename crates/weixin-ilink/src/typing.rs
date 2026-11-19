//! typing 票缓存与重试流程（TS `weixinProvider.ts:731-795` 的对齐）。
//!
//! `typing_ticket` 按 `bot:用户` 缓存；`/sendtyping` 失败按票过期处理——
//! 强刷一次再试，仍失败返回首个错误（docs/specs/im-typing-feedback.md：失败不允许静默）。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::json;
use serde_json::Value;

use crate::classify::WeixinError;
use crate::headers::WEIXIN_BOT_API_PREFIX;
use crate::http::WeixinHttp;

#[derive(Default)]
pub struct TypingTickets {
    inner: Mutex<HashMap<String, String>>,
}

impl TypingTickets {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// `/getconfig` 的 `typing_ticket`（TS :754-765）：现拉一次并更新缓存。
#[allow(clippy::too_many_arguments)]
pub async fn fetch_typing_ticket(
    http: &dyn WeixinHttp,
    token: &str,
    base_url: Option<&str>,
    tickets: &TypingTickets,
    bot_id: &str,
    provider_user_id: &str,
    provider_context_token: Option<&str>,
    refresh: bool,
) -> Result<Option<String>, WeixinError> {
    let key = format!("{bot_id}:{provider_user_id}");
    if !refresh {
        if let Ok(cached) = tickets.inner.lock() {
            if let Some(cached) = cached.get(&key) {
                return Ok(Some(cached.clone()));
            }
        }
    }
    let mut body = json!({ "ilink_user_id": provider_user_id });
    if let Some(context_token) = provider_context_token {
        body["context_token"] = json!(context_token);
    }
    let config = post(http, token, base_url, "/getconfig", body).await?;
    let ticket = config
        .get("typing_ticket")
        .or_else(|| {
            config
                .get("data")
                .and_then(|data| data.get("typing_ticket"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|ticket| !ticket.is_empty())
        .map(str::to_string);
    if let Ok(mut cache) = tickets.inner.lock() {
        match &ticket {
            Some(ticket) => {
                cache.insert(key, ticket.clone());
            }
            None => {
                cache.remove(&key);
            }
        }
    }
    Ok(ticket)
}

/// 开启（status=1）/取消（status=2）：票复用 + 失败强刷重试一次（TS :768-795）。
/// 票拿不到直接报错（TS :776-778 的修复：不静默）。
#[allow(clippy::too_many_arguments)]
pub async fn request_typing(
    http: &dyn WeixinHttp,
    token: &str,
    base_url: Option<&str>,
    tickets: &TypingTickets,
    bot_id: &str,
    provider_user_id: &str,
    provider_context_token: Option<&str>,
    status: u8,
) -> Result<(), WeixinError> {
    let key = format!("{bot_id}:{provider_user_id}");
    let ticket = fetch_typing_ticket(
        http,
        token,
        base_url,
        tickets,
        bot_id,
        provider_user_id,
        provider_context_token,
        false,
    )
    .await?;
    let Some(ticket) = ticket else {
        return Err(WeixinError::unknown(format!(
            "Weixin typing_ticket unavailable user={provider_user_id}"
        )));
    };
    let send_once = |value: &str| {
        post(
            http,
            token,
            base_url,
            "/sendtyping",
            json!({
                "ilink_user_id": provider_user_id,
                "typing_ticket": value,
                "status": status,
            }),
        )
    };
    if let Err(first_error) = send_once(&ticket).await {
        if let Ok(mut cache) = tickets.inner.lock() {
            cache.remove(&key);
        }
        let fresh = fetch_typing_ticket(
            http,
            token,
            base_url,
            tickets,
            bot_id,
            provider_user_id,
            provider_context_token,
            true,
        )
        .await?;
        let Some(fresh) = fresh else {
            return Err(first_error);
        };
        send_once(&fresh).await?;
    }
    Ok(())
}

async fn post(
    http: &dyn WeixinHttp,
    token: &str,
    base_url: Option<&str>,
    path: &str,
    body: Value,
) -> Result<Value, WeixinError> {
    let base = base_url.unwrap_or(crate::headers::DEFAULT_WEIXIN_ILINK_BASE_URL);
    let request = crate::http::WeixinRequest {
        url: format!(
            "{}{WEIXIN_BOT_API_PREFIX}{path}",
            base.trim_end_matches('/')
        ),
        token: token.to_string(),
        body,
        // TS 默认 15s 超时（providerRequest.ts 的 BOT_PROVIDER_REQUEST_TIMEOUT_MS）。
        timeout: std::time::Duration::from_secs(15),
    };
    http.post_json(request).await
}
