//! LAN transport：把 `/ws/host` 的 WS 帧接到 [`rpc_client::ChannelClient`]。
//!
//! 真源：`packages/server/src/remote/lanRemoteAttach.ts` 的 `attachLanRemoteConnection`
//! 与 `lanPeerTransport.ts` 的 URL 构造。规范见
//! `docs/specs/rpc-client-and-lan-transport.md`。
//!
//! 三步（与 TS 同序）：
//! 1. `POST /api/rpc-host-capability` 换一次性 host capability（30s TTL、先删后比）；
//! 2. 建 WS 到 `/ws/host`，带 capability header 与 `mode_lite_token` cookie；
//! 3. 建 [`ChannelClient`] 并把二进制帧双向搬运。
//!
//! **为什么客户端必须在 `await open` 之前构造**：服务端的 `Initialize` 与握手回包
//! 常在同一批到达。等 open 之后再建协议层，那帧会被静默丢弃且不重发，客户端永久停在
//! Uninitialized——所有 RPC 无报错无日志地排队（TS 里表现为目录浏览器卡「加载中…」）。
//! 这里的做法是：先把「帧入口」准备好（`ChannelClient` 与出站队列），再等 open。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{ChannelClient, FrameSink};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// TS `MODE_RPC_HOST_CAPABILITY_HEADER`（`packages/shared/src/channels.ts`）。
pub const MODE_RPC_HOST_CAPABILITY_HEADER: &str = "x-mode-rpc-host-capability";

const CAPABILITY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanAttachParams {
    pub host: String,
    pub port: u16,
    /// 主端点协议；`https` 用于经目录/中继的公网配对。
    pub scheme: Option<String>,
    /// 路径前缀（中继/反代，如 `/d/<tunnelId>`）；缺省根路径。
    pub path: Option<String>,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanAttachError {
    /// 换 capability 失败。
    Capability { message: String },
    /// WS 建连失败。
    Connect { message: String },
    /// 对端没返回 capability（协议不符）。
    MissingCapability,
}

impl std::fmt::Display for LanAttachError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Capability { message } => write!(formatter, "{message}"),
            Self::Connect { message } => write!(formatter, "{message}"),
            Self::MissingCapability => write!(formatter, "对端未返回连接票据"),
        }
    }
}

impl std::error::Error for LanAttachError {}

/// 归一化 base path：空 → 空串；否则必须以 `/` 开头、不以 `/` 结尾。
fn normalize_base_path(path: Option<&str>) -> String {
    let trimmed = path.unwrap_or("").trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let with_leading = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    with_leading.trim_end_matches('/').to_string()
}

fn http_base(params: &LanAttachParams) -> String {
    let scheme = params.scheme.as_deref().unwrap_or("http");
    let base_path = normalize_base_path(params.path.as_deref());
    format!(
        "{scheme}://{}:{}{base_path}",
        params.host.trim(),
        params.port
    )
}

/// TS `buildLanWsUrl`：`ws(s)://host:port<basePath>/ws/host`。
pub fn build_ws_url(params: &LanAttachParams) -> String {
    let scheme = match params.scheme.as_deref() {
        Some("https") => "wss",
        _ => "ws",
    };
    let base_path = normalize_base_path(params.path.as_deref());
    format!(
        "{scheme}://{}:{}{base_path}/ws/host",
        params.host.trim(),
        params.port
    )
}

/// TS `buildLanApiUrl`：在带前缀的 base 上追加 API 路径（保住中继的 `/d/<id>`）。
pub fn build_api_url(params: &LanAttachParams, api_path: &str) -> String {
    let base = http_base(params);
    let relative = api_path.trim_start_matches('/');
    format!("{}/{}", base.trim_end_matches('/'), relative)
}

#[derive(Deserialize)]
struct CapabilityPayload {
    capability: Option<String>,
}

/// 一条已建立的 LAN 连接。
pub struct LanRemoteConnection {
    pub client: Arc<ChannelClient>,
    /// 对端 `server-info`（供调用方取 name/workspaces/capabilities）。
    pub info: serde_json::Value,
    shutdown: Option<tokio::task::JoinHandle<()>>,
}

impl LanRemoteConnection {
    /// 主动断开（关闭 WS，让服务端释放订阅）。
    pub fn dispose(&mut self) {
        self.client.dispose();
        if let Some(handle) = self.shutdown.take() {
            handle.abort();
        }
    }
}

impl Drop for LanRemoteConnection {
    fn drop(&mut self) {
        self.client.dispose();
        if let Some(handle) = self.shutdown.take() {
            handle.abort();
        }
    }
}

