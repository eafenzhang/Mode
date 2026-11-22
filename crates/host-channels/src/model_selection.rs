//! `model-selection` 频道：ModelSelectionView 装配（生产口径 ordinary）。
//!
//! 规范见 `docs/specs/model-selection-channel.md`。数据源：Built-in/Personal provider
//! 配置快照 → `ProviderConfigResolver`；configuredDefault 与变更通知来自
//! `PersonalConfigRepository`。生产 `createNodeModelSelectionFacade` 未传 classifier，
//! 因此本实现只做 ordinary 分类（account-* 与 legacy reasoning 不在生产路径）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use model_selection::ModelSelection;
use provider_config::{Field, ModelConfig, ProviderVisibility};
use provider_config_file_codec::ProviderConfigLayerUpdate;
use provider_config_repository::{
    ChangeSubscription, PersonalConfigRepository, PersonalConfigRepositoryOptions, PollingInterval,
};
use provider_config_source::{read_snapshot, ProviderConfigSnapshot};
use provider_resolver::{ProviderConfigResolver, ProviderConfigResolverInput, RegistryProvider};
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

pub const MODEL_SELECTION_CHANNEL: &str = "model-selection";
pub const ON_DID_CHANGE: &str = "onDidChange";

type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ModelSelectionChannelOptions {
    pub bundled_config_path: String,
    pub personal_config_path: PathBuf,
    pub on_log: Option<LogSink>,
}

pub struct ModelSelectionChannel {
    inner: Arc<Inner>,
}

struct Inner {
    bundled_config_path: String,
    personal_config_path: PathBuf,
    repository: Arc<PersonalConfigRepository>,
    revision: AtomicU64,
    last_snapshot_revision: Mutex<Option<String>>,
    listeners: Mutex<HashMap<u64, EventCallback>>,
    next_listener: AtomicU64,
    change_subscription: Mutex<Option<ChangeSubscription>>,
    on_log: Option<LogSink>,
}

