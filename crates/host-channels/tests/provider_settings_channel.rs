//! `provider-settings` 频道契约（真源 `packages/services/src/model-provider/providerFacadeServices.ts`）。
//!
//! 覆盖：View 投影可读、写面返回新 View、**校验失败不落盘**、`onDidChange` 只在真正
//! 写入后广播、两个外部能力（连通性/远端目录）的失败语义。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use host_channels::provider_settings_channel::{
    ProviderSettingsChannel, ProviderSettingsChannelOptions, ON_DID_CHANGE,
};
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "provider-settings-channel-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        TempDir(dir)
    }

    fn personal(&self) -> PathBuf {
        self.0.join("provider_config.json")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bundled_path() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/provider/mode-builtin.json")
        .to_string_lossy()
        .into_owned()
}

fn channel(dir: &TempDir) -> ProviderSettingsChannel {
    ProviderSettingsChannel::new(ProviderSettingsChannelOptions {
        bundled_config_path: bundled_path(),
        personal_config_path: dir.personal(),
        agent_control: None,
        on_log: None,
    })
    .expect("channel")
}

async fn call(channel: &ProviderSettingsChannel, method: &str, args: Vec<Json>) -> Json {
    let value = channel
        .call(
            "ctx".into(),
            method.into(),
            args_value(args),
            CancellationToken::default(),
        )
        .await
        .expect("call");
    let Value::Json(text) = value else {
        panic!("expected json result for {method}");
    };
    serde_json::from_str(&text).expect("json result")
}

/// `ProxyChannel` 把位置参数包成数组；这里按同一线格式构造。
fn args_value(args: Vec<Json>) -> Value {
    Value::Json(Json::Array(args).to_string())
}

async fn call_err(channel: &ProviderSettingsChannel, method: &str, args: Vec<Json>) -> String {
    let error = channel
        .call(
            "ctx".into(),
            method.into(),
            args_value(args),
            CancellationToken::default(),
        )
        .await
        .expect_err("expected error");
    format!("{error:?}")
}

#[tokio::test]
async fn get_view_projects_bundled_templates_with_monotonic_revision() {
    let dir = TempDir::new("view");
    let channel = channel(&dir);
    let first = call(&channel, "getView", vec![]).await;
    assert!(first["revision"].as_u64().unwrap_or(0) >= 1);
    assert!(first["providerTemplates"].is_array());
    assert!(first["providerOrder"].is_array());
    assert!(first["providers"].is_array());
    // 无 Personal 文件时不得创建它（读面纯读）。
    assert!(!dir.personal().exists());
    let second = call(&channel, "getView", vec![]).await;
    assert_eq!(
        second["revision"], first["revision"],
        "快照未变时 revision 不推进"
    );
}

#[tokio::test]
async fn create_writes_provider_and_returns_new_view() {
    let dir = TempDir::new("create");
    let channel = channel(&dir);
    let created = call(&channel, "createPersonalProvider", vec![json!({})]).await;
    let provider_id = created["providerId"]
        .as_str()
        .expect("providerId")
        .to_string();
    assert!(dir.personal().exists(), "写入必须落盘");
    let view = &created["view"];
    assert!(
        view["providers"]
            .as_array()
            .expect("providers")
            .iter()
            .any(|item| item["providerId"] == provider_id.as_str()),
        "新 provider 必须出现在返回的 View 里"
    );

    // 删除后 View 不再包含它（且不会因为「不存在」而失败以外的事情发生）。
    let view = call(
        &channel,
        "deletePersonalProvider",
        vec![json!(provider_id.clone())],
    )
    .await;
    assert!(!view["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .any(|item| item["providerId"] == provider_id.as_str()));
}

#[tokio::test]
async fn failed_validation_does_not_touch_the_file() {
    let dir = TempDir::new("invalid");
    let channel = channel(&dir);
    // 不存在的 provider：必须报错，且不能创建/改写 Personal 文件。
    let error = call_err(
        &channel,
        "savePersonalProviderOverlay",
        vec![json!("missing-provider"), json!({})],
    )
    .await;
    assert!(
        error.contains("尚未创建") || error.contains("Provider 不存在"),
        "unexpected error: {error}"
    );
    assert!(
        !dir.personal().exists(),
        "校验失败不得落盘（否则会触发一轮无意义的 onDidChange）"
    );
}

#[tokio::test]
async fn on_did_change_fires_only_after_a_real_write() {
    let dir = TempDir::new("event");
    let channel = channel(&dir);
    let events: Arc<Mutex<Vec<Json>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let subscription = channel
        .listen(
            "ctx".into(),
            ON_DID_CHANGE.into(),
            Value::Undefined,
            Arc::new(move |value: Value| {
                if let Value::Json(text) = value {
                    if let Ok(view) = serde_json::from_str::<Json>(&text) {
                        sink.lock().unwrap().push(view);
                    }
                }
            }),
        )
        .expect("listen");

    // 失败写入不广播。
    let _ = call_err(
        &channel,
        "savePersonalProviderOverlay",
        vec![json!("not-exists"), json!({})],
    )
    .await;
    assert!(events.lock().unwrap().is_empty(), "失败写入不应广播");

    // 成功写入广播一次带新 View 的事件。
    let _ = call(&channel, "createPersonalProvider", vec![json!({})]).await;
    let seen = events.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "成功写入应广播一次");
    assert!(seen[0]["providers"].is_array());

    // 订阅释放函数必须显式调用（`Subscription` 是闭包，drop 不会自动退订）。
    subscription();
    let _ = call(&channel, "createPersonalProvider", vec![json!({})]).await;
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "退订后不再广播（订阅释放幂等）"
    );
}

