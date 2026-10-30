//! account-provider-overlay 的单元/边界测试。

use account_provider_overlay::{
    build_account_provider_states, create_account_provider_config_snapshot,
    resolve_account_provider_configs, AccountProviderConnectionResult,
    AccountProviderConnectionStatus, AccountProviderOverlayError, AccountProviderState,
    AccountProviderStates, AccountProviderUnavailableReason, ProviderConfigMap,
    ProviderFamilyConnectionSelection, ResolveAccountProviderConfigsInput,
};
use indexmap::IndexMap;
use provider_config::{parse_provider_config_map, JsonValue};

fn parse_map(text: &str) -> ProviderConfigMap {
    parse_provider_config_map(&JsonValue::from_json_str(text).expect("JSON"), &[])
        .expect("合法 ProviderConfigMap")
}

fn account_provider(provider_id: &str, mode: &str) -> String {
    format!(
        r#"[{{"providerId":"{provider_id}","config":{{"access":{{"type":"zhipu-account","accountType":"zai","mode":"{mode}"}}}}}}]"#
    )
}

#[test]
fn team_selection_serializes_in_schema_order() {
    let selection = ProviderFamilyConnectionSelection::TeamCodingPlan {
        product_id: "product".to_owned(),
        organization_id: "org".to_owned(),
        project_id: "project".to_owned(),
    };
    assert_eq!(
        serde_json::to_string(&selection).expect("selection"),
        r#"{"kind":"team-coding-plan","productId":"product","organizationId":"org","projectId":"project"}"#
    );
}