impl ModelSelectionChannel {
    pub fn new(options: ModelSelectionChannelOptions) -> Result<Self, String> {
        let repository = PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
            file_path: options.personal_config_path.clone(),
            import_legacy: None,
            on_recovery: None,
            on_polling_error: None,
            polling_interval: PollingInterval::Millis(1_000),
        })
        .map_err(|error| error.to_string())?;
        // 首次 read 启动轮询，与 usage-stats 装配同口径。
        let _ = repository.read();
        Ok(Self {
            inner: Arc::new(Inner {
                bundled_config_path: options.bundled_config_path,
                personal_config_path: options.personal_config_path,
                repository: Arc::new(repository),
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

impl ServerChannel for ModelSelectionChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            match command.as_str() {
                "getView" => {
                    let input = parse_view_input(&arg)?;
                    let view =
                        inner.compute_view(input.as_ref().map(|selection| selection.as_ref()))?;
                    json_value(&view)
                }
                "saveConfiguredDefault" => {
                    let selection: ModelSelection = parse_selection_arg(&arg)?;
                    inner.save_configured_default(&selection)?;
                    Ok(Value::Undefined)
                }
                other => Err(ChannelError::error(format!(
                    "model-selection channel: unknown or not-yet-migrated command {other}"
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
                "model-selection channel: unknown event {event}"
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
        let view = match self.compute_view(None) {
            Ok(view) => view,
            Err(error) => {
                self.log(format!("model-selection onDidChange 重建失败: {error:?}"));
                return;
            }
        };
        let text = match serde_json::to_string(&view) {
            Ok(text) => text,
            Err(error) => {
                self.log(format!("model-selection onDidChange 序列化失败: {error}"));
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

    fn save_configured_default(&self, selection: &ModelSelection) -> Result<(), ChannelError> {
        self.repository
            .update(|layer| ProviderConfigLayerUpdate {
                providers: layer.providers.clone(),
                models: layer.models.clone(),
                provider_order: layer.provider_order.clone(),
                default_model_selection: Some(selection.clone()),
            })
            .map(|_| ())
            .map_err(|error| ChannelError::error(error.to_string()))
    }

    fn compute_view(&self, input: Option<Option<&ModelSelection>>) -> Result<Json, ChannelError> {
        let personal = self.repository.read();
        let snapshot = read_snapshot(&self.bundled_config_path, &self.personal_config_path, None)
            .map_err(|error| ChannelError::error(error.to_string()))?;
        let revision = self.next_revision(&snapshot.revision);
        let providers = resolve_registry(&snapshot);
        let configured_default = personal
            .default_model_selection
            .as_ref()
            .map(from_source_selection);
        Ok(build_view(
            &providers,
            configured_default.as_ref(),
            input,
            revision,
        ))
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
}

/// `provider_config_source` 与 `model_selection` 各有一份同构的 Selection 类型。
fn from_source_selection(selection: &provider_config_source::ModelSelection) -> ModelSelection {
    ModelSelection {
        provider_id: selection.provider_id.clone(),
        model_id: selection.model_id.clone(),
        options: selection
            .options
            .as_ref()
            .map(|options| model_selection::SelectionOptions {
                reasoning_level: options.reasoning_level.clone(),
            }),
    }
}

fn resolve_registry(snapshot: &ProviderConfigSnapshot) -> Vec<RegistryProvider> {
    let account_providers = provider_config::ProviderConfigMap::empty();
    let resolution = ProviderConfigResolver.resolve(&ProviderConfigResolverInput {
        mode_builtin_providers: &snapshot.mode_builtin_providers,
        mode_builtin_provider_templates: Some(&snapshot.mode_builtin_provider_templates),
        personal_providers: &snapshot.personal_providers,
        mode_builtin_model_rules: &snapshot.mode_builtin_model_rules,
        personal_models: &snapshot.personal_models,
        account_providers: &account_providers,
        account_states: None,
        personal_provider_order: Some(&snapshot.personal_provider_order),
    });
    resolution.registry_providers
}

fn build_view(
    providers: &[RegistryProvider],
    configured_default: Option<&ModelSelection>,
    input: Option<Option<&ModelSelection>>,
    revision: u64,
) -> Json {
    let visible: Vec<&RegistryProvider> = providers
        .iter()
        .filter(|provider| !is_hidden(provider))
        .collect();
    let projected: Vec<Json> = visible
        .iter()
        .map(|provider| {
            json!({
                "providerId": provider.provider_id,
                "providerName": field_string(&provider.provider_name),
                "templateId": field_string(&provider.template_id),
                "config": provider.config.to_json(),
                "models": provider.models.iter().map(|model| json!({
                    "modelId": model.model_id,
                    "config": model.config.to_json(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut view = serde_json::Map::new();
    view.insert("revision".to_string(), json!(revision));
    view.insert("providers".to_string(), Json::Array(projected));
    if let Some(preferred) = preferred_selection(providers, configured_default) {
        view.insert(
            "preferredSelection".to_string(),
            serde_json::to_value(&preferred).unwrap_or(Json::Null),
        );
    }
    if let Some(selection) = input {
        let (effective, issue) = resolve_effective(providers, selection);
        view.insert(
            "effectiveSelection".to_string(),
            effective
                .as_ref()
                .map(|selection| serde_json::to_value(selection).unwrap_or(Json::Null))
                .unwrap_or(Json::Null),
        );
        if let Some(issue) = issue {
            view.insert("selectionIssue".to_string(), json!(issue));
        }
    }
    Json::Object(view)
}

fn preferred_selection(
    providers: &[RegistryProvider],
    configured_default: Option<&ModelSelection>,
) -> Option<ModelSelection> {
    if let Some(default) = configured_default {
        if is_selectable(providers, default) {
            return Some(default.clone());
        }
    }
    for provider in providers.iter().filter(|provider| !is_hidden(provider)) {
        for model in &provider.models {
            if let Some(selection) = complete_new_selection(provider, &model.model_id) {
                return Some(selection);
            }
        }
    }
    None
}

fn complete_new_selection(provider: &RegistryProvider, model_id: &str) -> Option<ModelSelection> {
    let model = provider
        .models
        .iter()
        .find(|model| model.model_id == model_id)?;
    let level = reasoning_levels(&model.config).pop()?;
    Some(ModelSelection {
        provider_id: provider.provider_id.clone(),
        model_id: model.model_id.clone(),
        options: Some(model_selection::SelectionOptions {
            reasoning_level: Some(level),
        }),
    })
}

fn resolve_effective(
    providers: &[RegistryProvider],
    selection: Option<&ModelSelection>,
) -> (Option<ModelSelection>, Option<&'static str>) {
    let Some(selection) = selection else {
        return (None, Some("selection-missing"));
    };
    let Some(provider) = providers
        .iter()
        .find(|provider| provider.provider_id == selection.provider_id)
    else {
        return (None, Some("provider-not-found"));
    };
    if is_hidden(provider) {
        return (None, Some("provider-not-found"));
    }
    let Some(model) = provider
        .models
        .iter()
        .find(|model| model.model_id == selection.model_id)
    else {
        return (None, Some("model-not-found"));
    };
    let level = selection
        .options
        .as_ref()
        .and_then(|options| options.reasoning_level.as_deref());
    let mut effective = ModelSelection {
        provider_id: provider.provider_id.clone(),
        model_id: model.model_id.clone(),
        options: None,
    };
    match level {
        None => (Some(effective), Some("reasoning-level-missing")),
        Some(level) => {
            if reasoning_levels(&model.config)
                .iter()
                .any(|value| value == level)
            {
                effective.options = Some(model_selection::SelectionOptions {
                    reasoning_level: Some(level.to_string()),
                });
                (Some(effective), None)
            } else {
                (Some(effective), Some("reasoning-level-not-supported"))
            }
        }
    }
}

fn is_selectable(providers: &[RegistryProvider], selection: &ModelSelection) -> bool {
    let Some(provider) = providers
        .iter()
        .find(|provider| provider.provider_id == selection.provider_id)
    else {
        return false;
    };
    if is_hidden(provider) {
        return false;
    }
    let Some(model) = provider
        .models
        .iter()
        .find(|model| model.model_id == selection.model_id)
    else {
        return false;
    };
    let Some(level) = selection
        .options
        .as_ref()
        .and_then(|options| options.reasoning_level.as_deref())
    else {
        return false;
    };
    reasoning_levels(&model.config)
        .iter()
        .any(|value| value == level)
}

fn reasoning_levels(config: &ModelConfig) -> Vec<String> {
    let Field::Value(specs) = &config.option_specs else {
        return Vec::new();
    };
    let Field::Value(reasoning) = &specs.reasoning_level else {
        return Vec::new();
    };
    match &reasoning.values {
        Field::Value(values) => values.clone(),
        _ => Vec::new(),
    }
}

fn is_hidden(provider: &RegistryProvider) -> bool {
    matches!(
        &provider.config.visibility,
        Field::Value(ProviderVisibility::Hidden)
    )
}

fn field_string(field: &Field<String>) -> String {
    match field {
        Field::Value(value) => value.clone(),
        _ => String::new(),
    }
}

fn parse_view_input(arg: &Value) -> Result<Option<Option<ModelSelection>>, ChannelError> {
    let json = arg.to_json();
    let value = match json {
        Json::Array(items) => items.into_iter().next(),
        Json::Null => None,
        other => Some(other),
    };
    let Some(value) = value else {
        return Ok(None);
    };
    let Json::Object(map) = value else {
        return Ok(None);
    };
    let Some(selection) = map.get("selection") else {
        return Ok(None);
    };
    match selection {
        Json::Null => Ok(Some(None)),
        other => serde_json::from_value::<ModelSelection>(other.clone())
            .map(|selection| Some(Some(selection)))
            .map_err(|error| ChannelError::error(format!("invalid getView selection: {error}"))),
    }
}

fn parse_selection_arg(arg: &Value) -> Result<ModelSelection, ChannelError> {
    let json = arg.to_json();
    let value = match json {
        Json::Array(items) => items.into_iter().next(),
        other => Some(other),
    };
    let Some(value) = value else {
        return Err(ChannelError::error(
            "saveConfiguredDefault requires a selection",
        ));
    };
    serde_json::from_value::<ModelSelection>(value)
        .map_err(|error| ChannelError::error(format!("invalid selection: {error}")))
}

fn json_value<T: serde::Serialize>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_string(value)
        .map(Value::Json)
        .map_err(|error| ChannelError::error(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use provider_config::{EnumOptionSpecConfig, ModelOptionSpecsConfig};
    use provider_resolver::RegistryModel;

    fn provider(id: &str, hidden: bool, levels: &[&str]) -> RegistryProvider {
        let mut config = provider_config::ProviderConfig::default();
        if hidden {
            config.visibility = Field::Value(ProviderVisibility::Hidden);
        }
        let model_config = ModelConfig {
            option_specs: Field::Value(ModelOptionSpecsConfig {
                reasoning_level: Field::Value(EnumOptionSpecConfig {
                    values: Field::Value(levels.iter().map(|level| level.to_string()).collect()),
                    map: Field::Absent,
                }),
                max_output_tokens: Field::Absent,
            }),
            ..ModelConfig::default()
        };
        RegistryProvider {
            provider_id: id.to_string(),
            provider_name: Field::Value(id.to_uppercase()),
            template_id: Field::Value("tpl".to_string()),
            config,
            models: vec![RegistryModel {
                model_id: "m1".to_string(),
                config: model_config,
            }],
        }
    }

    fn selection(provider: &str, level: Option<&str>) -> ModelSelection {
        ModelSelection {
            provider_id: provider.to_string(),
            model_id: "m1".to_string(),
            options: level.map(|level| model_selection::SelectionOptions {
                reasoning_level: Some(level.to_string()),
            }),
        }
    }

    #[test]
    fn preferred_uses_configured_default_then_registry() {
        let providers = vec![provider("a", false, &["low", "high"])];
        let view = build_view(&providers, Some(&selection("a", Some("low"))), None, 1);
        assert_eq!(
            view["preferredSelection"]["options"]["reasoningLevel"],
            "low"
        );
        // 默认失效（档位不支持）→ 回落 registry 最高档。
        let view = build_view(&providers, Some(&selection("a", Some("ultra"))), None, 1);
        assert_eq!(
            view["preferredSelection"]["options"]["reasoningLevel"],
            "high"
        );
        // 没有可见 provider → 省略字段。
        let view = build_view(&[provider("h", true, &["low"])], None, None, 1);
        assert!(view.get("preferredSelection").is_none());
        assert_eq!(view["providers"].as_array().expect("providers").len(), 0);
    }

    #[test]
    fn effective_reports_issues() {
        let providers = vec![provider("a", false, &["low", "high"])];
        let view = build_view(
            &providers,
            None,
            Some(Some(&selection("a", Some("low")))),
            1,
        );
        assert_eq!(
            view["effectiveSelection"]["options"]["reasoningLevel"],
            "low"
        );
        assert!(view.get("selectionIssue").is_none());

        let view = build_view(&providers, None, Some(Some(&selection("a", None))), 1);
        assert_eq!(view["selectionIssue"], "reasoning-level-missing");
        assert_eq!(view["effectiveSelection"]["providerId"], "a");
        assert!(view["effectiveSelection"].get("options").is_none());

        let view = build_view(
            &providers,
            None,
            Some(Some(&selection("a", Some("ultra")))),
            1,
        );
        assert_eq!(view["selectionIssue"], "reasoning-level-not-supported");

        let view = build_view(&providers, None, Some(Some(&selection("x", None))), 1);
        assert_eq!(view["selectionIssue"], "provider-not-found");
        assert_eq!(view["effectiveSelection"], Json::Null);

        // 无入参：不输出 effective 字段。
        let view = build_view(&providers, None, None, 1);
        assert!(view.get("effectiveSelection").is_none());
        assert!(view.get("selectionIssue").is_none());
    }

    #[tokio::test]
    async fn channel_reads_bundled_config() {
        let bundled = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/provider/mode-builtin.json");
        let personal = std::env::temp_dir().join(format!(
            "model-selection-personal-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        let channel = ModelSelectionChannel::new(ModelSelectionChannelOptions {
            bundled_config_path: bundled.to_string_lossy().into_owned(),
            personal_config_path: personal.clone(),
            on_log: None,
        })
        .expect("channel");
        let value = channel
            .call(
                "ctx".into(),
                "getView".into(),
                Value::Array(vec![]),
                CancellationToken::default(),
            )
            .await
            .expect("getView");
        let Value::Json(text) = value else {
            panic!("expected json");
        };
        let view: Json = serde_json::from_str(&text).expect("view json");
        assert!(view["revision"].as_u64().unwrap_or(0) >= 1);
        // 无 Personal 凭据时 Registry 可发布子集为空是正常结果；这里只验证装配可读。
        assert!(view["providers"].is_array());
        let _ = std::fs::remove_file(&personal);
    }

    #[test]
    fn hidden_provider_is_not_found() {
        let providers = vec![provider("h", true, &["low"])];
        let view = build_view(
            &providers,
            None,
            Some(Some(&selection("h", Some("low")))),
            1,
        );
        assert_eq!(view["selectionIssue"], "provider-not-found");
    }

    #[test]
    fn model_not_found_is_reported() {
        let providers = vec![provider("a", false, &["low"])];
        let missing = ModelSelection {
            provider_id: "a".to_string(),
            model_id: "missing".to_string(),
            options: Some(model_selection::SelectionOptions {
                reasoning_level: Some("low".to_string()),
            }),
        };
        let view = build_view(&providers, None, Some(Some(&missing)), 1);
        assert_eq!(view["selectionIssue"], "model-not-found");
    }
}
