//! `remote-desktop` 频道（LT-16.4b 首切）：被控设置面 + 逐设备授权 + 编码能力 +
//! 对端能力探测。
//!
//! 规范见 `docs/specs/remote-desktop-channel.md`。真源：
//! `packages/services/src/remote-desktop/remoteDesktopService.ts`（15 个成员）与
//! `packages/desktop/src/host/remoteDesktopServer.ts`（被控 server 的唯一写者）。
//!
//! 本切只搬**不依赖媒体/信令链路**的部分；会话面（startSession/sendSignal/
//! endSession/respondConsent/setLocalDisplays 与两个动态事件）经 [`RemoteDesktopPort`]
//! 注入，未装配时显式报错，不返回假成功。
//!
//! 三条状态事实：
//! - 被控总开关 = 凭据键 `remote-desktop:server-enabled`（值 `"1"` = 开）；
//! - 逐设备授权 = `lan:access:clients` 记录的 `remoteDesktopAuthorized` 字段（与同意窗
//!   「记住此设备」同源，不另建授权表）；
//! - 编码能力 = `{data_root}/v2/rd-encoder-capability.json` 的 `capability`
//!   （只由真实会话观测写入；这里只读 + 严格白名单）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use credential_store::CredentialStore;
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value as WireValue;
use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::lan_access::{peer_meta_key, peer_token_key, LanAccessClientRecord, LanPeerMeta};
use crate::lan_peer_client::{
    build_http_base, endpoint_probe_timeout, fetch_server_info_payload, request_client, ProbeError,
};

pub const REMOTE_DESKTOP_CHANNEL: &str = "remote-desktop";

pub const ON_DID_CHANGE_CONTROLLED_STATE: &str = "onDidChangeControlledState";
pub const ON_CONSENT_REQUEST: &str = "onConsentRequest";
pub const ON_DYNAMIC_SESSION_EVENT: &str = "onDynamicSessionEvent";
pub const ON_DID_CHANGE_SERVER_STATE: &str = "onDidChangeServerState";

/// 被控总开关的凭据键（TS 同值）。
const REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY: &str = "remote-desktop:server-enabled";
/// 编码能力缓存文件名（落在 app config 目录；TS 同值）。
const ENCODER_CAPABILITY_FILE: &str = "rd-encoder-capability.json";

/// 会话面端口（Host 进程内的被控 server + 主控 controller）。
///
/// 首切不搬会话链路：媒体 WS、信令、同意窗都在端口实现里。未注入时：
/// - `getControlledState` 返回 `{controlled:false}`（本机没有活跃被控会话）；
/// - 其余会话方法显式报错。
pub trait RemoteDesktopPort: Send + Sync {
    fn controlled_state(&self) -> bool;
    /// 关开关时的会话终止（TS 在 server 内同步 closeActiveSocket）。
    fn stop_sessions(&self, reason: &str);
    /// `onDidChangeControlledState`：幂等事实，实现应立即补发当前值。
    fn subscribe_controlled_state(&self, emit: EventCallback) -> Subscription;
    fn start_session(&self, params: Json) -> Result<Json, String>;
    fn send_signal(&self, params: Json) -> Result<(), String>;
    fn end_session(&self, params: Json) -> Result<(), String>;
    fn respond_consent(&self, params: Json) -> Result<(), String>;
    fn set_local_displays(&self, params: Json) -> Result<(), String>;
}

pub struct RemoteDesktopChannel {
    data_root: PathBuf,
    credentials: Arc<CredentialStore>,
    port: Option<Arc<dyn RemoteDesktopPort>>,
}

impl RemoteDesktopChannel {
    pub fn new(data_root: PathBuf, credentials: Arc<CredentialStore>) -> Self {
        Self {
            data_root,
            credentials,
            port: None,
        }
    }

    pub fn with_port(
        data_root: PathBuf,
        credentials: Arc<CredentialStore>,
        port: Arc<dyn RemoteDesktopPort>,
    ) -> Self {
        Self {
            data_root,
            credentials,
            port: Some(port),
        }
    }

