//! LAN 配对 HTTP 处理器：请求校验、限速、Cookie 与令牌提取。
//!
//! 真源：`packages/server/src/http.ts` 的 `/api/lan/pair`：
//! - 30 次/分钟限速（配对码只有 6 位，必须限制试探）；
//! - 失败统一 `401 {"error":"Invalid or expired pair code"}`，不区分码错/过期；
//! - 成功后写入 HttpOnly `mode_lite_token` Cookie，令牌同时出现在响应体。
//!
//! 处理器与传输同文件：`handle_pair` 是纯逻辑，`serve` 用 hyper 1.x 提供
//! `POST /api/lan/pair` 与四个管理端点（Bearer/Cookie/query 令牌鉴权）。
//! 监听视图（enabled/port/addresses/会话数/中继地址）由宿主写入。
//! `/ws`、公网中继与远程控制不在本模块。

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lan_pairing::{LanAccessPairResult, LanCredentialPort, LanPairing};
use serde_json::{json, Value};
use tokio::net::TcpListener;

pub const PAIR_PATH: &str = "/api/lan/pair";
pub const MODE_LITE_TOKEN_COOKIE: &str = "mode_lite_token";
pub const LEGACY_LITE_TOKEN_COOKIE: &str = "zcode_lite_token";
pub const PAIRING_ATTEMPT_LIMIT: usize = 30;
pub const PAIRING_ATTEMPT_WINDOW: Duration = Duration::from_secs(60);

pub struct LanPairHttp<C: LanCredentialPort + Send + 'static> {
    pairing: tokio::sync::Mutex<LanPairing<C>>,
    attempts: Mutex<VecDeque<Instant>>,
    attempt_limit: usize,
    attempt_window: Duration,
    /// 传输层维护的监听视图（enabled/port/addresses/会话数/中继地址）。
    view: Mutex<LanAccessView>,
}

/// 设置页状态快照里与监听生命周期相关的部分；由传输层/宿主更新。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanAccessView {
    pub enabled: bool,
    pub port: Option<u16>,
    pub addresses: Vec<String>,
    pub active_web_sessions: u32,
    pub public_relay_url: Option<String>,
}

/// 传输无关的配对响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// 仅成功时存在；值是完整的 `Set-Cookie` 串。
    pub set_cookie: Option<String>,
}