#[tokio::test]
async fn test_model_connectivity_reports_unavailable_before_reaching_the_agent() {
    let dir = TempDir::new("connectivity");
    let channel = channel(&dir);
    // 不存在的 provider：TS 语义是返回 success:false + code，而不是抛错。
    let result = call(
        &channel,
        "testModelConnectivity",
        vec![json!({
            "workspacePath": "C:/ws",
            "providerId": "missing-provider",
            "modelId": "missing-model",
        })],
    )
    .await;
    assert_eq!(result["success"], false);
    assert_eq!(result["error"]["code"], "provider-unavailable");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("currently unavailable"),
        "unexpected message: {result:?}"
    );

    // 新建的空白 provider 未启用/不可执行：同样是 provider-unavailable，且不落盘校验前置
    // 不会触达 agent（本用例未注入控制面，一旦走到那一步会显式报错）。
    let created = call(&channel, "createPersonalProvider", vec![json!({})]).await;
    let provider_id = created["providerId"]
        .as_str()
        .expect("providerId")
        .to_string();
    let result = call(
        &channel,
        "testModelConnectivity",
        vec![json!({
            "workspacePath": "C:/ws",
            "providerId": provider_id,
            "modelId": "m1",
        })],
    )
    .await;
    assert_eq!(result["success"], false);
    assert!(
        result["error"]["code"].is_string(),
        "unexpected result: {result:?}"
    );

    // 缺字段：显式报错（不做默认值兜底）。
    let error = call_err(&channel, "testModelConnectivity", vec![json!({})]).await;
    assert!(
        error.contains("providerId is required"),
        "unexpected: {error}"
    );
}

#[tokio::test]
async fn resolve_model_config_without_draft_reports_inherited_equal_to_effective() {
    // TS 无 personalConfig 分支：inheritedConfig 与 effectiveConfig 是同一份
    // composeEffective(builtin, personal) 结果；把它写成「只取 builtin」会让设置页在
    // 有 Personal 覆盖时预览到错误的继承值。
    let dir = TempDir::new("resolve-inherited");
    let channel = channel(&dir);
    let created = call(&channel, "createPersonalProvider", vec![json!({})]).await;
    let provider_id = created["providerId"]
        .as_str()
        .expect("providerId")
        .to_string();

    let resolution = call(
        &channel,
        "resolveModelConfig",
        vec![json!({ "providerId": provider_id, "modelId": "m1" })],
    )
    .await;
    assert_eq!(
        resolution["inheritedConfig"], resolution["effectiveConfig"],
        "无草稿时两个配置必须同源"
    );
}

#[tokio::test]
async fn resolve_model_config_prefixes_issue_paths_with_the_model_locator() {
    // TS 传 `validateComplete(["providers", providerId, "models", modelId])`：问题路径必须
    // 带定位前缀，否则 renderer 把每个问题都挂到根节点（行内提示全部错位）。
    let dir = TempDir::new("resolve");
    let channel = channel(&dir);
    let created = call(&channel, "createPersonalProvider", vec![json!({})]).await;
    let provider_id = created["providerId"]
        .as_str()
        .expect("providerId")
        .to_string();

    let resolution = call(
        &channel,
        "resolveModelConfig",
        vec![json!({ "providerId": provider_id, "modelId": "m1" })],
    )
    .await;
    // 空白 provider 的 m1 必然不完整 → 至少有 issues；重点是路径形状。
    let issues = resolution["issues"].as_array().expect("issues");
    assert!(!issues.is_empty(), "空白 Provider 的模型不应通过完整校验");
    for issue in issues {
        let path = issue["path"].as_array().expect("path");
        assert_eq!(path[0], "providers");
        assert_eq!(path[1], provider_id.as_str());
        assert_eq!(path[2], "models");
        assert_eq!(path[3], "m1");
        assert!(path.len() >= 5, "问题路径应续接字段段: {path:?}");
    }
}

#[tokio::test]
async fn list_remote_models_rejects_unsupported_api_type_without_touching_config() {
    let dir = TempDir::new("remote");
    let channel = channel(&dir);
    let result = call(
        &channel,
        "listRemoteModels",
        vec![json!({
            "apiType": "nope",
            "baseUrl": "https://example.invalid/v1",
        })],
    )
    .await;
    assert_eq!(result["success"], false);
    assert!(
        result["message"]
            .as_str()
            .unwrap_or_default()
            .contains("不支持的 Provider API 类型"),
        "unexpected message: {result:?}"
    );
    assert!(!dir.personal().exists(), "纯读操作不得写配置");
}

#[tokio::test]
async fn unknown_command_is_rejected() {
    let dir = TempDir::new("unknown");
    let channel = channel(&dir);
    let error = call_err(&channel, "nope", vec![]).await;
    assert!(
        error.contains("unknown command"),
        "unexpected error: {error}"
    );
}
