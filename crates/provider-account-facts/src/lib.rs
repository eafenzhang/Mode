//! 账号 Provider 事实注入：设置、OAuth 身份、缓存 Key 与 Coding Plan 可用性。
//!
//! 真源：`packages/services/src/model-provider/accountProviderConnectionResolver.ts` 的生产
//! options（`readSettings` / `loadAccountIdentity` / `loadCodingPlanApiKey` /
//! `resolveFamilyAvailability`）。本 crate 把它们组合成
//! `account_provider_overlay::AccountProviderOverlayOptions`，供 `provider-source-chain`
//! 与壳侧 `AgentProviderSource` 直接使用。
//!
//! 凭据与设置由端口注入；真实 HTTP 由 `coding-plan-http::CodingPlanHttpPort` 注入。

use std::sync::Arc;

use account_api_key_resolver::{
    AccountApiHttpPort, AccountApiKeyResolver, AccountApiProvider,
};
use agent_protocol::provider_runtime_headers::{
    ProviderAccountAccess, ProviderRuntimeAccountMode,
};
use account_provider_overlay::{
    AccountProviderConnectionSettings, AccountProviderFamilyAvailabilityInput,
    AccountProviderOverlayError, AccountProviderOverlayOptions, BoxFuture,
    CodingPlanAvailabilityResult, CodingPlanAvailabilityProvider, PlanKind, ProviderFamilyDomain,
};
use coding_plan_http::{CodingPlanAvailability, CodingPlanHttpPort};
use indexmap::IndexMap;
use serde_json::Value;

pub const MODE_JWT_TOKEN_KEY: &str = "zcodejwttoken";
const START_PLAN_MISSING_AUTH: &str = "";

/// 设置端口：返回已经解析好的连接设置（宿主从 settings store 读取）。
pub trait AccountSettingsPort: Send + Sync {
    fn read(&self) -> AccountProviderConnectionSettings;
}

/// 凭据端口：与 `credential-store` 的 load/save 同形。
pub trait AccountCredentialPort: Send + Sync {
    fn load(&self, key: &str) -> Option<String>;
    fn save(&self, key: &str, value: &str) -> Result<(), String>;
}

/// OAuth access token → Coding Plan API Key 的交换端口。
pub trait CodingPlanKeyExchange: Send + Sync {
    fn resolve<'a>(
        &'a self,
        family: ProviderFamilyDomain,
        access_token: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>,
    >;
}

/// 团队 Coding Plan 项目 Key 解析端口（需要 team org/project）。
pub trait TeamCodingPlanKeyResolver: Send + Sync {
    fn resolve_team<'a>(
        &'a self,
        family: ProviderFamilyDomain,
        access_token: &'a str,
        organization_id: &'a str,
        project_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>;
}

/// 把 [`AccountApiKeyResolver`] 适配成个人/团队交换端口。
pub struct AccountApiKeyExchange<P: AccountApiHttpPort> {
    resolver: Arc<AccountApiKeyResolver<P>>,
}

impl<P: AccountApiHttpPort> AccountApiKeyExchange<P> {
    pub fn new(resolver: AccountApiKeyResolver<P>) -> Self {
        Self {
            resolver: Arc::new(resolver),
        }
    }
}

impl<P: AccountApiHttpPort> Clone for AccountApiKeyExchange<P> {
    fn clone(&self) -> Self {
        Self {
            resolver: Arc::clone(&self.resolver),
        }
    }
}

impl<P: AccountApiHttpPort + 'static> CodingPlanKeyExchange for AccountApiKeyExchange<P> {
    fn resolve<'a>(
        &'a self,
        family: ProviderFamilyDomain,
        access_token: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>,
    > {
        let provider = match family {
            ProviderFamilyDomain::Zai => AccountApiProvider::Zai,
            ProviderFamilyDomain::Bigmodel => AccountApiProvider::Bigmodel,
        };
        Box::pin(self.resolver.resolve(provider, access_token))
    }
}

impl<P: AccountApiHttpPort + 'static> TeamCodingPlanKeyResolver for AccountApiKeyExchange<P> {
    fn resolve_team<'a>(
        &'a self,
        family: ProviderFamilyDomain,
        access_token: &'a str,
        organization_id: &'a str,
        project_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        let provider = match family {
            ProviderFamilyDomain::Zai => AccountApiProvider::Zai,
            ProviderFamilyDomain::Bigmodel => AccountApiProvider::Bigmodel,
        };
        Box::pin(self.resolver.resolve_team_key(
            provider,
            access_token,
            organization_id,
            project_id,
        ))
    }
}

