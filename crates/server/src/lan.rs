//! LAN 管理路由桥接（LT-15 P2 首切）。
//!
//! 状态/令牌/限速/配对的唯一所有者仍是 `crates/lan-http`（`LanPairHttp`）；
//! 本模块只把它的传输无关管理面挂到 axum，错误码与鉴权口径不复制。
//! `/api/lan/pair`（配对端点，含 Set-Cookie）与 `/api/lan/enable` 仍待迁移。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::routing::{delete, get, post};
use axum::Router;
use lan_http::LanPairHttp;
use serde_json::Value;

/// axum 侧的凭据端口对象安全包装：宿主装配时注入具体实现（未来接 credential-store）。
pub trait LanCredentialPort: lan_pairing::LanCredentialPort + Send + Sync + 'static {}
impl<T: lan_pairing::LanCredentialPort + Send + Sync + 'static> LanCredentialPort for T {}

// Box<dyn …> 自身也要满足 lan_pairing::LanCredentialPort（转发到底层对象），
// 这样 `LanPairHttp<Box<dyn LanCredentialPort>>` 才能作为共享状态。
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

/// LAN 管理面四个端点；鉴权（Bearer/Cookie/query token）与错误码沿用 `lan-http`。
pub fn lan_router(pairing: SharedLanHttp) -> Router {
    Router::new()
        .route("/api/lan/state", get(lan_state))
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
