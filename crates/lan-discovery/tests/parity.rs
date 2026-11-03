use std::collections::HashSet;

use lan_discovery::{
    collect_broadcast_targets, compute_broadcast_address, filter_self_entries, is_ipv4_address,
    is_tailscale_range_address, merge_lan_discovered_peers, parse_tailscale_status_ips,
    sort_discovered_peers, LanDiscoveredPeer,
};

fn peer(server_id: &str, host: &str, port: u16) -> LanDiscoveredPeer {
    LanDiscoveredPeer {
        server_id: server_id.to_string(),
        name: Some(format!("{server_id}-{host}")),
        version: "0.0.0".to_string(),
        port,
        requires_pairing: true,
        platform: Some("win32".to_string()),
        host: host.to_string(),
        extra_hosts: None,
    }
}

#[test]
fn tailscale_status_collects_self_and_peers_without_v6() {
    let stdout = r#"{
      "Self": {"TailscaleIPs": ["100.64.1.2", "fd7a:115c::1"]},
      "Peer": {
        "a": {"TailscaleIPs": ["100.64.1.3", "100.64.1.2"]},
        "b": {"TailscaleIPs": ["fd7a:115c::2"]},
        "c": {}
      }
    }"#;
    assert_eq!(
        parse_tailscale_status_ips(stdout),
        vec!["100.64.1.2".to_string(), "100.64.1.3".to_string()]
    );
}

#[test]
fn tailscale_status_degrades_to_empty() {
    assert!(parse_tailscale_status_ips("not json").is_empty());
    assert!(parse_tailscale_status_ips("[]").is_empty());
    assert!(parse_tailscale_status_ips("null").is_empty());
    assert!(parse_tailscale_status_ips(r#"{"Self":{}}"#).is_empty());
}

#[test]
fn merge_prefers_lan_address_and_moves_tailnet_to_extra_hosts() {
    let merged = merge_lan_discovered_peers(vec![
        peer("srv", "100.68.49.120", 45880),
        peer("srv", "192.168.1.8", 45880),
    ]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].host, "192.168.1.8");
    assert_eq!(
        merged[0].extra_hosts,
        Some(vec!["100.68.49.120".to_string()])
    );
}

#[test]
fn merge_keeps_different_ports_apart_and_last_host_wins() {
    let split = merge_lan_discovered_peers(vec![
        peer("srv", "192.168.1.8", 45880),
        peer("srv", "192.168.1.8", 45881),
    ]);
    assert_eq!(split.len(), 2);

    let mut first = peer("srv", "192.168.1.8", 45880);
    first.name = Some("first".to_string());
    let mut last = peer("srv", "192.168.1.8", 45880);
    last.name = Some("last".to_string());
    let merged = merge_lan_discovered_peers(vec![first, last]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name.as_deref(), Some("last"));
}

#[test]
fn merge_sorts_physical_lan_addresses_lexicographically() {
    let merged = merge_lan_discovered_peers(vec![
        peer("srv", "192.168.1.20", 45880),
        peer("srv", "192.168.1.8", 45880),
        peer("srv", "100.64.0.9", 45880),
    ]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].host, "192.168.1.20");
    assert_eq!(
        merged[0].extra_hosts,
        Some(vec!["192.168.1.8".to_string(), "100.64.0.9".to_string()])
    );
}

#[test]
fn ipv4_validation_is_strict() {
    assert!(is_ipv4_address("1.2.3.4"));
    assert!(is_ipv4_address("192.168.001.010"));
    assert!(!is_ipv4_address("1.2.3"));
    assert!(!is_ipv4_address("1.2.3.256"));
    assert!(!is_ipv4_address("+1.2.3.4"));
    assert!(!is_ipv4_address("1.2.3.4 "));
    assert!(!is_ipv4_address("fd7a:115c::1"));
}

#[test]
fn tailscale_range_boundaries() {
    assert!(!is_tailscale_range_address("100.63.255.255"));
    assert!(is_tailscale_range_address("100.64.0.0"));
    assert!(is_tailscale_range_address("100.127.255.255"));
    assert!(!is_tailscale_range_address("100.128.0.0"));
    assert!(!is_tailscale_range_address("10.0.0.1"));
}

#[test]
fn broadcast_address_and_targets_match_interface_rules() {
    assert_eq!(
        compute_broadcast_address("192.168.1.10", "255.255.255.0").as_deref(),
        Some("192.168.1.255")
    );
    assert_eq!(
        compute_broadcast_address("10.0.0.5", "255.255.255.252").as_deref(),
        Some("10.0.0.7")
    );
    assert_eq!(compute_broadcast_address("bad", "255.255.255.0"), None);

    let targets = collect_broadcast_targets(vec![
        (
            "192.168.1.10".to_string(),
            Some("255.255.255.0".to_string()),
            false,
        ),
        ("127.0.0.1".to_string(), Some("255.0.0.0".to_string()), true),
        ("bad".to_string(), Some("255.255.255.0".to_string()), false),
        ("192.168.1.10".to_string(), None, false),
    ]);
    assert_eq!(
        targets,
        vec!["255.255.255.255".to_string(), "192.168.1.255".to_string()]
    );
}

fn peer_named(server_id: &str, host: &str, name: Option<&str>) -> LanDiscoveredPeer {
    LanDiscoveredPeer {
        name: name.map(str::to_string),
        ..peer(server_id, host, 45880)
    }
}

#[test]
fn self_entries_are_filtered_only_when_local_set_provided() {
    let mut local = HashSet::new();
    local.insert("127.0.0.1".to_string());
    local.insert("192.168.1.5".to_string());
    let entries = vec![
        peer_named("self-lan", "192.168.1.5", Some("self-lan")),
        peer_named("remote", "10.0.0.2", Some("remote")),
        peer_named("self-loop", "127.0.0.1", Some("self-loop")),
    ];
    let filtered = filter_self_entries(entries.clone(), Some(&local));
    assert_eq!(
        filtered
            .iter()
            .map(|entry| entry.server_id.as_str())
            .collect::<Vec<_>>(),
        vec!["remote"]
    );
    // TS `includeSelf: true` 把本机地址集置空；Rust 侧 `None` 不过滤。
    assert_eq!(filter_self_entries(entries.clone(), None).len(), 3);
    // 空集同样不过滤（没有可命中的地址）。
    assert_eq!(filter_self_entries(entries, Some(&HashSet::new())).len(), 3);
}

#[test]
fn discovered_peers_sort_by_name_with_server_id_fallback() {
    let sorted = sort_discovered_peers(vec![
        peer_named("srv-fallback", "10.0.0.4", None),
        peer_named("srv-b", "10.0.0.2", Some("Beta")),
        peer_named("srv-a", "10.0.0.3", Some("ALPHA")),
        peer_named("srv-c", "10.0.0.1", Some("alpha")),
    ]);
    // JS localeCompare 确定性口径：小写折叠 alpha < ALPHA < Beta < srv-fallback。
    assert_eq!(
        sorted
            .iter()
            .map(|entry| entry.server_id.as_str())
            .collect::<Vec<_>>(),
        vec!["srv-c", "srv-a", "srv-b", "srv-fallback"]
    );
    // 排序是稳定语义：同 key 的两条保持原相对顺序。
    let stable = sort_discovered_peers(vec![
        peer_named("first", "10.0.0.1", Some("same")),
        peer_named("second", "10.0.0.2", Some("same")),
    ]);
    assert_eq!(
        stable
            .iter()
            .map(|entry| entry.server_id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}
