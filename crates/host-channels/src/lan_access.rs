//! `lan-access` 频道（LT-16.4a 首切：对端配对 registry 的只读/重命名/删除）。
//!
//! 真源：`packages/services/src/lan-access/` 的 `ILanAccessService`、
//! `packages/desktop/src/host-standalone/lanAccessService.ts`（管理面）与
//! `packages/server/src/remote/lanPairedPeers.ts`（本机「我配对的对端」registry）。
//!
//! 本切只做**不依赖 LAN server supervisor** 的 client 侧 registry：
//! - `listPairedPeers`：凭据 `lan:peer:*:token` 枚举 + `:meta` 归一化（v1→v2 迁移、
//!   公网端点强制 HTTPS、公网优先排序），令牌本体不读不返回；
//! - `removePairedPeer`：token 与 meta 同生命周期、幂等删除；
//! - `renamePairedPeer`：只改本机 meta 展示名（1–64，缺 meta 显式报错）。
//! - `getState`：监听未开启时的只读快照（TS `readDisabledLanAccessState`）——
//!   enabled=false / port=null / addresses=[] / pairCode=null，clients 仍从凭据
//!   读取（配对记录与逐设备授权是持久化事实，监听开关只管「现在能不能连进来」）。
//!
//! server 侧（setEnabled/createPairCode/removeClient/resetTokens/onDidChange）
//! 通过 [`LanAccessHostPort`] 注入：宿主实现 supervisor（监听生命周期 + 状态聚合），
//! 未装配时显式报错，不返回假状态。网络 side（discoverPeers/pairPeer/
//! getPeerWorkspaces）留 16.4a 续。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use credential_store::CredentialStore;
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value as WireValue;

pub const LAN_ACCESS_CHANNEL: &str = "lan-access";
pub const ON_DID_CHANGE: &str = "onDidChange";

/// 本机 LAN server supervisor 的注入端口（宿主实现）。
///
/// 同步签名：频道调用已在 `spawn_blocking` 中执行，宿主可用自己的运行时桥接
/// （bind/serve 是 async，由实现内部收敛）。
pub trait LanAccessHostPort: Send + Sync {
    fn state(&self) -> Result<serde_json::Value, String>;
    fn set_enabled(&self, enabled: bool) -> Result<serde_json::Value, String>;
    fn create_pair_code(&self) -> Result<serde_json::Value, String>;
    fn remove_client(&self, client_id: &str) -> Result<serde_json::Value, String>;
    fn reset_tokens(&self) -> Result<serde_json::Value, String>;
    /// `onDidChange`：注册监听者；实现应立即补发当前状态快照（TS 幂等事实语义）。
    fn subscribe_changes(&self, emit: EventCallback) -> Subscription;
}

const PEER_PREFIX: &str = "lan:peer:";
const TOKEN_SUFFIX: &str = ":token";
const META_SUFFIX: &str = ":meta";
/// 已配对客户端注册表（JSON 数组）；LAN server 与设置页快照共用的持久事实。
const LAN_ACCESS_CLIENTS_CREDENTIAL_KEY: &str = "lan:access:clients";

pub struct LanAccessChannel {
    credentials: Arc<CredentialStore>,
    /// LAN server supervisor；`None` 时管理面方法显式报错。
    host: Option<Arc<dyn LanAccessHostPort>>,
}

impl LanAccessChannel {
    /// 无 supervisor（getState 走 offline 快照；管理面方法显式报错）。
    pub fn new(credentials: Arc<CredentialStore>) -> Self {
        Self {
            credentials,
            host: None,
        }
    }

