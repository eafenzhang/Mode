//! LAN 发现运行时：UDP 探测、收集窗口与 Tailscale 单播补充。
//!
//! 真源：`packages/server/src/remote/lanDiscovery.ts` 的 `discoverLanPeers`。
//! 纯计算（公告解析、合并、排序、广播目标推导）在 [`crate`] 根模块；本模块只做 IO。
//!
//! 广播目标 = 全局 `255.255.255.255` + 每个非 internal IPv4 网卡的定向广播；
//! 本机地址来自 [`crate::interfaces::local_ipv4_addresses`]，并对探测目标补一次
//! UDP connect 路由推导作为兜底。Tailscale 单播同 TS。

use std::collections::HashSet;
use std::io::Read;
use std::net::{SocketAddr, UdpSocket};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::announcement::{
    build_lan_probe_payload, parse_lan_announcement, LAN_ACCESS_DISCOVERY_PORT,
};
use crate::{
    filter_self_entries, merge_lan_discovered_peers, parse_tailscale_status_ips,
    sort_discovered_peers, LanDiscoveredPeer,
};

pub const DISCOVERY_WINDOW: Duration = Duration::from_millis(1_500);
pub const TAILSCALE_STATUS_TIMEOUT: Duration = Duration::from_millis(800);
const RECEIVE_POLL: Duration = Duration::from_millis(50);
const MAX_DATAGRAM_BYTES: usize = 64 * 1024;

