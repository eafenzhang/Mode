//! `lan_peer_client` 契约（LT-16.4a：pairPeer / getPeerWorkspaces）。
//!
//! 真实临时凭据库 + 本地一次性 HTTP 服务（手写 HTTP/1.1），覆盖配对写凭据、
//! 公网端点自动生成、重复配对合并、401/回退与工作区读取。

use std::path::PathBuf;

use credential_store::CredentialStore;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use host_channels::lan_peer_client::{
    get_peer_workspaces, pair_peer, PairPeerParams,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lan-peer-client-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store(dir: &TempDir) -> CredentialStore {
    CredentialStore::with_secret(&dir.0, "lan-peer-client-test-secret")
}

/// 一次性 HTTP 服务：读请求（头 + Content-Length body）后回固定响应。
async fn spawn_http_server(
    status: &'static str,
    body: String,
) -> (u16, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut data = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut content_length: Option<usize> = None;
        let mut header_end: Option<usize> = None;
        loop {
            let read = socket.read(&mut buffer).await.expect("read");
            if read == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..read]);
            if header_end.is_none() {
                if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                    header_end = Some(index + 4);
                    let head = String::from_utf8_lossy(&data[..index]).to_lowercase();
                    content_length = head.lines().find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    });
                }
            }
            if let (Some(end), Some(length)) = (header_end, content_length) {
                if data.len() >= end + length {
                    break;
                }
            } else if header_end.is_some() && content_length.is_none() {
                break;
            }
        }
        let request = String::from_utf8_lossy(&data).into_owned();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
        request
    });
    (port, handle)
}

fn primary_meta(endpoints: Value, name: &str) -> String {
    json!({ "endpoints": endpoints, "name": name, "pairedAt": 1 }).to_string()
}

#[tokio::test]
async fn pair_peer_writes_token_and_merged_endpoints() {
    let dir = TempDir::new("pair");
    let credentials = store(&dir);
    // 旧配对：已有 LAN + 旧公网端点（重复配对应保留旧 LAN 与旧公网）。
    credentials
        .save(
            "lan:peer:peer-1:meta",
            &primary_meta(
                json!([
                    { "scheme": "https", "host": "old.example.com", "port": 443, "kind": "public" },
                    { "scheme": "http", "host": "10.0.0.9", "port": 45880, "kind": "lan" }
                ]),
                "旧名字",
            ),
        )
        .expect("seed meta");

    let (port, server) = spawn_http_server(
        "200 OK",
        json!({
            "token": "tok-1",
            "serverId": "peer-1",
            "name": "书房",
            "publicRelay": { "baseUrl": "https://relay.example.com", "tunnelId": "abcdefghijklmnop" }
        })
        .to_string(),
    )
    .await;

    let result = pair_peer(
        &credentials,
        PairPeerParams {
            host: "127.0.0.1".to_string(),
            port,
            code: " ab12cd ".to_string(),
            label: Some("我的电脑".to_string()),
            scheme: None,
            path: None,
        },
    )
    .await
    .expect("pair");
    assert_eq!(result["token"], "tok-1");
    assert_eq!(result["serverId"], "peer-1");
    assert_eq!(result["name"], "书房");
    assert_eq!(result["publicRelay"]["tunnelId"], "abcdefghijklmnop");

    let request = server.await.expect("server");
    assert!(request.starts_with("POST /api/lan/pair"), "{request}");
    assert!(request.contains("\"code\":\"AB12CD\""), "{request}");
    assert!(request.contains("\"label\":\"我的电脑\""), "{request}");

    assert_eq!(
        credentials.load("lan:peer:peer-1:token").expect("token"),
        Some("tok-1".to_string())
    );
    let meta: Value = serde_json::from_str(
        &credentials
            .load("lan:peer:peer-1:meta")
            .expect("meta")
            .expect("exists"),
    )
    .expect("meta json");
    let endpoints = meta["endpoints"].as_array().expect("endpoints");
    // 新配对带公网端点 → 旧公网被替换（TS 同）；新端点优先，旧 LAN 保留在后。
    assert_eq!(endpoints.len(), 3, "{endpoints:?}");
    assert_eq!(endpoints[0]["kind"], "public");
    assert_eq!(endpoints[0]["host"], "relay.example.com");
    assert_eq!(endpoints[0]["path"], "/d/abcdefghijklmnop");
    assert_eq!(endpoints[1]["kind"], "lan");
    assert_eq!(endpoints[1]["host"], "127.0.0.1");
    assert_eq!(endpoints[2]["host"], "10.0.0.9");
    // 响应带名称 → 覆盖旧展示名。
    assert_eq!(meta["name"], "书房");
}

