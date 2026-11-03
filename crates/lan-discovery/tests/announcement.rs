use lan_discovery::{
    build_lan_announcement_payload, build_lan_probe_payload, parse_lan_announcement,
    LanPeerAnnouncementInput, LAN_ACCESS_DISCOVERY_PORT, LAN_ACCESS_PROBE,
    LAN_ACCESS_PROTOCOL_VERSION,
};

fn input() -> LanPeerAnnouncementInput {
    LanPeerAnnouncementInput {
        server_id: "srv-1".to_string(),
        name: Some("desk".to_string()),
        version: "0.0.0".to_string(),
        port: 45880,
        requires_pairing: true,
        platform: Some("win32".to_string()),
    }
}

#[test]
fn constants_match_contract() {
    assert_eq!(LAN_ACCESS_DISCOVERY_PORT, 45879);
    assert_eq!(LAN_ACCESS_PROBE, "mode-lan-discover-v1");
    assert_eq!(LAN_ACCESS_PROTOCOL_VERSION, 1);
    assert_eq!(build_lan_probe_payload(), LAN_ACCESS_PROBE);
}

#[test]
fn announcement_payload_has_contract_key_order_and_round_trips() {
    let payload = build_lan_announcement_payload(&input());
    assert_eq!(
        payload,
        r#"{"magic":"mode-lan","protocolVersion":1,"serverId":"srv-1","name":"desk","version":"0.0.0","port":45880,"requiresPairing":true,"platform":"win32"}"#
    );
    let parsed = parse_lan_announcement(&payload).expect("round trip");
    assert_eq!(parsed.server_id, "srv-1");
    assert_eq!(parsed.name.as_deref(), Some("desk"));
    assert_eq!(parsed.port, 45880);
    assert!(parsed.requires_pairing);
    assert_eq!(parsed.platform.as_deref(), Some("win32"));
}

#[test]
fn announcement_omits_absent_optional_fields() {
    let payload = build_lan_announcement_payload(&LanPeerAnnouncementInput {
        name: None,
        platform: None,
        ..input()
    });
    assert!(!payload.contains("\"name\""));
    assert!(!payload.contains("\"platform\""));
    assert!(parse_lan_announcement(&payload).is_some());
}

#[test]
fn announcement_trims_server_id_and_name_and_ignores_unknown_keys() {
    let parsed = parse_lan_announcement(
        r#"{"magic":"mode-lan","protocolVersion":1,"serverId":"  srv-1  ","name":"  desk  ","version":"0.0.0","port":1,"requiresPairing":false,"extra":42}"#,
    )
    .expect("valid");
    assert_eq!(parsed.server_id, "srv-1");
    assert_eq!(parsed.name.as_deref(), Some("desk"));
    assert!(!parsed.requires_pairing);
}

#[test]
fn announcement_rejects_invalid_fields() {
    let base = r#""version":"0.0.0","port":45880,"requiresPairing":true"#;
    for payload in [
        format!(r#"{{"magic":"other","protocolVersion":1,"serverId":"s","{base}}}"#),
        format!(r#"{{"magic":"mode-lan","protocolVersion":2,"serverId":"s","{base}}}"#),
        format!(r#"{{"magic":"mode-lan","protocolVersion":1,"serverId":"   ","{base}}}"#),
        r#"{"magic":"mode-lan","protocolVersion":1,"serverId":"s","version":"0.0.0","port":0,"requiresPairing":true}"#.to_string(),
        r#"{"magic":"mode-lan","protocolVersion":1,"serverId":"s","version":"0.0.0","port":65536,"requiresPairing":true}"#.to_string(),
        r#"{"magic":"mode-lan","protocolVersion":1,"serverId":"s","version":"0.0.0","port":1.5,"requiresPairing":true}"#.to_string(),
        r#"{"magic":"mode-lan","protocolVersion":1,"serverId":"s","version":"0.0.0","port":45880,"requiresPairing":"yes"}"#.to_string(),
        r#"{"magic":"mode-lan","protocolVersion":1,"serverId":"s","version":"0.0.0","port":45880,"requiresPairing":true,"name":"   "}"#.to_string(),
        "not json".to_string(),
        "[]".to_string(),
    ] {
        assert!(
            parse_lan_announcement(&payload).is_none(),
            "应拒绝：{payload}"
        );
    }
}
