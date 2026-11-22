//! OrcaRouter 模型目录：live 拉取 + 能力过滤 + last-known-good / verified seed 降级。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::credential::OrcaCredentialStore;
use crate::origins::{build_orca_models_url, OrcaOrigins};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrcaCapability {
    Chat,
    Embedding,
    Image,
    Video,
    Rerank,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrcaModelRecord {
    pub id: String,
    pub supported_endpoint_types: Vec<String>,
    pub input_modalities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrcaCatalogSource {
    Live,
    LastKnownGood,
    VerifiedSeed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaCatalogView {
    pub capability: OrcaCapability,
    pub source: OrcaCatalogSource,
    pub degraded: bool,
    pub fetched_at: i64,
    pub total_before_filter: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub models: Vec<OrcaModelRecordView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaModelRecordView {
    pub model_id: String,
    pub input_modalities: Vec<String>,
    pub supported_endpoint_types: Vec<String>,
}

pub struct OrcaCatalogInput {
    pub capability: OrcaCapability,
    pub required_input_modality: Option<&'static str>,
    pub force_refresh: bool,
}

const VERIFIED_SEED: [(&str, &[&str]); 5] = [
    ("openai/gpt-5.5", &["text", "image"]),
    ("anthropic/claude-opus-4.8", &["text", "image"]),
    ("google/gemini-3.5-flash", &["text", "image"]),
    ("deepseek/deepseek-v4-pro", &["text"]),
    ("orcarouter/auto", &["text"]),
];

fn seed_records() -> Vec<OrcaModelRecord> {
    VERIFIED_SEED
        .iter()
        .map(|(id, modalities)| OrcaModelRecord {
            id: id.to_string(),
            supported_endpoint_types: vec!["openai".to_string(), "anthropic".to_string()],
            input_modalities: modalities.iter().map(|value| value.to_string()).collect(),
        })
        .collect()
}

fn read_string_array(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn parse_model_record(value: &Value) -> Option<OrcaModelRecord> {
    let object = value.as_object()?;
    let id = object.get("id")?.as_str()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let supported_endpoint_types = read_string_array(object.get("supported_endpoint_types"));
    let input_modalities = object
        .get("architecture")
        .and_then(Value::as_object)
        .and_then(|architecture| architecture.get("input_modalities"))
        .map(|value| read_string_array(Some(value)))
        .unwrap_or_default();
    Some(OrcaModelRecord {
        id,
        supported_endpoint_types,
        input_modalities,
    })
}

pub fn parse_catalog(value: &Value, max_items: usize) -> Result<Vec<OrcaModelRecord>, String> {
    let data = value
        .as_object()
        .and_then(|object| object.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| "OrcaRouter 模型目录响应缺少 data 数组".to_string())?;
    Ok(data
        .iter()
        .take(max_items)
        .filter_map(parse_model_record)
        .collect())
}

fn supports_text_chat(record: &OrcaModelRecord) -> bool {
    const TEXT_ENDPOINTS: [&str; 4] = ["openai", "anthropic", "gemini", "openai-response"];
    record
        .supported_endpoint_types
        .iter()
        .any(|value| TEXT_ENDPOINTS.contains(&value.as_str()))
}

fn has_non_text_endpoint(record: &OrcaModelRecord) -> bool {
    const NON_TEXT: [&str; 4] = [
        "image-generation",
        "openai-video",
        "jina-rerank",
        "embeddings",
    ];
    record
        .supported_endpoint_types
        .iter()
        .any(|value| NON_TEXT.contains(&value.as_str()))
}

pub fn filter_models(
    records: &[OrcaModelRecord],
    capability: OrcaCapability,
    required_input_modality: Option<&str>,
) -> Vec<OrcaModelRecord> {
    records
        .iter()
        .filter(|record| match capability {
            OrcaCapability::Chat => {
                if !supports_text_chat(record) || has_non_text_endpoint(record) {
                    return false;
                }
                required_input_modality.is_none_or(|modality| {
                    record
                        .input_modalities
                        .iter()
                        .any(|value| value == modality)
                })
            }
            OrcaCapability::Embedding => record
                .supported_endpoint_types
                .iter()
                .any(|value| value == "embeddings"),
            OrcaCapability::Image => record
                .supported_endpoint_types
                .iter()
                .any(|value| value == "image-generation"),
            OrcaCapability::Video => record
                .supported_endpoint_types
                .iter()
                .any(|value| value == "openai-video"),
            OrcaCapability::Rerank => record
                .supported_endpoint_types
                .iter()
                .any(|value| value == "jina-rerank"),
        })
        .cloned()
        .collect()
}

pub type CatalogLog = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct OrcaCatalogOptions {
    pub credential_store: Arc<OrcaCredentialStore>,
    pub origins: OrcaOrigins,
    pub timeout_ms: u64,
    pub ttl_ms: i64,
    pub max_response_bytes: usize,
    pub max_items: usize,
    pub now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub log: Option<CatalogLog>,
}

impl OrcaCatalogOptions {
    pub fn new(credential_store: Arc<OrcaCredentialStore>, origins: OrcaOrigins) -> Self {
        Self {
            credential_store,
            origins,
            timeout_ms: 10_000,
            ttl_ms: 60_000,
            max_response_bytes: 512 * 1024,
            max_items: 500,
            now_ms: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as i64)
                    .unwrap_or(0)
            }),
            log: None,
        }
    }
}

struct CachedCatalog {
    at: i64,
    records: Vec<OrcaModelRecord>,
}

pub struct OrcaCatalogService {
    store: Arc<OrcaCredentialStore>,
    origins: OrcaOrigins,
    client: reqwest::Client,
    timeout: Duration,
    ttl_ms: i64,
    max_response_bytes: usize,
    max_items: usize,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    log: Option<CatalogLog>,
    cache: Mutex<Option<CachedCatalog>>,
    epoch: AtomicU64,
    fetch_lock: tokio::sync::Mutex<()>,
}

impl OrcaCatalogService {
    pub fn new(options: OrcaCatalogOptions) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(options.timeout_ms.max(1)))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            store: options.credential_store,
            origins: options.origins,
            client,
            timeout: Duration::from_millis(options.timeout_ms.max(1)),
            ttl_ms: options.ttl_ms,
            max_response_bytes: options.max_response_bytes,
            max_items: options.max_items,
            now_ms: options.now_ms,
            log: options.log,
            cache: Mutex::new(None),
            epoch: AtomicU64::new(0),
            fetch_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn log(&self, level: &str, message: &str) {
        if let Some(log) = &self.log {
            log(level, message);
        }
    }

    fn cached_records(&self) -> Option<Vec<OrcaModelRecord>> {
        self.cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .map(|cached| cached.records.clone())
    }

    /// 丢弃缓存（切换 provider / 手动刷新 / 凭据变化时调用）。
    pub fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        *self
            .cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
    }

    async fn fetch_live(&self) -> Result<Vec<OrcaModelRecord>, String> {
        let credential = self
            .store
            .load_usable()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "尚未连接 OrcaRouter，无法获取模型目录".to_string())?;
        let response = self
            .client
            .get(build_orca_models_url(&self.origins.api_base))
            .header(reqwest::header::ACCEPT, "application/json")
            .bearer_auth(&credential.api_key)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    "模型目录请求超时".to_string()
                } else {
                    "无法连接 OrcaRouter 模型目录".to_string()
                }
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("模型目录请求失败：HTTP {status}"));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "模型目录响应读取失败".to_string())?;
        if bytes.len() > self.max_response_bytes {
            return Err("模型目录响应超过大小上限".to_string());
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "模型目录响应不是合法 JSON".to_string())?;
        parse_catalog(&value, self.max_items)
    }

    async fn load_records(&self, force_refresh: bool) -> Result<Vec<OrcaModelRecord>, String> {
        if !force_refresh {
            if let Some(records) = self.fresh_records() {
                return Ok(records);
            }
        }
        let _guard = self.fetch_lock.lock().await;
        // 等锁期间可能有其它调用已完成刷新。
        if !force_refresh {
            if let Some(records) = self.fresh_records() {
                return Ok(records);
            }
        }
        let request_epoch = self.epoch.load(Ordering::SeqCst);
        let records = self.fetch_live().await?;
        if request_epoch != self.epoch.load(Ordering::SeqCst) {
            return Err("模型目录请求已被更新的凭据取代".to_string());
        }
        *self
            .cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(CachedCatalog {
            at: (self.now_ms)(),
            records: records.clone(),
        });
        Ok(records)
    }

    fn fresh_records(&self) -> Option<Vec<OrcaModelRecord>> {
        let guard = self
            .cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let cached = guard.as_ref()?;
        if (self.now_ms)() - cached.at < self.ttl_ms {
            Some(cached.records.clone())
        } else {
            None
        }
    }

    pub async fn list(&self, input: OrcaCatalogInput) -> OrcaCatalogView {
        let fetched_at = (self.now_ms)();
        let required = input.required_input_modality;
        match self.load_records(input.force_refresh).await {
            Ok(records) => self.view(
                &records,
                input.capability,
                required,
                OrcaCatalogSource::Live,
                false,
                fetched_at,
                None,
            ),
            Err(message) => {
                if let Some(cached) = self.cached_records() {
                    self.log(
                        "warn",
                        &format!("模型目录刷新失败，使用最近一次成功目录：{message}"),
                    );
                    return self.view(
                        &cached,
                        input.capability,
                        required,
                        OrcaCatalogSource::LastKnownGood,
                        true,
                        fetched_at,
                        Some(message),
                    );
                }
                self.log(
                    "warn",
                    &format!("模型目录不可用，回退到已验证 seed：{message}"),
                );
                self.view(
                    &seed_records(),
                    input.capability,
                    required,
                    OrcaCatalogSource::VerifiedSeed,
                    true,
                    fetched_at,
                    Some(message),
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn view(
        &self,
        records: &[OrcaModelRecord],
        capability: OrcaCapability,
        required: Option<&str>,
        source: OrcaCatalogSource,
        degraded: bool,
        fetched_at: i64,
        error: Option<String>,
    ) -> OrcaCatalogView {
        let models = filter_models(records, capability, required)
            .into_iter()
            .map(|record| OrcaModelRecordView {
                model_id: record.id,
                input_modalities: record.input_modalities,
                supported_endpoint_types: record.supported_endpoint_types,
            })
            .collect();
        OrcaCatalogView {
            capability,
            source,
            degraded,
            fetched_at,
            total_before_filter: records.len(),
            error,
            models,
        }
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout.as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parse_and_filter_match_shared_rules() {
        let payload = serde_json::json!({
            "data": [
                { "id": "openai/gpt", "supported_endpoint_types": ["openai", "anthropic"],
                  "architecture": { "input_modalities": ["text", "image"] } },
                { "id": "img", "supported_endpoint_types": ["image-generation"] },
                { "id": "embed", "supported_endpoint_types": ["embeddings"] },
                { "id": "chat-no-modality", "supported_endpoint_types": ["openai"] },
                { "id": "", "supported_endpoint_types": ["openai"] }
            ]
        });
        let records = parse_catalog(&payload, 500).expect("records");
        assert_eq!(records.len(), 4, "{records:?}");

        let chat = filter_models(&records, OrcaCapability::Chat, None);
        assert_eq!(
            chat.iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            vec!["openai/gpt", "chat-no-modality"]
        );
        // 多模态入口 fail closed：未声明 input_modalities 的模型被排除。
        let image = filter_models(&records, OrcaCapability::Chat, Some("image"));
        assert_eq!(image.len(), 1);
        assert_eq!(image[0].id, "openai/gpt");
        assert_eq!(
            filter_models(&records, OrcaCapability::Embedding, None)[0].id,
            "embed"
        );
        assert_eq!(
            filter_models(&records, OrcaCapability::Image, None)[0].id,
            "img"
        );
    }

    #[test]
    fn parse_catalog_requires_data_array() {
        assert!(parse_catalog(&serde_json::json!({}), 10).is_err());
        assert!(parse_catalog(&serde_json::json!({ "data": [] }), 10)
            .expect("empty")
            .is_empty());
    }

    #[tokio::test]
    async fn live_catalog_uses_bearer_and_filters() {
        let dir = std::env::temp_dir().join(format!(
            "orca-catalog-live-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let store = Arc::new(OrcaCredentialStore::new(Arc::new(
            credential_store::CredentialStore::with_secret(&dir, "catalog-test-secret"),
        )));
        store
            .save(
                "sk-orca-abcdef123456",
                crate::credential::OrcaCredentialSource::ApiKey,
                None,
                None,
            )
            .expect("save");
        let (port, server) = spawn_catalog_server(
            "200 OK",
            serde_json::json!({
                "data": [
                    { "id": "openai/gpt", "supported_endpoint_types": ["openai"] },
                    { "id": "embed", "supported_endpoint_types": ["embeddings"] }
                ]
            })
            .to_string(),
        )
        .await;
        let service = OrcaCatalogService::new(OrcaCatalogOptions {
            credential_store: Arc::clone(&store),
            origins: OrcaOrigins {
                auth_base: "https://auth.example".to_string(),
                api_base: format!("http://127.0.0.1:{port}"),
            },
            ..OrcaCatalogOptions::new(
                Arc::clone(&store),
                OrcaOrigins {
                    auth_base: "https://auth.example".to_string(),
                    api_base: "https://api.example".to_string(),
                },
            )
        })
        .expect("service");
        let view = service
            .list(OrcaCatalogInput {
                capability: OrcaCapability::Chat,
                required_input_modality: None,
                force_refresh: false,
            })
            .await;
        assert_eq!(view.source, OrcaCatalogSource::Live, "{:?}", view.error);
        assert!(!view.degraded);
        assert_eq!(view.models.len(), 1);
        assert_eq!(view.models[0].model_id, "openai/gpt");
        let request = server.await.expect("server");
        assert!(request.starts_with("GET /v1/models"), "{request}");
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer sk-orca-abcdef123456"),
            "{request}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_credential_falls_back_to_seed() {
        let dir = std::env::temp_dir().join(format!(
            "orca-catalog-seed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let store = Arc::new(OrcaCredentialStore::new(Arc::new(
            credential_store::CredentialStore::with_secret(&dir, "catalog-test-secret"),
        )));
        let service = OrcaCatalogService::new(OrcaCatalogOptions::new(
            Arc::clone(&store),
            OrcaOrigins {
                auth_base: "https://auth.example".to_string(),
                api_base: "https://api.example".to_string(),
            },
        ))
        .expect("service");
        let view = service
            .list(OrcaCatalogInput {
                capability: OrcaCapability::Chat,
                required_input_modality: None,
                force_refresh: false,
            })
            .await;
        assert_eq!(view.source, OrcaCatalogSource::VerifiedSeed);
        assert!(view.degraded);
        assert!(view
            .error
            .as_deref()
            .is_some_and(|error| error.contains("尚未连接")));
        assert_eq!(view.models.len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn spawn_catalog_server(
        status: &'static str,
        body: String,
    ) -> (u16, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut data = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.expect("read");
                if read == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..read]);
                if data.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&data).into_owned();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
            request
        });
        (port, handle)
    }

    #[test]
    fn seed_is_text_chat_compatible() {
        let records = seed_records();
        assert_eq!(filter_models(&records, OrcaCapability::Chat, None).len(), 5);
        assert_eq!(
            filter_models(&records, OrcaCapability::Chat, Some("image")).len(),
            3
        );
    }
}
