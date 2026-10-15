//! 深链事件载荷的线协议形状（跨端契约）。
//!
//! 这些断言存在的理由：渲染层按 `kind` 过滤事件、按 `DEEP_LINK_EVENT` 装监听，
//! 两者写错都是**静默失效**（订阅在、事件不在）。所以取值逐个钉死，
//! 且 TS 侧由 `scripts/generate-tauri-protocol.mjs --check` 从同一份 schema 导出。
use protocol::{DeepLinkEvent, DeepLinkEventKind, DEEP_LINK_EVENT};

#[test]
fn event_name_is_stable_because_the_renderer_subscribes_by_string() {
    assert_eq!(DEEP_LINK_EVENT, "mode:deep-link");
}

#[test]
fn kind_wire_values_are_pinned_not_derived_from_naming_rules() {
    // 刻意不用 `rename_all = "kebab-case"`：它会把 `OAuthCallback` 切成
    // `o-auth-callback`（连续大写被当成词边界）。这里把线上取值逐个钉住。
    let cases = [
        (DeepLinkEventKind::OAuthCallback, "oauth-callback"),
        (DeepLinkEventKind::PaymentCallback, "payment-callback"),
        (DeepLinkEventKind::WorkspaceOpen, "workspace-open"),
        (DeepLinkEventKind::ShareImport, "share-import"),
        (DeepLinkEventKind::Unrecognized, "unrecognized"),
    ];
    for (kind, wire) in cases {
        assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(wire));
    }
}

#[test]
fn payload_is_camel_case_and_omits_absent_options() {
    let payload = DeepLinkEvent {
        kind: DeepLinkEventKind::OAuthCallback,
        url: "mode://oauth/callback?state=s1".to_string(),
        path: None,
        code: None,
    };
    assert_eq!(
        serde_json::to_value(&payload).unwrap(),
        serde_json::json!({
            "kind": "oauth-callback",
            "url": "mode://oauth/callback?state=s1",
        }),
        "可选字段缺席而非 null（与 crates/protocol 其余类型的约定一致）"
    );
}

#[test]
fn payload_round_trips_with_the_optional_fields_present() {
    let payload = DeepLinkEvent {
        kind: DeepLinkEventKind::ShareImport,
        url: "mode://share/import?code=abc".to_string(),
        path: None,
        code: Some("abc".to_string()),
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert_eq!(
        serde_json::from_str::<DeepLinkEvent>(&json).unwrap(),
        payload
    );
}

#[test]
fn every_kind_deserializes_from_its_wire_value() {
    // 反向：渲染层发回来的（或日志里存的）载荷要能解回同一变体。
    for (kind, wire) in [
        (DeepLinkEventKind::OAuthCallback, "oauth-callback"),
        (DeepLinkEventKind::PaymentCallback, "payment-callback"),
        (DeepLinkEventKind::WorkspaceOpen, "workspace-open"),
        (DeepLinkEventKind::ShareImport, "share-import"),
        (DeepLinkEventKind::Unrecognized, "unrecognized"),
    ] {
        let parsed: DeepLinkEventKind = serde_json::from_value(serde_json::json!(wire)).unwrap();
        assert_eq!(parsed, kind);
    }
}
