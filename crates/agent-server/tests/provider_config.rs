//! Provider 配置与凭据解析（Task 4b）。
//!
//! 覆盖面：
//! - `provider/updateAccountConfig` 的严格解析、revision 去重与回执
//!   `{receivedRevision, providerCount, status}`（回执版本恒等于交付版本，发送方
//!   才会把该 revision 标记为已交付）；
//! - 账号 Overlay（内存快照）+ Personal/Built-in 文件层经 `provider-resolver` 合并后，
//!   `providerId/modelId` 解析出 base URL、API key、apiFormat 与模型能力；
//! - 缺 key / 缺 URL / 不支持的 apiFormat（Anthropic/Responses 属 C2）显式报错；
//! - `Debug`/`Display` 永不打印 API key。
//!
//! 本文件全部测试只读临时目录与内存快照，不创建任何网络连接；resolved base URL
//! 是测试自写的 mock 端点，官方平台端点（api.z.ai / open.bigmodel.cn 等）断言不出现。

use std::path::{Path, PathBuf};

use agent_server::{AgentServer, ProviderConfigError, ProviderConfigHub, ResolvedProvider};
use model_selection::{ModelSelection, SelectionOptions};
use provider_http::OpenAiChatConfig;
use serde_json::{json, Value};

/// 官方平台端点片段：resolved 端点与任何输出都不得出现（本模块不做网络请求）。
const OFFICIAL_ENDPOINT_FRAGMENTS: [&str; 5] = [
    "api.z.ai",
    "open.bigmodel.cn",
    "api.moonshot.cn",
    "api.openai.com",
    "openrouter.ai",
];

