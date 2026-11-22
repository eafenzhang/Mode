//! OrcaRouter 凭据 → provider 推理配置的唯一投递口。
//!
//! 推理路径只读 provider 自身的 `access.apiKey`；本模块把 store 里的同一把 key
//! 写回模板实例的 Personal Overlay（断开时写 `null`），让推理路径 fail closed。

use std::sync::Arc;

use provider_config::{ApiKeyAccess, ApiKeyAccessType, Field, ProviderAccess, ProviderConfigRule};
use provider_config_file_codec::ProviderConfigLayerUpdate;
use provider_config_repository::PersonalConfigRepository;

use crate::credential::OrcaCredentialStore;

pub const ORCAROUTER_PROVIDER_TEMPLATE_ID: &str = "orcarouter";

type ErrorSink = Arc<dyn Fn(&str) + Send + Sync>;

pub struct OrcaProviderCredentialBinding {
    store: Arc<OrcaCredentialStore>,
    repository: Arc<PersonalConfigRepository>,
    on_error: Option<ErrorSink>,
}

impl OrcaProviderCredentialBinding {
    pub fn new(store: Arc<OrcaCredentialStore>, repository: Arc<PersonalConfigRepository>) -> Self {
        Self {
            store,
            repository,
            on_error: None,
        }
    }

    pub fn with_error_sink(mut self, on_error: ErrorSink) -> Self {
        self.on_error = Some(on_error);
        self
    }

    fn report(&self, message: &str) {
        if let Some(on_error) = &self.on_error {
            on_error(message);
        }
    }

