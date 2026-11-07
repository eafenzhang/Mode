//! OAuth access token → Coding Plan API Key。
//!
//! 真源：`packages/services/src/model-provider/accountProviderApiKeyResolver.ts`。
//! 流程：customer/getCustomerInfo → 选默认机构/项目 → list api_keys →
//! 必要时 POST 创建 `mode-api-key` → copy/<apiKey> 取 secretKey。
//!
//! 网络由 [`AccountApiHttpPort`] 注入；本模块只做请求构造、业务信封判定与结果组合。

use std::future::Future;
use std::pin::Pin;

use serde_json::{json, Value};

pub const DEFAULT_ORG_NAME: &str = "默认机构";
pub const DEFAULT_PROJECT_NAME: &str = "默认项目";
pub const MODE_API_KEY_NAME: &str = "mode-api-key";
pub const MODE_TEAM_API_KEY_NAME: &str = "mode-team-api-key";
const TEAM_API_KEY_TYPE: u64 = 2;

pub type ApiFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ApiResponse, String>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountApiProvider {
    Zai,
    Bigmodel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub method: ApiMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait AccountApiHttpPort: Send + Sync {
    fn send<'a>(&'a self, request: &'a ApiRequest) -> ApiFuture<'a>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountApiHosts {
    pub zai: String,
    pub bigmodel: String,
}

/// `fetchRemoteData`：只接受成功业务码，返回 `data`。
pub fn successful_data(response: &ApiResponse) -> Option<Value> {
    if !(200..300).contains(&response.status) {
        return None;
    }
    let payload: Value = serde_json::from_slice(&response.body).ok()?;
    let object = payload.as_object()?;
    if !successful_code(object.get("code")) {
        return None;
    }
    object.get("data").cloned().filter(|value| !value.is_null())
}

fn successful_code(code: Option<&Value>) -> bool {
    match code {
        None | Some(Value::Null) => true,
        Some(Value::Number(number)) => matches!(number.as_i64(), Some(0 | 200)),
        Some(Value::String(value)) => value == "0" || value == "200",
        Some(_) => false,
    }
}

/// `pickOrgAndProject`：排除 Team 项目（projectType=2），优先默认名称。
pub fn pick_org_and_project(customer_info: &Value) -> Option<(String, String)> {
    let organizations = customer_info
        .get("organizations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let personal: Vec<(&serde_json::Map<String, Value>, Vec<&Value>)> = organizations
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|organization| {
            let projects: Vec<&Value> = organization
                .get("projects")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|project| {
                    project
                        .get("projectType")
                        .map(|value| match value {
                            Value::Null => true,
                            Value::Number(number) => {
                                number.as_f64().is_none_or(|value| value != 2.0)
                            }
                            Value::String(text) => text.trim() != "2",
                            _ => true,
                        })
                        .unwrap_or(true)
                })
                .collect();
            (!projects.is_empty()
                && organization
                    .get("organizationId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()))
            .then_some((organization, projects))
        })
        .collect();
    let (organization, projects) = personal
        .iter()
        .find(|(organization, _)| {
            organization
                .get("organizationName")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains(DEFAULT_ORG_NAME))
        })
        .or_else(|| personal.first())?;
    let organization_id = organization
        .get("organizationId")
        .and_then(Value::as_str)?
        .to_string();
    let project = projects
        .iter()
        .find(|project| {
            project
                .get("projectName")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains(DEFAULT_PROJECT_NAME))
        })
        .or_else(|| projects.first())?;
    let project_id = project.get("projectId").and_then(Value::as_str)?.to_string();
    Some((organization_id, project_id))
}

pub struct AccountApiKeyResolver<P: AccountApiHttpPort> {
    http: P,
    hosts: AccountApiHosts,
}

impl<P: AccountApiHttpPort> AccountApiKeyResolver<P> {
    pub fn new(http: P, hosts: AccountApiHosts) -> Self {
        Self { http, hosts }
    }

    pub async fn resolve(
        &self,
        provider: AccountApiProvider,
        access_token: &str,
    ) -> Option<String> {
        let access_token = access_token.trim();
        if access_token.is_empty() {
            return None;
        }
        match provider {
            AccountApiProvider::Bigmodel => {
                self.resolve_biz(&self.hosts.bigmodel, access_token, false)
                    .await
            }
            AccountApiProvider::Zai => {
                let authorization = format!("Bearer {access_token}");
                self.resolve_biz(&self.hosts.zai, &authorization, true)
                    .await
            }
        }
    }

    /// Team Plan 项目级 Key：先校验 customerInfo 里存在该 org/project，再 list/create/copy。
    pub async fn resolve_team_key(
        &self,
        provider: AccountApiProvider,
        access_token: &str,
        organization_id: &str,
        project_id: &str,
    ) -> Option<String> {
        let access_token = access_token.trim();
        if access_token.is_empty() || organization_id.trim().is_empty() || project_id.trim().is_empty()
        {
            return None;
        }
        let host = match provider {
            AccountApiProvider::Zai => self.hosts.zai.trim_end_matches('/'),
            AccountApiProvider::Bigmodel => self.hosts.bigmodel.trim_end_matches('/'),
        };
        let login_headers = vec![
            ("Authorization".to_string(), access_token.to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let customer = self
            .fetch_data(ApiRequest {
                method: ApiMethod::Get,
                url: format!("{host}/api/biz/customer/getCustomerInfo"),
                headers: login_headers.clone(),
                body: None,
            })
            .await?;
        let has_project = customer
            .get("organizations")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(Value::as_object)
            .any(|organization| {
                organization
                    .get("organizationId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    == Some(organization_id.trim())
                    && organization
                        .get("projects")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                        .iter()
                        .any(|project| {
                            project.get("projectId").and_then(Value::as_str).map(str::trim)
                                == Some(project_id.trim())
                        })
            });
        if !has_project {
            return None;
        }
        let list_url = format!(
            "{host}/api/biz/v1/organization/{}/projects/{}/api_keys",
            encode_uri_component(organization_id),
            encode_uri_component(project_id)
        );
        let headers = vec![
            ("Authorization".to_string(), access_token.to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "bigmodel-organization".to_string(),
                organization_id.to_string(),
            ),
            ("bigmodel-project".to_string(), project_id.to_string()),
        ];
        let usable = |entry: &Value| {
            entry.get("name").and_then(Value::as_str) == Some(MODE_TEAM_API_KEY_NAME)
                && entry.get("keyType").and_then(Value::as_u64) == Some(TEAM_API_KEY_TYPE)
                && entry
                    .get("apiKey")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        };
        let keys = self
            .fetch_data(ApiRequest {
                method: ApiMethod::Get,
                url: list_url.clone(),
                headers: headers.clone(),
                body: None,
            })
            .await
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let mut entry = keys.iter().find(|entry| usable(entry)).cloned();
        if entry.is_none() {
            entry = self
                .fetch_data(ApiRequest {
                    method: ApiMethod::Post,
                    url: list_url.clone(),
                    headers: headers.clone(),
                    body: Some(
                        serde_json::to_vec(&json!({
                            "name": MODE_TEAM_API_KEY_NAME,
                            "keyType": TEAM_API_KEY_TYPE
                        }))
                        .ok()?,
                    ),
                })
                .await
                .filter(|value| usable(value));
        }
        let api_key = entry?
            .get("apiKey")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?
            .to_string();
        let copy_url = format!("{list_url}/copy/{}", encode_uri_component(&api_key));
        let secret = self
            .fetch_data(ApiRequest {
                method: ApiMethod::Get,
                url: copy_url,
                headers,
                body: None,
            })
            .await
            .and_then(|value| {
                value
                    .get("secretKey")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });
        Some(match secret {
            Some(secret) => format!("{api_key}.{secret}"),
            None => api_key,
        })
    }

    async fn resolve_biz(
        &self,
        host: &str,
        authorization: &str,
        require_secret_key: bool,
    ) -> Option<String> {
        let host = host.trim_end_matches('/');
        let headers = vec![
            ("Authorization".to_string(), authorization.to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let customer = self
            .fetch_data(ApiRequest {
                method: ApiMethod::Get,
                url: format!("{host}/api/biz/customer/getCustomerInfo"),
                headers: headers.clone(),
                body: None,
            })
            .await?;
        let (organization_id, project_id) = pick_org_and_project(&customer)?;
        let list_url = format!(
            "{host}/api/biz/v1/organization/{organization_id}/projects/{project_id}/api_keys"
        );
        let keys = self
            .fetch_data(ApiRequest {
                method: ApiMethod::Get,
                url: list_url.clone(),
                headers: headers.clone(),
                body: None,
            })
            .await
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let mut entry = keys
            .iter()
            .find(|entry| {
                entry.get("name").and_then(Value::as_str) == Some(MODE_API_KEY_NAME)
            })
            .cloned();
        if entry.is_none() {
            entry = self
                .fetch_data(ApiRequest {
                    method: ApiMethod::Post,
                    url: list_url.clone(),
                    headers: headers.clone(),
                    body: Some(serde_json::to_vec(&json!({ "name": MODE_API_KEY_NAME })).ok()?),
                })
                .await;
        }
        let api_key = entry?
            .get("apiKey")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?
            .to_string();
        let copy_url = format!("{list_url}/copy/{}", encode_uri_component(&api_key));
        let secret = self
            .fetch_data(ApiRequest {
                method: ApiMethod::Get,
                url: copy_url,
                headers,
                body: None,
            })
            .await
            .and_then(|value| {
                value
                    .get("secretKey")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });
        match secret {
            Some(secret) => Some(format!("{api_key}.{secret}")),
            None if require_secret_key => None,
            None => Some(api_key),
        }
    }

    async fn fetch_data(&self, request: ApiRequest) -> Option<Value> {
        let response = self.http.send(&request).await.ok()?;
        successful_data(&response)
    }
}

/// 生产传输：reqwest + rustls（ring provider）。
#[derive(Clone)]
pub struct ReqwestAccountApiHttp {
    client: reqwest::Client,
}

impl ReqwestAccountApiHttp {
    pub fn new(timeout: std::time::Duration) -> Result<Self, reqwest::Error> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self { client })
    }
}

impl AccountApiHttpPort for ReqwestAccountApiHttp {
    fn send<'a>(&'a self, request: &'a ApiRequest) -> ApiFuture<'a> {
        Box::pin(async move {
            let mut builder = match request.method {
                ApiMethod::Get => self.client.get(&request.url),
                ApiMethod::Post => self.client.post(&request.url),
            };
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = &request.body {
                builder = builder.body(body.clone());
            }
            let response = builder.send().await.map_err(|error| error.to_string())?;
            let status = response.status().as_u16();
            let body = response
                .bytes()
                .await
                .map_err(|error| error.to_string())?
                .to_vec();
            Ok(ApiResponse { status, body })
        })
    }
}

fn encode_uri_component(value: &str) -> String {
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
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct FakeHttp {
        requests: Mutex<Vec<ApiRequest>>,
        responses: Mutex<VecDeque<Value>>,
    }

    impl FakeHttp {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into()),
            }
        }

        fn requests(&self) -> Vec<ApiRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl AccountApiHttpPort for FakeHttp {
        fn send<'a>(&'a self, request: &'a ApiRequest) -> ApiFuture<'a> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request.clone());
                let value = self.responses.lock().unwrap().pop_front().unwrap();
                Ok(ApiResponse {
                    status: 200,
                    body: serde_json::to_vec(&value).unwrap(),
                })
            })
        }
    }

    impl AccountApiKeyResolver<FakeHttp> {
        fn requests(&self) -> Vec<ApiRequest> {
            self.http.requests()
        }
    }

    fn customer_info() -> Value {
        json!({
            "code": 0,
            "data": {
                "organizations": [
                    {
                        "organizationId": "team-org",
                        "organizationName": "普通机构",
                        "projects": [{ "projectId": "team", "projectType": 2 }]
                    },
                    {
                        "organizationId": "org-1",
                        "organizationName": "默认机构",
                        "projects": [
                            { "projectId": "team-project", "projectType": "2" },
                            { "projectId": "proj-1", "projectName": "默认项目" }
                        ]
                    }
                ]
            }
        })
    }

    #[test]
    fn picks_personal_default_org_and_project() {
        assert_eq!(
            pick_org_and_project(&customer_info()["data"]),
            Some(("org-1".to_string(), "proj-1".to_string()))
        );
        assert_eq!(pick_org_and_project(&json!({})), None);
    }

    #[tokio::test]
    async fn bigmodel_returns_bare_key_when_secret_missing() {
        let http = FakeHttp::new(vec![
            customer_info(),
            json!({ "code": 0, "data": [{ "name": MODE_API_KEY_NAME, "apiKey": "key-1" }] }),
            json!({ "code": 0, "data": {} }),
        ]);
        let resolver = AccountApiKeyResolver::new(
            http,
            AccountApiHosts {
                zai: "https://zai.test".to_string(),
                bigmodel: "https://bm.test".to_string(),
            },
        );
        assert_eq!(
            resolver
                .resolve(AccountApiProvider::Bigmodel, "token")
                .await
                .as_deref(),
            Some("key-1")
        );
        assert_eq!(
            resolver.requests()[0].headers[0],
            ("Authorization".to_string(), "token".to_string())
        );
    }

    #[tokio::test]
    async fn zai_requires_secret_key_and_uses_bearer() {
        let http = FakeHttp::new(vec![
            customer_info(),
            json!({ "data": [{ "name": MODE_API_KEY_NAME, "apiKey": "key-1" }] }),
            json!({ "data": { "secretKey": "secret-1" } }),
        ]);
        let resolver = AccountApiKeyResolver::new(
            http,
            AccountApiHosts {
                zai: "https://zai.test/".to_string(),
                bigmodel: "https://bm.test".to_string(),
            },
        );
        assert_eq!(
            resolver
                .resolve(AccountApiProvider::Zai, "token")
                .await
                .as_deref(),
            Some("key-1.secret-1")
        );
        let requests = resolver.requests();
        assert_eq!(
            requests[0].headers[0],
            ("Authorization".to_string(), "Bearer token".to_string())
        );
        assert_eq!(
            requests[2].url,
            "https://zai.test/api/biz/v1/organization/org-1/projects/proj-1/api_keys/copy/key-1"
        );
    }

    #[tokio::test]
    async fn zai_without_secret_is_none() {
        let http = FakeHttp::new(vec![
            customer_info(),
            json!({ "data": [{ "name": MODE_API_KEY_NAME, "apiKey": "key-1" }] }),
            json!({ "data": {} }),
        ]);
        let resolver = AccountApiKeyResolver::new(
            http,
            AccountApiHosts {
                zai: "https://zai.test".to_string(),
                bigmodel: "https://bm.test".to_string(),
            },
        );
        assert_eq!(resolver.resolve(AccountApiProvider::Zai, "token").await, None);
    }

    #[tokio::test]
    async fn creates_key_when_list_is_empty() {
        let http = FakeHttp::new(vec![
            customer_info(),
            json!({ "data": [] }),
            json!({ "data": { "name": MODE_API_KEY_NAME, "apiKey": "created" } }),
            json!({ "data": {} }),
        ]);
        let resolver = AccountApiKeyResolver::new(
            http,
            AccountApiHosts {
                zai: "https://zai.test".to_string(),
                bigmodel: "https://bm.test".to_string(),
            },
        );
        assert_eq!(
            resolver
                .resolve(AccountApiProvider::Bigmodel, "token")
                .await
                .as_deref(),
            Some("created")
        );
        let requests = resolver.requests();
        assert_eq!(requests[2].method, ApiMethod::Post);
        assert_eq!(
            requests[2].body.as_deref(),
            Some(br#"{"name":"mode-api-key"}"#.as_slice())
        );
    }

    #[tokio::test]
    async fn team_key_checks_project_and_creates_typed_key() {
        let team_customer = json!({
            "data": {
                "organizations": [{
                    "organizationId": "org-1",
                    "projects": [{ "projectId": "team-1", "projectType": 2 }]
                }]
            }
        });
        let http = FakeHttp::new(vec![
            team_customer,
            json!({ "data": [] }),
            json!({ "data": { "name": MODE_TEAM_API_KEY_NAME, "keyType": 2, "apiKey": "team" } }),
            json!({ "data": { "secretKey": "secret" } }),
        ]);
        let resolver = AccountApiKeyResolver::new(
            http,
            AccountApiHosts {
                zai: "https://zai.test".to_string(),
                bigmodel: "https://bm.test".to_string(),
            },
        );
        assert_eq!(
            resolver
                .resolve_team_key(AccountApiProvider::Bigmodel, "token", "org-1", "team-1")
                .await
                .as_deref(),
            Some("team.secret")
        );
        let requests = resolver.requests();
        assert_eq!(requests[2].method, ApiMethod::Post);
        assert!(requests[2]
            .headers
            .contains(&("bigmodel-organization".to_string(), "org-1".to_string())));
        let body: Value = serde_json::from_slice(requests[2].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["keyType"], 2);
        assert_eq!(body["name"], MODE_TEAM_API_KEY_NAME);
    }

    #[tokio::test]
    async fn team_key_missing_project_is_none() {
        let http = FakeHttp::new(vec![json!({ "data": { "organizations": [] } })]);
        let resolver = AccountApiKeyResolver::new(
            http,
            AccountApiHosts {
                zai: "https://zai.test".to_string(),
                bigmodel: "https://bm.test".to_string(),
            },
        );
        assert_eq!(
            resolver
                .resolve_team_key(AccountApiProvider::Bigmodel, "token", "org-1", "team-1")
                .await,
            None
        );
    }

    #[tokio::test]
    async fn business_failure_is_none() {
        let http = FakeHttp::new(vec![json!({ "code": 401, "data": {} })]);
        let resolver = AccountApiKeyResolver::new(
            http,
            AccountApiHosts {
                zai: "https://zai.test".to_string(),
                bigmodel: "https://bm.test".to_string(),
            },
        );
        assert_eq!(
            resolver
                .resolve(AccountApiProvider::Bigmodel, "token")
                .await,
            None
        );
    }
}