impl<C: LanCredentialPort + Send + 'static> LanPairHttp<C> {
    pub fn new(pairing: LanPairing<C>) -> Self {
        Self {
            pairing: tokio::sync::Mutex::new(pairing),
            attempts: Mutex::new(VecDeque::new()),
            attempt_limit: PAIRING_ATTEMPT_LIMIT,
            attempt_window: PAIRING_ATTEMPT_WINDOW,
            view: Mutex::new(LanAccessView::default()),
        }
    }

    /// 传输层更新监听状态（启用/端口/地址/会话数/中继地址）。
    pub fn set_view(&self, view: LanAccessView) {
        *self.view.lock().expect("lan access view poisoned") = view;
    }

    /// `GET /api/lan/state`。
    pub async fn get_state(&self) -> Value {
        self.state_value().await
    }

    /// `POST /api/lan/pair-code`：监听未开启时拒绝。
    pub async fn create_pair_code(&self) -> Result<Value, &'static str> {
        if !self.is_enabled() {
            return Err("LAN access is not enabled");
        }
        self.pairing.lock().await.create_pair_code();
        Ok(self.state_value().await)
    }

    /// `DELETE /api/lan/clients/:clientId`。
    pub async fn remove_client(&self, client_id: &str) -> Result<Value, &'static str> {
        if !self.is_enabled() {
            return Err("LAN access is not enabled");
        }
        self.pairing
            .lock()
            .await
            .remove_client(client_id)
            .map_err(|_| "Failed to persist LAN clients")?;
        Ok(self.state_value().await)
    }

    /// `POST /api/lan/reset`。
    pub async fn reset_tokens(&self) -> Result<Value, &'static str> {
        if !self.is_enabled() {
            return Err("LAN access is not enabled");
        }
        self.pairing
            .lock()
            .await
            .reset_tokens()
            .map_err(|_| "Failed to persist LAN clients")?;
        Ok(self.state_value().await)
    }

    fn is_enabled(&self) -> bool {
        self.view.lock().expect("lan access view poisoned").enabled
    }

    async fn state_value(&self) -> Value {
        let mut pairing = self.pairing.lock().await;
        let pair_code = pairing.current_pair_code();
        let clients = pairing.clients().to_vec();
        drop(pairing);
        let view = self.view.lock().expect("lan access view poisoned").clone();
        json!({
            "enabled": view.enabled,
            "port": view.port,
            "addresses": view.addresses,
            "pairCode": pair_code,
            "clients": clients,
            "activeWebSessions": view.active_web_sessions,
            "publicRelayUrl": view.public_relay_url,
        })
    }

    /// 仅用于测试/诊断：自定义限速窗口。
    pub fn with_attempt_limit(mut self, limit: usize, window: Duration) -> Self {
        self.attempt_limit = limit.max(1);
        self.attempt_window = window;
        self
    }

    /// 处理 `POST /api/lan/pair` 的请求体。
    pub async fn handle_pair(&self, body: &[u8]) -> PairHttpResponse {
        if !self.allow_attempt() {
            return error(429, "Too many pairing attempts");
        }
        let raw: Value = match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(_) => return error(400, "Invalid JSON body"),
        };
        let Some(object) = raw.as_object() else {
            return error(400, "Invalid JSON body");
        };
        let Some(code) = object
            .get("code")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|code| (4..=32).contains(&code.len()))
        else {
            return error(400, "Invalid request body: code");
        };
        let label = match object.get("label") {
            None | Some(Value::Null) => None,
            Some(Value::String(label)) => {
                let label = label.trim();
                if label.len() > 64 {
                    return error(400, "Invalid request body: label");
                }
                Some(label)
            }
            Some(_) => return error(400, "Invalid request body: label"),
        };

        let mut pairing = self.pairing.lock().await;
        match pairing.redeem(code, label) {
            Ok(Some(result)) => success(result),
            Ok(None) => error(401, "Invalid or expired pair code"),
            Err(_) => error(500, "Failed to persist pairing result"),
        }
    }

    fn allow_attempt(&self) -> bool {
        let now = Instant::now();
        let mut attempts = self.attempts.lock().expect("pair attempts poisoned");
        while attempts
            .front()
            .is_some_and(|attempt| now.duration_since(*attempt) > self.attempt_window)
        {
            attempts.pop_front();
        }
        if attempts.len() >= self.attempt_limit {
            return false;
        }
        attempts.push_back(now);
        true
    }
}

/// 接受连接并逐个 serve；调用方负责监听端口的生命周期。
pub async fn serve<C>(listener: TcpListener, state: Arc<LanPairHttp<C>>) -> std::io::Result<()>
where
    C: LanCredentialPort + Send + 'static,
{
    loop {
        let (stream, _peer) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |request: Request<Incoming>| {
                let state = Arc::clone(&state);
                async move { Ok::<_, Infallible>(dispatch(&state, request).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });
    }
}

async fn dispatch<C>(
    state: &Arc<LanPairHttp<C>>,
    request: Request<Incoming>,
) -> Response<Full<Bytes>>
where
    C: LanCredentialPort + Send + 'static,
{
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    if method == Method::POST && path == PAIR_PATH {
        let body = match http_body_util::BodyExt::collect(request.into_body()).await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return json_response(400, json!({ "error": "Invalid JSON body" })),
        };
        return pair_response(state.handle_pair(&body).await);
    }

    let is_management = (method == Method::GET && path == "/api/lan/state")
        || (method == Method::POST && path == "/api/lan/pair-code")
        || (method == Method::POST && path == "/api/lan/reset")
        || (method == Method::DELETE && path.starts_with("/api/lan/clients/"));
    if !is_management {
        return json_response(404, json!({ "error": "Not Found" }));
    }

    let query_token = request
        .uri()
        .query()
        .and_then(|query| {
            query.split('&').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                (key == "token").then(|| value.to_string())
            })
        });
    if !authorized(state, request.headers(), query_token.as_deref()).await {
        return json_response(401, json!({ "error": "Unauthorized" }));
    }
    let result = match (method, path.as_str()) {
        (Method::GET, "/api/lan/state") => Ok(state.get_state().await),
        (Method::POST, "/api/lan/pair-code") => state.create_pair_code().await,
        (Method::POST, "/api/lan/reset") => state.reset_tokens().await,
        (Method::DELETE, value) if value.starts_with("/api/lan/clients/") => {
            let client_id = &value["/api/lan/clients/".len()..];
            if client_id.is_empty() {
                return json_response(400, json!({ "error": "Missing clientId" }));
            }
            state.remove_client(client_id).await
        }
        _ => return json_response(404, json!({ "error": "Not Found" })),
    };
    match result {
        Ok(value) => json_response(200, value),
        Err(message) => json_response(409, json!({ "error": message })),
    }
}