/// URL 端口：订阅列表与 Start Plan balance 的业务地址由宿主解析。
pub trait CodingPlanUrlPort: Send + Sync {
    fn subscription_list_url(&self, family: ProviderFamilyDomain) -> String;
    fn start_plan_balance_url(&self) -> String;
    /// 团队/账号业务接口的 host（ZAI Business / BigModel origin）。
    fn team_host(&self, family: ProviderFamilyDomain) -> String;
}

pub const DEFAULT_MODE_ENDPOINT_ORIGIN: &str = "https://zcode.z.ai";
pub const DEFAULT_BIGMODEL_API_ORIGIN: &str = "https://bigmodel.cn";
pub const DEFAULT_ZAI_BUSINESS_BASE_URL: &str = "https://api.z.ai";
const SUBSCRIPTION_LIST_PATH: &str = "/api/biz/subscription/list";
const START_PLAN_BALANCE_PATH: &str = "/api/v1/mode-plan/billing/balance";

/// 环境变量驱动的业务地址解析（`MODE_ENDPOINT_ORIGIN` / `MODE_BASE_URL` /
/// `ZAI_BUSINESS_BASE_URL` / `BIGMODEL_API_BASE_URL`），与 TS `modeEndpoint.ts` 同口径。
#[derive(Clone)]
pub struct EnvCodingPlanUrls {
    mode_origin: String,
    zai_business_base: String,
    bigmodel_origin: String,
    app_version: String,
}

impl EnvCodingPlanUrls {
    pub fn from_process(app_version: impl Into<String>) -> Self {
        Self::with_env(std::env::vars().collect(), app_version)
    }