const TAILSCALE_CLI_CANDIDATES: [&str; 3] = [
    "tailscale",
    "C:\\Program Files\\Tailscale\\tailscale.exe",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// Tailscale 单播来源：默认关闭由调用方显式选择，避免每次发现都 fork CLI。
#[derive(Clone)]
pub enum TailscaleSource {
    Disabled,
    Cli,
    Injected(Arc<dyn Fn() -> Vec<String> + Send + Sync>),
}

#[derive(Clone)]
pub struct LanDiscoveryOptions {
    pub timeout: Duration,
    /// 默认 false：本机应答不进结果。
    pub include_self: bool,
    /// 探测端口；默认 `LAN_ACCESS_DISCOVERY_PORT`（测试可注入）。
    pub port: u16,
    /// 广播目标；`None` 用 `255.255.255.255`。
    pub broadcast_targets: Option<Vec<String>>,
    /// 本机 IPv4 地址集；`None` 时按探测目标做路由推导。
    pub local_addresses: Option<HashSet<String>>,
    pub tailscale: TailscaleSource,
    pub tailscale_timeout: Duration,
}

impl Default for LanDiscoveryOptions {
    fn default() -> Self {
        Self {
            timeout: DISCOVERY_WINDOW,
            include_self: false,
            port: LAN_ACCESS_DISCOVERY_PORT,
            broadcast_targets: None,
            local_addresses: None,
            tailscale: TailscaleSource::Cli,
            tailscale_timeout: TAILSCALE_STATUS_TIMEOUT,
        }
    }
}

/// 向广播目标发一次探测，收集窗口内的应答；返回合并/排序后的节点。
///
/// 绑定失败返回空列表（与 TS 一致），不向上抛错。
pub fn discover_lan_peers(options: &LanDiscoveryOptions) -> Vec<LanDiscoveredPeer> {
    let Ok(socket) = UdpSocket::bind(("0.0.0.0", 0)) else {
        return Vec::new();
    };
    if socket.set_broadcast(true).is_err() {
        return Vec::new();
    }
    let _ = socket.set_read_timeout(Some(RECEIVE_POLL));

    let targets = options
        .broadcast_targets
        .clone()
        .unwrap_or_else(crate::interfaces::directed_broadcast_targets);
    let local_addresses = if options.include_self {
        None
    } else {
        Some(
            options
                .local_addresses
                .clone()
                .unwrap_or_else(|| default_local_addresses(&targets, options.port)),
        )
    };

    let (tailscale_tx, tailscale_rx) = mpsc::channel::<Vec<String>>();
    match &options.tailscale {
        TailscaleSource::Disabled => drop(tailscale_tx),
        TailscaleSource::Cli => {
            let timeout = options.tailscale_timeout;
            thread::spawn(move || {
                let _ = tailscale_tx.send(run_tailscale_status_ips(timeout));
            });
        }
        TailscaleSource::Injected(source) => {
            let source = Arc::clone(source);
            thread::spawn(move || {
                let _ = tailscale_tx.send(source());
            });
        }
    }

    let payload = build_lan_probe_payload().as_bytes();
    let mut tailscale_sent = false;
    for target in &targets {
        let _ = socket.send_to(payload, (target.as_str(), options.port));
    }

    let started = Instant::now();
    let mut peers = std::collections::HashMap::<String, LanDiscoveredPeer>::new();
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    loop {
        if !tailscale_sent {
            if let Ok(ips) = tailscale_rx.try_recv() {
                tailscale_sent = true;
                for ip in ips {
                    let _ = socket.send_to(payload, (ip.as_str(), options.port));
                }
            }
        }
        if started.elapsed() >= options.timeout {
            break;
        }
        match socket.recv_from(&mut buffer) {
            Ok((len, from)) => {
                let Some(text) = std::str::from_utf8(&buffer[..len]).ok() else {
                    continue;
                };
                let Some(announcement) = parse_lan_announcement(text) else {
                    continue;
                };
                let host = from.ip().to_string();
                if local_addresses
                    .as_ref()
                    .is_some_and(|local| local.contains(&host))
                {
                    continue;
                }
                let key = format!("{}@{host}:{}", announcement.server_id, announcement.port);
                peers.insert(
                    key,
                    LanDiscoveredPeer {
                        server_id: announcement.server_id,
                        name: announcement.name,
                        version: announcement.version,
                        port: announcement.port,
                        requires_pairing: announcement.requires_pairing,
                        platform: announcement.platform,
                        host,
                        extra_hosts: None,
                    },
                );
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }

    let peers = peers.into_values().collect::<Vec<_>>();
    let peers = merge_lan_discovered_peers(peers);
    sort_discovered_peers(filter_self_entries(peers, local_addresses.as_ref()))
}

/// 本机地址：网卡枚举（含回环）+ 对每个探测目标做一次 UDP connect（不发包）的路由兜底。
fn default_local_addresses(targets: &[String], port: u16) -> HashSet<String> {
    let mut local = crate::interfaces::local_ipv4_addresses();
    local.extend(route_hint_addresses(targets, port));
    local
}

fn route_hint_addresses(targets: &[String], port: u16) -> HashSet<String> {
    let mut local = HashSet::new();
    for target in targets {
        let Ok(socket) = UdpSocket::bind(("0.0.0.0", 0)) else {
            continue;
        };
        if socket.connect((target.as_str(), port)).is_err() {
            continue;
        }
        if let Ok(SocketAddr::V4(address)) = socket.local_addr() {
            local.insert(address.ip().to_string());
        }
    }
    local
}

fn run_tailscale_status_ips(timeout: Duration) -> Vec<String> {
    for candidate in TAILSCALE_CLI_CANDIDATES {
        let Ok(mut child) = Command::new(candidate)
            .args(["status", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() < timeout => {
                    thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let Some(status) = status else {
            continue;
        };
        if !status.success() {
            continue;
        }
        let Some(mut stdout) = child.stdout.take() else {
            continue;
        };
        let mut text = String::new();
        if stdout.read_to_string(&mut text).is_ok() {
            return parse_tailscale_status_ips(&text);
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announcement::{build_lan_announcement_payload, LanPeerAnnouncementInput};

    fn announcement(server_id: &str) -> String {
        build_lan_announcement_payload(&LanPeerAnnouncementInput {
            server_id: server_id.to_string(),
            name: Some("Peer".to_string()),
            version: "1.0.0".to_string(),
            port: 45880,
            requires_pairing: false,
            platform: Some("win32".to_string()),
        })
    }

    fn options(port: u16) -> LanDiscoveryOptions {
        LanDiscoveryOptions {
            timeout: Duration::from_millis(600),
            include_self: true,
            port,
            broadcast_targets: Some(vec!["127.0.0.1".to_string()]),
            local_addresses: Some(HashSet::new()),
            tailscale: TailscaleSource::Disabled,
            tailscale_timeout: Duration::from_millis(100),
        }
    }

    #[test]
    fn discovers_reply_over_real_udp() {
        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = responder.local_addr().unwrap().port();
        let reply = announcement("server-1");
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            let (len, from) = responder.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..len], build_lan_probe_payload().as_bytes());
            responder.send_to(reply.as_bytes(), from).unwrap();
        });

        let peers = discover_lan_peers(&options(port));
        handle.join().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].server_id, "server-1");
        assert_eq!(peers[0].host, "127.0.0.1");
    }

    #[test]
    fn filters_local_replies_unless_include_self() {
        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = responder.local_addr().unwrap().port();
        let reply = announcement("server-1");
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            let (_, from) = responder.recv_from(&mut buffer).unwrap();
            responder.send_to(reply.as_bytes(), from).unwrap();
        });

        let mut filtered = options(port);
        filtered.include_self = false;
        filtered.local_addresses = Some(HashSet::from(["127.0.0.1".to_string()]));
        let peers = discover_lan_peers(&filtered);
        handle.join().unwrap();
        assert!(peers.is_empty());
    }

    #[test]
    fn sends_probes_to_injected_tailscale_ips() {
        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = responder.local_addr().unwrap().port();
        let reply = announcement("server-ts");
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            let (_, from) = responder.recv_from(&mut buffer).unwrap();
            responder.send_to(reply.as_bytes(), from).unwrap();
        });

        let mut with_tailscale = options(port);
        with_tailscale.broadcast_targets = Some(Vec::new());
        with_tailscale.tailscale = TailscaleSource::Injected(Arc::new(|| {
            vec!["127.0.0.1".to_string()]
        }));
        let peers = discover_lan_peers(&with_tailscale);
        handle.join().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].server_id, "server-ts");
    }
}
