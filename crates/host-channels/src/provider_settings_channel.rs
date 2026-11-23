//! `provider-settings` 频道：设置页 Provider/Model 唯一写面的 RPC 适配器。
//!
//! 规范见 `docs/specs/provider-settings-channel.md` §5。纯投影与写入语义在
//! [`crate::provider_settings`]；本模块只做位置参数解析、锁内读-改-写编排、
//! 新 View 回填与两个外部能力（连通性测试 / 远端模型目录）。
//!
//! 写入的单一所有者仍是 `PersonalConfigRepository`（跨进程锁 + 原子写）：
//! 校验失败必须**不落盘**（`try_update`），否则一次非法输入会重写文件并触发
//! 一轮无意义的 `onDidChange`。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use agent_process::{AgentControlClient, AgentWorkspaceTarget};
use provider_config_file_codec::ProviderConfigLayerUpdate;
use provider_config_repository::{
    ChangeSubscription, PersonalConfigRepository, PersonalConfigRepositoryOptions, PollingInterval,
};
use provider_config_source::{read_snapshot, PersonalLayer, ProviderConfigSnapshot};
use provider_resolver::{
    ProviderConfigResolution, ProviderConfigResolver, ProviderConfigResolverInput,
    ProviderModelSource, ResolvedProvider,
};
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

use crate::provider_settings::{
    add_personal_model, build_provider_settings_view, create_personal_provider,
    delete_personal_model, delete_personal_provider, rename_personal_model,
    reorder_personal_models, reorder_personal_providers, save_personal_model_draft,
    save_personal_provider_overlay, set_personal_model_enabled, CreatePersonalProviderInput,
    ProviderModelMembershipRef, SaveOverlayMetadata,
};

pub const PROVIDER_SETTINGS_CHANNEL: &str = "provider-settings";
pub const ON_DID_CHANGE: &str = "onDidChange";

type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ProviderSettingsChannelOptions {
    pub bundled_config_path: String,
    pub personal_config_path: PathBuf,
    /// 连通性测试的 agent 控制面；`None` 时该方法显式报「未装配」。
    pub agent_control: Option<Arc<AgentControlClient>>,
    pub on_log: Option<LogSink>,
}

pub struct ProviderSettingsChannel {
    inner: Arc<Inner>,
}

struct Inner {
    bundled_config_path: String,
    personal_config_path: PathBuf,
    repository: Arc<PersonalConfigRepository>,
    agent_control: Option<Arc<AgentControlClient>>,
    revision: AtomicU64,
    last_snapshot_revision: Mutex<Option<String>>,
    listeners: Mutex<HashMap<u64, EventCallback>>,
    next_listener: AtomicU64,
    change_subscription: Mutex<Option<ChangeSubscription>>,
    on_log: Option<LogSink>,
}

impl ProviderSettingsChannel {
    pub fn new(options: ProviderSettingsChannelOptions) -> Result<Self, String> {
        let repository = PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
            file_path: options.personal_config_path.clone(),
            import_legacy: None,
            on_recovery: None,
            on_polling_error: None,
            polling_interval: PollingInterval::Millis(1_000),
        })
        .map_err(|error| error.to_string())?;
        // 首次 read 启动轮询（与 model-selection / usage-stats 同口径）。
        let _ = repository.read();
        Ok(Self {
            inner: Arc::new(Inner {
                bundled_config_path: options.bundled_config_path,
                personal_config_path: options.personal_config_path,
                repository: Arc::new(repository),
                agent_control: options.agent_control,
                revision: AtomicU64::new(0),
                last_snapshot_revision: Mutex::new(None),
                listeners: Mutex::new(HashMap::new()),
                next_listener: AtomicU64::new(1),
                change_subscription: Mutex::new(None),
                on_log: options.on_log,
            }),
        })
    }
}