async fn authorized<C>(
    state: &Arc<LanPairHttp<C>>,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> bool
where
    C: LanCredentialPort + Send + 'static,
{
    let authorization = headers.get(AUTHORIZATION).and_then(|value| value.to_str().ok());
    let cookie = headers.get(COOKIE).and_then(|value| value.to_str().ok());
    let Some(token) = presented_token(authorization, cookie, query_token) else {
        return false;
    };
    state
        .pairing
        .lock()
        .await
        .verify_token(&token)
        .is_some()
}

fn json_response(status: u16, value: Value) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(&value).unwrap_or_default();
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("valid response")
}

fn pair_response(response: PairHttpResponse) -> Response<Full<Bytes>> {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json");
    if let Some(cookie) = response.set_cookie {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            builder = builder.header(SET_COOKIE, value);
        }
    }
    builder
        .body(Full::new(Bytes::from(response.body)))
        .expect("valid response")
}

fn success(result: LanAccessPairResult) -> PairHttpResponse {
    PairHttpResponse {
        status: 200,
        body: serde_json::to_vec(&result).unwrap_or_else(|_| b"{}".to_vec()),
        set_cookie: Some(format!(
            "{MODE_LITE_TOKEN_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax",
            result.token
        )),
    }
}

fn error(status: u16, message: &str) -> PairHttpResponse {
    PairHttpResponse {
        status,
        body: serde_json::to_vec(&json!({ "error": message })).unwrap_or_default(),
        set_cookie: None,
    }
}

/// 读取请求里的令牌：`Authorization: Bearer`、`?token=`、两种 Cookie 名。
/// 与 TS `readPresentedLiteToken` 同口径，供后续 `/api/*` 鉴权中间件复用。
pub fn presented_token(
    authorization: Option<&str>,
    cookie_header: Option<&str>,
    query_token: Option<&str>,
) -> Option<String> {
    if let Some(token) = query_token.map(str::trim).filter(|token| !token.is_empty()) {
        return Some(token.to_string());
    }
    if let Some(token) = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return Some(token.to_string());
    }
    let cookies = parse_cookies(cookie_header?);
    cookies
        .get(MODE_LITE_TOKEN_COOKIE)
        .or_else(|| cookies.get(LEGACY_LITE_TOKEN_COOKIE))
        .cloned()
}

fn parse_cookies(header: &str) -> std::collections::HashMap<String, String> {
    let mut cookies = std::collections::HashMap::new();
    for part in header.split(';') {
        let Some(separator) = part.find('=') else {
            continue;
        };
        if separator == 0 {
            continue;
        }
        cookies.insert(
            part[..separator].trim().to_string(),
            part[separator + 1..].trim().to_string(),
        );
    }
    cookies
}

#[cfg(test)]
mod tests {
    use super::*;
    use lan_pairing::{mint_client_token, LanAccessClientRecord};
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct MemoryCredentials(RefCell<HashMap<String, String>>);

    impl Default for MemoryCredentials {
        fn default() -> Self {
            Self(RefCell::new(HashMap::new()))
        }
    }

