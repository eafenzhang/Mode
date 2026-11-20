//! LAN 管理路由桥接（LT-15 P2 首切）。
//!
//! 状态/令牌/限速/配对的唯一所有者仍是 `crates/lan-http`（`LanPairHttp`）；
//! 本模块只把它的传输无关管理面挂到 axum，错误码与鉴权口径不复制。
//! 六个端点全部桥接：state / pair（Set-Cookie 配对）/ enable / pair-code / reset /
//! clients 删除；鉴权、限速与错误码沿用 `lan-http`，不复制。

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use lan_http::LanPairHttp;
use serde_json::Value;

/// axum 侧的凭据端口对象安全包装：宿主装配时注入具体实现（未来接 credential-store）。
pub trait LanCredentialPort: lan_pairing::LanCredentialPort + Send + Sync + 'static {}
impl<T: lan_pairing::LanCredentialPort + Send + Sync + 'static> LanCredentialPort for T {}

// Box<dyn …> 自身也要满足 lan_pairing::LanCredentialPort（转发到底层对象），
// 这样 `LanPairHttp<Box<dyn LanCredentialPort>>` 才能作为共享状态。
// （Box<T> blanket 对 foreign type 不合法——E0210——逐具体类型实现。）
impl lan_pairing::LanCredentialPort for Box<dyn LanCredentialPort> {
    fn load(&self, key: &str) -> Result<Option<String>, String> {
        (**self).load(key)
    }
    fn save(&self, key: &str, value: &str) -> Result<(), String> {
        (**self).save(key, value)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        (**self).delete(key)
    }
}

pub type SharedLanHttp = Arc<LanPairHttp<Box<dyn LanCredentialPort>>>;

/// credential-store 适配：LAN 配对凭据落 `{data_root}/v2/credentials.json`
/// （与桌面壳 `CredentialChannel` 同一文件与加密口径）。
pub struct StoreLanCredentials(pub credential_store::CredentialStore);

impl lan_pairing::LanCredentialPort for StoreLanCredentials {
    fn load(&self, key: &str) -> Result<Option<String>, String> {
        self.0.load(key).map_err(|error| error.to_string())
    }

