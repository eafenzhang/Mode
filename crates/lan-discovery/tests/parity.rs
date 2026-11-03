use lan_discovery::{
    collect_broadcast_targets, compute_broadcast_address, is_ipv4_address,
    is_tailscale_range_address, merge_lan_discovered_peers, parse_tailscale_status_ips,
    LanDiscoveredPeer,
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