    impl LanCredentialPort for MemoryCredentials {
        fn load(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.borrow().get(key).cloned())
        }
        fn save(&self, key: &str, value: &str) -> Result<(), String> {
            self.0.borrow_mut().insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.borrow_mut().remove(key);
            Ok(())
        }
    }

    fn pairing() -> LanPairing<MemoryCredentials> {
        let uuid = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let random_uuid = {
            let uuid = std::sync::Arc::clone(&uuid);
            move || format!("client-{}", uuid.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
        };
        LanPairing::load(
            MemoryCredentials::default(),
            lan_pairing::LanPairingOptions {
                machine_name: Some("Test Machine".to_string()),
                random_uuid: Box::new(random_uuid),
                random_hex: Box::new(|| "a1b2c3".to_string()),
                random_token: Box::new(|| mint_client_token(&[9u8; 32])),
                now_ms: Box::new(|| 1_000),
            },
        )
        .unwrap()
    }

    fn body(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[tokio::test]
    async fn pairs_with_valid_code_and_sets_cookie() {
        let mut pairing = pairing();
        pairing.create_pair_code();
        let http = LanPairHttp::new(pairing);

        let response = http
            .handle_pair(&body(&json!({ "code": "a1b2c3", "label": "phone" })))
            .await;
        assert_eq!(response.status, 200);
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["serverId"], "client-0");
        assert_eq!(value["name"], "Test Machine");
        assert!(value["token"].as_str().is_some_and(|token| !token.is_empty()));
        let cookie = response.set_cookie.expect("cookie");
        assert!(cookie.starts_with("mode_lite_token="));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
    }

    #[tokio::test]
    async fn invalid_code_is_401_without_detail() {
        let mut pairing = pairing();
        pairing.create_pair_code();
        let http = LanPairHttp::new(pairing);
        let response = http.handle_pair(&body(&json!({ "code": "ffffff" }))).await;
        assert_eq!(response.status, 401);
        assert!(response.set_cookie.is_none());
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["error"], "Invalid or expired pair code");
    }

    #[tokio::test]
    async fn invalid_json_is_400() {
        let http = LanPairHttp::new(pairing());
        let response = http.handle_pair(b"{").await;
        assert_eq!(response.status, 400);
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["error"], "Invalid JSON body");
    }

    #[tokio::test]
    async fn attempt_limit_returns_429() {
        let mut pairing = pairing();
        pairing.create_pair_code();
        let http = LanPairHttp::new(pairing).with_attempt_limit(2, Duration::from_secs(60));
        assert_eq!(
            http.handle_pair(&body(&json!({ "code": "ffffff" }))).await.status,
            401
        );
        assert_eq!(
            http.handle_pair(&body(&json!({ "code": "ffffff" }))).await.status,
            401
        );
        assert_eq!(
            http.handle_pair(&body(&json!({ "code": "ffffff" }))).await.status,
            429
        );
    }

    #[tokio::test]
    async fn state_covers_disabled_and_enabled_shapes() {
        let mut pairing = pairing();
        pairing.create_pair_code();
        let http = LanPairHttp::new(pairing);

        let state = http.get_state().await;
        assert_eq!(state["enabled"], false);
        assert!(state["port"].is_null());
        assert_eq!(state["clients"], json!([]));
        assert_eq!(state["activeWebSessions"], 0);
        assert!(state["publicRelayUrl"].is_null());
        assert!(state["pairCode"].is_object());

        http.set_view(LanAccessView {
            enabled: true,
            port: Some(45880),
            addresses: vec!["192.168.1.5".to_string()],
            active_web_sessions: 1,
            public_relay_url: Some("https://relay.example/d/t1".to_string()),
        });
        let state = http.get_state().await;
        assert_eq!(state["enabled"], true);
        assert_eq!(state["port"], 45880);
        assert_eq!(state["addresses"], json!(["192.168.1.5"]));
        assert_eq!(state["activeWebSessions"], 1);
        assert_eq!(state["publicRelayUrl"], "https://relay.example/d/t1");
    }

    #[tokio::test]
    async fn management_actions_require_enabled_listener() {
        let mut pairing = pairing();
        pairing.create_pair_code();
        let http = LanPairHttp::new(pairing);
        assert_eq!(
            http.create_pair_code().await.unwrap_err(),
            "LAN access is not enabled"
        );
        assert_eq!(
            http.remove_client("client-1").await.unwrap_err(),
            "LAN access is not enabled"
        );
        assert_eq!(
            http.reset_tokens().await.unwrap_err(),
            "LAN access is not enabled"
        );
    }

    #[tokio::test]
    async fn remove_and_reset_update_state_clients() {
        let mut pairing = pairing();
        pairing.create_pair_code();
        let http = LanPairHttp::new(pairing);
        http.set_view(LanAccessView {
            enabled: true,
            ..LanAccessView::default()
        });

        let response = http
            .handle_pair(&body(&json!({ "code": "a1b2c3", "label": "phone" })))
            .await;
        assert_eq!(response.status, 200);
        let state = http.get_state().await;
        assert_eq!(state["clients"].as_array().unwrap().len(), 1);
        let client_id = state["clients"][0]["id"].as_str().unwrap().to_string();

        let state = http.remove_client(&client_id).await.unwrap();
        assert_eq!(state["clients"], json!([]));

        let state = http.reset_tokens().await.unwrap();
        assert_eq!(state["clients"], json!([]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serves_pair_over_http_transport() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut pairing = pairing();
        pairing.create_pair_code();
        let state = Arc::new(LanPairHttp::new(pairing));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(serve(listener, Arc::clone(&state)));

        let body = r#"{"code":"a1b2c3","label":"phone"}"#;
        let request = format!(
            "POST {PAIR_PATH} HTTP/1.1
Host: 127.0.0.1
Content-Type: application/json
Content-Length: {}
Connection: close

{}",
            body.len(),
            body
        );
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            response.to_lowercase().contains("set-cookie: mode_lite_token="),
            "{response}"
        );
        task.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn management_endpoints_require_token() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn request(address: std::net::SocketAddr, raw: String) -> String {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            stream.write_all(raw.as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            response
        }

        let mut pairing = pairing();
        pairing.create_pair_code();
        let state = Arc::new(LanPairHttp::new(pairing));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(serve(listener, Arc::clone(&state)));

        let body = r#"{"code":"a1b2c3"}"#;
        let pair = request(
            address,
            format!(
                "POST {PAIR_PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            ),
        )
        .await;
        let payload: Value =
            serde_json::from_str(pair.split("\r\n\r\n").nth(1).unwrap_or("{}")).unwrap();
        let token = payload["token"].as_str().unwrap().to_string();

        let unauthorized = request(
            address,
            "GET /api/lan/state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
                .to_string(),
        )
        .await;
        assert!(unauthorized.starts_with("HTTP/1.1 401"), "{unauthorized}");

        let authorized = request(
            address,
            format!(
                "GET /api/lan/state HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(authorized.starts_with("HTTP/1.1 200"), "{authorized}");
        let state_payload: Value =
            serde_json::from_str(authorized.split("\r\n\r\n").nth(1).unwrap_or("{}")).unwrap();
        assert_eq!(state_payload["clients"].as_array().unwrap().len(), 1);
        task.abort();
    }

    #[test]
    fn presented_token_prefers_query_then_bearer_then_cookie() {
        assert_eq!(
            presented_token(None, None, Some("query-token")).as_deref(),
            Some("query-token")
        );
        assert_eq!(
            presented_token(Some("Bearer bearer-token"), Some("mode_lite_token=cookie"), None)
                .as_deref(),
            Some("bearer-token")
        );
        assert_eq!(
            presented_token(None, Some("mode_lite_token=cookie-token"), None).as_deref(),
            Some("cookie-token")
        );
        assert_eq!(presented_token(None, None, None), None);
    }

    #[test]
    fn legacy_cookie_name_is_accepted() {
        assert_eq!(
            presented_token(None, Some("zcode_lite_token=legacy"), None).as_deref(),
            Some("legacy")
        );
    }

    #[test]
    fn client_record_serializes_camel_case() {
        let record = LanAccessClientRecord {
            id: "c1".to_string(),
            label: "phone".to_string(),
            created_at: 1,
            last_used_at: None,
            last_workspace_path: None,
            remote_desktop_authorized: None,
        };
        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["createdAt"], 1);
        assert!(value.get("remoteDesktopAuthorized").is_none());
    }
}