    fn save(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.save(key, value).map_err(|error| error.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.delete(key).map_err(|error| error.to_string())
    }
}

fn now_wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 生产随机源（TS `randomUUID`/`randomBytes(3).toString("hex")`/`randomBytes(32)
/// .toString("base64url")` 的对应）：workspace 无 rand/getrandom，统一取
/// uuid v4 的字节（122bit/次）。
fn production_pairing_options() -> lan_pairing::LanPairingOptions {
    lan_pairing::LanPairingOptions {
        machine_name: None,
        random_uuid: Box::new(|| uuid::Uuid::new_v4().to_string()),
        random_hex: Box::new(|| {
            let bytes = uuid::Uuid::new_v4().into_bytes();
            bytes[..3]
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>()
        }),
        random_token: Box::new(|| {
            // TS randomBytes(32)：两个 uuid v4 拼 32 字节（256bit 熵对齐）。
            let mut bytes = [0u8; 32];
            let (first, second) = (uuid::Uuid::new_v4().into_bytes(), uuid::Uuid::new_v4().into_bytes());
            bytes[..16].copy_from_slice(&first);
            bytes[16..].copy_from_slice(&second);
            lan_pairing::mint_client_token(&bytes)
        }),
        now_ms: Box::new(now_wall_ms),
    }
}

/// server-mode 生产装配：`{data_root}` 的 credential-store 持久化 LAN 配对凭据。
///
/// `machine_name` 缺省由 pairing 层按 host 解析；坏凭据文件走 `on_corrupt`
/// 告警（server-mode 无壳日志，留 None 或注入）。
pub fn lan_pairing_from_store(
    data_root: &std::path::Path,
    on_corrupt: Option<credential_store::CorruptListener>,
) -> Result<SharedLanHttp, String> {
    let mut store = credential_store::CredentialStore::new(data_root);
    if let Some(on_corrupt) = on_corrupt {
        store = store.with_corrupt_listener(on_corrupt);
    }
    let credentials: Box<dyn LanCredentialPort> = Box::new(StoreLanCredentials(store));
    let pairing = lan_pairing::LanPairing::load(credentials, production_pairing_options())
        .map_err(|error| error.to_string())?;
    Ok(Arc::new(LanPairHttp::new(pairing)))
}

/// LAN 管理面四个端点；鉴权（Bearer/Cookie/query token）与错误码沿用 `lan-http`。
pub fn lan_router(pairing: SharedLanHttp) -> Router {
    Router::new()
        .route("/api/lan/state", get(lan_state))
        .route("/api/lan/pair", post(lan_pair))
        .route("/api/lan/enable", post(lan_enable))
        .route("/api/lan/pair-code", post(lan_pair_code))
        .route("/api/lan/reset", post(lan_reset))
        .route("/api/lan/clients/{client_id}", delete(lan_remove_client))
        .with_state(pairing)
}

async fn lan_state(
    State(state): State<SharedLanHttp>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    respond(&state, "GET", "/api/lan/state", &headers, &query).await
}

/// 配对端点：限速、一次性码、成功下发 HttpOnly Cookie 的语义全部沿用 lan-http。
async fn lan_pair(State(state): State<SharedLanHttp>, body: Bytes) -> Response {
    let response = state.handle_pair(&body).await;
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    let mut builder = (status, Json(value)).into_response();
    if let Some(cookie) = response.set_cookie {
        if let Ok(header_value) = HeaderValue::from_str(&cookie) {
            builder.headers_mut().insert(SET_COOKIE, header_value);
        }
    }
    builder
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LanEnableRequest {
    enabled: bool,
}

/// 启用开关：持久化属宿主凭据面（待接 credential-store），这里只更新监听视图。
async fn lan_enable(
    State(state): State<SharedLanHttp>,
    Json(request): Json<LanEnableRequest>,
) -> (StatusCode, Json<Value>) {
    let state_value = state.set_enabled(request.enabled).await;
    (StatusCode::OK, Json(state_value))
}

async fn lan_pair_code(
    State(state): State<SharedLanHttp>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    respond(&state, "POST", "/api/lan/pair-code", &headers, &query).await
}

async fn lan_reset(
    State(state): State<SharedLanHttp>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    respond(&state, "POST", "/api/lan/reset", &headers, &query).await
}

async fn lan_remove_client(
    State(state): State<SharedLanHttp>,
    Path(client_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    respond(
        &state,
        "DELETE",
        &format!("/api/lan/clients/{client_id}"),
        &headers,
        &query,
    )
    .await
}

async fn respond(
    state: &SharedLanHttp,
    method: &str,
    path: &str,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> (StatusCode, Json<Value>) {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let cookie = headers.get(COOKIE).and_then(|value| value.to_str().ok());
    let token = query.get("token").cloned();
    let (status, body) = state
        .management_request(method, path, authorization, cookie, token.as_deref())
        .await;
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(body),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use lan_http::LanAccessView;
    use lan_pairing::LanPairingOptions;
    use serde_json::json;
    use tower::ServiceExt;

    #[derive(Default)]
    struct MemoryCredentials(std::sync::Mutex<HashMap<String, String>>);

    impl lan_pairing::LanCredentialPort for MemoryCredentials {
        fn load(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().expect("memory credentials").get(key).cloned())
        }
        fn save(&self, key: &str, value: &str) -> Result<(), String> {
            self.0
                .lock()
                .expect("memory credentials")
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.lock().expect("memory credentials").remove(key);
            Ok(())
        }
    }

    #[test]
    fn store_credentials_round_trip_via_pairing_flow() {
        let dir = std::env::temp_dir().join(format!(
            "lan-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        // 固定密钥（不依赖进程 env 差异）+ 真实加密落盘。
        let store = credential_store::CredentialStore::with_secret(&dir, "test-secret");
        let mut pairing = lan_pairing::LanPairing::load(
            Box::new(StoreLanCredentials(store)) as Box<dyn LanCredentialPort>,
            options(),
        )
        .expect("pairing");
        // 配对码生成走 save（写凭据文件）；重启后（重新 load）客户端记录仍在。
        let code = pairing.create_pair_code();
        // 测试 options 的 random_uuid 固定 "uuid-1"（客户端记录 id）；redeem 收码文本。
        let first_client = pairing
            .redeem(&code.code, Some("client-a"))
            .expect("redeem")
            .expect("paired");
        assert!(!first_client.token.is_empty());
        let store = credential_store::CredentialStore::with_secret(&dir, "test-secret");
        let pairing = lan_pairing::LanPairing::load(
            Box::new(StoreLanCredentials(store)) as Box<dyn LanCredentialPort>,
            options(),
        )
        .expect("reload pairing");
        assert!(
            pairing.clients().iter().any(|client| client.id == "uuid-1"),
            "凭据落盘后重启客户端记录应保留"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn production_options_derive_uuid_sources() {
        let options = production_pairing_options();
        let uuid = (options.random_uuid)();
        assert_eq!(uuid.len(), 36, "TS randomUUID 形状");
        let hex = (options.random_hex)();
        assert_eq!(hex.len(), 6, "3 字节 hex 配对码");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        let token = (options.random_token)();
        assert_eq!(token.len(), 43, "32 字节 base64url");
        assert!((options.now_ms)() > 0);
    }

    fn options() -> LanPairingOptions {
        LanPairingOptions {
            machine_name: Some("test-machine".to_string()),
            random_uuid: Box::new(|| "uuid-1".to_string()),
            random_hex: Box::new(|| "a1b2c3".to_string()),
            random_token: Box::new(|| "token-1".to_string()),
            now_ms: Box::new(|| 1_000),
        }
    }

    /// 建好 http 实例并预配一个客户端（先造配对码再走 handle_pair 兑换）。
    async fn lan_http_with_client() -> (SharedLanHttp, String) {
        let mut pairing = lan_pairing::LanPairing::load(
            Box::new(MemoryCredentials::default()) as Box<dyn LanCredentialPort>,
            options(),
        )
        .expect("pairing");
        let code = pairing.create_pair_code();
        let http = LanPairHttp::new(pairing);
        http.set_view(LanAccessView {
            enabled: true,
            port: Some(1234),
            addresses: vec!["192.168.1.5".to_string()],
            active_web_sessions: 0,
            public_relay_url: None,
        });
        let pair_response = http
            .handle_pair(
                serde_json::to_string(&serde_json::json!({
                    "code": code.code,
                    "label": "test client",
                }))
                .unwrap()
                .as_bytes(),
            )
            .await;
        assert_eq!(pair_response.status, 200);
        let body: Value = serde_json::from_slice(&pair_response.body).unwrap();
        (Arc::new(http), body["token"].as_str().unwrap().to_string())
    }

    async fn request_json(
        router: Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn state_requires_token_and_reports_enabled_view() {
        let (http, token) = lan_http_with_client().await;
        let router = lan_router(Arc::clone(&http));

        let (status, body) = request_json(router.clone(), "GET", "/api/lan/state", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "Unauthorized");

        let (status, body) = request_json(router, "GET", "/api/lan/state", Some(&token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["enabled"], true);
        assert_eq!(body["port"], 1234);
        assert_eq!(body["addresses"][0], "192.168.1.5");
        assert_eq!(body["clients"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn pair_code_reset_and_client_removal_follow_management_semantics() {
        let (http, token) = lan_http_with_client().await;
        let router = lan_router(Arc::clone(&http));

        let (status, body) =
            request_json(router.clone(), "POST", "/api/lan/pair-code", Some(&token)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["pairCode"].is_object());

        let client_id = body["clients"][0]["id"]
            .as_str()
            .expect("client id")
            .to_string();
        let (status, body) = request_json(
            router.clone(),
            "DELETE",
            &format!("/api/lan/clients/{client_id}"),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["clients"].as_array().map(Vec::len), Some(0));

        // 删除客户端会吊销其令牌：同一令牌再调管理面必须 401（lan-http 语义）。
        let (status, _) = request_json(router, "POST", "/api/lan/reset", Some(&token)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pair_endpoint_issues_token_and_enable_toggles_view() {
        let (http, first_token) = lan_http_with_client().await;
        let router = lan_router(Arc::clone(&http));

        // 先造配对码（管理面需要已配对客户端令牌）。
        let (status, body) = request_json(
            router.clone(),
            "POST",
            "/api/lan/pair-code",
            Some(&first_token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = body["pairCode"]["code"]
            .as_str()
            .expect("pair code")
            .to_string();

        // 用码配对：200 + 令牌 + HttpOnly Set-Cookie。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/lan/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({ "code": code, "label": "second" })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert!(set_cookie
            .as_deref()
            .is_some_and(|cookie| cookie.starts_with("mode_lite_token=")));
        let pair_body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let second_token = pair_body["token"].as_str().expect("token").to_string();

        // 新令牌可直接调管理面。
        let (status, state_body) =
            request_json(router.clone(), "GET", "/api/lan/state", Some(&second_token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state_body["clients"].as_array().map(Vec::len), Some(2));

        // enable false：关闭监听、清配对码；enable true 恢复开关。
        for enabled in [false, true] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/lan/enable")
                        .header("content-type", "application/json")
                        .body(Body::from(json!({ "enabled": enabled }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(body["enabled"], enabled);
            if !enabled {
                assert!(body["pairCode"].is_null());
            }
        }
    }

    #[tokio::test]
    async fn unknown_route_is_404() {
        // 未注册路径由 axum 兜底 404（空响应体）；lan-http 的 404 只在已注册
        // 路由组内的形状差异中出现。
        let (http, _token) = lan_http_with_client().await;
        let response = lan_router(http)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/lan/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
