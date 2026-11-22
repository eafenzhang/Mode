//! OrcaRouter 服务编排：凭据入口统一、目录、连接与 provider 推理配置对齐。

use std::sync::Arc;

use serde::Serialize;

use crate::catalog::{OrcaCapability, OrcaCatalogInput, OrcaCatalogService, OrcaCatalogView};
use crate::connect::{OrcaConnectController, OrcaConnectOptions, OrcaConnectState};
use crate::credential::{
    looks_like_orca_api_key, mask_orca_secret, OrcaCredentialSource, OrcaCredentialStatus,
    OrcaCredentialStore,
};
use crate::origins::{build_orca_v1_base, OrcaOrigins};
use crate::overlay::OrcaProviderCredentialBinding;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaRouterEndpoints {
    pub auth_base: String,
    pub api_base: String,
    pub inference_base: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaRouterResolvedCredential {
    pub masked: String,
    pub source: OrcaCredentialSource,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaConnectOutcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<OrcaConnectState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaUnauthorizedOutcome {
    pub marked: bool,
    pub status: OrcaCredentialStatus,
}

pub type ServiceLog = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct OrcaRouterServiceOptions {
    pub store: Arc<OrcaCredentialStore>,
    pub origins: OrcaOrigins,
    pub connect: OrcaConnectController,
    pub catalog: OrcaCatalogService,
    pub binding: Option<Arc<OrcaProviderCredentialBinding>>,
    pub log: Option<ServiceLog>,
}

pub struct OrcaRouterService {
    store: Arc<OrcaCredentialStore>,
    origins: OrcaOrigins,
    connect: OrcaConnectController,
    catalog: OrcaCatalogService,
    binding: Option<Arc<OrcaProviderCredentialBinding>>,
    log: Option<ServiceLog>,
}

impl OrcaRouterService {
    pub fn new(options: OrcaRouterServiceOptions) -> Self {
        Self {
            store: options.store,
            origins: options.origins,
            connect: options.connect,
            catalog: options.catalog,
            binding: options.binding,
            log: options.log,
        }
    }

    /// 由 store + origins 装配连接/目录服务；binding 由宿主注入（需要 Personal 仓库）。
    pub fn build(
        store: Arc<OrcaCredentialStore>,
        origins: OrcaOrigins,
        binding: Option<Arc<OrcaProviderCredentialBinding>>,
        log: Option<ServiceLog>,
    ) -> Result<Self, String> {
        let log_for_connect = log.clone();
        let connect = OrcaConnectController::new(OrcaConnectOptions {
            credential_store: Arc::clone(&store),
            origins: origins.clone(),
            app_name: "Mode".to_string(),
            timeout_ms: 30_000,
            generate: crate::connect::default_random_source(),
            now_ms: Arc::new(now_millis),
            log: log_for_connect,
        })?;
        let mut catalog_options =
            crate::catalog::OrcaCatalogOptions::new(Arc::clone(&store), origins.clone());
        catalog_options.log = log.clone();
        let catalog = OrcaCatalogService::new(catalog_options)?;
        Ok(Self::new(OrcaRouterServiceOptions {
            store,
            origins,
            connect,
            catalog,
            binding,
            log,
        }))
    }

    fn log(&self, level: &str, message: &str) {
        if let Some(log) = &self.log {
            log(level, message);
        }
    }

    pub fn endpoints(&self) -> OrcaRouterEndpoints {
        OrcaRouterEndpoints {
            auth_base: self.origins.auth_base.clone(),
            api_base: self.origins.api_base.clone(),
            inference_base: build_orca_v1_base(&self.origins.api_base),
        }
    }

    pub fn credential_status(&self) -> Result<OrcaCredentialStatus, String> {
        self.store.status()
    }

    pub fn connect_state(&self) -> OrcaConnectState {
        self.connect.get_state()
    }

    /// 凭据变更后对齐 provider 推理配置；失败只告警，不反转凭据事实。
    fn sync_credential_to_provider(&self) {
        let Some(binding) = &self.binding else {
            return;
        };
        if let Err(error) = binding.sync() {
            self.log(
                "warn",
                &format!("OrcaRouter 凭据未能写入 Provider 推理配置：{error}"),
            );
        }
    }

    /// 空值等价于清除；格式明显有误时拒绝。
    pub fn save_api_key(&self, api_key: &str) -> Result<OrcaCredentialStatus, String> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            self.store.clear()?;
            self.catalog.invalidate();
            self.sync_credential_to_provider();
            return self.store.status();
        }
        if !looks_like_orca_api_key(trimmed) {
            return Err("OrcaRouter API Key 应以 sk-orca- 开头，请检查后重试".to_string());
        }
        self.store
            .save(trimmed, OrcaCredentialSource::ApiKey, None, None)?;
        self.catalog.invalidate();
        self.sync_credential_to_provider();
        self.log("info", "OrcaRouter API Key 已保存");
        self.store.status()
    }

    pub fn clear_credential(&self) -> Result<OrcaCredentialStatus, String> {
        self.connect.cancel("用户清除凭据");
        self.store.clear()?;
        self.catalog.invalidate();
        self.sync_credential_to_provider();
        self.log("info", "OrcaRouter 凭据已清除");
        self.store.status()
    }

    pub fn resolve_credential(&self, source: &str) -> Result<OrcaRouterResolvedCredential, String> {
        let expected = match source {
            "pkce" => OrcaCredentialSource::Pkce,
            "api-key" => OrcaCredentialSource::ApiKey,
            other => return Err(format!("未知的 OrcaRouter 凭据来源：{other}")),
        };
        let Some(credential) = self.store.load_usable()? else {
            return Err(match expected {
                OrcaCredentialSource::Pkce => "尚未完成 OrcaRouter 授权登录".to_string(),
                OrcaCredentialSource::ApiKey => "尚未配置 OrcaRouter API Key".to_string(),
            });
        };
        if expected == OrcaCredentialSource::Pkce && credential.source != expected {
            return Err("尚未完成 OrcaRouter 授权登录".to_string());
        }
        Ok(OrcaRouterResolvedCredential {
            masked: mask_orca_secret(Some(&credential.api_key)),
            source: expected,
            generation: credential.generation,
        })
    }

    pub fn begin_connect(&self) -> Result<OrcaConnectState, String> {
        // 切换认证方式 / 重新发起前先失效旧尝试。
        self.connect.cancel("重新发起授权");
        self.connect.begin().map_err(|error| error.message)
    }

    pub async fn submit_connect_code(&self, code: &str) -> OrcaConnectOutcome {
        match self.connect.submit_code(code).await {
            Ok(_) => {
                self.catalog.invalidate();
                self.sync_credential_to_provider();
                OrcaConnectOutcome {
                    ok: true,
                    state: Some(self.connect.get_state()),
                    kind: None,
                    message: None,
                }
            }
            Err(error) => {
                if error.kind == crate::connect::OrcaConnectErrorKind::Cancelled {
                    self.log("warn", "OrcaRouter 授权码换取失败");
                }
                OrcaConnectOutcome {
                    ok: false,
                    state: Some(self.connect.get_state()),
                    kind: Some(error.kind.as_str().to_string()),
                    message: Some(error.message),
                }
            }
        }
    }

    pub fn cancel_connect(&self, reason: Option<&str>) -> OrcaConnectState {
        self.connect.cancel(reason.unwrap_or("用户取消"))
    }

    pub fn invalidate_connect(&self, reason: &str) -> OrcaConnectState {
        let state = self.connect.invalidate_for_page_hide();
        self.log("info", &format!("OrcaRouter 登录状态已清理（{reason}）"));
        state
    }

    pub async fn list_models(
        &self,
        capability: OrcaCapability,
        required_input_modality: Option<&'static str>,
        force_refresh: bool,
    ) -> OrcaCatalogView {
        self.catalog
            .list(OrcaCatalogInput {
                capability,
                required_input_modality,
                force_refresh,
            })
            .await
    }

    pub fn report_unauthorized(
        &self,
        generation: Option<u64>,
        account_id: Option<&str>,
    ) -> Result<OrcaUnauthorizedOutcome, String> {
        let marked = self.store.mark_needs_reauth(generation, account_id)?;
        if marked {
            // loadUsable 已返回 null；同步即清除 provider 上的死 key。
            self.sync_credential_to_provider();
            self.log("warn", "OrcaRouter 凭据被上游拒绝，已标记为需要重新认证");
        }
        Ok(OrcaUnauthorizedOutcome {
            marked,
            status: self.store.status()?,
        })
    }

    pub fn reconcile_provider_credential(&self) -> Result<OrcaCredentialStatus, String> {
        self.sync_credential_to_provider();
        self.store.status()
    }

    /// 凭据存储（catalog/connect 之外的调用方共用同一事实）。
    pub fn store(&self) -> &Arc<OrcaCredentialStore> {
        &self.store
    }

    /// 测试/诊断：直接触发一次凭据对齐。
    pub fn sync_binding_now(&self) -> Result<(), String> {
        match &self.binding {
            Some(binding) => binding.sync(),
            None => Ok(()),
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use credential_store::CredentialStore;
    use provider_config::{
        ApiKeyAccess, ApiKeyAccessType, Field, ProviderAccess, ProviderConfig, ProviderConfigMap,
        ProviderConfigRule,
    };
    use provider_config_file_codec::ProviderConfigLayerUpdate;
    use provider_config_repository::{
        PersonalConfigRepository, PersonalConfigRepositoryOptions, PollingInterval,
    };

    use crate::overlay::ORCAROUTER_PROVIDER_TEMPLATE_ID;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "orca-service-{label}-{}-{}",
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

    fn service(dir: &TempDir) -> OrcaRouterService {
        let store = Arc::new(OrcaCredentialStore::new(Arc::new(
            CredentialStore::with_secret(&dir.0, "orca-service-secret"),
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
        let binding = Arc::new(OrcaProviderCredentialBinding::new(
            Arc::clone(&store),
            Arc::clone(&repository),
        ));
        OrcaRouterService::build(
            Arc::clone(&store),
            OrcaOrigins {
                auth_base: "https://auth.example".to_string(),
                api_base: "https://api.example".to_string(),
            },
            Some(binding),
            None,
        )
        .expect("service")
    }

    #[test]
    fn endpoints_derive_v1_base() {
        let dir = TempDir::new("endpoints");
        let service = service(&dir);
        let endpoints = service.endpoints();
        assert_eq!(endpoints.auth_base, "https://auth.example");
        assert_eq!(endpoints.api_base, "https://api.example");
        assert_eq!(endpoints.inference_base, "https://api.example/v1");
    }

    #[test]
    fn save_api_key_validates_and_syncs_provider() {
        let dir = TempDir::new("save");
        let service = service(&dir);
        assert!(service.save_api_key("bad-key").is_err());
        let status = service.save_api_key("sk-orca-abcdef123456").expect("save");
        assert!(status.connected);
        assert_eq!(status.source, Some(OrcaCredentialSource::ApiKey));
        let credential = service.resolve_credential("api-key").expect("resolve");
        assert_eq!(credential.masked, "sk-orc…3456");

        // 空值等价清除。
        let status = service.save_api_key("   ").expect("clear");
        assert!(!status.connected);
    }

    #[test]
    fn clear_and_reauth_clear_provider_key() {
        let dir = TempDir::new("reauth");
        let service = service(&dir);
        service.save_api_key("sk-orca-abcdef123456").expect("save");
        assert_eq!(service.store().status().expect("status").generation, 1);
        let outcome = service.report_unauthorized(None, None).expect("report");
        assert!(outcome.marked);
        assert!(outcome.status.needs_reauth);
        assert!(service.store().load_usable().expect("load").is_none());
        assert!(service.resolve_credential("api-key").is_err());

        service.clear_credential().expect("clear");
        assert!(!service.credential_status().expect("status").connected);
    }
}