fn temp_root(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-server-provider-config-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 最小 Built-in Release：一个 zai-family 账号族 provider `acct-chat`，模型白名单
/// 交给账号 Overlay 下发（`builtinModelIds` 留空），用于钉住「模型只在账号快照
/// 交付后出现」的合并事实。
fn write_builtin(root: &Path) -> String {
    let path = root.join("mode-builtin.json");
    std::fs::write(
        &path,
        r#"{
          "schemaVersion": 1,
          "revision": 7,
          "config": {
            "providerConfigRules": {
              "templateRules": [],
              "providerRules": [
                {
                  "providerId": "acct-chat",
                  "config": {
                    "group": "zai-family",
                    "access": { "type": "zhipu-account", "accountType": "bigmodel", "mode": "start-plan" },
                    "api": { "type": "openai-chat-completions", "baseUrl": "https://acct.example/api" }
                  }
                }
              ]
            },
            "modelConfigRules": {
              "modelRules": [],
              "modelApiRules": [],
              "providerSiteRules": [],
              "templateModelRules": [],
              "builtinProviderModelRules": []
            }
          }
        }"#,
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

fn personal_provider_rule(
    provider_id: &str,
    api_type: &str,
    base_url: Option<&str>,
    access: Value,
) -> Value {
    let mut api = json!({ "type": api_type });
    if let Some(base_url) = base_url {
        api["baseUrl"] = json!(base_url);
    }
    json!({
        "providerId": provider_id,
        "providerName": "Mock Chat",
        "enabled": true,
        "config": {
            "group": "standard-personal",
            "visibility": "visible",
            "access": access,
            "personalModelIds": ["mock-1"],
            "api": api
        }
    })
}

fn personal_model_rules(provider_id: &str, model_id: &str) -> Value {
    json!({
        "providerModelRules": [
            {
                "providerId": provider_id,
                "modelId": model_id,
                "config": {
                    "enabled": true,
                    "properties": {
                        "contextWindow": 200000,
                        "supportsToolCall": true,
                        "inputFormat": { "supportsText": true },
                        "outputFormat": { "supportsText": true }
                    },
                    "optionSpecs": {
                        "reasoningLevel": { "values": ["disabled", "enabled"] },
                        "maxOutputTokens": { "max": 8192 }
                    }
                }
            }
        ],
        "manualProviderModelRules": []
    })
}

fn write_personal_file(path: &Path, provider_rules: &[Value], model_rules: Value) {
    let document = json!({
        "schemaVersion": 1,
        "config": {
            "providerConfigRules": { "providerRules": provider_rules },
            "modelConfigRules": model_rules
        }
    });
    std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

fn empty_model_rules() -> Value {
    json!({
        "providerModelRules": [],
        "manualProviderModelRules": []
    })
}

fn api_key_access(api_key: Value) -> Value {
    let mut access = json!({ "type": "api-key" });
    if !api_key.is_null() {
        access["apiKey"] = api_key;
    }
    access
}

fn selection(provider_id: &str, model_id: &str) -> ModelSelection {
    ModelSelection {
        provider_id: provider_id.to_owned(),
        model_id: model_id.to_owned(),
        options: Some(SelectionOptions {
            reasoning_level: Some("disabled".to_owned()),
        }),
    }
}

fn account_delivery(revision: &str, providers: Value, states: Value) -> Value {
    json!({
        "revision": revision,
        "basedOnModeBuiltinRevision": "7",
        "providers": providers,
        "states": states
    })
}

fn account_snapshot_delivery(revision: &str) -> Value {
    account_delivery(
        revision,
        json!({
            "acct-chat": {
                "access": { "type": "zhipu-account", "entitled": true },
                "builtinModelIds": ["acct-model"]
            }
        }),
        json!({
            "acct-chat": {
                "availability": "available",
                "entitled": true,
                "current": true
            }
        }),
    )
}

// ---------------------------------------------------------------------------
// provider/updateAccountConfig：严格解析、revision 去重、回执
// ---------------------------------------------------------------------------

#[test]
fn account_config_receipt_echoes_delivered_revision() {
    let hub = ProviderConfigHub::with_paths(
        "D:/definitely-missing-builtin.json",
        Path::new("D:/definitely-missing-personal.json"),
    );
    // 回执版本恒等于交付版本：发送方只在 `receivedRevision == revision` 时才把该
    // revision 标记为已交付；不一致 → 报错且不标记（见 agent-process.md 交付节）。
    let receipt = hub
        .handle_account_config(account_snapshot_delivery("acct-rev-1"))
        .expect("合法账号快照必须被接受");
    assert_eq!(
        receipt,
        json!({ "receivedRevision": "acct-rev-1", "providerCount": 1, "status": "received" })
    );

    let receipt = hub
        .handle_account_config(account_delivery("acct-rev-1", json!({}), json!({})))
        .expect("重复 revision 去重");
    assert_eq!(
        receipt,
        json!({ "receivedRevision": "acct-rev-1", "providerCount": 1, "status": "unchanged" })
    );
}

#[test]
fn account_config_dedupe_does_not_replace_snapshot() {
    let hub = ProviderConfigHub::with_paths(
        "D:/definitely-missing-builtin.json",
        Path::new("D:/definitely-missing-personal.json"),
    );
    hub.handle_account_config(account_snapshot_delivery("acct-rev-1"))
        .expect("首次交付");
    // 同 revision 重投（内容不同）→ unchanged，且 providerCount 取已存储快照
    // （1），不是重投参数里的 0：去重路径不得替换快照。
    let receipt = hub
        .handle_account_config(account_delivery("acct-rev-1", json!({}), json!({})))
        .expect("重复交付去重");
    assert_eq!(receipt["status"], json!("unchanged"));
    assert_eq!(receipt["providerCount"], json!(1));
    // 新 revision → received，providerCount 用本次交付的数量。
    let receipt = hub
        .handle_account_config(account_delivery("acct-rev-2", json!({}), json!({})))
        .expect("新 revision 交付");
    assert_eq!(
        receipt,
        json!({ "receivedRevision": "acct-rev-2", "providerCount": 0, "status": "received" })
    );
}

#[test]
fn failed_delivery_keeps_previous_snapshot_and_does_not_mark_revision() {
    let hub = ProviderConfigHub::with_paths(
        "D:/definitely-missing-builtin.json",
        Path::new("D:/definitely-missing-personal.json"),
    );
    hub.handle_account_config(account_snapshot_delivery("acct-rev-1"))
        .expect("首次交付");

    // 非法交付（provider 配置不是对象）→ 显式错误；已存储快照与 revision 不受影响。
    let error = hub
        .handle_account_config(account_delivery(
            "acct-rev-2",
            json!({ "acct-chat": "not-an-object" }),
            json!({}),
        ))
        .expect_err("非法 provider 配置必须被拒绝");
    assert!(
        matches!(error, ProviderConfigError::InvalidParams { .. }),
        "实际错误：{error:?}"
    );

    // 失败的 rev-2 没有被标记为已交付：rev-1 仍按去重返回 unchanged，
    // 重投合法 rev-2 时仍是 received。
    let receipt = hub
        .handle_account_config(account_snapshot_delivery("acct-rev-1"))
        .expect("已接受 revision 不受失败交付影响");
    assert_eq!(receipt["status"], json!("unchanged"));
    let receipt = hub
        .handle_account_config(account_delivery("acct-rev-2", json!({}), json!({})))
        .expect("失败交付后合法重投");
    assert_eq!(receipt["status"], json!("received"));
}

#[test]
fn invalid_account_config_params_are_rejected() {
    let hub = ProviderConfigHub::with_paths(
        "D:/definitely-missing-builtin.json",
        Path::new("D:/definitely-missing-personal.json"),
    );
    for params in [
        // 缺 revision。
        json!({
            "basedOnModeBuiltinRevision": "7",
            "providers": {},
            "states": {}
        }),
        // 信封未知键。
        json!({
            "revision": "r",
            "basedOnModeBuiltinRevision": "7",
            "providers": {},
            "states": {},
            "unexpected": true
        }),
        // providers 不是对象。
        json!({
            "revision": "r",
            "basedOnModeBuiltinRevision": "7",
            "providers": [],
            "states": {}
        }),
        // Account 模式 provider 配置不允许 api-key access（只能 zhipu-account）。
        json!({
            "revision": "r",
            "basedOnModeBuiltinRevision": "7",
            "providers": { "acct-chat": { "access": { "type": "api-key", "apiKey": "k" } } },
            "states": {}
        }),
        // states 条目缺 availability。
        json!({
            "revision": "r",
            "basedOnModeBuiltinRevision": "7",
            "providers": {},
            "states": { "acct-chat": { "entitled": true } }
        }),
        // 参数不是对象。
        json!(null),
    ] {
        let error = hub
            .handle_account_config(params.clone())
            .expect_err("非法参数必须被拒绝");
        assert!(
            matches!(error, ProviderConfigError::InvalidParams { .. }),
            "params={params}, 实际错误：{error:?}"
        );
        assert_eq!(error.code(), -32602, "params={params}");
    }
}

// ---------------------------------------------------------------------------
// resolve：账号 Overlay + 文件层合并
// ---------------------------------------------------------------------------

#[test]
fn account_overlay_delivery_makes_builtin_family_model_resolvable() {
    let root = temp_root("acct-merge");
    let builtin = write_builtin(&root);
    // Personal 文件不存在 → 空层；账号快照是唯一的动态层。
    let personal = root.join("provider_config.json");
    let hub = ProviderConfigHub::with_paths(builtin, &personal);

    // 未交付账号快照：builtin 没有白名单，模型不存在。
    let error = hub
        .resolve(&selection("acct-chat", "acct-model"))
        .expect_err("账号快照交付前模型不存在");
    assert!(matches!(error, ProviderConfigError::ModelNotFound { .. }));
    assert!(error.to_string().contains("acct-chat"));
    assert!(error.to_string().contains("acct-model"));

    // 交付账号快照：模型白名单来自 Overlay，base URL 来自 Built-in 层。
    hub.handle_account_config(account_snapshot_delivery("acct-rev-1"))
        .expect("合法账号快照");
    // zhipu-account access 没有进程内 API key（OAuth 兑换属后续任务）→ 显式报错，
    // 但 base URL 已能从合并结果取出（错误只带 provider/model 与缺失字段）。
    let error = hub
        .resolve(&selection("acct-chat", "acct-model"))
        .expect_err("账号族没有进程内 API key");
    let message = error.to_string();
    assert!(message.contains("acct-chat"), "message={message}");
    assert!(message.contains("acct-model"), "message={message}");
    assert!(message.contains("apiKey"), "message={message}");

    // Personal 层覆盖 access（api-key）→ 解析成功：端点来自 Built-in，凭据来自 Personal。
    write_personal_file(
        &personal,
        &[json!({
            "providerId": "acct-chat",
            "enabled": true,
            "config": { "access": api_key_access(json!("acct-key")) }
        })],
        empty_model_rules(),
    );
    let resolved = hub
        .resolve(&selection("acct-chat", "acct-model"))
        .expect("personal 覆盖 access 后可解析");
    assert_eq!(resolved.provider_id, "acct-chat");
    assert_eq!(resolved.model_id, "acct-model");
    assert_eq!(resolved.base_url, "https://acct.example/api");
    assert_eq!(resolved.api_key, "acct-key");
    // 无模型规则 → 能力缺省（不编数字）。
    assert_eq!(resolved.context_window, None);
    assert_eq!(resolved.max_output_tokens, None);
}

#[test]
fn resolve_chat_completions_personal_provider_with_capabilities() {
    let root = temp_root("chat-resolve");
    let builtin = write_builtin(&root);
    let personal = root.join("provider_config.json");
    write_personal_file(
        &personal,
        &[personal_provider_rule(
            "mock-chat",
            "openai-chat-completions",
            Some("http://127.0.0.1:9/v1"),
            api_key_access(json!("mock-key-abc")),
        )],
        personal_model_rules("mock-chat", "mock-1"),
    );
    let hub = ProviderConfigHub::with_paths(builtin, &personal);

    let resolved = hub
        .resolve(&selection("mock-chat", "mock-1"))
        .expect("personal chat-completions provider 可解析");
    assert_eq!(resolved.provider_id, "mock-chat");
    assert_eq!(resolved.model_id, "mock-1");
    // mock 端点是测试自写的本地地址；官方端点不出现（本模块无网络请求）。
    assert_eq!(resolved.base_url, "http://127.0.0.1:9/v1");
    assert_eq!(resolved.api_key, "mock-key-abc");
    assert_eq!(resolved.context_window, Some(200_000));
    assert_eq!(resolved.max_output_tokens, Some(8192));
    for fragment in OFFICIAL_ENDPOINT_FRAGMENTS {
        assert!(
            !resolved.base_url.contains(fragment),
            "resolved 端点不得指向官方平台: {}",
            resolved.base_url
        );
    }
}

#[test]
fn resolve_reports_missing_api_key_and_base_url_explicitly() {
    let root = temp_root("missing-fields");
    let builtin = write_builtin(&root);
    let personal = root.join("provider_config.json");
    let hub = ProviderConfigHub::with_paths(builtin, &personal);

    // 缺 apiKey。
    write_personal_file(
        &personal,
        &[personal_provider_rule(
            "mock-chat",
            "openai-chat-completions",
            Some("http://127.0.0.1:9/v1"),
            api_key_access(json!(null)),
        )],
        personal_model_rules("mock-chat", "mock-1"),
    );
    let error = hub
        .resolve(&selection("mock-chat", "mock-1"))
        .expect_err("缺 API key 必须显式报错");
    let message = error.to_string();
    assert!(message.contains("mock-chat"), "message={message}");
    assert!(message.contains("mock-1"), "message={message}");
    assert!(message.contains("apiKey"), "message={message}");

    // 空白 apiKey 同样按缺失处理。
    write_personal_file(
        &personal,
        &[personal_provider_rule(
            "mock-chat",
            "openai-chat-completions",
            Some("http://127.0.0.1:9/v1"),
            api_key_access(json!("   ")),
        )],
        personal_model_rules("mock-chat", "mock-1"),
    );
    let error = hub
        .resolve(&selection("mock-chat", "mock-1"))
        .expect_err("空白 API key 必须显式报错");
    assert!(error.to_string().contains("apiKey"));

    // 缺 baseUrl。
    write_personal_file(
        &personal,
        &[personal_provider_rule(
            "mock-chat",
            "openai-chat-completions",
            None,
            api_key_access(json!("mock-key-abc")),
        )],
        personal_model_rules("mock-chat", "mock-1"),
    );
    let error = hub
        .resolve(&selection("mock-chat", "mock-1"))
        .expect_err("缺 base URL 必须显式报错");
    let message = error.to_string();
    assert!(message.contains("mock-chat"), "message={message}");
    assert!(message.contains("mock-1"), "message={message}");
    assert!(message.contains("baseUrl"), "message={message}");
}

#[test]
fn resolve_rejects_unsupported_api_formats() {
    let root = temp_root("unsupported-format");
    let builtin = write_builtin(&root);
    let personal = root.join("provider_config.json");
    let hub = ProviderConfigHub::with_paths(builtin, &personal);

    for api_type in ["anthropic-messages", "openai-responses"] {
        write_personal_file(
            &personal,
            &[personal_provider_rule(
                "mock-chat",
                api_type,
                Some("http://127.0.0.1:9/v1"),
                api_key_access(json!("mock-key-abc")),
            )],
            personal_model_rules("mock-chat", "mock-1"),
        );
        let error = hub
            .resolve(&selection("mock-chat", "mock-1"))
            .expect_err("Anthropic/Responses 属 C2，C1 显式报不支持");
        assert!(
            matches!(error, ProviderConfigError::UnsupportedApiFormat { .. }),
            "实际错误：{error:?}"
        );
        let message = error.to_string();
        assert!(message.contains("mock-chat"), "message={message}");
        assert!(message.contains("mock-1"), "message={message}");
        assert!(message.contains(api_type), "message={message}");
    }
}

#[test]
fn resolve_reports_missing_provider_and_model_explicitly() {
    let root = temp_root("not-found");
    let builtin = write_builtin(&root);
    let personal = root.join("provider_config.json");
    write_personal_file(
        &personal,
        &[personal_provider_rule(
            "mock-chat",
            "openai-chat-completions",
            Some("http://127.0.0.1:9/v1"),
            api_key_access(json!("mock-key-abc")),
        )],
        personal_model_rules("mock-chat", "mock-1"),
    );
    let hub = ProviderConfigHub::with_paths(builtin, &personal);

    let error = hub
        .resolve(&selection("ghost", "mock-1"))
        .expect_err("未知 provider 显式报错");
    assert!(matches!(
        error,
        ProviderConfigError::ProviderNotFound { .. }
    ));
    assert!(error.to_string().contains("ghost"));

    let error = hub
        .resolve(&selection("mock-chat", "ghost-model"))
        .expect_err("未知 model 显式报错");
    assert!(matches!(error, ProviderConfigError::ModelNotFound { .. }));
    assert!(error.to_string().contains("mock-chat"));
    assert!(error.to_string().contains("ghost-model"));
}

// ---------------------------------------------------------------------------
// 安全面：Debug / Display 不打印 API key；错误文案只带 provider/model 与缺失字段
// ---------------------------------------------------------------------------

#[test]
fn resolved_provider_never_prints_api_key() {
    let resolved = ResolvedProvider {
        provider_id: "mock-chat".to_owned(),
        model_id: "mock-1".to_owned(),
        base_url: "http://127.0.0.1:9/v1".to_owned(),
        api_key: "super-secret-key".to_owned(),
        api_format: agent_server::ApiFormat::OpenAiChatCompletions,
        max_output_tokens: Some(8192),
        context_window: Some(200_000),
        headers: vec![("Authorization".to_owned(), "super-secret-key".to_owned())],
    };
    for rendered in [format!("{resolved:?}"), resolved.to_string()] {
        assert!(
            !rendered.contains("super-secret-key"),
            "rendered={rendered}"
        );
        assert!(rendered.contains("mock-chat"), "rendered={rendered}");
        assert!(rendered.contains("127.0.0.1:9/v1"), "rendered={rendered}");
    }
    // 映射到 provider-http 适配器后同样不打印（镜像 OpenAiChatConfig 的掩码）。
    let config: OpenAiChatConfig = resolved.to_openai_chat_config().expect("chat-completions");
    let rendered = format!("{config:?}");
    assert!(
        !rendered.contains("super-secret-key"),
        "rendered={rendered}"
    );
}

#[test]
fn error_text_only_carries_provider_model_and_missing_field() {
    let resolved = ResolvedProvider {
        provider_id: "mock-chat".to_owned(),
        model_id: "mock-1".to_owned(),
        base_url: "http://127.0.0.1:9/v1".to_owned(),
        api_key: "unused".to_owned(),
        api_format: agent_server::ApiFormat::AnthropicMessages,
        max_output_tokens: None,
        context_window: None,
        headers: Vec::new(),
    };
    let error = resolved
        .to_openai_chat_config()
        .expect_err("Anthropic 属 C2");
    for rendered in [format!("{error:?}"), error.to_string()] {
        assert!(!rendered.contains("unused"), "rendered={rendered}");
        assert!(rendered.contains("mock-chat"), "rendered={rendered}");
        assert!(rendered.contains("mock-1"), "rendered={rendered}");
        assert!(
            rendered.contains("anthropic-messages"),
            "rendered={rendered}"
        );
    }
}

#[test]
fn hub_debug_never_prints_delivered_content() {
    let hub = ProviderConfigHub::with_paths(
        "D:/definitely-missing-builtin.json",
        Path::new("D:/definitely-missing-personal.json"),
    );
    hub.handle_account_config(account_snapshot_delivery("acct-rev-1"))
        .expect("合法账号快照");
    let rendered = format!("{hub:?}");
    assert!(!rendered.contains("mock-key"), "rendered={rendered}");
    assert!(rendered.contains("acct-rev-1"), "rendered={rendered}");
}

// ---------------------------------------------------------------------------
// provider-http 适配映射
// ---------------------------------------------------------------------------

#[test]
fn to_openai_chat_config_maps_endpoint_headers_and_limits() {
    let resolved = ResolvedProvider {
        provider_id: "mock-chat".to_owned(),
        model_id: "mock-1".to_owned(),
        base_url: "http://127.0.0.1:9/v1/".to_owned(),
        api_key: "mock-key-abc".to_owned(),
        api_format: agent_server::ApiFormat::OpenAiChatCompletions,
        max_output_tokens: Some(8192),
        context_window: Some(200_000),
        headers: vec![
            ("X-Custom".to_owned(), "1".to_owned()),
            ("Authorization".to_owned(), "Bearer gateway".to_owned()),
        ],
    };
    let config = resolved.to_openai_chat_config().expect("chat-completions");
    assert_eq!(config.base_url, "http://127.0.0.1:9/v1/");
    assert_eq!(config.api_key, "mock-key-abc");
    assert_eq!(config.model, "mock-1");
    assert_eq!(config.max_output_tokens, Some(8192));
    assert_eq!(config.extra_headers.len(), 2);
    assert_eq!(config.extra_headers[1].1, "Bearer gateway");
}

// ---------------------------------------------------------------------------
// 分派接线：provider/updateAccountConfig 进入 hub，其余方法保持 -32601
// ---------------------------------------------------------------------------

#[test]
fn dispatch_wires_provider_update_account_config() {
    let server = AgentServer::new();
    let receipt = server
        .dispatch(
            "provider/updateAccountConfig",
            account_snapshot_delivery("acct-rev-1"),
        )
        .expect("合法交付走 hub");
    assert_eq!(
        receipt,
        json!({ "receivedRevision": "acct-rev-1", "providerCount": 1, "status": "received" })
    );

    // 重复交付去重仍从分派层可用。
    let receipt = server
        .dispatch(
            "provider/updateAccountConfig",
            account_snapshot_delivery("acct-rev-1"),
        )
        .expect("去重交付");
    assert_eq!(receipt["status"], json!("unchanged"));

    // 非法参数 → -32602。
    let error = server
        .dispatch("provider/updateAccountConfig", json!({ "revision": 1 }))
        .expect_err("非法参数");
    assert_eq!(error.code(), -32602);

    // compact / goal 明确不在 C1 方法表内，保持显式 -32601（Task 7 接入其余 session 面）。
    for method in [
        "session/compact",
        "session/goal",
        "provider/unknown",
        "provider/updatePersonalConfig",
    ] {
        let error = server
            .dispatch(method, json!({}))
            .expect_err("未接入方法必须显式未实现");
        assert_eq!(error.code(), -32601, "method={method}");
    }
}