    /// `{data_root}/v2/rd-encoder-capability.json`（与 `getAppConfigDir()` 同口径）。
    fn encoder_capability_path(&self) -> PathBuf {
        self.data_root.join("v2").join(ENCODER_CAPABILITY_FILE)
    }
}

impl ServerChannel for RemoteDesktopChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: WireValue,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<WireValue, ChannelError>> {
        let channel = RemoteDesktopChannel {
            data_root: self.data_root.clone(),
            credentials: Arc::clone(&self.credentials),
            port: self.port.clone(),
        };
        Box::pin(async move {
            match command.as_str() {
                // 网络 side：探对端 server-info（reqwest，失败归 offline 不抛）。
                "getPeerCapability" => {
                    let parsed: PeerParams = parse_args(&arg, "getPeerCapability")?;
                    json_text(probe_peer_capability(&channel.credentials, &parsed.server_id).await)
                }
                // 同步面：凭据 / 文件 / 端口调用（spawn_blocking 防阻塞 runtime）。
                _ => tokio::task::spawn_blocking(move || run(&channel, &command, &arg))
                    .await
                    .map_err(|error| {
                        ChannelError::error(format!("remote-desktop channel task failed: {error}"))
                    })?,
            }
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
            ON_DID_CHANGE_CONTROLLED_STATE => match &self.port {
                Some(port) => Ok(port.subscribe_controlled_state(emit)),
                // 无端口：被控态恒 false，登记一个即补发当前值的订阅（幂等事实）。
                None => {
                    emit(WireValue::Json(json!({ "controlled": false }).to_string()));
                    Ok(Box::new(|| {}))
                }
            },
            // 会话事件（同意请求 / 主控会话事件）：端口未装配时只登记、永不触发。
            ON_CONSENT_REQUEST | ON_DYNAMIC_SESSION_EVENT => Ok(Box::new(|| {})),
            ON_DID_CHANGE_SERVER_STATE => Err(ChannelError::error(
                "remote-desktop channel: onDidChangeServerState is not emitted by the host yet",
            )),
            other => Err(ChannelError::error(format!(
                "remote-desktop channel: unknown event {other}"
            ))),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerParams {
    server_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientParams {
    client_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetServerEnabledParams {
    enabled: bool,
}

fn run(
    channel: &RemoteDesktopChannel,
    command: &str,
    arg: &WireValue,
) -> Result<WireValue, ChannelError> {
    match command {
        "getServerState" | "refreshServerState" => {
            json_text(server_state(channel).map_err(ChannelError::error)?)
        }
        "setServerEnabled" => {
            let parsed: SetServerEnabledParams = parse_args(arg, command)?;
            persist_server_enabled(&channel.credentials, parsed.enabled)
                .map_err(ChannelError::error)?;
            if !parsed.enabled {
                // 关开关 = 本地结束（spec 事件顺序 ⑥）。端口未装配时只落盘并留痕，
                // 不假装已经终止了不存在的会话。
                if let Some(port) = &channel.port {
                    port.stop_sessions("server disabled");
                }
            }
            json_text(server_state(channel).map_err(ChannelError::error)?)
        }
        "authorizeClient" => {
            let parsed: ClientParams = parse_args(arg, command)?;
            set_authorized(&channel.credentials, &parsed.client_id, true)
                .map_err(ChannelError::error)?;
            json_text(server_state(channel).map_err(ChannelError::error)?)
        }
        "revokeClient" => {
            let parsed: ClientParams = parse_args(arg, command)?;
            set_authorized(&channel.credentials, &parsed.client_id, false)
                .map_err(ChannelError::error)?;
            json_text(server_state(channel).map_err(ChannelError::error)?)
        }
        "getControlledState" => {
            let controlled = channel
                .port
                .as_ref()
                .map(|port| port.controlled_state())
                .unwrap_or(false);
            json_text(json!({ "controlled": controlled }))
        }
        "startSession" | "sendSignal" | "endSession" | "respondConsent" | "setLocalDisplays" => {
            let port = channel.port.as_ref().ok_or_else(|| {
                ChannelError::error(format!(
                    "remote-desktop channel: {command} requires the host session port (not assembled)"
                ))
            })?;
            let payload = first_arg_object(arg).map_err(|_| {
                ChannelError::error(format!(
                    "remote-desktop channel: {command} requires arguments"
                ))
            })?;
            let value = Json::Object(payload);
            match command {
                "startSession" => {
                    json_text(port.start_session(value).map_err(ChannelError::error)?)
                }
                "sendSignal" => {
                    port.send_signal(value).map_err(ChannelError::error)?;
                    Ok(WireValue::Undefined)
                }
                "endSession" => {
                    port.end_session(value).map_err(ChannelError::error)?;
                    Ok(WireValue::Undefined)
                }
                "respondConsent" => {
                    port.respond_consent(value).map_err(ChannelError::error)?;
                    Ok(WireValue::Undefined)
                }
                _ => {
                    port.set_local_displays(value)
                        .map_err(ChannelError::error)?;
                    Ok(WireValue::Undefined)
                }
            }
        }
        other => Err(ChannelError::error(format!(
            "remote-desktop channel: unknown command {other}"
        ))),
    }
}

/// TS `RemoteDesktopServerState`：总开关 + 已授权设备 + 编码能力。
fn server_state(channel: &RemoteDesktopChannel) -> Result<Json, String> {
    let enabled = load_server_enabled(&channel.credentials)?;
    let authorized: Vec<Json> = read_authorized_clients(&channel.credentials)?
        .into_iter()
        .filter(|record| record.remote_desktop_authorized == Some(true))
        .map(|record| json!({ "id": record.id, "label": record.label }))
        .collect();
    Ok(json!({
        "enabled": enabled,
        "authorizedClients": authorized,
        "encoderCapability": load_encoder_capability(&channel.encoder_capability_path()),
    }))
}

fn load_server_enabled(credentials: &CredentialStore) -> Result<bool, String> {
    Ok(credentials
        .load(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY)
        .map_err(|error| error.to_string())?
        .as_deref()
        .map(str::trim)
        == Some("1"))
}

/// TS `persistEnabled`：开 = 写 `"1"`；关 = **删键**（不是写 "0"）。
fn persist_server_enabled(credentials: &CredentialStore, enabled: bool) -> Result<(), String> {
    if enabled {
        credentials
            .save(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY, "1")
            .map_err(|error| error.to_string())
    } else {
        credentials
            .delete(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY)
            .map_err(|error| error.to_string())
    }
}

/// TS `setAuthorized`：授权 = 置 true；撤销 = **删字段**（不是置 false）。
///
/// 未知 `clientId` 幂等无副作用（TS 同：找不到目标就 return）。
fn set_authorized(
    credentials: &CredentialStore,
    client_id: &str,
    authorized: bool,
) -> Result<(), String> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        return Err("clientId 不能为空".to_string());
    }
    let mut records = read_client_records(credentials)?;
    let Some(target) = records.iter_mut().find(|record| record.id == client_id) else {
        return Ok(());
    };
    if authorized {
        target.remote_desktop_authorized = Some(true);
    } else {
        target.remote_desktop_authorized = None;
    }
    write_client_records(credentials, &records)
}

fn read_authorized_clients(
    credentials: &CredentialStore,
) -> Result<Vec<LanAccessClientRecord>, String> {
    read_client_records(credentials)
}

fn parse_args<T: serde::de::DeserializeOwned>(
    arg: &WireValue,
    command: &str,
) -> Result<T, ChannelError> {
    let object = first_arg_object(arg)?;
    serde_json::from_value(Json::Object(object))
        .map_err(|error| ChannelError::error(format!("invalid {command} arguments: {error}")))
}

fn first_arg_object(arg: &WireValue) -> Result<serde_json::Map<String, Json>, ChannelError> {
    match arg.to_json() {
        Json::Array(items) => match items.into_iter().next() {
            Some(Json::Object(object)) => Ok(object),
            Some(other) => Err(ChannelError::error(format!(
                "remote-desktop channel: arguments must be an object, got {other}"
            ))),
            None => Ok(serde_json::Map::new()),
        },
        Json::Object(object) => Ok(object),
        Json::Null => Ok(serde_json::Map::new()),
        other => Err(ChannelError::error(format!(
            "remote-desktop channel: arguments must be an object, got {other}"
        ))),
    }
}

fn json_text(value: Json) -> Result<WireValue, ChannelError> {
    serde_json::to_string(&value)
        .map(WireValue::Json)
        .map_err(|error| ChannelError::error(error.to_string()))
}

/// TS `sanitizeStoredEncoderCapability` + `loadPersistedEncoderCapability`：
/// 缺失/损坏/未知枚举值一律 `unknown`（下次真实会话观测后写回）。
fn load_encoder_capability(path: &Path) -> &'static str {
    let Ok(text) = std::fs::read_to_string(path) else {
        return "unknown";
    };
    let Ok(value) = serde_json::from_str::<Json>(&text) else {
        return "unknown";
    };
    match value.get("capability").and_then(Json::as_str) {
        Some("available") => "available",
        Some("software") => "software",
        Some("unavailable") => "unavailable",
        _ => "unknown",
    }
}

/// TS `probeRemoteDesktopPeer`：未配对/不可达一律 offline（不抛）。
///
/// 命中端点后读 `capabilities.remoteDesktop*`：缺键 = 旧对端 = false。
async fn probe_peer_capability(credentials: &CredentialStore, server_id: &str) -> Json {
    let offline = json!({
        "remoteDesktop": false,
        "remoteDesktopNative": false,
        "online": false,
    });
    let server_id = server_id.trim();
    if server_id.is_empty() {
        return offline;
    }
    let Ok(Some(raw)) = credentials.load(&peer_meta_key(server_id)) else {
        return offline;
    };
    let Some(meta) = LanPeerMeta::parse(Some(raw)) else {
        return offline;
    };
    let Ok(Some(token)) = credentials.load(&peer_token_key(server_id)) else {
        return offline;
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        return offline;
    }
    let endpoint_count = meta.endpoints.len();
    for endpoint in &meta.endpoints {
        let timeout = endpoint_probe_timeout(endpoint, endpoint_count);
        let _ = build_http_base(
            &endpoint.host,
            endpoint.port,
            &endpoint.scheme,
            endpoint.path.as_deref().unwrap_or(""),
        );
        // 客户端构造失败（TLS provider 等）等同不可达：继续下一个端点。
        if request_client(timeout).is_err() {
            continue;
        }
        match fetch_server_info_payload(endpoint, &token, timeout).await {
            Ok(info) => return project_peer_capability(&info, endpoint),
            Err(ProbeError {
                retryable: true, ..
            }) => continue,
            Err(_) => return offline,
        }
    }
    offline
}

fn project_peer_capability(info: &Json, endpoint: &crate::lan_access::LanPeerEndpoint) -> Json {
    let capabilities = info.get("capabilities");
    let mut result = json!({
        "online": true,
        "remoteDesktop": capabilities
            .and_then(|value| value.get("remoteDesktop"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        "remoteDesktopNative": capabilities
            .and_then(|value| value.get("remoteDesktopNative"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        "endpointKind": endpoint.kind,
        "endpointHost": endpoint.host,
    });
    if let Some(name) = info.get("name").and_then(Json::as_str) {
        if !name.is_empty() {
            result["name"] = json!(name);
        }
    }
    result
}

/// LAN 客户端记录读写：唯一真相是 `lan:access:clients`（与 lan-access 频道同键同形）。
fn read_client_records(
    credentials: &CredentialStore,
) -> Result<Vec<LanAccessClientRecord>, String> {
    crate::lan_access::read_client_records(credentials)
}

fn write_client_records(
    credentials: &CredentialStore,
    records: &[LanAccessClientRecord],
) -> Result<(), String> {
    crate::lan_access::write_client_records(credentials, records)
}

#[cfg(test)]
mod tests {
    use super::*;

    use credential_store::CredentialStore;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "rd-channel-{label}-{}-{}",
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

    fn store(dir: &TempDir) -> Arc<CredentialStore> {
        Arc::new(CredentialStore::with_secret(
            &dir.0,
            "rd-channel-test-secret",
        ))
    }

    #[test]
    fn encoder_capability_whitelists_known_values() {
        let dir = TempDir::new("capability");
        let path = dir.0.join("rd-encoder-capability.json");
        assert_eq!(
            load_encoder_capability(&path),
            "unknown",
            "缺失文件 = unknown"
        );

        std::fs::write(&path, "{}").unwrap();
        assert_eq!(load_encoder_capability(&path), "unknown", "缺键 = unknown");

        std::fs::write(&path, r#"{"capability":"software"}"#).unwrap();
        assert_eq!(load_encoder_capability(&path), "software");

        std::fs::write(&path, r#"{"capability":"available"}"#).unwrap();
        assert_eq!(load_encoder_capability(&path), "available");

        // 未知枚举值不猜：按无缓存处理。
        std::fs::write(&path, r#"{"capability":"turbo"}"#).unwrap();
        assert_eq!(load_encoder_capability(&path), "unknown");

        // 坏 JSON 不影响运行。
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(load_encoder_capability(&path), "unknown");
    }

    #[test]
    fn server_enabled_persists_as_one_and_deletes_on_disable() {
        let dir = TempDir::new("enabled");
        let credentials = store(&dir);
        assert!(!load_server_enabled(&credentials).unwrap());

        persist_server_enabled(&credentials, true).unwrap();
        assert!(load_server_enabled(&credentials).unwrap());
        assert_eq!(
            credentials
                .load(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY)
                .unwrap()
                .as_deref(),
            Some("1")
        );

        // 关 = 删键（不是写 "0"），否则会把「从未开启」和「开过又关」混为一谈。
        persist_server_enabled(&credentials, false).unwrap();
        assert!(!load_server_enabled(&credentials).unwrap());
        assert!(credentials
            .load(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY)
            .unwrap()
            .is_none());
    }

    #[test]
    fn authorize_and_revoke_use_the_shared_client_record() {
        let dir = TempDir::new("authorize");
        let credentials = store(&dir);
        write_client_records(
            &credentials,
            &[
                LanAccessClientRecord {
                    id: "client-a".to_string(),
                    label: "Desk".to_string(),
                    created_at: 1,
                    last_used_at: None,
                    last_workspace_path: None,
                    remote_desktop_authorized: None,
                },
                LanAccessClientRecord {
                    id: "client-b".to_string(),
                    label: "Laptop".to_string(),
                    created_at: 2,
                    last_used_at: None,
                    last_workspace_path: None,
                    remote_desktop_authorized: None,
                },
            ],
        )
        .unwrap();

        set_authorized(&credentials, "client-a", true).unwrap();
        let records = read_client_records(&credentials).unwrap();
        assert_eq!(records[0].remote_desktop_authorized, Some(true));
        assert_eq!(records[1].remote_desktop_authorized, None);

        // 撤销 = 删字段（不是置 false）：序列化后不应出现该键。
        set_authorized(&credentials, "client-a", false).unwrap();
        let raw = credentials
            .load(crate::lan_access::LAN_ACCESS_CLIENTS_CREDENTIAL_KEY)
            .unwrap()
            .unwrap();
        assert!(!raw.contains("remoteDesktopAuthorized"), "{raw}");

        // 未知 clientId 幂等：不报错、不改其他记录。
        set_authorized(&credentials, "client-missing", true).unwrap();
        let records = read_client_records(&credentials).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .all(|r| r.remote_desktop_authorized.is_none()));

        assert!(set_authorized(&credentials, "  ", true).is_err());
    }

    #[tokio::test]
    async fn unpaired_peer_is_offline_and_does_not_throw() {
        let dir = TempDir::new("probe");
        let credentials = store(&dir);
        let capability = probe_peer_capability(&credentials, "unknown-peer").await;
        assert_eq!(capability["online"], false);
        assert_eq!(capability["remoteDesktop"], false);
        assert_eq!(capability["remoteDesktopNative"], false);

        // 空 serverId 同样 offline。
        let empty = probe_peer_capability(&credentials, "   ").await;
        assert_eq!(empty["online"], false);
    }
}