#[test]
fn snapshot_omits_states_but_revision_keeps_null_placeholder() {
    let providers = ProviderConfigMap::empty();
    let snapshot = create_account_provider_config_snapshot("rev-1", providers, None);
    assert_eq!(snapshot.revision, r#"account:["rev-1",[],null]"#);
    assert!(snapshot.states.is_none());
    assert_eq!(
        snapshot.to_json().to_json_string(),
        r#"{"revision":"account:[\"rev-1\",[],null]","basedOnModeBuiltinRevision":"rev-1","providers":[]}"#
    );
}

#[test]
fn snapshot_keeps_empty_states_object() {
    let providers = ProviderConfigMap::empty();
    let snapshot = create_account_provider_config_snapshot(
        "rev-1",
        providers,
        Some(AccountProviderStates::new()),
    );
    assert_eq!(snapshot.revision, r#"account:["rev-1",[],{}]"#);
    assert!(snapshot.states.is_some());
    assert_eq!(
        snapshot.to_json().to_json_string(),
        r#"{"revision":"account:[\"rev-1\",[],{}]","basedOnModeBuiltinRevision":"rev-1","providers":[],"states":{}}"#
    );
}

#[test]
fn unknown_state_inherits_last_known_good_but_not_connection_key() {
    let providers = parse_map(&account_provider(
        "zai-individual",
        "individual-coding-plan",
    ));
    let connections = vec![AccountProviderConnectionResult {
        provider_id: "zai-individual".to_owned(),
        status: AccountProviderConnectionStatus::Unknown,
        unavailable_reason: None,
        current: Some(false),
        connection_key: Some("new-key".to_owned()),
        models: None,
        effective_at: None,
        reset_previous: false,
    }];
    let mut previous_states = AccountProviderStates::new();
    previous_states.insert(
        "zai-individual".to_owned(),
        AccountProviderState {
            availability: provider_config::AccountProviderAvailability::Available,
            entitled: true,
            unavailable_reason: Some(AccountProviderUnavailableReason::NotEntitled),
            current: Some(true),
            connection_key: Some("old-key".to_owned()),
            effective_at: Some(9),
        },
    );

    let states = build_account_provider_states(&connections, &providers, Some(&previous_states));
    let state = states.get("zai-individual").expect("state");
    assert_eq!(
        state.availability,
        provider_config::AccountProviderAvailability::Available
    );
    // entitled 取本轮 providers 投影（unknown 且无 previous providers → false）。
    assert!(!state.entitled);
    assert_eq!(
        state.unavailable_reason,
        Some(AccountProviderUnavailableReason::NotEntitled)
    );
    assert_eq!(state.current, Some(false));
    assert_eq!(state.connection_key.as_deref(), Some("new-key"));
    assert_eq!(state.effective_at, Some(9));
}

#[test]
fn reset_previous_drops_last_known_good_state() {
    let providers = parse_map(&account_provider(
        "zai-individual",
        "individual-coding-plan",
    ));
    let connections = vec![AccountProviderConnectionResult {
        provider_id: "zai-individual".to_owned(),
        status: AccountProviderConnectionStatus::Unknown,
        unavailable_reason: None,
        current: Some(false),
        connection_key: Some("new-key".to_owned()),
        models: None,
        effective_at: None,
        reset_previous: true,
    }];
    let mut previous_states = AccountProviderStates::new();
    previous_states.insert(
        "zai-individual".to_owned(),
        AccountProviderState {
            availability: provider_config::AccountProviderAvailability::Available,
            entitled: true,
            unavailable_reason: Some(AccountProviderUnavailableReason::NotEntitled),
            current: Some(true),
            connection_key: Some("old-key".to_owned()),
            effective_at: Some(9),
        },
    );

    let states = build_account_provider_states(&connections, &providers, Some(&previous_states));
    let state = states.get("zai-individual").expect("state");
    assert_eq!(
        state.availability,
        provider_config::AccountProviderAvailability::Unknown
    );
    assert!(!state.entitled);
    assert_eq!(state.unavailable_reason, None);
    assert_eq!(state.current, Some(false));
    assert_eq!(state.connection_key.as_deref(), Some("new-key"));
    assert_eq!(state.effective_at, None);
}

#[test]
fn config_resolver_rejects_invalid_connection_sets() {
    let configured = parse_map(&account_provider(
        "zai-individual",
        "individual-coding-plan",
    ));
    let duplicate = vec![
        AccountProviderConnectionResult::simple(
            "zai-individual",
            AccountProviderConnectionStatus::Available,
        ),
        AccountProviderConnectionResult::simple(
            "zai-individual",
            AccountProviderConnectionStatus::Available,
        ),
    ];
    let error = resolve_account_provider_configs(ResolveAccountProviderConfigsInput {
        configured_providers: &configured,
        previous_providers: &configured,
        connections: &duplicate,
    })
    .expect_err("重复连接必须失败");
    assert_eq!(
        error,
        AccountProviderOverlayError::new("重复 Account Provider 连接结果: zai-individual")
    );

    let unconfigured = vec![AccountProviderConnectionResult::simple(
        "ghost",
        AccountProviderConnectionStatus::Available,
    )];
    let error = resolve_account_provider_configs(ResolveAccountProviderConfigsInput {
        configured_providers: &configured,
        previous_providers: &configured,
        connections: &unconfigured,
    })
    .expect_err("未配置 provider 必须失败");
    assert_eq!(
        error,
        AccountProviderOverlayError::new("Account 连接指向未配置 Provider: ghost")
    );
}

#[test]
fn start_plan_overlay_writes_normalized_models_only() {
    let configured = parse_map(&account_provider("zai-start", "start-plan"));
    let connections = vec![AccountProviderConnectionResult {
        provider_id: "zai-start".to_owned(),
        status: AccountProviderConnectionStatus::Available,
        unavailable_reason: None,
        current: None,
        connection_key: None,
        models: Some(vec![" a ".to_owned(), String::new(), "b".to_owned()]),
        effective_at: None,
        reset_previous: false,
    }];
    let providers = resolve_account_provider_configs(ResolveAccountProviderConfigsInput {
        configured_providers: &configured,
        previous_providers: &configured,
        connections: &connections,
    })
    .expect("overlay");
    assert_eq!(
        providers.to_json().to_json_string(),
        r#"[{"providerId":"zai-start","config":{"access":{"type":"zhipu-account","entitled":true},"builtinModelIds":["a","b"]}}]"#
    );
}

#[test]
fn states_keep_connection_order() {
    let providers = ProviderConfigMap::empty();
    let connections = vec![
        AccountProviderConnectionResult::simple(
            "second",
            AccountProviderConnectionStatus::Available,
        ),
        AccountProviderConnectionResult::simple(
            "first",
            AccountProviderConnectionStatus::Unavailable,
        ),
    ];
    let states = build_account_provider_states(&connections, &providers, None);
    assert_eq!(
        states.keys().cloned().collect::<Vec<_>>(),
        vec!["second".to_owned(), "first".to_owned()]
    );
}

#[test]
fn provider_config_state_serialization_order_is_stable() {
    let mut states: AccountProviderStates = IndexMap::new();
    states.insert(
        "p".to_owned(),
        AccountProviderState {
            availability: provider_config::AccountProviderAvailability::Unavailable,
            entitled: false,
            unavailable_reason: Some(AccountProviderUnavailableReason::NotConnected),
            current: Some(false),
            connection_key: Some("key".to_owned()),
            effective_at: Some(1),
        },
    );
    assert_eq!(
        serde_json::to_string(&states).expect("states"),
        r#"{"p":{"availability":"unavailable","entitled":false,"unavailableReason":"not-connected","current":false,"connectionKey":"key","effectiveAt":1}}"#
    );
}
