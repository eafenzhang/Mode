//! 已配对对端的 HTTP 客户端（真源 `packages/server/src/remote/lanRemoteAttach.ts`
//! 的 `pairLanPeer`/`fetchLanPeerInfo` 与 `lanPeerEndpoints.ts` 的端点选择）。
//!
//! - `pairPeer`：一次性配对码 → 对端 `POST /api/lan/pair` 换长期令牌，写凭据；
//!   配对响应里的 `publicRelay` 自动生成公网端点（主控不再手填地址）。
//! - `getPeerWorkspaces`：按端点优先级（公网→LAN）GET `/api/server-info?token=`，
//!   401/403 立即收口，传输失败/404/5xx/协议不符才回退下一个端点。
//!
//! 端点归一化（去重、公网优先、公网强制 HTTPS）复用 `lan_access::LanPeerMeta`。

use std::time::Duration;

use credential_store::CredentialStore;
use serde_json::{json, Value};

use crate::lan_access::{peer_meta_key, peer_token_key, LanPeerEndpoint, LanPeerMeta};

const PAIR_TIMEOUT: Duration = Duration::from_secs(10);
const INFO_TIMEOUT: Duration = Duration::from_secs(10);
const PUBLIC_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const LAN_PROBE_TIMEOUT: Duration = Duration::from_millis(2_500);

pub struct PairPeerParams {
    pub host: String,
    pub port: u16,
    pub code: String,
    pub label: Option<String>,
    pub scheme: Option<String>,
    pub path: Option<String>,
}

fn ensure_tls_provider() {
    // reqwest 以 rustls-no-provider 编译；重复安装 ring provider 安全。
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn normalize_base_path(path: Option<&str>) -> Result<String, String> {
    let trimmed = path.unwrap_or("").trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if !trimmed.starts_with('/')
        || trimmed.contains('?')
        || trimmed.contains('#')
        || trimmed.chars().count() > 512
    {
        return Err("路径前缀无效".to_string());
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

pub(crate) fn build_http_base(host: &str, port: u16, scheme: &str, base_path: &str) -> String {
    format!("{scheme}://{host}:{port}{base_path}")
}

pub(crate) fn request_client(timeout: Duration) -> Result<reqwest::Client, String> {
    ensure_tls_provider();
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())
}

fn local_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Mode".to_string())
}

/// TS `parseLanPublicBaseUrl`：只接受无 userinfo、无 query/hash 的 HTTPS 地址；
/// 允许省略协议与端口（默认 443）；根路径归一为缺省。
fn parse_public_base_url(value: &str) -> Option<LanPeerEndpoint> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") {
        if !trimmed.to_ascii_lowercase().starts_with("https://") {
            return None;
        }
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = reqwest::Url::parse(&candidate).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let path = match url.path() {
        "/" | "" => None,
        path => {
            let trimmed = path.trim_end_matches('/');
            if trimmed.is_empty() || trimmed.chars().count() > 512 {
                return None;
            }
            Some(trimmed.to_string())
        }
    };
    let port = url.port_or_known_default()?;
    if port == 0 {
        return None;
    }
    Some(LanPeerEndpoint {
        scheme: "https".to_string(),
        host: url.host_str()?.to_string(),
        port,
        kind: "public".to_string(),
        path,
    })
}

fn parse_public_relay(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    let base_url = object.get("baseUrl")?.as_str()?.trim().to_string();
    let tunnel_id = object.get("tunnelId")?.as_str()?.to_string();
    if tunnel_id.len() < 16
        || tunnel_id.len() > 128
        || !tunnel_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return None;
    }
    let url = reqwest::Url::parse(&base_url).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    Some((base_url, tunnel_id))
}

