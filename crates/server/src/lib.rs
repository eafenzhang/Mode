//! Mode Web/远程服务端（LT-9）第一段纵切：axum 路由骨架 + 健康/版本面。
//!
//! 现有 `packages/server` 的 HTTP/stdio/RPC 面还没迁；本 crate 先固定 axum 入口与
//! 基础契约，后续按路由组逐块移植并对拍（spec `docs/specs/server-axum.md`）。

use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

/// 当前纵切只有健康检查与版本；业务路由在后续任务按组接入。
pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/version", get(version))
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn version() -> Json<Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get_json(uri: &str) -> (StatusCode, Value) {
        let response = router()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let (status, body) = get_json("/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn version_returns_package_version() {
        let (status, body) = get_json("/api/version").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn unknown_route_is_404() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/api/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
