//! `orcarouter` 频道适配器：设置页 OrcaRouter 面板的唯一 RPC 面。
//!
//! 编排在 `orcarouter-service`；本适配器只做位置参数解析与结果序列化。

use std::sync::Arc;

use orcarouter_service::service::OrcaRouterService;
use orcarouter_service::OrcaCapability;
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value as Json;

pub const ORCAROUTER_CHANNEL: &str = "orcarouter";

pub struct OrcaRouterChannel {
    service: Arc<OrcaRouterService>,
}

impl OrcaRouterChannel {
    pub fn new(service: Arc<OrcaRouterService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for OrcaRouterChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            match command.as_str() {
                "getEndpoints" => json_value(&service.endpoints()),
                "getCredentialStatus" => {
                    json_value(&service.credential_status().map_err(ChannelError::error)?)
                }
                "saveApiKey" => {
                    let params: SaveApiKeyParams = parse_params("saveApiKey", arg)?;
                    json_value(
                        &service
                            .save_api_key(&params.api_key)
                            .map_err(ChannelError::error)?,
                    )
                }
                "clearCredential" => {
                    json_value(&service.clear_credential().map_err(ChannelError::error)?)
                }
                "resolveCredential" => {
                    let params: ResolveParams = parse_params("resolveCredential", arg)?;
                    json_value(
                        &service
                            .resolve_credential(&params.source)
                            .map_err(ChannelError::error)?,
                    )
                }
                "beginConnect" => {
                    json_value(&service.begin_connect().map_err(ChannelError::error)?)
                }
                "submitConnectCode" => {
                    let params: SubmitCodeParams = parse_params("submitConnectCode", arg)?;
                    json_value(&service.submit_connect_code(&params.code).await)
                }
                "cancelConnect" => {
                    let params: CancelParams = parse_params("cancelConnect", arg)?;
                    json_value(&service.cancel_connect(params.reason.as_deref()))
                }
                "invalidateConnect" => {
                    let params: InvalidateParams = parse_params("invalidateConnect", arg)?;
                    json_value(&service.invalidate_connect(&params.reason))
                }
                "listModels" => {
                    // ProxyChannel 把方法入参包成数组；必须取首元素，否则 capability 恒空。
                    let params: ListModelsParams =
                        parse_params("listModels", arg).unwrap_or_default();
                    let capability = capability_from(&params.capability)?;
                    let modality = match params.required_input_modality.as_deref() {
                        None => None,
                        Some("image") => Some("image"),
                        Some("audio") => Some("audio"),
                        Some("video") => Some("video"),
                        Some(other) => {
                            return Err(ChannelError::error(format!(
                                "invalid requiredInputModality: {other}"
                            )))
                        }
                    };
                    let view = service
                        .list_models(capability, modality, params.force_refresh)
                        .await;
                    json_value(&view)
                }
                "reportUnauthorized" => {
                    let params: ReportParams = parse_params("reportUnauthorized", arg)?;
                    json_value(
                        &service
                            .report_unauthorized(params.generation, params.account_id.as_deref())
                            .map_err(ChannelError::error)?,
                    )
                }
                "reconcileProviderCredential" => json_value(
                    &service
                        .reconcile_provider_credential()
                        .map_err(ChannelError::error)?,
                ),
                other => Err(ChannelError::error(format!(
                    "orcarouter channel: unknown command {other}"
                ))),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveApiKeyParams {
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct ResolveParams {
    source: String,
}

#[derive(Debug, Deserialize)]
struct SubmitCodeParams {
    code: String,
}

#[derive(Debug, Default, Deserialize)]
struct CancelParams {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InvalidateParams {
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListModelsParams {
    #[serde(default)]
    capability: String,
    #[serde(default)]
    required_input_modality: Option<String>,
    #[serde(default)]
    force_refresh: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportParams {
    #[serde(default)]
    generation: Option<u64>,
    #[serde(default)]
    account_id: Option<String>,
}

fn capability_from(value: &str) -> Result<OrcaCapability, ChannelError> {
    match value {
        "chat" => Ok(OrcaCapability::Chat),
        "embedding" => Ok(OrcaCapability::Embedding),
        "image" => Ok(OrcaCapability::Image),
        "video" => Ok(OrcaCapability::Video),
        "rerank" => Ok(OrcaCapability::Rerank),
        other => Err(ChannelError::error(format!(
            "invalid OrcaRouter capability: {other}"
        ))),
    }
}

fn parse_params<T: DeserializeOwned>(command: &str, arg: Value) -> Result<T, ChannelError> {
    let json: Json = arg.to_json();
    let value = match json {
        Json::Array(items) => items.into_iter().next().unwrap_or(Json::Null),
        other => other,
    };
    if value.is_null() {
        return serde_json::from_str("{}")
            .map_err(|error| ChannelError::error(format!("invalid {command} params: {error}")));
    }
    serde_json::from_value(value)
        .map_err(|error| ChannelError::error(format!("invalid {command} params: {error}")))
}

fn json_value<T: serde::Serialize>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_string(value)
        .map(Value::Json)
        .map_err(|error| ChannelError::error(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use credential_store::CredentialStore;
    use provider_config_file_codec::ProviderConfigLayerUpdate;
    use provider_config_repository::{
        PersonalConfigRepository, PersonalConfigRepositoryOptions, PollingInterval,
    };
    use rpc_wire::Value;

    use orcarouter_service::credential::OrcaCredentialStore;
    use orcarouter_service::origins::OrcaOrigins;
    use orcarouter_service::overlay::OrcaProviderCredentialBinding;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "orca-channel-{label}-{}-{}",
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

    fn channel(dir: &TempDir) -> OrcaRouterChannel {
        let store = Arc::new(OrcaCredentialStore::new(Arc::new(
            CredentialStore::with_secret(&dir.0, "orca-channel-secret"),
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
            .update(|_| ProviderConfigLayerUpdate {
                providers: Default::default(),
                models: Default::default(),
                provider_order: None,
                default_model_selection: None,
            })
            .expect("seed");
        let binding = Arc::new(OrcaProviderCredentialBinding::new(
            Arc::clone(&store),
            Arc::clone(&repository),
        ));
        let service = OrcaRouterService::build(
            store,
            OrcaOrigins {
                auth_base: "https://auth.example".to_string(),
                api_base: "https://api.example".to_string(),
            },
            Some(binding),
            None,
        )
        .expect("service");
        OrcaRouterChannel::new(Arc::new(service))
    }

    async fn call(channel: &OrcaRouterChannel, method: &str, arg: Json) -> Json {
        let value = channel
            .call(
                "ctx".into(),
                method.into(),
                Value::Json(arg.to_string()),
                CancellationToken::default(),
            )
            .await
            .expect("call");
        let Value::Json(text) = value else {
            panic!("expected json");
        };
        serde_json::from_str(&text).expect("json")
    }

    #[tokio::test]
    async fn endpoints_and_credential_lifecycle() {
        let dir = TempDir::new("lifecycle");
        let channel = channel(&dir);
        let endpoints = call(&channel, "getEndpoints", serde_json::json!({})).await;
        assert_eq!(endpoints["inferenceBase"], "https://api.example/v1");

        let status = call(
            &channel,
            "saveApiKey",
            serde_json::json!({ "apiKey": "sk-orca-abcdef123456" }),
        )
        .await;
        assert_eq!(status["connected"], true);
        let resolved = call(
            &channel,
            "resolveCredential",
            serde_json::json!({ "source": "api-key" }),
        )
        .await;
        assert_eq!(resolved["masked"], "sk-orc…3456");

        let status = call(&channel, "clearCredential", serde_json::json!({})).await;
        assert_eq!(status["connected"], false);
    }

    #[tokio::test]
    async fn list_models_accepts_proxychannel_array_args() {
        let dir = TempDir::new("array-args");
        let channel = channel(&dir);
        let value = channel
            .call(
                "ctx".into(),
                "listModels".into(),
                Value::Array(vec![Value::Json(
                    serde_json::json!({ "capability": "chat" }).to_string(),
                )]),
                CancellationToken::default(),
            )
            .await
            .expect("call");
        let Value::Json(text) = value else {
            panic!("expected json");
        };
        let view: Json = serde_json::from_str(&text).expect("json");
        assert_eq!(view["source"], "verified-seed");
    }

    #[tokio::test]
    async fn list_models_falls_back_to_seed_without_credential() {
        let dir = TempDir::new("seed");
        let channel = channel(&dir);
        let view = call(
            &channel,
            "listModels",
            serde_json::json!({ "capability": "chat" }),
        )
        .await;
        assert_eq!(view["source"], "verified-seed");
        assert_eq!(view["degraded"], true);
        assert_eq!(view["models"].as_array().expect("models").len(), 5);
    }

    #[tokio::test]
    async fn invalid_capability_and_unknown_command_error() {
        let dir = TempDir::new("errors");
        let channel = channel(&dir);
        let error = channel
            .call(
                "ctx".into(),
                "listModels".into(),
                Value::Json(serde_json::json!({ "capability": "nope" }).to_string()),
                CancellationToken::default(),
            )
            .await
            .expect_err("capability");
        assert!(format!("{error:?}").contains("invalid OrcaRouter capability"));
        let error = channel
            .call(
                "ctx".into(),
                "notARealMethod".into(),
                Value::Json("{}".to_string()),
                CancellationToken::default(),
            )
            .await
            .expect_err("unknown");
        assert!(format!("{error:?}").contains("unknown command"));
    }
}