/// TS `pairLanPeer`：配对 + 写令牌/元数据；返回 `{token, serverId, name?, publicRelay?}`。
pub async fn pair_peer(
    credentials: &CredentialStore,
    params: PairPeerParams,
) -> Result<Value, String> {
    let scheme = match params.scheme.as_deref() {
        Some("https") => "https",
        _ => "http",
    };
    let host = params.host.trim().to_string();
    if host.is_empty() {
        return Err("缺少对端地址".to_string());
    }
    let base_path = normalize_base_path(params.path.as_deref())?;
    let primary_endpoint = LanPeerEndpoint {
        scheme: scheme.to_string(),
        host: host.clone(),
        port: params.port,
        kind: if scheme == "https" { "public" } else { "lan" }.to_string(),
        path: if base_path.is_empty() {
            None
        } else {
            Some(base_path.clone())
        },
    };
    let url = format!(
        "{}/api/lan/pair",
        build_http_base(&host, params.port, scheme, &base_path)
    );
    let label = params
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .unwrap_or_else(local_hostname);
    let client = request_client(PAIR_TIMEOUT)?;
    let response = client
        .post(&url)
        .json(&json!({
            "code": params.code.trim().to_uppercase(),
            "label": label,
        }))
        .send()
        .await
        .map_err(|error| format!("配对失败：{error}"))?;
    match response.status().as_u16() {
        401 => return Err("配对码无效或已过期，请在对端重新生成".to_string()),
        429 => return Err("配对尝试过于频繁，请稍后再试".to_string()),
        status if !(200..300).contains(&status) => {
            return Err(format!("配对失败（HTTP {status}）"));
        }
        _ => {}
    }
    let payload: Value = response
        .json()
        .await
        .map_err(|error| format!("配对响应不是合法 JSON：{error}"))?;
    let token = payload
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "配对响应缺少令牌".to_string())?
        .to_string();
    let server_id = payload
        .get("serverId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let public_relay = payload
        .get("publicRelay")
        .and_then(parse_public_relay);
    let auto_public_endpoint = public_relay.as_ref().and_then(|(base_url, tunnel_id)| {
        parse_public_base_url(&format!(
            "{}/d/{tunnel_id}",
            base_url.trim_end_matches('/')
        ))
    });
    let mut new_endpoints = vec![primary_endpoint];
    if let Some(public) = auto_public_endpoint {
        new_endpoints.push(public);
    }

    if !server_id.is_empty() {
        credentials
            .save(&peer_token_key(&server_id), &token)
            .map_err(|error| error.to_string())?;
        let existing_raw = credentials
            .load(&peer_meta_key(&server_id))
            .map_err(|error| error.to_string())?;
        let existing_meta = LanPeerMeta::parse(existing_raw);
        let existing_endpoints = existing_meta
            .as_ref()
            .map(|meta| meta.endpoints.clone())
            .unwrap_or_default();
        let new_public_count = new_endpoints
            .iter()
            .filter(|endpoint| endpoint.kind == "public")
            .count();
        // 重复配对合并端点：本次端点优先；旧 LAN 端点保留在后；新配对无公网端点时
        // 保留旧公网端点。
        let mut merged = new_endpoints;
        for endpoint in &existing_endpoints {
            if endpoint.kind == "lan" {
                merged.push(endpoint.clone());
            }
        }
        if new_public_count == 0 {
            for endpoint in &existing_endpoints {
                if endpoint.kind == "public" {
                    merged.push(endpoint.clone());
                }
            }
        }
        let merged_name = name
            .clone()
            .or_else(|| existing_meta.as_ref().and_then(|meta| meta.name.clone()));
        let meta = LanPeerMeta::from_endpoints(merged, merged_name, Some(crate::lan_access::now_ms()))
            .ok_or_else(|| "配对端点归一化失败".to_string())?;
        credentials
            .save(
                &peer_meta_key(&server_id),
                &serde_json::to_string(&meta).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
    }

    let mut result = json!({ "token": token, "serverId": server_id });
    if let Some(name) = name {
        result["name"] = json!(name);
    }
    if let Some((base_url, tunnel_id)) = public_relay {
        result["publicRelay"] = json!({ "baseUrl": base_url, "tunnelId": tunnel_id });
    }
    Ok(result)
}

/// 端点探测错误：`retryable` 决定是否回退下一个端点。
pub(crate) struct ProbeError {
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

pub(crate) fn endpoint_probe_timeout(endpoint: &LanPeerEndpoint, endpoint_count: usize) -> Duration {
    if endpoint_count <= 1 {
        return INFO_TIMEOUT;
    }
    if endpoint.kind == "lan" {
        LAN_PROBE_TIMEOUT
    } else {
        PUBLIC_PROBE_TIMEOUT
    }
}

/// 拉取对端 `server-info` 的**原始**载荷（不做 schema 收窄）。
///
/// 供 `remote-desktop` 的能力位探测复用：那里要读 `capabilities.remoteDesktop*`，
/// 而 [`parse_server_info`] 只保留 workspaces/name 子集。传输/状态码口径与
/// [`probe_endpoint`] 完全一致（同 URL、同超时、同 retryable 判定）。
pub(crate) async fn fetch_server_info_payload(
    endpoint: &LanPeerEndpoint,
    token: &str,
    timeout: Duration,
) -> Result<Value, ProbeError> {
    let base_path = endpoint.path.as_deref().unwrap_or("");
    let base = build_http_base(&endpoint.host, endpoint.port, &endpoint.scheme, base_path);
    let mut url = reqwest::Url::parse(&format!("{base}/api/server-info")).map_err(|error| ProbeError {
        message: format!("对端地址无效：{error}"),
        retryable: true,
    })?;
    url.query_pairs_mut().append_pair("token", token);
    let client = request_client(timeout).map_err(|message| ProbeError {
        message,
        retryable: true,
    })?;
    let response = client.get(url).send().await.map_err(|error| ProbeError {
        message: format!("连接对端失败：{error}"),
        retryable: true,
    })?;
    let status = response.status().as_u16();
    match status {
        401 => {
            return Err(ProbeError {
                message: "对端拒绝了本次访问：令牌无效或已被重置，需要重新配对".to_string(),
                retryable: false,
            })
        }
        403 => {
            return Err(ProbeError {
                message: "对端拒绝了本次访问（HTTP 403）".to_string(),
                retryable: false,
            })
        }
        status if !(200..300).contains(&status) => {
            return Err(ProbeError {
                message: format!("读取对端信息失败（HTTP {status}）"),
                retryable: matches!(status, 404 | 502 | 503 | 504) || status >= 500,
            });
        }
        _ => {}
    }
    response.json().await.map_err(|_| ProbeError {
        message: "对端返回的信息不是合法 JSON".to_string(),
        retryable: true,
    })
}

async fn probe_endpoint(
    endpoint: &LanPeerEndpoint,
    token: &str,
    timeout: Duration,
) -> Result<Value, ProbeError> {
    let payload = fetch_server_info_payload(endpoint, token, timeout).await?;
    parse_server_info(&payload).ok_or_else(|| ProbeError {
        message: "对端返回的信息不符合局域网连接协议，请确认两端 Mode 版本一致".to_string(),
        retryable: true,
    })
}

/// `serverRemoteInfoSchema` 的宿主消费子集：name? + workspaces[{path,label?,identity?}]。
fn parse_server_info(payload: &Value) -> Option<Value> {
    let object = payload.as_object()?;
    if object.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let workspaces = object.get("workspaces")?.as_array()?;
    let mut mapped = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        let workspace = workspace.as_object()?;
        let path = workspace.get("path")?.as_str()?.trim();
        if path.is_empty() {
            return None;
        }
        let mut value = json!({ "path": path });
        if let Some(label) = workspace.get("label").and_then(Value::as_str) {
            value["label"] = json!(label);
        }
        if let Some(identity) = workspace.get("workspaceIdentity").and_then(Value::as_str) {
            value["workspaceIdentity"] = json!(identity);
        }
        mapped.push(value);
    }
    let mut result = json!({ "workspaces": mapped });
    if let Some(name) = object.get("name").and_then(Value::as_str) {
        if !name.is_empty() {
            result["name"] = json!(name);
        }
    }
    Some(result)
}

/// TS `getLanPairedPeerWorkspaces`：按端点优先级探测，首个成功即返回。
pub async fn get_peer_workspaces(
    credentials: &CredentialStore,
    server_id: &str,
) -> Result<Value, String> {
    let server_id = server_id.trim();
    if server_id.is_empty() {
        return Err("缺少 serverId".to_string());
    }
    let raw = credentials
        .load(&peer_meta_key(server_id))
        .map_err(|error| error.to_string())?;
    let meta = LanPeerMeta::parse(raw)
        .ok_or_else(|| "该配对缺少地址信息，无法获取工作区".to_string())?;
    let token = credentials
        .load(&peer_token_key(server_id))
        .map_err(|error| error.to_string())?
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "尚未配对".to_string())?;
    let endpoint_count = meta.endpoints.len();
    let mut last_error = "对端不可达".to_string();
    for endpoint in &meta.endpoints {
        let timeout = endpoint_probe_timeout(endpoint, endpoint_count);
        match probe_endpoint(endpoint, &token, timeout).await {
            Ok(info) => return Ok(info),
            Err(error) => {
                last_error = error.message;
                if !error.retryable {
                    return Err(last_error);
                }
            }
        }
    }
    Err(last_error)
}