    /// 装配 supervisor（监听生命周期/配对码/clients 由宿主实现）。
    pub fn with_host(
        credentials: Arc<CredentialStore>,
        host: Arc<dyn LanAccessHostPort>,
    ) -> Self {
        Self {
            credentials,
            host: Some(host),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LanPeerEndpoint {
    scheme: String,
    host: String,
    port: u16,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

/// `lanPeerMetaSchema`：v2（多端点）与 v1（单内网地址，读取时迁移）。
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawLanPeerMeta {
    V2(RawLanPeerMetaV2),
    V1(RawLanPeerMetaV1),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawLanPeerMetaV2 {
    endpoints: Vec<LanPeerEndpoint>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    paired_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLanPeerMetaV1 {
    host: String,
    port: u16,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "pairedAt")]
    paired_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LanPeerMeta {
    endpoints: Vec<LanPeerEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paired_at: Option<i64>,
}

impl LanPeerMeta {
    fn parse(raw: Option<String>) -> Option<Self> {
        let raw = raw?;
        let value: RawLanPeerMeta = serde_json::from_str(&raw).ok()?;
        Self::normalize(value)
    }

    fn normalize(value: RawLanPeerMeta) -> Option<Self> {
        let (endpoints, name, paired_at) = match value {
            RawLanPeerMeta::V2(RawLanPeerMetaV2 {
                endpoints,
                name,
                paired_at,
            }) => (endpoints, name, paired_at),
            RawLanPeerMeta::V1(RawLanPeerMetaV1 {
                host,
                port,
                name,
                paired_at,
            }) => (
                vec![LanPeerEndpoint {
                    scheme: "http".to_string(),
                    host,
                    port,
                    kind: "lan".to_string(),
                    path: None,
                }],
                name,
                paired_at,
            ),
        };
        // 公网端点只允许 HTTPS：凭据被手改成 http 时也不能进入连接路径。
        let mut endpoints: Vec<LanPeerEndpoint> = endpoints
            .into_iter()
            .filter(|endpoint| endpoint.kind != "public" || endpoint.scheme == "https")
            .collect();
        let mut seen = std::collections::HashSet::new();
        endpoints.retain(|endpoint| {
            seen.insert(format!(
                "{}://{}:{}{}",
                endpoint.scheme,
                endpoint.host,
                endpoint.port,
                endpoint.path.as_deref().unwrap_or("")
            ))
        });
        // 公网在前、LAN 在后（稳定排序，同 kind 保持原顺序）。
        endpoints.sort_by_key(|endpoint| u8::from(endpoint.kind != "public"));
        if endpoints.is_empty() {
            return None;
        }
        let name = name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());
        Some(Self {
            endpoints,
            name,
            paired_at,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredLanAccessClientRecord {
    id: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    last_used_at: Option<i64>,
    #[serde(default)]
    last_workspace_path: Option<String>,
    #[serde(default)]
    remote_desktop_authorized: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LanAccessClientRecord {
    id: String,
    label: String,
    created_at: i64,
    last_used_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_workspace_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_desktop_authorized: Option<bool>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// TS `readLanAccessClientRecords`：只认 id 非空的记录；坏 JSON/非数组 = []。
fn read_client_records(credentials: &CredentialStore) -> Result<Vec<LanAccessClientRecord>, String> {
    let Some(raw) = credentials
        .load(LAN_ACCESS_CLIENTS_CREDENTIAL_KEY)
        .map_err(|error| error.to_string())?
    else {
        return Ok(Vec::new());
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(Vec::new());
    };
    let Some(items) = parsed.as_array() else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for item in items {
        let Ok(record) = serde_json::from_value::<StoredLanAccessClientRecord>(item.clone()) else {
            continue;
        };
        if record.id.is_empty() {
            continue;
        }
        records.push(LanAccessClientRecord {
            id: record.id,
            label: record.label.unwrap_or_default(),
            created_at: record.created_at.unwrap_or_else(now_ms),
            last_used_at: record.last_used_at,
            // 只保留非空字符串（TS 同）。
            last_workspace_path: record
                .last_workspace_path
                .filter(|path| !path.is_empty()),
            // 远程桌面逐设备授权：只认显式 true（缺键/旧数据 = 未授权）。
            remote_desktop_authorized: record
                .remote_desktop_authorized
                .filter(|authorized| *authorized),
        });
    }
    Ok(records)
}

/// TS `readDisabledLanAccessState`：监听未开启时的只读快照。
fn disabled_lan_access_state(credentials: &CredentialStore) -> Result<serde_json::Value, String> {
    let clients = read_client_records(credentials)?;
    Ok(serde_json::json!({
        "enabled": false,
        "port": serde_json::Value::Null,
        "addresses": [],
        "pairCode": serde_json::Value::Null,
        "clients": clients,
        "activeWebSessions": 0,
        "publicRelayUrl": serde_json::Value::Null,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairedPeer {
    server_id: String,
    host: String,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    endpoints: Option<Vec<LanPeerEndpoint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paired_at: Option<i64>,
}

fn peer_token_key(server_id: &str) -> String {
    format!("{PEER_PREFIX}{server_id}{TOKEN_SUFFIX}")
}

fn peer_meta_key(server_id: &str) -> String {
    format!("{PEER_PREFIX}{server_id}{META_SUFFIX}")
}

fn list_paired_peers(credentials: &CredentialStore) -> Result<Vec<PairedPeer>, String> {
    let keys = credentials
        .list(PEER_PREFIX)
        .map_err(|error| error.to_string())?;
    let mut peers = Vec::new();
    for key in keys {
        let Some(server_id) = key
            .strip_prefix(PEER_PREFIX)
            .and_then(|rest| rest.strip_suffix(TOKEN_SUFFIX))
        else {
            continue;
        };
        if server_id.is_empty() {
            continue;
        }
        let raw = credentials
            .load(&peer_meta_key(server_id))
            .map_err(|error| error.to_string())?;
        let meta = LanPeerMeta::parse(raw);
        peers.push(match meta {
            Some(meta) => {
                let primary = &meta.endpoints[0];
                PairedPeer {
                    server_id: server_id.to_string(),
                    host: primary.host.clone(),
                    port: primary.port,
                    endpoints: Some(meta.endpoints),
                    name: meta.name,
                    paired_at: meta.paired_at,
                }
            }
            // 旧配对没有 meta 时保留一行（地址未知），至少还能删除。
            None => PairedPeer {
                server_id: server_id.to_string(),
                host: String::new(),
                port: 0,
                endpoints: None,
                name: None,
                paired_at: None,
            },
        });
    }
    // pairedAt 降序；缺失按 0（Rust sort_by 稳定，与 TS 稳定排序同）。
    peers.sort_by(|left, right| {
        right
            .paired_at
            .unwrap_or(0)
            .cmp(&left.paired_at.unwrap_or(0))
    });
    Ok(peers)
}

fn remove_paired_peer(credentials: &CredentialStore, server_id: &str) -> Result<(), String> {
    let server_id = server_id.trim();
    if server_id.is_empty() {
        return Err("缺少 serverId".to_string());
    }
    // 令牌与元数据同生命周期；缺任意一个都不影响幂等删除。
    credentials
        .delete(&peer_token_key(server_id))
        .map_err(|error| error.to_string())?;
    credentials
        .delete(&peer_meta_key(server_id))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn rename_paired_peer(
    credentials: &CredentialStore,
    server_id: &str,
    name: &str,
) -> Result<(), String> {
    let server_id = server_id.trim();
    if server_id.is_empty() {
        return Err("缺少 serverId".to_string());
    }
    let name = name.trim();
    // TS 用 JS `.length`（UTF-16 code units）校验 1–64。
    let name_len = name.encode_utf16().count();
    if name_len == 0 || name_len > 64 {
        return Err("名称长度必须为 1–64 个字符".to_string());
    }
    let raw = credentials
        .load(&peer_meta_key(server_id))
        .map_err(|error| error.to_string())?;
    let Some(mut meta) = LanPeerMeta::parse(raw) else {
        return Err("该配对缺少元数据，无法重命名".to_string());
    };
    // 只改本机元数据里的展示名：令牌/地址不动（下次连接照常）。
    meta.name = Some(name.to_string());
    let text = serde_json::to_string(&meta).map_err(|error| error.to_string())?;
    credentials
        .save(&peer_meta_key(server_id), &text)
        .map_err(|error| error.to_string())
}

fn first_arg_object(
    arg: &WireValue,
) -> Result<serde_json::Map<String, serde_json::Value>, ChannelError> {
    let first = match arg {
        WireValue::Array(items) => items.first().cloned(),
        WireValue::Undefined => None,
        other => Some(other.clone()),
    };
    let Some(first) = first else {
        return Err(ChannelError::error("missing arguments"));
    };
    match first.to_json() {
        serde_json::Value::Object(object) => Ok(object),
        _ => Err(ChannelError::error("arguments must be an object")),
    }
}

fn require_host<'a>(
    channel: &'a LanAccessChannel,
    command: &str,
) -> Result<&'a Arc<dyn LanAccessHostPort>, ChannelError> {
    channel.host.as_ref().ok_or_else(|| {
        ChannelError::error(format!(
            "lan-access channel: {command} requires the LAN server supervisor (16.4a, not assembled)"
        ))
    })
}

fn json_text(value: serde_json::Value) -> Result<WireValue, ChannelError> {
    let text =
        serde_json::to_string(&value).map_err(|error| ChannelError::error(error.to_string()))?;
    Ok(WireValue::Json(text))
}

fn run(
    channel: &LanAccessChannel,
    command: &str,
    arg: &WireValue,
) -> Result<WireValue, ChannelError> {
    match command {
        "listPairedPeers" => {
            let peers = list_paired_peers(&channel.credentials).map_err(ChannelError::error)?;
            let text = serde_json::to_string(&peers)
                .map_err(|error| ChannelError::error(error.to_string()))?;
            Ok(WireValue::Json(text))
        }
        "getState" => {
            match &channel.host {
                Some(host) => json_text(host.state().map_err(ChannelError::error)?),
                // 无 supervisor：监听必然未开启，按 TS readDisabledLanAccessState
                // 输出真实快照（clients 来自凭据）。
                None => {
                    let state = disabled_lan_access_state(&channel.credentials)
                        .map_err(ChannelError::error)?;
                    json_text(state)
                }
            }
        }
        "setEnabled" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                enabled: bool,
            }
            let host = require_host(channel, command)?;
            let first = first_arg_object(arg)?;
            let parsed: P = serde_json::from_value(serde_json::Value::Object(first))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            json_text(
                host.set_enabled(parsed.enabled)
                    .map_err(ChannelError::error)?,
            )
        }
        "createPairCode" => {
            let host = require_host(channel, command)?;
            json_text(host.create_pair_code().map_err(ChannelError::error)?)
        }
        "removeClient" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                client_id: String,
            }
            let host = require_host(channel, command)?;
            let first = first_arg_object(arg)?;
            let parsed: P = serde_json::from_value(serde_json::Value::Object(first))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            json_text(
                host.remove_client(&parsed.client_id)
                    .map_err(ChannelError::error)?,
            )
        }
        "resetTokens" => {
            let host = require_host(channel, command)?;
            json_text(host.reset_tokens().map_err(ChannelError::error)?)
        }
        "removePairedPeer" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                server_id: String,
            }
            let first = first_arg_object(arg)?;
            let parsed: P = serde_json::from_value(serde_json::Value::Object(first))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            remove_paired_peer(&channel.credentials, &parsed.server_id)
                .map_err(ChannelError::error)?;
            // TS `removePairedPeer(): Promise<void>`。
            Ok(WireValue::Undefined)
        }
        "renamePairedPeer" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                server_id: String,
                name: String,
            }
            let first = first_arg_object(arg)?;
            let parsed: P = serde_json::from_value(serde_json::Value::Object(first))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            rename_paired_peer(&channel.credentials, &parsed.server_id, &parsed.name)
                .map_err(ChannelError::error)?;
            Ok(WireValue::Undefined)
        }
        // server supervisor / 网络 side 未装配：显式报错，不返回假状态。
        other => Err(ChannelError::error(format!(
            "lan-access channel: {other} requires the LAN server supervisor (16.4a, not assembled)"
        ))),
    }
}

impl ServerChannel for LanAccessChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: WireValue,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<WireValue, ChannelError>> {
        let credentials = Arc::clone(&self.credentials);
        let host = self.host.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let channel = LanAccessChannel { credentials, host };
                run(&channel, &command, &arg)
            })
            .await
            .map_err(|error| {
                ChannelError::error(format!("lan-access channel task failed: {error}"))
            })?
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: WireValue,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        match event.as_str() {
            ON_DID_CHANGE => match &self.host {
                Some(host) => Ok(host.subscribe_changes(emit)),
                None => Err(ChannelError::error(
                    "lan-access channel: onDidChange requires the LAN server supervisor (16.4a, not assembled)",
                )),
            },
            other => Err(ChannelError::error(format!(
                "lan-access channel: unknown event {other}"
            ))),
        }
    }
}
