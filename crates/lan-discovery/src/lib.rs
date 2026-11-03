//! 局域网发现纯逻辑：Tailscale 状态解析、单卡合并与广播目标推导。
//!
//! 真源：`packages/server/src/remote/lanDiscovery.ts` 与
//! `docs/specs/lan-discovery.md`。UDP 收发、`tailscale` CLI runner 与本机网卡枚举
//! 留在宿主装配层；本 crate 只做可注入的纯计算。

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde_json::Value;

/// 发现结果：声明字段只保留合并所需部分；magic/protocol 校验仍由公告解析层负责。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanDiscoveredPeer {
    pub server_id: String,
    pub name: Option<String>,
    pub version: String,
    pub port: u16,
    pub requires_pairing: bool,
    pub platform: Option<String>,
    pub host: String,
    pub extra_hosts: Option<Vec<String>>,
}

/// 解析 `tailscale status --json`：收集 Self 与全部 Peer 的 IPv4（去重、滤 IPv6，
/// 离线 peer 也保留）。任何解析失败返回空数组。
pub fn parse_tailscale_status_ips(stdout: &str) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let Some(root) = parsed.as_object() else {
        return Vec::new();
    };
    let mut ips = Vec::new();
    let mut seen = HashSet::new();
    let mut collect = |entry: Option<&Value>| {
        let Some(entry) = entry.and_then(Value::as_object) else {
            return;
        };
        let Some(entries) = entry.get("TailscaleIPs").and_then(Value::as_array) else {
            return;
        };
        for ip in entries {
            let Some(ip) = ip.as_str() else {
                continue;
            };
            if is_ipv4_address(ip) && seen.insert(ip.to_string()) {
                ips.push(ip.to_string());
            }
        }
    };
    collect(root.get("Self"));
    if let Some(peers) = root.get("Peer").and_then(Value::as_object) {
        for peer in peers.values() {
            collect(Some(peer));
        }
    }
    ips
}

/// 100.64.0.0/10（Tailscale CGNAT 段）判定。
pub fn is_tailscale_range_address(host: &str) -> bool {
    let mut parts = host.split('.');
    let (Some(first), Some(second)) = (parts.next(), parts.next()) else {
        return false;
    };
    first == "100"
        && second
            .parse::<u16>()
            .map(|value| (64..=127).contains(&value))
            .unwrap_or(false)
}

pub fn is_ipv4_address(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.chars().all(|ch| ch.is_ascii_digit())
                && part
                    .parse::<u16>()
                    .map(|value| value <= 255)
                    .unwrap_or(false)
        })
}

fn ipv4_to_int(address: &str) -> Option<u32> {
    if !is_ipv4_address(address) {
        return None;
    }
    let mut value: u32 = 0;
    for part in address.split('.') {
        let byte: u32 = part.parse().ok()?;
        value = (value << 8) | byte;
    }
    Some(value)
}

fn int_to_ipv4(value: u32) -> String {
    [24, 16, 8, 0]
        .iter()
        .map(|shift| ((value >> shift) & 0xff).to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// 单卡合并：同一 serverId+port 的多地址合并为一条；主地址优先物理局域网
/// （非 100.64.0.0/10），其余按序进 extraHosts；端口不同不合并。
pub fn merge_lan_discovered_peers(entries: Vec<LanDiscoveredPeer>) -> Vec<LanDiscoveredPeer> {
    let mut groups: IndexMap<String, Vec<LanDiscoveredPeer>> = IndexMap::new();
    for entry in entries {
        groups
            .entry(format!("{}|{}", entry.server_id, entry.port))
            .or_default()
            .push(entry);
    }
    let mut merged = Vec::new();
    for group in groups.into_values() {
        if group.len() == 1 {
            merged.extend(group);
            continue;
        }
        // 同一 host 只保留最后一个（对应 TS `byHost.set` 覆盖语义）。
        let mut by_host: HashMap<String, LanDiscoveredPeer> = HashMap::new();
        for entry in group {
            by_host.insert(entry.host.clone(), entry);
        }
        let mut ordered: Vec<LanDiscoveredPeer> = by_host.into_values().collect();
        ordered.sort_by(|left, right| {
            is_tailscale_range_address(&left.host)
                .cmp(&is_tailscale_range_address(&right.host))
                .then_with(|| left.host.cmp(&right.host))
        });
        let Some(mut first) = ordered.first().cloned() else {
            continue;
        };
        let extra_hosts: Vec<String> = ordered
            .iter()
            .skip(1)
            .map(|item| item.host.clone())
            .collect();
        if !extra_hosts.is_empty() {
            first.extra_hosts = Some(extra_hosts);
        }
        merged.push(first);
    }
    merged
}

/// 由本机地址与掩码推导定向广播地址（部分系统不转发 255.255.255.255）。
pub fn compute_broadcast_address(address: &str, netmask: &str) -> Option<String> {
    let address_int = ipv4_to_int(address)?;
    let mask_int = ipv4_to_int(netmask)?;
    Some(int_to_ipv4((address_int & mask_int) | (!mask_int)))
}

/// 收集广播目标：固定全局广播 + 每个非 internal IPv4 网卡的定向广播。
/// 网卡信息由宿主注入（`(address, netmask, internal)`）。
pub fn collect_broadcast_targets(
    interfaces: impl IntoIterator<Item = (String, Option<String>, bool)>,
) -> Vec<String> {
    let mut targets = vec!["255.255.255.255".to_string()];
    let mut seen: HashSet<String> = targets.iter().cloned().collect();
    for (address, netmask, internal) in interfaces {
        if internal {
            continue;
        }
        let Some(netmask) = netmask.as_deref() else {
            continue;
        };
        if !is_ipv4_address(&address) || !is_ipv4_address(netmask) {
            continue;
        }
        if let Some(broadcast) = compute_broadcast_address(&address, netmask) {
            if seen.insert(broadcast.clone()) {
                targets.push(broadcast);
            }
        }
    }
    targets
}