    /// 已连接 → 写入同一把 key；未连接/needsReauth → 清除 provider 上的 key。
    /// 幂等：与 store 一致时不写盘。
    pub fn sync(&self) -> Result<(), String> {
        match self.sync_inner() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.report(&error);
                Err(error)
            }
        }
    }

    fn sync_inner(&self) -> Result<(), String> {
        let api_key = self
            .store
            .load_usable()
            .map_err(|error| error.to_string())?
            .map(|credential| credential.api_key);
        let layer = self.repository.read();
        let Some((provider_id, rule)) = layer
            .providers
            .entries()
            .map(|(provider_id, rule)| (provider_id.clone(), rule.clone()))
            .find(|(_, rule)| {
                matches!(&rule.template_id, Field::Value(value) if value == ORCAROUTER_PROVIDER_TEMPLATE_ID)
            })
        else {
            // 没有模板实例：Built-in 固定 Provider 的 access 由 Built-in Config 声明。
            return Ok(());
        };
        if read_api_key(&rule.config) == api_key {
            return Ok(());
        }
        let updated_rule = merge_access(rule, api_key.as_deref());
        self.repository
            .update(|current| {
                let mut providers = current.providers.clone();
                providers
                    .values
                    .insert(provider_id.clone(), updated_rule.clone());
                ProviderConfigLayerUpdate {
                    providers,
                    models: current.models.clone(),
                    provider_order: current.provider_order.clone(),
                    default_model_selection: current
                        .default_model_selection
                        .as_ref()
                        .map(to_update_selection),
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Personal 层与文件 codec 各有一份同构 Selection 类型。
fn to_update_selection(
    selection: &provider_config_source::ModelSelection,
) -> model_selection::ModelSelection {
    model_selection::ModelSelection {
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

fn read_api_key(config: &provider_config::ProviderConfig) -> Option<String> {
    let Field::Value(ProviderAccess::ApiKey(access)) = &config.access else {
        return None;
    };
    match &access.api_key {
        Field::Value(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn merge_access(rule: ProviderConfigRule, api_key: Option<&str>) -> ProviderConfigRule {
    let management_url = match &rule.config.access {
        Field::Value(ProviderAccess::ApiKey(access)) => access.api_key_management_url.clone(),
        _ => Field::Absent,
    };
    let mut config = rule.config.clone();
    config.access = Field::Value(ProviderAccess::ApiKey(ApiKeyAccess {
        access_type: ApiKeyAccessType::ApiKey,
        api_key: match api_key {
            Some(value) => Field::Value(value.to_string()),
            None => Field::Null,
        },
        api_key_management_url: management_url,
    }));
    ProviderConfigRule { config, ..rule }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use credential_store::CredentialStore;
    use provider_config::{ProviderConfig, ProviderConfigMap};
    use provider_config_repository::{PersonalConfigRepositoryOptions, PollingInterval};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "orca-overlay-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(
        dir: &TempDir,
        seed_template: bool,
    ) -> (Arc<OrcaCredentialStore>, Arc<PersonalConfigRepository>) {
        let store = Arc::new(OrcaCredentialStore::new(Arc::new(
            CredentialStore::with_secret(&dir.0, "orca-overlay-secret"),
        )));
        let repository = Arc::new(
            PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
                file_path: dir.0.join("provider_config.json"),
                import_legacy: None,
                on_recovery: None,
                on_polling_error: None,
                polling_interval: PollingInterval::Disabled,
            })
            .expect("repository"),
        );
        if seed_template {
            repository
                .update(|_| {
                    let mut providers = ProviderConfigMap::empty();
                    providers.values.insert(
                        "p-orca".to_string(),
                        ProviderConfigRule {
                            provider_id: "p-orca".to_string(),
                            template_id: Field::Value(ORCAROUTER_PROVIDER_TEMPLATE_ID.to_string()),
                            provider_name: Field::Value("OrcaRouter".to_string()),
                            enabled: Field::Value(true),
                            config: ProviderConfig {
                                access: Field::Value(ProviderAccess::ApiKey(ApiKeyAccess {
                                    access_type: ApiKeyAccessType::ApiKey,
                                    api_key: Field::Absent,
                                    api_key_management_url: Field::Absent,
                                })),
                                ..ProviderConfig::default()
                            },
                        },
                    );
                    ProviderConfigLayerUpdate {
                        providers,
                        models: Default::default(),
                        provider_order: None,
                        default_model_selection: None,
                    }
                })
                .expect("seed");
        }
        (store, repository)
    }

    #[test]
    fn sync_writes_key_and_is_idempotent() {
        let dir = TempDir::new("write");
        let (store, repository) = setup(&dir, true);
        store
            .save(
                "sk-orca-abcdef123456",
                crate::credential::OrcaCredentialSource::ApiKey,
                None,
                None,
            )
            .expect("save");
        let binding = OrcaProviderCredentialBinding::new(store, Arc::clone(&repository));
        binding.sync().expect("sync");
        let layer = repository.read();
        let rule = layer.providers.get("p-orca").expect("rule");
        assert_eq!(
            read_api_key(&rule.config).as_deref(),
            Some("sk-orca-abcdef123456")
        );

        let revision = layer.revision;
        binding.sync().expect("idempotent");
        assert_eq!(repository.read().revision, revision, "一致时不写盘");
    }

    #[test]
    fn disconnect_writes_null_and_clears_key() {
        let dir = TempDir::new("clear");
        let (store, repository) = setup(&dir, true);
        store
            .save(
                "sk-orca-abcdef123456",
                crate::credential::OrcaCredentialSource::ApiKey,
                None,
                None,
            )
            .expect("save");
        let binding =
            OrcaProviderCredentialBinding::new(Arc::clone(&store), Arc::clone(&repository));
        binding.sync().expect("sync");
        store.clear().expect("clear");
        binding.sync().expect("disconnect");
        let layer = repository.read();
        let rule = layer.providers.get("p-orca").expect("rule");
        assert!(read_api_key(&rule.config).is_none());
        let Field::Value(ProviderAccess::ApiKey(access)) = &rule.config.access else {
            panic!("api-key access");
        };
        assert_eq!(access.api_key, Field::Null);
    }

    #[test]
    fn missing_template_instance_is_noop() {
        let dir = TempDir::new("missing");
        let (store, repository) = setup(&dir, false);
        store
            .save(
                "sk-orca-abcdef123456",
                crate::credential::OrcaCredentialSource::ApiKey,
                None,
                None,
            )
            .expect("save");
        let revision = repository.read().revision;
        let binding = OrcaProviderCredentialBinding::new(store, Arc::clone(&repository));
        binding.sync().expect("sync");
        assert_eq!(repository.read().revision, revision);
    }
}