impl ServerChannel for ProviderSettingsChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let args = positional(&arg);
            match command.as_str() {
                "getView" | "refresh" => {
                    // TS `refresh(reason)` 先 refreshSources 再读；Rust 每次 getView 都
                    // 重新读文件，reason 只进日志（见 spec §5 差异 3）。
                    if command == "refresh" {
                        if let Some(reason) = args.first().and_then(Json::as_str) {
                            inner.log(format!("provider-settings refresh: {reason}"));
                        }
                    }
                    json_value(&inner.compute_view()?)
                }
                "createPersonalProvider" => {
                    let input = match args.first() {
                        Some(Json::Object(_)) => args[0].clone(),
                        _ => Json::Object(Default::default()),
                    };
                    let parsed = parse_create_input(&input)?;
                    let provider_id = inner.write(move |snapshot, current| {
                        create_personal_provider(snapshot, current, parsed)
                            .map(|created| (created.update, created.provider_id))
                    })?;
                    let view = inner.compute_view()?;
                    json_value(&json!({ "providerId": provider_id, "view": view }))
                }
                "resolveModelConfig" => {
                    let input = args.first().cloned().unwrap_or(Json::Null);
                    json_value(&resolve_model_config(&inner.read_snapshot()?, &input)?)
                }
                "savePersonalProviderOverlay" => {
                    let provider_id = arg_string(&args, 0, "providerId")?;
                    let config = parse_provider_config_arg(&args, 1)?;
                    let metadata = parse_overlay_metadata(args.get(2))?;
                    inner.write(move |snapshot, current| {
                        with_membership(snapshot, current, &provider_id, |membership| {
                            save_personal_provider_overlay(
                                snapshot,
                                current,
                                &provider_id,
                                &config,
                                metadata.as_ref(),
                                membership,
                            )
                            .map(|update| (update, ()))
                        })
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "deletePersonalProvider" => {
                    let provider_id = arg_string(&args, 0, "providerId")?;
                    inner.write(move |_snapshot, current| {
                        delete_personal_provider(current, &provider_id).map(|update| (update, ()))
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "reorderPersonalProviders" => {
                    let provider_ids = arg_string_list(&args, 0, "providerIds")?;
                    inner.write(move |_snapshot, current| {
                        Ok((reorder_personal_providers(current, &provider_ids), ()))
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "reorderPersonalModels" => {
                    let provider_id = arg_string(&args, 0, "providerId")?;
                    let model_ids = arg_string_list(&args, 1, "modelIds")?;
                    inner.write(move |snapshot, current| {
                        with_membership(snapshot, current, &provider_id, |membership| {
                            reorder_personal_models(
                                snapshot,
                                current,
                                &provider_id,
                                &model_ids,
                                membership,
                            )
                            .map(|update| (update, ()))
                        })
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "addPersonalModel" => {
                    let provider_id = arg_string(&args, 0, "providerId")?;
                    let model_id = arg_string(&args, 1, "modelId")?;
                    let config = parse_model_config_arg(&args, 2)?;
                    let use_recommended = args.get(3).and_then(Json::as_bool);
                    inner.write(move |snapshot, current| {
                        with_membership(snapshot, current, &provider_id, |membership| {
                            add_personal_model(
                                snapshot,
                                current,
                                &provider_id,
                                &model_id,
                                &config,
                                membership,
                                use_recommended,
                            )
                            .map(|update| (update, ()))
                        })
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "renamePersonalModel" => {
                    let provider_id = arg_string(&args, 0, "providerId")?;
                    let from = arg_string(&args, 1, "currentModelId")?;
                    let to = arg_string(&args, 2, "nextModelId")?;
                    inner.write(move |snapshot, current| {
                        with_membership(snapshot, current, &provider_id, |membership| {
                            rename_personal_model(
                                snapshot,
                                current,
                                &provider_id,
                                &from,
                                &to,
                                membership,
                            )
                            .map(|update| (update, ()))
                        })
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "deletePersonalModel" => {
                    let provider_id = arg_string(&args, 0, "providerId")?;
                    let model_id = arg_string(&args, 1, "modelId")?;
                    inner.write(move |snapshot, current| {
                        with_membership(snapshot, current, &provider_id, |membership| {
                            delete_personal_model(
                                snapshot,
                                current,
                                &provider_id,
                                &model_id,
                                membership,
                            )
                            .map(|update| (update, ()))
                        })
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "setPersonalModelEnabled" => {
                    let provider_id = arg_string(&args, 0, "providerId")?;
                    let model_id = arg_string(&args, 1, "modelId")?;
                    let enabled = args
                        .get(2)
                        .and_then(Json::as_bool)
                        .ok_or_else(|| ChannelError::error("enabled is required"))?;
                    inner.write(move |snapshot, current| {
                        with_membership(snapshot, current, &provider_id, |membership| {
                            set_personal_model_enabled(
                                snapshot,
                                current,
                                &provider_id,
                                &model_id,
                                enabled,
                                membership,
                            )
                            .map(|update| (update, ()))
                        })
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "savePersonalModelDraft" => {
                    let input = args.first().cloned().unwrap_or(Json::Null);
                    let draft = parse_draft_input(&input)?;
                    inner.write(move |snapshot, current| {
                        with_membership(snapshot, current, &draft.provider_id, |membership| {
                            save_personal_model_draft(
                                snapshot,
                                current,
                                &draft.provider_id,
                                &draft.original_model_id,
                                &draft.next_model_id,
                                &draft.config,
                                &current.revision,
                                draft.use_recommended_config,
                                membership,
                            )
                            .map(|update| (update, ()))
                        })
                    })?;
                    json_value(&inner.compute_view()?)
                }
                "testModelConnectivity" => {
                    let input = args.first().cloned().unwrap_or(Json::Null);
                    json_value(&inner.test_model_connectivity(&input).await?)
                }
                "listRemoteModels" => {
                    let input = args.first().cloned().unwrap_or(Json::Null);
                    json_value(&list_remote_models(&input).await?)
                }
                other => Err(ChannelError::error(format!(
                    "provider-settings channel: unknown command {other}"
                ))),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: Value,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        if event != ON_DID_CHANGE {
            return Err(ChannelError::error(format!(
                "provider-settings channel: unknown event {event}"
            )));
        }
        let id = self.inner.next_listener.fetch_add(1, Ordering::SeqCst);
        self.inner
            .listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(id, emit);
        self.inner.ensure_change_subscription();
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        Ok(Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner
                    .listeners
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .remove(&id);
            }
        }))
    }
}

impl Inner {
    fn log(&self, message: impl AsRef<str>) {
        if let Some(on_log) = &self.on_log {
            on_log(message.as_ref());
        }
    }

    fn ensure_change_subscription(self: &Arc<Self>) {
        {
            let mut guard = self
                .change_subscription
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if guard.is_some() {
                return;
            }
            let weak = Arc::downgrade(self);
            *guard = Some(self.repository.on_did_change(Arc::new(move |_revision| {
                if let Some(inner) = weak.upgrade() {
                    inner.broadcast();
                }
            })));
        }
    }

    fn broadcast(&self) {
        let view = match self.compute_view() {
            Ok(view) => view,
            Err(error) => {
                self.log(format!("provider-settings onDidChange 重建失败: {error:?}"));
                return;
            }
        };
        let text = match serde_json::to_string(&view) {
            Ok(text) => text,
            Err(error) => {
                self.log(format!("provider-settings onDidChange 序列化失败: {error}"));
                return;
            }
        };
        let listeners: Vec<EventCallback> = self
            .listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .values()
            .cloned()
            .collect();
        for listener in listeners {
            listener(Value::Json(text.clone()));
        }
    }

    /// Built-in 层（只读文件）+ 当前 Personal 文件的组合快照。
    fn read_snapshot(&self) -> Result<ProviderConfigSnapshot, ChannelError> {
        read_snapshot(&self.bundled_config_path, &self.personal_config_path, None)
            .map_err(|error| ChannelError::error(error.to_string()))
    }

    fn compute_view(&self) -> Result<Json, ChannelError> {
        let snapshot = self.read_snapshot()?;
        let revision = self.next_revision(&snapshot.revision);
        Ok(build_provider_settings_view(&snapshot, revision))
    }

    fn next_revision(&self, snapshot_revision: &str) -> u64 {
        let mut last = self
            .last_snapshot_revision
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if last.as_deref() != Some(snapshot_revision) {
            *last = Some(snapshot_revision.to_string());
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
        self.revision.load(Ordering::SeqCst).max(1)
    }

    /// 锁内读-改-写：变换拿到锁眼下的 Personal 层与组合快照，校验失败**不落盘**。
    fn write<T, F>(&self, transform: F) -> Result<T, ChannelError>
    where
        F: FnOnce(
            &ProviderConfigSnapshot,
            &PersonalLayer,
        ) -> Result<(ProviderConfigLayerUpdate, T), String>,
    {
        // Built-in 层与锁无关（只读文件）；Personal 层必须来自锁内。
        let builtin = self.read_snapshot()?;
        let captured: Mutex<Option<T>> = Mutex::new(None);
        let outcome = self
            .repository
            .try_update(|current| {
                let snapshot = with_personal_layer(&builtin, current);
                match transform(&snapshot, current) {
                    Ok((update, value)) => {
                        *captured.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(value);
                        Ok(update)
                    }
                    Err(message) => Err(message),
                }
            })
            .map_err(|error| ChannelError::error(error.to_string()))?;
        if let Err(message) = outcome {
            return Err(ChannelError::error(message));
        }
        captured
            .into_inner()
            .unwrap_or_else(|poison| poison.into_inner())
            .ok_or_else(|| ChannelError::error("provider-settings write produced no result"))
    }

    async fn test_model_connectivity(&self, input: &Json) -> Result<Json, ChannelError> {
        let provider_id = required_json_string(input, "providerId")?;
        let model_id = required_json_string(input, "modelId")?;
        let workspace_path = required_json_string(input, "workspacePath")?;
        // 只在操作收敛后的公共资格上判定（禁用/无效模型不进 Registry）。
        let view = self.compute_view()?;
        if let Some(code) = connectivity_unavailable(&view, &provider_id, &model_id) {
            let message = match code {
                "provider-unavailable" => {
                    "This provider is currently unavailable for connectivity testing."
                }
                _ => "This model is currently unavailable for connectivity testing.",
            };
            return Ok(json!({
                "success": false,
                "error": { "message": message, "code": code },
            }));
        }
        let Some(control) = self.agent_control.clone() else {
            return Err(ChannelError::error(
                "provider-settings channel: testModelConnectivity requires the agent control plane (not assembled)",
            ));
        };
        let mut target = AgentWorkspaceTarget::new(workspace_path);
        if let Some(identity) = input
            .get("workspaceIdentity")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            target = target.with_identity(identity.to_string());
        }
        let selection = model_selection::ModelSelection {
            provider_id,
            model_id,
            options: None,
        };
        match control.test_model_connectivity(&target, &selection).await {
            Ok(()) => Ok(json!({ "success": true })),
            Err(error) => Ok(json!({
                "success": false,
                "error": { "message": error.to_string() },
            })),
        }
    }
}

/// TS `testModelConnectivity` 的前置资格判定（`createProviderSettingsService` 同语义）。
fn connectivity_unavailable(
    view: &Json,
    provider_id: &str,
    model_id: &str,
) -> Option<&'static str> {
    let Some(providers) = view.get("providers").and_then(Json::as_array) else {
        return Some("provider-unavailable");
    };
    let Some(provider) = providers
        .iter()
        .find(|item| item.get("providerId").and_then(Json::as_str) == Some(provider_id))
    else {
        // View 里没有该 provider（hidden/不存在）：TS 语义同样是 provider-unavailable。
        return Some("provider-unavailable");
    };
    if provider.get("enabled").and_then(Json::as_bool) != Some(true) {
        return Some("provider-unavailable");
    }
    let model = provider
        .get("models")
        .and_then(Json::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|item| item.get("modelId").and_then(Json::as_str) == Some(model_id))
        });
    let Some(model) = model else {
        return Some("model-unavailable");
    };
    let has_issues = model
        .get("issues")
        .and_then(Json::as_array)
        .is_some_and(|issues| !issues.is_empty());
    if model.get("enabled").and_then(Json::as_bool) != Some(true) || has_issues {
        return Some("model-unavailable");
    }
    if provider.get("executable").and_then(Json::as_bool) != Some(true) {
        return Some("provider-unavailable");
    }
    if model.get("executable").and_then(Json::as_bool) != Some(true) {
        return Some("model-unavailable");
    }
    None
}

/// TS `resolveModelConfig`：无 `personalConfig` 走继承；有则临时 `setExact` 预览（不落盘）。
fn resolve_model_config(
    snapshot: &ProviderConfigSnapshot,
    input: &Json,
) -> Result<Json, ChannelError> {
    let provider_id = required_json_string(input, "providerId")?;
    let model_id = required_json_string(input, "modelId")?;
    let resolution = resolve_snapshot(snapshot);
    let provider = resolution
        .resolved_providers
        .iter()
        .find(|provider| provider.provider_id == provider_id)
        .ok_or_else(|| ChannelError::error(format!("Provider 不存在: {provider_id}")))?;
    let resolve_input = resolution_input(provider, &model_id);

    // 无 `personalConfig`：TS 用 composeEffective(builtin, personal) 解析，
    // inherited 与 effective 是同一份（草稿里没有未保存的自动配置）。
    // 有 `personalConfig`：inherited 只取 Built-in 规则，effective 才叠加草稿。
    let (inherited, effective) = match input.get("personalConfig").filter(|value| !value.is_null())
    {
        None => {
            let composed = provider_config::ModelConfigRules::compose_effective(
                &snapshot.mode_builtin_model_rules,
                &snapshot.personal_models,
            )
            .resolve(&resolve_input);
            (composed.clone(), composed)
        }
        Some(value) => {
            let personal = parse_model_config(value)?;
            let inherited = snapshot.mode_builtin_model_rules.resolve(&resolve_input);
            let original_model_id =
                json_string(input, "originalModelId").unwrap_or_else(|| model_id.clone());
            let mut personal_rules = snapshot.personal_models.clone();
            if original_model_id != model_id {
                personal_rules =
                    personal_rules.rename_exact_model(&provider_id, &original_model_id, &model_id);
            }
            // TS 这里固定传 `useRecommendedConfig = true`：草稿预览不沿用旧的固定标记。
            personal_rules =
                personal_rules.set_exact(&provider_id, &model_id, personal, Some(true));
            let effective = provider_config::ModelConfigRules::compose_effective(
                &snapshot.mode_builtin_model_rules,
                &personal_rules,
            )
            .resolve(&resolve_input);
            (inherited, effective)
        }
    };
    let issues = effective.validate_complete();
    Ok(json!({
        "inheritedConfig": to_serde(&inherited.to_json()),
        "effectiveConfig": to_serde(&effective.to_json()),
        // TS 用 `validateComplete(["providers", providerId, "models", modelId])`：
        // 问题路径带定位前缀，renderer 据此把问题挂到具体模型行。缺前缀会让所有
        // 问题路径都相对根节点，行内提示会全部落到错误位置。
        "issues": issues
            .iter()
            .map(|issue| issue_json(issue, &provider_id, &model_id))
            .collect::<Vec<_>>(),
    }))
}

fn resolution_input<'a>(
    provider: &'a ResolvedProvider,
    model_id: &'a str,
) -> provider_config::ModelConfigRuleResolutionInput<'a> {
    let (api_type, base_url) = match &provider.config.api {
        provider_config::Field::Value(api) => (
            api.api_type.value().map(|value| value.as_str()),
            api.base_url.value().map(String::as_str),
        ),
        _ => (None, None),
    };
    provider_config::ModelConfigRuleResolutionInput {
        provider_id: provider.provider_id.as_str(),
        template_id: provider.template_id.value().map(String::as_str),
        model_id,
        api_type,
        base_url,
    }
}

fn resolve_snapshot(snapshot: &ProviderConfigSnapshot) -> ProviderConfigResolution {
    let account_providers = provider_config::ProviderConfigMap::empty();
    ProviderConfigResolver.resolve(&ProviderConfigResolverInput {
        mode_builtin_providers: &snapshot.mode_builtin_providers,
        mode_builtin_provider_templates: Some(&snapshot.mode_builtin_provider_templates),
        personal_providers: &snapshot.personal_providers,
        mode_builtin_model_rules: &snapshot.mode_builtin_model_rules,
        personal_models: &snapshot.personal_models,
        account_providers: &account_providers,
        account_states: None,
        personal_provider_order: Some(&snapshot.personal_provider_order),
    })
}

/// TS `#modelMembership`：继承名单 = 该 provider 解析后的 Built-in 模型；revision 取锁内层。
///
/// 继承名单是本次调用算出来的，必须与 membership 同栈存活，因此用「带闭包」的形态，
/// 不把借用值泄漏到调用方。
fn with_membership<T>(
    snapshot: &ProviderConfigSnapshot,
    personal: &PersonalLayer,
    provider_id: &str,
    run: impl FnOnce(Option<&ProviderModelMembershipRef<'_>>) -> Result<T, String>,
) -> Result<T, String> {
    let resolution = resolve_snapshot(snapshot);
    let Some(provider) = resolution
        .resolved_providers
        .iter()
        .find(|provider| provider.provider_id == provider_id)
    else {
        // TS `#modelMembership` 在 provider 缺失时抛错；这里交给写函数自己的
        // 「Provider 不存在」判定，保证错误文案只有一处。
        return run(None);
    };
    let inherited: Vec<String> = provider
        .models
        .iter()
        .filter(|model| matches!(model.source, ProviderModelSource::Builtin))
        .map(|model| model.model_id.clone())
        .collect();
    let membership = ProviderModelMembershipRef {
        provider_id,
        inherited_model_ids: &inherited,
        personal_revision: &personal.revision,
    };
    run(Some(&membership))
}

fn to_serde(value: &provider_config::JsonValue) -> Json {
    serde_json::from_str(&value.to_json_string()).unwrap_or(Json::Null)
}

/// `resolveModelConfig` 的问题路径前缀：TS 传 `["providers", providerId, "models", modelId]`。
fn issue_json(issue: &provider_config::Issue, provider_id: &str, model_id: &str) -> Json {
    let mut path = vec![
        "providers".to_string(),
        provider_id.to_string(),
        "models".to_string(),
        model_id.to_string(),
    ];
    path.extend(issue.path.iter().cloned());
    json!({
        "code": issue.code,
        "path": path,
        "message": issue.message,
    })
}

fn parse_create_input(input: &Json) -> Result<CreatePersonalProviderInput, ChannelError> {
    Ok(CreatePersonalProviderInput {
        template_id: json_string(input, "templateId"),
        provider_name: json_string(input, "providerName"),
        locale: json_string(input, "locale"),
        initial_config: match input.get("initialConfig") {
            None | Some(Json::Null) => None,
            Some(value) => Some(parse_provider_config(value)?),
        },
    })
}

struct DraftInput {
    provider_id: String,
    original_model_id: String,
    next_model_id: String,
    config: provider_config::ModelConfig,
    use_recommended_config: Option<bool>,
}

fn parse_draft_input(input: &Json) -> Result<DraftInput, ChannelError> {
    Ok(DraftInput {
        provider_id: required_json_string(input, "providerId")?,
        original_model_id: required_json_string(input, "originalModelId")?,
        next_model_id: required_json_string(input, "nextModelId")?,
        config: parse_model_config(
            input
                .get("personalConfig")
                .filter(|value| !value.is_null())
                .ok_or_else(|| ChannelError::error("personalConfig is required"))?,
        )?,
        use_recommended_config: input.get("useRecommendedConfig").and_then(Json::as_bool),
    })
}

fn parse_overlay_metadata(
    value: Option<&Json>,
) -> Result<Option<SaveOverlayMetadata>, ChannelError> {
    let Some(Json::Object(object)) = value else {
        return Ok(None);
    };
    Ok(Some(SaveOverlayMetadata {
        template_id: object
            .get("templateId")
            .and_then(Json::as_str)
            .map(str::to_string),
        enabled: object.get("enabled").and_then(Json::as_bool),
        provider_name: object
            .get("providerName")
            .and_then(Json::as_str)
            .map(str::to_string),
    }))
}

fn parse_provider_config(value: &Json) -> Result<provider_config::ProviderConfig, ChannelError> {
    let text = serde_json::to_string(value)
        .map_err(|error| ChannelError::error(format!("invalid provider config: {error}")))?;
    let parsed = provider_config::JsonValue::from_json_str(&text)
        .map_err(|error| ChannelError::error(format!("invalid provider config: {error}")))?;
    provider_config::parse_provider_config(&parsed, &[])
        .map_err(|error| ChannelError::error(format!("invalid provider config: {error:?}")))
}

fn parse_model_config(value: &Json) -> Result<provider_config::ModelConfig, ChannelError> {
    let text = serde_json::to_string(value)
        .map_err(|error| ChannelError::error(format!("invalid model config: {error}")))?;
    let parsed = provider_config::JsonValue::from_json_str(&text)
        .map_err(|error| ChannelError::error(format!("invalid model config: {error}")))?;
    provider_config::parse_model_config(&parsed, &[])
        .map_err(|error| ChannelError::error(format!("invalid model config: {error:?}")))
}

fn parse_headers(
    value: Option<&Json>,
) -> Result<Option<std::collections::BTreeMap<String, String>>, ChannelError> {
    let Some(Json::Object(object)) = value else {
        return Ok(None);
    };
    let mut headers = std::collections::BTreeMap::new();
    for (key, value) in object {
        let Some(text) = value.as_str() else {
            return Err(ChannelError::error(format!(
                "header {key} must be a string"
            )));
        };
        headers.insert(key.clone(), text.to_string());
    }
    Ok(Some(headers))
}

async fn list_remote_models(input: &Json) -> Result<Json, ChannelError> {
    let request = provider_model_listing::RemoteModelCatalogRequest {
        api_type: required_json_string(input, "apiType")?,
        base_url: required_json_string(input, "baseUrl")?,
        api_key: input
            .get("apiKey")
            .and_then(Json::as_str)
            .map(str::to_string),
        headers: parse_headers(input.get("headers"))?,
    };
    match provider_model_listing::fetch_remote_model_catalog(&request).await {
        Ok(models) => Ok(json!({ "success": true, "models": models })),
        Err(message) => Ok(json!({ "success": false, "message": message })),
    }
}

/// 把锁内 Personal 层注入 Built-in 组合快照（写函数要看到本次写入基线的完整视图）。
fn with_personal_layer(
    builtin: &ProviderConfigSnapshot,
    personal: &PersonalLayer,
) -> ProviderConfigSnapshot {
    ProviderConfigSnapshot {
        revision: provider_config::JsonValue::Array(vec![
            provider_config::JsonValue::String(builtin.mode_builtin_revision.clone()),
            provider_config::JsonValue::String(personal.revision.clone()),
        ])
        .to_json_string(),
        mode_builtin_revision: builtin.mode_builtin_revision.clone(),
        personal_revision: personal.revision.clone(),
        mode_builtin_providers: builtin.mode_builtin_providers.clone(),
        mode_builtin_provider_templates: builtin.mode_builtin_provider_templates.clone(),
        personal_providers: personal.providers.clone(),
        mode_builtin_model_rules: builtin.mode_builtin_model_rules.clone(),
        personal_models: personal.models.clone(),
        personal_provider_order: personal.provider_order.clone().unwrap_or_default(),
    }
}

fn positional(arg: &Value) -> Vec<Json> {
    match arg.to_json() {
        Json::Array(items) => items,
        Json::Null => Vec::new(),
        other => vec![other],
    }
}

fn arg_string(args: &[Json], index: usize, name: &str) -> Result<String, ChannelError> {
    args.get(index)
        .and_then(Json::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ChannelError::error(format!("{name} is required")))
}

fn arg_string_list(args: &[Json], index: usize, name: &str) -> Result<Vec<String>, ChannelError> {
    let Some(Json::Array(items)) = args.get(index) else {
        return Err(ChannelError::error(format!("{name} must be an array")));
    };
    Ok(items
        .iter()
        .filter_map(Json::as_str)
        .map(str::to_string)
        .collect())
}

fn parse_provider_config_arg(
    args: &[Json],
    index: usize,
) -> Result<provider_config::ProviderConfig, ChannelError> {
    let value = args
        .get(index)
        .ok_or_else(|| ChannelError::error("config is required"))?;
    parse_provider_config(value)
}

fn parse_model_config_arg(
    args: &[Json],
    index: usize,
) -> Result<provider_config::ModelConfig, ChannelError> {
    let value = args
        .get(index)
        .ok_or_else(|| ChannelError::error("config is required"))?;
    parse_model_config(value)
}

fn required_json_string(value: &Json, key: &str) -> Result<String, ChannelError> {
    json_string(value, key).ok_or_else(|| ChannelError::error(format!("{key} is required")))
}

fn json_string(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn json_value<T: serde::Serialize>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_string(value)
        .map(Value::Json)
        .map_err(|error| ChannelError::error(error.to_string()))
}