fn ensure_tls_provider() {
    // reqwest 以 rustls-no-provider 编译；重复安装 ring provider 安全。
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// 换一次性 host capability（`POST /api/rpc-host-capability`）。
///
/// 令牌走 Cookie 而不是 query：TS 注释说明服务端只从 `?token=` 或
/// `mode_lite_token` Cookie 取，用 Cookie 可以避免令牌进 URL 与访问日志。
pub async fn request_host_capability(params: &LanAttachParams) -> Result<String, LanAttachError> {
    ensure_tls_provider();
    let url = build_api_url(params, "/api/rpc-host-capability");
    let client = reqwest::Client::builder()
        .timeout(CAPABILITY_TIMEOUT)
        .build()
        .map_err(|error| LanAttachError::Capability {
            message: format!("构造 HTTP 客户端失败：{error}"),
        })?;
    let response = client
        .post(&url)
        .header(
            "cookie",
            format!("mode_lite_token={}", urlencode(&params.token)),
        )
        .send()
        .await
        .map_err(|error| LanAttachError::Capability {
            message: format!("申请对端连接票据失败：{error}"),
        })?;
    if !response.status().is_success() {
        return Err(LanAttachError::Capability {
            message: format!(
                "申请对端连接票据失败（HTTP {}）",
                response.status().as_u16()
            ),
        });
    }
    // workspace 的 reqwest 未启用 `json` feature：这里手动解析，避免为取一个字段
    // 把 feature 面撑大。
    let text = response
        .text()
        .await
        .map_err(|_| LanAttachError::Capability {
            message: "对端返回的票据不是合法 JSON".to_string(),
        })?;
    let payload: CapabilityPayload =
        serde_json::from_str(&text).map_err(|_| LanAttachError::Capability {
            message: "对端返回的票据不是合法 JSON".to_string(),
        })?;
    match payload.capability {
        Some(capability) if !capability.is_empty() => Ok(capability),
        _ => Err(LanAttachError::MissingCapability),
    }
}

/// 最小化 URL 编码（Cookie 值只需处理会破坏 header 的字符）。
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 建立 LAN 连接并返回已初始化的客户端。
///
/// `info` 允许由调用方传入（端点探测阶段已经拿过一次，避免重复请求）。
pub async fn attach_lan_remote_connection(
    params: LanAttachParams,
    info: Option<serde_json::Value>,
) -> Result<LanRemoteConnection, LanAttachError> {
    let capability = request_host_capability(&params).await?;
    let ws_url = build_ws_url(&params);
    let mut request = ws_url
        .into_client_request()
        .map_err(|error| LanAttachError::Connect {
            message: format!("对端地址无效：{error}"),
        })?;
    request.headers_mut().insert(
        MODE_RPC_HOST_CAPABILITY_HEADER,
        capability.parse().map_err(|_| LanAttachError::Connect {
            message: "连接票据含非法字符".to_string(),
        })?,
    );
    request.headers_mut().insert(
        "cookie",
        format!("mode_lite_token={}", urlencode(&params.token))
            .parse()
            .map_err(|_| LanAttachError::Connect {
                message: "令牌含非法字符".to_string(),
            })?,
    );

    let (socket, _response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| LanAttachError::Connect {
            message: format!("连接对端通道失败：{error}"),
        })?;

    // **关键顺序**：先把客户端与出站队列准备好，再开始读帧。
    // 服务端的 Initialize 可能已经在 socket 缓冲区里，读循环一启动就要有人接。
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let sink: FrameSink = Arc::new(move |frame: Vec<u8>| {
        let _ = out_tx.send(frame);
    });
    let client = Arc::new(ChannelClient::new(sink));

    let (mut write, mut read) = {
        use futures_util::StreamExt;
        socket.split()
    };

    // 出站：ChannelClient 的帧 → WS。
    let writer = {
        use futures_util::SinkExt;
        tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                if write.send(Message::Binary(frame.into())).await.is_err() {
                    break;
                }
            }
        })
    };

    // 入站：WS → ChannelClient。
    let client_for_reader = Arc::clone(&client);
    let reader = {
        use futures_util::StreamExt;
        tokio::spawn(async move {
            while let Some(message) = read.next().await {
                match message {
                    Ok(Message::Binary(data)) => client_for_reader.handle_frame(&data),
                    Ok(Message::Text(text)) => client_for_reader.handle_frame(text.as_bytes()),
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            // 对端断开：客户端必须 fail-closed，否则在途 RPC 永久挂起。
            client_for_reader.dispose();
        })
    };

    let shutdown = tokio::spawn(async move {
        let _ = tokio::join!(writer, reader);
    });

    Ok(LanRemoteConnection {
        client,
        info: info.unwrap_or(serde_json::Value::Null),
        shutdown: Some(shutdown),
    })
}

/// 供测试注入：不清空的那个 `Mutex` 只用于测试里统计帧数。
pub type FrameLog = Arc<Mutex<Vec<Vec<u8>>>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn params(path: Option<&str>) -> LanAttachParams {
        LanAttachParams {
            host: " 10.0.0.5 ".to_string(),
            port: 45879,
            scheme: None,
            path: path.map(str::to_string),
            token: "tok/en+=".to_string(),
        }
    }

    #[test]
    fn ws_url_follows_the_ts_shape() {
        assert_eq!(build_ws_url(&params(None)), "ws://10.0.0.5:45879/ws/host");
        assert_eq!(
            build_ws_url(&params(Some("/d/tunnel-1/"))),
            "ws://10.0.0.5:45879/d/tunnel-1/ws/host",
            "中继前缀必须保留，且尾斜杠不能叠加"
        );
        let mut https = params(Some("relay"));
        https.scheme = Some("https".to_string());
        assert_eq!(
            build_ws_url(&https),
            "wss://10.0.0.5:45879/relay/ws/host",
            "https 端点必须走 wss"
        );
    }

    #[test]
    fn api_url_keeps_the_base_path_prefix() {
        // 与 TS 同：`new URL("/api/...", base)` 会丢掉前缀，所以用相对拼接。
        assert_eq!(
            build_api_url(&params(Some("/d/tunnel-1")), "/api/rpc-host-capability"),
            "http://10.0.0.5:45879/d/tunnel-1/api/rpc-host-capability"
        );
        assert_eq!(
            build_api_url(&params(None), "api/server-info"),
            "http://10.0.0.5:45879/api/server-info"
        );
    }

    #[test]
    fn cookie_encoding_protects_header_integrity() {
        assert_eq!(urlencode("tok/en+="), "tok%2Fen%2B%3D");
        assert_eq!(urlencode("abc-123_X.Y~"), "abc-123_X.Y~");
    }
}
