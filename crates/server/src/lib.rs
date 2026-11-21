//! Mode Web/远程服务端（LT-9）第一段纵切：axum 路由骨架 + 健康/版本/server-info。
//!
//! 现有 `packages/server` 的 HTTP/stdio/RPC 面还没迁；本 crate 先固定 axum 入口与
//! 基础契约，后续按路由组逐块移植并对拍（spec `docs/specs/server-axum.md`）。

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

pub mod lan;
pub mod lan_host;
pub mod server_info;
pub mod ws_host;

pub use ws_host::extract_workspace_path_from_frame;

use server_info::{ServerInfoConfig, ServerRemoteInfo};

/// 当前纵切：健康、版本、server-info；业务路由在后续任务按组接入。
pub fn router(config: ServerInfoConfig) -> Router {
    let state = Arc::new(config);
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/version", get(version))
        .route("/api/server-info", get(server_info))
        .with_state(state)
}

pub fn router_from_env() -> Router {
    router(ServerInfoConfig::from_env())
}

/// base 路由 + LAN 管理面（`lan-http` 桥接）；装配方注入具体凭据端口。
pub fn router_with_lan(config: ServerInfoConfig, pairing: lan::SharedLanHttp) -> Router {
    router(config).merge(lan::lan_router(pairing))
}

/// base + LAN + `/ws/host`（trusted-host WS RPC 帧桥）。
///
/// capability store 进程内单例；频道装配用 [`host_channels::register_service_channels`]
/// （server-mode 无 Agent 控制面，agent 频道不可构造留痕跳过）；
/// `lan_pairing` 提供时挂「最近打开目录」录制。
pub fn router_with_ws_host(
    config: ServerInfoConfig,
    data_root: std::path::PathBuf,
    lan_pairing: Option<lan::SharedLanHttp>,
) -> Router {
    let deps = ws_host::WsHostDeps {
        host_capabilities: Arc::new(ws_host::HostCapabilityStore::new()),
        agent_control: None,
        data_root,
        lan_pairing,
    };
    router(config).merge(ws_host::ws_host_router(deps))
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn version(State(config): State<Arc<ServerInfoConfig>>) -> Json<Value> {
    Json(json!({ "version": config.version }))
}

async fn server_info(State(config): State<Arc<ServerInfoConfig>>) -> Json<ServerRemoteInfo> {
    Json(config.projection())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use server_info::{ServerInfoConfig, ServerRemoteWorkspaceInfo};
    use tower::ServiceExt;

    fn config() -> ServerInfoConfig {
        ServerInfoConfig {
            server_id: "srv-1".to_string(),
            name: Some("开发机".to_string()),
            version: "9.9.9".to_string(),
            auth_required: true,
            workspaces: vec![ServerRemoteWorkspaceInfo {
                path: "/workspace/mode".to_string(),
                label: Some("mode".to_string()),
                workspace_identity: None,
            }],
            public_relay: None,
            computer_use: None,
            remote_desktop: Some(true),
            remote_desktop_native: None,
            lan_peer_controller: Some(true),
        }
    }

    async fn get_json(uri: &str) -> (StatusCode, Value) {
        let response = router(config())
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
    async fn version_returns_configured_version() {
        let (status, body) = get_json("/api/version").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "version": "9.9.9" }));
    }

    #[tokio::test]
    async fn server_info_matches_shared_schema() {
        let (status, body) = get_json("/api/server-info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["serverId"], "srv-1");
        assert_eq!(body["name"], "开发机");
        assert_eq!(body["version"], "9.9.9");
        assert_eq!(body["protocolVersion"], 1);
        assert_eq!(body["authRequired"], true);
        assert_eq!(body["workspaces"][0]["path"], "/workspace/mode");
        assert_eq!(body["workspaces"][0]["label"], "mode");
        assert_eq!(body["capabilities"]["desktopContinuous"], true);
        assert_eq!(body["capabilities"]["websocketRpc"], true);
        assert_eq!(body["capabilities"]["remoteDesktop"], true);
        assert_eq!(body["capabilities"]["lanPeerController"], true);
        assert!(body["capabilities"].get("computerUse").is_none());
        assert!(body.get("publicRelay").is_none());
    }

    #[tokio::test]
    async fn unknown_route_is_404() {
        let response = router(config())
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