#[tokio::test]
async fn pair_peer_reports_invalid_code() {
    let dir = TempDir::new("pair401");
    let credentials = store(&dir);
    let (port, server) =
        spawn_http_server("401 Unauthorized", json!({ "error": "bad" }).to_string()).await;
    let error = pair_peer(
        &credentials,
        PairPeerParams {
            host: "127.0.0.1".to_string(),
            port,
            code: "AAAAAA".to_string(),
            label: None,
            scheme: None,
            path: None,
        },
    )
    .await
    .expect_err("401");
    assert!(error.contains("配对码无效或已过期"), "{error}");
    let _ = server.await;
    assert!(credentials
        .load("lan:peer:peer-1:token")
        .expect("token")
        .is_none());
}

#[tokio::test]
async fn get_peer_workspaces_reads_server_info() {
    let dir = TempDir::new("workspaces");
    let credentials = store(&dir);
    let (port, server) = spawn_http_server(
        "200 OK",
        json!({
            "protocolVersion": 1,
            "serverId": "peer-1",
            "name": "书房",
            "workspaces": [
                { "path": "/work/a", "label": "A" },
                { "path": "/work/b", "workspaceIdentity": "remote-1" }
            ]
        })
        .to_string(),
    )
    .await;
    credentials
        .save("lan:peer:peer-1:token", "tok-1")
        .expect("token");
    credentials
        .save(
            "lan:peer:peer-1:meta",
            &primary_meta(
                json!([{ "scheme": "http", "host": "127.0.0.1", "port": port, "kind": "lan" }]),
                "书房",
            ),
        )
        .expect("meta");

    let result = get_peer_workspaces(&credentials, "peer-1")
        .await
        .expect("workspaces");
    assert_eq!(result["name"], "书房");
    let workspaces = result["workspaces"].as_array().expect("workspaces");
    assert_eq!(workspaces.len(), 2);
    assert_eq!(workspaces[0]["path"], "/work/a");
    assert_eq!(workspaces[0]["label"], "A");
    assert_eq!(workspaces[1]["workspaceIdentity"], "remote-1");
    let request = server.await.expect("server");
    assert!(request.starts_with("GET /api/server-info?token=tok-1"), "{request}");
}

#[tokio::test]
async fn get_peer_workspaces_falls_back_to_next_endpoint() {
    let dir = TempDir::new("fallback");
    let credentials = store(&dir);
    // 先占用一个端口再释放，得到“连接被拒”的端点。
    let refused_port = {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let (port, server) = spawn_http_server(
        "200 OK",
        json!({
            "protocolVersion": 1,
            "workspaces": [{ "path": "/work/a" }]
        })
        .to_string(),
    )
    .await;
    credentials
        .save("lan:peer:peer-1:token", "tok-1")
        .expect("token");
    credentials
        .save(
            "lan:peer:peer-1:meta",
            &primary_meta(
                json!([
                    { "scheme": "http", "host": "127.0.0.1", "port": refused_port, "kind": "lan" },
                    { "scheme": "http", "host": "127.0.0.1", "port": port, "kind": "lan" }
                ]),
                "书房",
            ),
        )
        .expect("meta");

    let result = get_peer_workspaces(&credentials, "peer-1")
        .await
        .expect("fallback workspaces");
    assert_eq!(
        result["workspaces"].as_array().expect("workspaces").len(),
        1
    );
    let _ = server.await;
}

#[tokio::test]
async fn get_peer_workspaces_unauthorized_is_not_retried() {
    let dir = TempDir::new("unauthorized");
    let credentials = store(&dir);
    let (port, server) =
        spawn_http_server("401 Unauthorized", json!({ "error": "no" }).to_string()).await;
    credentials
        .save("lan:peer:peer-1:token", "old")
        .expect("token");
    credentials
        .save(
            "lan:peer:peer-1:meta",
            &primary_meta(
                json!([{ "scheme": "http", "host": "127.0.0.1", "port": port, "kind": "lan" }]),
                "书房",
            ),
        )
        .expect("meta");
    let error = get_peer_workspaces(&credentials, "peer-1")
        .await
        .expect_err("401");
    assert!(error.contains("令牌无效或已被重置"), "{error}");
    let _ = server.await;
}