    pub fn with_env(
        env: std::collections::BTreeMap<String, String>,
        app_version: impl Into<String>,
    ) -> Self {
        let read = |key: &str| {
            env.get(key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let mode_origin = read("MODE_BASE_URL")
            .or_else(|| read("MODE_ENDPOINT_ORIGIN"))
            .unwrap_or_else(|| DEFAULT_MODE_ENDPOINT_ORIGIN.to_string());
        Self {
            mode_origin: normalize_origin(&mode_origin, DEFAULT_MODE_ENDPOINT_ORIGIN),
            zai_business_base: normalize_origin(
                &read("ZAI_BUSINESS_BASE_URL")
                    .unwrap_or_else(|| DEFAULT_ZAI_BUSINESS_BASE_URL.to_string()),
                DEFAULT_ZAI_BUSINESS_BASE_URL,
            ),
            bigmodel_origin: normalize_origin(
                &read("BIGMODEL_API_BASE_URL")
                    .unwrap_or_else(|| DEFAULT_BIGMODEL_API_ORIGIN.to_string()),
                DEFAULT_BIGMODEL_API_ORIGIN,
            ),
            app_version: app_version.into(),
        }
    }
}

impl EnvCodingPlanUrls {
    pub fn mode_origin(&self) -> &str {
        &self.mode_origin
    }

    pub fn zai_business_base(&self) -> &str {
        &self.zai_business_base
    }

    pub fn bigmodel_origin(&self) -> &str {
        &self.bigmodel_origin
    }
}

impl CodingPlanUrlPort for EnvCodingPlanUrls {
    fn subscription_list_url(&self, family: ProviderFamilyDomain) -> String {
        match family {
            ProviderFamilyDomain::Zai => {
                format!("{}{SUBSCRIPTION_LIST_PATH}", self.zai_business_base)
            }
            ProviderFamilyDomain::Bigmodel => {
                format!("{}{SUBSCRIPTION_LIST_PATH}", self.bigmodel_origin)
            }
        }
    }

    fn start_plan_balance_url(&self) -> String {
        let base = format!("{}{START_PLAN_BALANCE_PATH}", self.mode_origin);
        zai_start_plan_billing::build_zai_start_plan_balance_url_for(&base, &self.app_version)
            .unwrap_or(base)
    }

    fn team_host(&self, family: ProviderFamilyDomain) -> String {
        match family {
            ProviderFamilyDomain::Zai => self.zai_business_base.clone(),
            ProviderFamilyDomain::Bigmodel => self.bigmodel_origin.clone(),
        }
    }
}

fn normalize_origin(value: &str, fallback: &str) -> String {
    match url::Url::parse(value.trim()) {
        Ok(url) if url.scheme() == "http" || url.scheme() == "https" => {
            url.origin().ascii_serialization()
        }
        _ => fallback.to_string(),
    }
}

pub struct ProviderAccountFacts<P: CodingPlanHttpPort> {
    http: CodingPlanAvailability<P>,
    settings: Arc<dyn AccountSettingsPort>,
    credentials: Arc<dyn AccountCredentialPort>,
    urls: Arc<dyn CodingPlanUrlPort>,
    now_seconds: Arc<dyn Fn() -> f64 + Send + Sync>,
    key_exchange: Option<Arc<dyn CodingPlanKeyExchange>>,
    team_key_resolver: Option<Arc<dyn TeamCodingPlanKeyResolver>>,
}

impl<P: CodingPlanHttpPort> ProviderAccountFacts<P> {
    pub fn new(
        http: P,
        settings: Arc<dyn AccountSettingsPort>,
        credentials: Arc<dyn AccountCredentialPort>,
        urls: Arc<dyn CodingPlanUrlPort>,
        now_seconds: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> Self {
        Self {
            http: CodingPlanAvailability::new(http),
            settings,
            credentials,
            urls,
            now_seconds,
            key_exchange: None,
            team_key_resolver: None,
        }
    }

    /// 注入 OAuth→API Key 兑换端口；未注入时 `force_refresh` 仍只读缓存。
    pub fn with_key_exchange(mut self, exchange: Arc<dyn CodingPlanKeyExchange>) -> Self {
        self.key_exchange = Some(exchange);
        self
    }

    /// 注入团队项目 Key 解析端口。
    pub fn with_team_key_resolver(
        mut self,
        resolver: Arc<dyn TeamCodingPlanKeyResolver>,
    ) -> Self {
        self.team_key_resolver = Some(resolver);
        self
    }

    /// `interaction/requestProviderRuntimeHeaders` 的凭据解析：
    /// Start Plan → mode JWT；Individual → 缓存/兑换；Team → 团队项目 Key。
    pub async fn resolve_runtime_api_key(
        &self,
        provider_id: &str,
        access: &ProviderAccountAccess,
    ) -> Option<String> {
        let family = match access.account_type {
            agent_protocol::provider_runtime_headers::ProviderRuntimeAccountType::Zai => {
                ProviderFamilyDomain::Zai
            }
            agent_protocol::provider_runtime_headers::ProviderRuntimeAccountType::Bigmodel => {
                ProviderFamilyDomain::Bigmodel
            }
        };
        match access.mode {
            ProviderRuntimeAccountMode::StartPlan => self
                .credentials
                .load(MODE_JWT_TOKEN_KEY)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            ProviderRuntimeAccountMode::IndividualCodingPlan
            | ProviderRuntimeAccountMode::OffPeak => {
                let identity = self.account_identity(family)?;
                if let Some(cached) = self.cached_coding_plan_key(provider_id, &identity) {
                    return Some(cached);
                }
                let exchange = self.key_exchange.as_ref()?;
                let provider = self.provider_slug(family);
                let token = self.oauth_access_token(provider)?;
                let key = exchange.resolve(family, &token).await?;
                let key_name = format!(
                    "account-provider:coding-plan:{provider_id}:account:{}:api-key",
                    encode_uri_component(&identity)
                );
                let _ = self.credentials.save(&key_name, &key);
                Some(key)
            }
            ProviderRuntimeAccountMode::TeamCodingPlan => {
                let resolver = self.team_key_resolver.as_ref()?;
                let settings = self.settings.read();
                let selection = settings.selections.get(family)?;
                let account_provider_overlay::ProviderFamilyConnectionSelection::TeamCodingPlan {
                    organization_id,
                    project_id,
                    ..
                } = selection
                else {
                    return None;
                };
                let provider = self.provider_slug(family);
                let token = self.oauth_access_token(provider)?;
                if family == ProviderFamilyDomain::Bigmodel {
                    let mode_jwt = self
                        .credentials
                        .load(MODE_JWT_TOKEN_KEY)
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty());
                    if mode_jwt.as_deref() == Some(token.as_str()) {
                        return None;
                    }
                }
                resolver
                    .resolve_team(family, &token, organization_id, project_id)
                    .await
            }
        }
    }

    fn provider_slug(&self, family: ProviderFamilyDomain) -> &'static str {
        match family {
            ProviderFamilyDomain::Zai => "zai",
            ProviderFamilyDomain::Bigmodel => "bigmodel",
        }
    }

    fn oauth_access_token(&self, provider: &str) -> Option<String> {
        self.credentials
            .load(&format!("oauth:{provider}:access_token"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    async fn team_availability(
        &self,
        family: ProviderFamilyDomain,
    ) -> CodingPlanAvailabilityResult {
        let settings = self.settings.read();
        let Some(selection) = settings.selections.get(family) else {
            return CodingPlanAvailabilityResult::Unknown;
        };
        let team = match selection {
            account_provider_overlay::ProviderFamilyConnectionSelection::TeamCodingPlan {
                organization_id,
                project_id,
                ..
            } => (organization_id.clone(), project_id.clone()),
            _ => return CodingPlanAvailabilityResult::Unknown,
        };
        let provider = match family {
            ProviderFamilyDomain::Zai => "zai",
            ProviderFamilyDomain::Bigmodel => "bigmodel",
        };
        let token = self
            .credentials
            .load(&format!("oauth:{provider}:access_token"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let Some(token) = token else {
            return CodingPlanAvailabilityResult::Unavailable {
                reason: account_provider_overlay::CodingPlanUnavailableReason::CodingPlanNotConnected,
            };
        };
        if family == ProviderFamilyDomain::Bigmodel {
            let mode_jwt = self
                .credentials
                .load(MODE_JWT_TOKEN_KEY)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            if mode_jwt.as_deref() == Some(token.as_str()) {
                return CodingPlanAvailabilityResult::Unavailable {
                    reason:
                        account_provider_overlay::CodingPlanUnavailableReason::CodingPlanNotConnected,
                };
            }
        }
        self.http
            .team_availability(
                family,
                &self.urls.team_host(family),
                &token,
                &team.0,
                &team.1,
            )
            .await
    }

    fn account_identity(&self, family: ProviderFamilyDomain) -> Option<String> {
        let provider = match family {
            ProviderFamilyDomain::Zai => "zai",
            ProviderFamilyDomain::Bigmodel => "bigmodel",
        };
        let raw = self
            .credentials
            .load(&format!("oauth:{provider}:user_info"))?;
        let value: Value = serde_json::from_str(&raw).ok()?;
        value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    fn cached_coding_plan_key(&self, provider_id: &str, identity: &str) -> Option<String> {
        let key = format!(
            "account-provider:coding-plan:{provider_id}:account:{}:api-key",
            encode_uri_component(identity)
        );
        self.credentials
            .load(&key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    async fn availability(
        &self,
        input: AccountProviderFamilyAvailabilityInput,
    ) -> IndexMap<String, CodingPlanAvailabilityResult> {
        let mut results = IndexMap::new();
        for provider in input.providers {
            let result = self.provider_availability(&provider).await;
            results.insert(provider.provider_id, result);
        }
        results
    }

    async fn provider_availability(
        &self,
        provider: &CodingPlanAvailabilityProvider,
    ) -> CodingPlanAvailabilityResult {
        match provider.plan_kind {
            PlanKind::StartPlan => {
                let authorization = self
                    .credentials
                    .load(MODE_JWT_TOKEN_KEY)
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .map(|jwt| format!("Bearer {jwt}"))
                    .or_else(|| {
                        provider
                            .api_key
                            .as_deref()
                            .map(strip_bearer)
                            .filter(|value| !value.is_empty())
                    })
                    .unwrap_or_else(|| START_PLAN_MISSING_AUTH.to_string());
                self.http
                    .start_plan_availability(
                        &self.urls.start_plan_balance_url(),
                        &authorization,
                        (self.now_seconds)(),
                    )
                    .await
            }
            PlanKind::IndividualCodingPlan => {
                let authorization = provider
                    .api_key
                    .as_deref()
                    .map(strip_bearer)
                    .unwrap_or_default();
                let url = self.urls.subscription_list_url(provider.family);
                self.http
                    .subscription_availability(&url, &authorization)
                    .await
            }
            PlanKind::TeamCodingPlan => self.team_availability(provider.family).await,
        }
    }
}

impl<P: CodingPlanHttpPort> AccountProviderOverlayOptions for ProviderAccountFacts<P> {
    fn read_settings(
        &self,
    ) -> BoxFuture<'_, Result<AccountProviderConnectionSettings, AccountProviderOverlayError>> {
        let settings = self.settings.read();
        Box::pin(async move { Ok(settings) })
    }

    fn load_account_identity(
        &self,
        family: ProviderFamilyDomain,
    ) -> BoxFuture<'_, Result<Option<String>, AccountProviderOverlayError>> {
        let identity = self.account_identity(family);
        Box::pin(async move { Ok(identity) })
    }

    fn load_coding_plan_api_key<'a>(
        &'a self,
        provider_id: &'a str,
        family: ProviderFamilyDomain,
        account_identity: &'a str,
        force_refresh: bool,
    ) -> BoxFuture<'a, Result<Option<String>, AccountProviderOverlayError>> {
        Box::pin(async move {
            if !force_refresh {
                if let Some(cached) = self.cached_coding_plan_key(provider_id, account_identity) {
                    return Ok(Some(cached));
                }
            }
            let Some(exchange) = &self.key_exchange else {
                return Ok(None);
            };
            let provider = match family {
                ProviderFamilyDomain::Zai => "zai",
                ProviderFamilyDomain::Bigmodel => "bigmodel",
            };
            let access_token = self
                .credentials
                .load(&format!("oauth:{provider}:access_token"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let Some(access_token) = access_token else {
                return Ok(None);
            };
            let Some(api_key) = exchange.resolve(family, &access_token).await else {
                return Ok(None);
            };
            let key = format!(
                "account-provider:coding-plan:{provider_id}:account:{}:api-key",
                encode_uri_component(account_identity)
            );
            let _ = self.credentials.save(&key, &api_key);
            Ok(Some(api_key))
        })
    }

    fn resolve_family_availability<'a>(
        &'a self,
        input: AccountProviderFamilyAvailabilityInput,
    ) -> BoxFuture<
        'a,
        Result<IndexMap<String, CodingPlanAvailabilityResult>, AccountProviderOverlayError>,
    > {
        Box::pin(async move { Ok(self.availability(input).await) })
    }
}

fn strip_bearer(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix("Bearer ")
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// `encodeURIComponent`：保留 `A-Za-z0-9-_.!~*'()`，其余 UTF-8 字节做百分号编码。
pub fn encode_uri_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if keep {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use account_provider_overlay::{
        CodingPlanUnavailableReason, ProviderFamilyConnectionSelectionSettings,
    };
    use coding_plan_http::{HttpFuture, HttpRequest, HttpResponse, HttpTransportError};
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    struct FakeHttp {
        requests: Mutex<Vec<HttpRequest>>,
        response: Mutex<Result<HttpResponse, HttpTransportError>>,
    }

    impl FakeHttp {
        fn ok(body: Value) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(Ok(HttpResponse {
                    status: 200,
                    body: serde_json::to_vec(&body).unwrap(),
                })),
            }
        }
    }

    impl CodingPlanHttpPort for FakeHttp {
        fn get<'a>(&'a self, request: &'a HttpRequest) -> HttpFuture<'a> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request.clone());
                self.response.lock().unwrap().clone()
            })
        }
    }

    struct QueueHttp {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<VecDeque<HttpResponse>>,
    }

    impl QueueHttp {
        fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(
                    responses
                        .into_iter()
                        .map(|body| HttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&body).unwrap(),
                        })
                        .collect(),
                ),
            }
        }
    }

    impl CodingPlanHttpPort for QueueHttp {
        fn get<'a>(&'a self, request: &'a HttpRequest) -> HttpFuture<'a> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request.clone());
                Ok(self
                    .responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("queued response"))
            })
        }
    }

    struct Settings(AccountProviderConnectionSettings);
    impl AccountSettingsPort for Settings {
        fn read(&self) -> AccountProviderConnectionSettings {
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct Credentials(HashMap<String, String>);
    impl AccountCredentialPort for Credentials {
        fn load(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn save(&self, _key: &str, _value: &str) -> Result<(), String> {
            Ok(())
        }
    }

    struct Urls;
    impl CodingPlanUrlPort for Urls {
        fn subscription_list_url(&self, family: ProviderFamilyDomain) -> String {
            match family {
                ProviderFamilyDomain::Zai => "https://zai.test/list".to_string(),
                ProviderFamilyDomain::Bigmodel => "https://bigmodel.test/list".to_string(),
            }
        }
        fn start_plan_balance_url(&self) -> String {
            "https://zai.test/balance".to_string()
        }
        fn team_host(&self, family: ProviderFamilyDomain) -> String {
            match family {
                ProviderFamilyDomain::Zai => "https://zai.test".to_string(),
                ProviderFamilyDomain::Bigmodel => "https://bigmodel.test".to_string(),
            }
        }
    }

    impl ProviderAccountFacts<FakeHttp> {
        fn http_requests(&self) -> Vec<HttpRequest> {
            self.http.port().requests.lock().unwrap().clone()
        }
    }

    fn settings() -> AccountProviderConnectionSettings {
        AccountProviderConnectionSettings {
            provider_family_domain: Some(ProviderFamilyDomain::Zai),
            selections: ProviderFamilyConnectionSelectionSettings::default(),
            unresolved_families: None,
        }
    }

    fn facts(
        http: FakeHttp,
        credentials: Credentials,
    ) -> ProviderAccountFacts<FakeHttp> {
        ProviderAccountFacts::new(
            http,
            Arc::new(Settings(settings())),
            Arc::new(credentials),
            Arc::new(Urls),
            Arc::new(|| 1_000.0),
        )
    }

    #[tokio::test]
    async fn reads_settings_and_oauth_identity() {
        let mut credentials = Credentials::default();
        credentials.0.insert(
            "oauth:zai:user_info".to_string(),
            r#"{"id":" user-1 "}"#.to_string(),
        );
        let facts = facts(FakeHttp::ok(serde_json::json!({})), credentials);
        assert_eq!(facts.read_settings().await.unwrap(), settings());
        assert_eq!(
            facts
                .load_account_identity(ProviderFamilyDomain::Zai)
                .await
                .unwrap()
                .as_deref(),
            Some("user-1")
        );
        assert_eq!(
            facts
                .load_account_identity(ProviderFamilyDomain::Bigmodel)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn coding_plan_key_uses_encoded_identity_and_skips_force_refresh() {
        let mut credentials = Credentials::default();
        credentials.0.insert(
            "account-provider:coding-plan:p1:account:user%40example.com:api-key".to_string(),
            " sk-cached ".to_string(),
        );
        let facts = facts(FakeHttp::ok(serde_json::json!({})), credentials);
        assert_eq!(
            facts
                .load_coding_plan_api_key("p1", ProviderFamilyDomain::Zai, "user@example.com", false)
                .await
                .unwrap()
                .as_deref(),
            Some("sk-cached")
        );
        assert_eq!(
            facts
                .load_coding_plan_api_key("p1", ProviderFamilyDomain::Zai, "user@example.com", true)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn individual_availability_uses_api_key() {
        let http = FakeHttp::ok(serde_json::json!({
            "success": true,
            "data": [{
                "productId": "mode-coding-plan",
                "status": "VALID",
                "inCurrentPeriod": true
            }]
        }));
        let facts = facts(http, Credentials::default());
        let result = facts
            .resolve_family_availability(AccountProviderFamilyAvailabilityInput {
                family: ProviderFamilyDomain::Zai,
                providers: vec![CodingPlanAvailabilityProvider {
                    provider_id: "p1".to_string(),
                    family: ProviderFamilyDomain::Zai,
                    plan_kind: PlanKind::IndividualCodingPlan,
                    api_key: Some("Bearer sk-test".to_string()),
                }],
                selections: ProviderFamilyConnectionSelectionSettings::default(),
            })
            .await
            .unwrap();
        assert_eq!(
            result.get("p1"),
            Some(&CodingPlanAvailabilityResult::Available { models: None })
        );
        let requests = facts.http_requests();
        assert_eq!(requests[0].url, "https://zai.test/list");
        assert_eq!(
            requests[0].headers,
            vec![("Authorization".to_string(), "sk-test".to_string())]
        );
    }

    #[tokio::test]
    async fn start_plan_uses_mode_jwt() {
        let payload = serde_json::json!({
            "success": true,
            "data": {
                "plans": [{ "plan_id": "mode-v3-start-plan", "status": "active" }],
                "balances": []
            }
        });
        let mut credentials = Credentials::default();
        credentials
            .0
            .insert(MODE_JWT_TOKEN_KEY.to_string(), " jwt ".to_string());
        let facts = facts(FakeHttp::ok(payload), credentials);
        let result = facts
            .resolve_family_availability(AccountProviderFamilyAvailabilityInput {
                family: ProviderFamilyDomain::Bigmodel,
                providers: vec![CodingPlanAvailabilityProvider {
                    provider_id: "start".to_string(),
                    family: ProviderFamilyDomain::Bigmodel,
                    plan_kind: PlanKind::StartPlan,
                    api_key: None,
                }],
                selections: ProviderFamilyConnectionSelectionSettings::default(),
            })
            .await
            .unwrap();
        assert!(matches!(
            result.get("start"),
            Some(CodingPlanAvailabilityResult::Available { .. })
        ));
        let requests = facts.http_requests();
        assert_eq!(
            requests[0].headers,
            vec![("Authorization".to_string(), "Bearer jwt".to_string())]
        );
    }

    #[tokio::test]
    async fn missing_api_key_is_unknown() {
        let facts = facts(FakeHttp::ok(serde_json::json!({})), Credentials::default());
        let result = facts
            .resolve_family_availability(AccountProviderFamilyAvailabilityInput {
                family: ProviderFamilyDomain::Zai,
                providers: vec![CodingPlanAvailabilityProvider {
                    provider_id: "p1".to_string(),
                    family: ProviderFamilyDomain::Zai,
                    plan_kind: PlanKind::IndividualCodingPlan,
                    api_key: None,
                }],
                selections: ProviderFamilyConnectionSelectionSettings::default(),
            })
            .await
            .unwrap();
        assert_eq!(
            result.get("p1"),
            Some(&CodingPlanAvailabilityResult::Unknown)
        );
    }

    #[test]
    fn env_urls_use_defaults_and_app_version() {
        let urls = EnvCodingPlanUrls::with_env(std::collections::BTreeMap::new(), "1.2.3");
        assert_eq!(
            urls.subscription_list_url(ProviderFamilyDomain::Zai),
            "https://api.z.ai/api/biz/subscription/list"
        );
        assert_eq!(
            urls.subscription_list_url(ProviderFamilyDomain::Bigmodel),
            "https://bigmodel.cn/api/biz/subscription/list"
        );
        let balance = urls.start_plan_balance_url();
        assert!(balance.starts_with("https://zcode.z.ai/api/v1/mode-plan/billing/balance?"));
        assert!(balance.contains("app_version=1.2.3"), "{balance}");
    }

    #[test]
    fn env_urls_normalize_overrides() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("MODE_ENDPOINT_ORIGIN".to_string(), "https://mode.test/".to_string());
        env.insert(
            "ZAI_BUSINESS_BASE_URL".to_string(),
            "https://zai.test/".to_string(),
        );
        env.insert(
            "BIGMODEL_API_BASE_URL".to_string(),
            "https://bm.test/".to_string(),
        );
        let urls = EnvCodingPlanUrls::with_env(env, "9");
        assert_eq!(
            urls.subscription_list_url(ProviderFamilyDomain::Zai),
            "https://zai.test/api/biz/subscription/list"
        );
        assert_eq!(
            urls.subscription_list_url(ProviderFamilyDomain::Bigmodel),
            "https://bm.test/api/biz/subscription/list"
        );
        assert!(urls
            .start_plan_balance_url()
            .starts_with("https://mode.test/api/v1/mode-plan/billing/balance?"));
    }

    struct FakeExchange(Option<String>);
    impl CodingPlanKeyExchange for FakeExchange {
        fn resolve<'a>(
            &'a self,
            _family: ProviderFamilyDomain,
            _access_token: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>,
        > {
            let value = self.0.clone();
            Box::pin(async move { value })
        }
    }

    #[tokio::test]
    async fn key_exchange_uses_oauth_access_token() {
        let mut credentials = Credentials::default();
        credentials
            .0
            .insert("oauth:zai:access_token".to_string(), " token ".to_string());
        let with_token = facts(FakeHttp::ok(serde_json::json!({})), credentials)
            .with_key_exchange(Arc::new(FakeExchange(Some("exchanged".to_string()))));
        assert_eq!(
            with_token
                .load_coding_plan_api_key("p1", ProviderFamilyDomain::Zai, "user", true)
                .await
                .unwrap()
                .as_deref(),
            Some("exchanged")
        );

        let empty = facts(
            FakeHttp::ok(serde_json::json!({})),
            Credentials::default(),
        )
        .with_key_exchange(Arc::new(FakeExchange(Some("exchanged".to_string()))));
        assert_eq!(
            empty
                .load_coding_plan_api_key("p1", ProviderFamilyDomain::Zai, "user", true)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn team_availability_uses_selected_team_context() {
        let selection = account_provider_overlay::ProviderFamilyConnectionSelection::TeamCodingPlan {
            product_id: "product".to_string(),
            organization_id: "org-1".to_string(),
            project_id: "proj-1".to_string(),
        };
        let selections = ProviderFamilyConnectionSelectionSettings {
            zai: Some(selection),
            bigmodel: None,
        };
        let settings = AccountProviderConnectionSettings {
            provider_family_domain: Some(ProviderFamilyDomain::Zai),
            selections,
            unresolved_families: None,
        };
        let mut credentials = Credentials::default();
        credentials
            .0
            .insert("oauth:zai:access_token".to_string(), "token".to_string());
        let http = QueueHttp::new(vec![
            serde_json::json!({
                "data": { "organizations": [{ "organizationId": "org-1",
                    "projects": [{ "projectId": "proj-1", "projectType": 2 }] }] }
            }),
            serde_json::json!({
                "data": { "hasSubscription": true, "status": "EFFECTIVE",
                    "memberGrantStatus": "VALID" }
            }),
        ]);
        let facts = ProviderAccountFacts::new(
            http,
            Arc::new(Settings(settings)),
            Arc::new(credentials),
            Arc::new(Urls),
            Arc::new(|| 1_000.0),
        );
        let result = facts
            .resolve_family_availability(AccountProviderFamilyAvailabilityInput {
                family: ProviderFamilyDomain::Zai,
                providers: vec![CodingPlanAvailabilityProvider {
                    provider_id: "team".to_string(),
                    family: ProviderFamilyDomain::Zai,
                    plan_kind: PlanKind::TeamCodingPlan,
                    api_key: None,
                }],
                selections: ProviderFamilyConnectionSelectionSettings::default(),
            })
            .await
            .unwrap();
        assert_eq!(
            result.get("team"),
            Some(&CodingPlanAvailabilityResult::Available { models: None })
        );
        let requests = facts.http.port().requests.lock().unwrap();
        assert_eq!(
            requests[1].headers[0],
            ("Authorization".to_string(), "Bearer token".to_string())
        );
    }

    struct FakeTeamResolver(Option<String>);
    impl TeamCodingPlanKeyResolver for FakeTeamResolver {
        fn resolve_team<'a>(
            &'a self,
            _family: ProviderFamilyDomain,
            _access_token: &'a str,
            _organization_id: &'a str,
            _project_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>
        {
            let value = self.0.clone();
            Box::pin(async move { value })
        }
    }

    #[tokio::test]
    async fn runtime_api_key_resolves_start_cached_and_team() {
        use agent_protocol::provider_runtime_headers::{
            ProviderAccountAccess, ProviderRuntimeAccessType, ProviderRuntimeAccountMode,
            ProviderRuntimeAccountType,
        };

        let mut credentials = Credentials::default();
        credentials
            .0
            .insert(MODE_JWT_TOKEN_KEY.to_string(), " jwt ".to_string());
        credentials.0.insert(
            "oauth:zai:user_info".to_string(),
            r#"{"id":"user"}"#.to_string(),
        );
        credentials.0.insert(
            "account-provider:coding-plan:p1:account:user:api-key".to_string(),
            "cached".to_string(),
        );
        credentials
            .0
            .insert("oauth:zai:access_token".to_string(), "token".to_string());
        let start_access = ProviderAccountAccess {
            access_type: ProviderRuntimeAccessType::ZhipuAccount,
            account_type: ProviderRuntimeAccountType::Zai,
            mode: ProviderRuntimeAccountMode::StartPlan,
            entitled: true,
        };
        let individual_access = ProviderAccountAccess {
            mode: ProviderRuntimeAccountMode::IndividualCodingPlan,
            ..start_access
        };
        let team_access = ProviderAccountAccess {
            mode: ProviderRuntimeAccountMode::TeamCodingPlan,
            ..start_access
        };

        let facts = facts(FakeHttp::ok(serde_json::json!({})), credentials)
            .with_team_key_resolver(Arc::new(FakeTeamResolver(Some("team-key".to_string()))));
        assert_eq!(
            facts
                .resolve_runtime_api_key("p0", &start_access)
                .await
                .as_deref(),
            Some("jwt")
        );
        assert_eq!(
            facts
                .resolve_runtime_api_key("p1", &individual_access)
                .await
                .as_deref(),
            Some("cached")
        );
        let team_selections = ProviderFamilyConnectionSelectionSettings {
            zai: Some(
                account_provider_overlay::ProviderFamilyConnectionSelection::TeamCodingPlan {
                    product_id: "product".to_string(),
                    organization_id: "org-1".to_string(),
                    project_id: "proj-1".to_string(),
                },
            ),
            bigmodel: None,
        };
        let mut team_credentials = Credentials::default();
        team_credentials
            .0
            .insert("oauth:zai:access_token".to_string(), "token".to_string());
        let team_facts = ProviderAccountFacts::new(
            FakeHttp::ok(serde_json::json!({})),
            Arc::new(Settings(AccountProviderConnectionSettings {
                provider_family_domain: Some(ProviderFamilyDomain::Zai),
                selections: team_selections,
                unresolved_families: None,
            })),
            Arc::new(team_credentials),
            Arc::new(Urls),
            Arc::new(|| 1_000.0),
        )
        .with_team_key_resolver(Arc::new(FakeTeamResolver(Some("team-key".to_string()))));
        assert_eq!(
            team_facts
                .resolve_runtime_api_key("p2", &team_access)
                .await
                .as_deref(),
            Some("team-key")
        );
    }

    #[test]
    fn encode_uri_component_matches_js() {
        assert_eq!(encode_uri_component("user@example.com"), "user%40example.com");
        assert_eq!(encode_uri_component("a b+c"), "a%20b%2Bc");
        assert_eq!(encode_uri_component("ok-_.!~*'()"), "ok-_.!~*'()");
        assert_eq!(
            encode_uri_component("用户"),
            "%E7%94%A8%E6%88%B7"
        );
    }

    #[test]
    fn missing_start_plan_auth_is_not_authenticated() {
        // 由 coding-plan-http 映射；这里只验证空授权分支不会伪装成 available。
        let facts = facts(FakeHttp::ok(serde_json::json!({})), Credentials::default());
        let result = futures_block_on(facts.provider_availability(&CodingPlanAvailabilityProvider {
            provider_id: "s".to_string(),
            family: ProviderFamilyDomain::Zai,
            plan_kind: PlanKind::StartPlan,
            api_key: None,
        }));
        assert_eq!(
            result,
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotAuthenticated
            }
        );
    }

    fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }
}
