//! LAN 发现公告契约：常量、严格解析与 payload 构造。
//!
//! 真源：`packages/shared/src/lanAccess.ts`。zod `.object` 默认丢弃未知键；
//! 本模块同样忽略未知字段，只校验已知字段的类型与约束。

use serde::Serialize;
use serde_json::Value;

pub const LAN_ACCESS_DISCOVERY_PORT: u16 = 45879;
pub const LAN_ACCESS_PROBE: &str = "mode-lan-discover-v1";
pub const LAN_ACCESS_PROTOCOL_VERSION: u32 = 1;
pub const LAN_ACCESS_PAIR_CODE_TTL_MS: u64 = 5 * 60_000;
pub const LAN_ACCESS_DEFAULT_PORT: u16 = 45880;
pub const DEFAULT_PUBLIC_RELAY_BASE_URL: &str = "https://relay.mymode.cloud";
pub const LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY: &str = "lan:access:relay-tunnel-id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanPeerAnnouncement {
    pub server_id: String,
    pub name: Option<String>,
    pub version: String,
    pub port: u16,
    pub requires_pairing: bool,
    pub platform: Option<String>,
}

/// 公告 payload 的输入（magic/protocolVersion 由构造器补齐）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanPeerAnnouncementInput {
    pub server_id: String,
    pub name: Option<String>,
    pub version: String,
    pub port: u16,
    pub requires_pairing: bool,
    pub platform: Option<String>,
}

pub fn build_lan_probe_payload() -> &'static str {
    LAN_ACCESS_PROBE
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnouncementPayload<'a> {
    magic: &'static str,
    protocol_version: u32,
    server_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    version: &'a str,
    port: u16,
    requires_pairing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<&'a str>,
}

pub fn build_lan_announcement_payload(info: &LanPeerAnnouncementInput) -> String {
    serde_json::to_string(&AnnouncementPayload {
        magic: "mode-lan",
        protocol_version: LAN_ACCESS_PROTOCOL_VERSION,
        server_id: &info.server_id,
        name: info.name.as_deref(),
        version: &info.version,
        port: info.port,
        requires_pairing: info.requires_pairing,
        platform: info.platform.as_deref(),
    })
    .expect("公告 JSON 序列化不会失败")
}

/// 解析公告文本；任何字段不合法或 magic/协议版本不匹配都返回 `None`。
pub fn parse_lan_announcement(text: &str) -> Option<LanPeerAnnouncement> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.get("magic").and_then(Value::as_str) != Some("mode-lan") {
        return None;
    }
    if object.get("protocolVersion").and_then(Value::as_u64)
        != Some(u64::from(LAN_ACCESS_PROTOCOL_VERSION))
    {
        return None;
    }
    let server_id = trimmed_non_empty(object.get("serverId").and_then(Value::as_str))?;
    let name = match object.get("name") {
        None | Some(Value::Null) => None,
        Some(value) => Some(trimmed_non_empty(value.as_str())?),
    };
    let version = object.get("version").and_then(Value::as_str)?.to_string();
    let port = object.get("port").and_then(Value::as_u64)?;
    if port == 0 || port > 65_535 {
        return None;
    }
    let requires_pairing = object.get("requiresPairing").and_then(Value::as_bool)?;
    let platform = match object.get("platform") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str()?.to_string()),
    };
    Some(LanPeerAnnouncement {
        server_id,
        name,
        version,
        port: port as u16,
        requires_pairing,
        platform,
    })
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
