//! Coding Plan 可用性 HTTP 编排：请求构造、状态分类与解析接线。
//!
//! 真源：`packages/services/src/model-provider/codingPlanProviderAvailability.ts`。
//! 本模块不做真实网络：GET 由注入的 [`CodingPlanHttpPort`] 完成，便于壳内复用
//! 自己的 HTTP 客户端与测试。

use std::future::Future;
use std::pin::Pin;

use account_provider_overlay::{
    CodingPlanAvailabilityResult, CodingPlanUnavailableReason, ProviderFamilyDomain,
};
use coding_plan_entitlement::{
    parse_team_coding_plan_entitlement, resolve_subscription_list_availability,
    CodingPlanEntitlement, TeamCodingPlanSubscription,
};
use serde_json::Value;

pub type HttpFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HttpResponse, HttpTransportError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTransportError(pub String);

/// GET 端口：生产实现落在宿主 HTTP 客户端，测试注入脚本化响应。
pub trait CodingPlanHttpPort: Send + Sync {
    fn get<'a>(&'a self, request: &'a HttpRequest) -> HttpFuture<'a>;
}

/// 生产 GET 传输：reqwest + rustls（TLS provider 由本构造器安装，重复安装安全）。
pub struct ReqwestCodingPlanHttp {
    client: reqwest::Client,
}

impl ReqwestCodingPlanHttp {
    pub fn new(timeout: std::time::Duration) -> Result<Self, reqwest::Error> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self { client })
    }
}

impl CodingPlanHttpPort for ReqwestCodingPlanHttp {
    fn get<'a>(&'a self, request: &'a HttpRequest) -> HttpFuture<'a> {
        Box::pin(async move {
            let mut builder = self.client.get(&request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            let response = builder
                .send()
                .await
                .map_err(|error| HttpTransportError(error.to_string()))?;
            let status = response.status().as_u16();
            let body = response
                .bytes()
                .await
                .map_err(|error| HttpTransportError(error.to_string()))?
                .to_vec();
            Ok(HttpResponse { status, body })
        })
    }
}

pub struct CodingPlanAvailability<P: CodingPlanHttpPort> {
    http: P,
}

impl<P: CodingPlanHttpPort> CodingPlanAvailability<P> {
    pub fn new(http: P) -> Self {
        Self { http }
    }

    /// 访问注入的传输端点（诊断/测试用）。
    pub fn port(&self) -> &P {
        &self.http
    }

    /// 个人 Coding Plan 订阅列表（`/api/biz/subscription/list` 等）。
    pub async fn subscription_availability(
        &self,
        url: &str,
        authorization: &str,
    ) -> CodingPlanAvailabilityResult {
        let authorization = authorization.trim();
        if authorization.is_empty() {
            return CodingPlanAvailabilityResult::Unknown;
        }
        let response = match self
            .http
            .get(&HttpRequest {
                url: url.to_string(),
                headers: vec![("Authorization".to_string(), authorization.to_string())],
            })
            .await
        {
            Ok(response) => response,
            Err(_) => return CodingPlanAvailabilityResult::Unknown,
        };
        if matches!(response.status, 401 | 403) {
            return CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanAuthFailed,
            };
        }
        if !(200..300).contains(&response.status) {
            return CodingPlanAvailabilityResult::Unknown;
        }
        let Ok(payload) = serde_json::from_slice::<Value>(&response.body) else {
            return CodingPlanAvailabilityResult::Unknown;
        };
        resolve_subscription_list_availability(&payload, true)
    }

    /// Start Plan 余额信封（`billing/balance`）。
    pub async fn start_plan_availability(
        &self,
        url: &str,
        authorization: &str,
        now_seconds: f64,
    ) -> CodingPlanAvailabilityResult {
        let authorization = authorization.trim();
        if authorization.is_empty() {
            return CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotAuthenticated,
            };
        }
        let response = match self
            .http
            .get(&HttpRequest {
                url: url.to_string(),
                headers: vec![("Authorization".to_string(), authorization.to_string())],
            })
            .await
        {
            Ok(response) => response,
            Err(_) => return CodingPlanAvailabilityResult::Unknown,
        };
        if matches!(response.status, 401 | 403) {
            return CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanAuthFailed,
            };
        }
        if !(200..300).contains(&response.status) {
            return CodingPlanAvailabilityResult::Unknown;
        }
        let Ok(payload) = serde_json::from_slice::<Value>(&response.body) else {
            return CodingPlanAvailabilityResult::Unknown;
        };
        zai_start_plan_billing::resolve_start_plan_availability(&payload, now_seconds)
    }

    /// 团队 Coding Plan 可用性：customerInfo 项目校验 + 团队订阅详情。
    pub async fn team_availability(
        &self,
        family: ProviderFamilyDomain,
        host: &str,
        access_token: &str,
        organization_id: &str,
        project_id: &str,
    ) -> CodingPlanAvailabilityResult {
        let access_token = access_token.trim();
        if access_token.is_empty() {
            return CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotConnected,
            };
        }
        let host = host.trim_end_matches('/');
        let login_headers = vec![
            ("Authorization".to_string(), access_token.to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let customer = match self
            .http
            .get(&HttpRequest {
                url: format!("{host}/api/biz/customer/getCustomerInfo"),
                headers: login_headers,
            })
            .await
        {
            Ok(response) if (200..300).contains(&response.status) => {
                match serde_json::from_slice::<Value>(&response.body) {
                    Ok(payload) => payload,
                    Err(_) => return CodingPlanAvailabilityResult::Unknown,
                }
            }
            Ok(_) | Err(_) => return CodingPlanAvailabilityResult::Unknown,
        };
        if customer.get("success").and_then(Value::as_bool) == Some(false)
            || customer
                .get("code")
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 0 && code != 200)
        {
            return CodingPlanAvailabilityResult::Unknown;
        }
        let organizations = customer
            .get("data")
            .and_then(Value::as_object)
            .and_then(|data| data.get("organizations"))
            .and_then(Value::as_array);
        let Some(organizations) = organizations else {
            return CodingPlanAvailabilityResult::Unknown;
        };
        let team_project = organizations
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
                        .filter_map(Value::as_object)
                        .any(|project| {
                            project.get("projectId").and_then(Value::as_str).map(str::trim)
                                == Some(project_id.trim())
                                && project
                                    .get("projectType")
                                    .map(|value| match value {
                                        Value::Number(number) => {
                                            number.as_f64().is_some_and(|value| value == 2.0)
                                        }
                                        Value::String(text) => text.trim() == "2",
                                        _ => false,
                                    })
                                    .unwrap_or(false)
                        })
            });
        if !team_project {
            return CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotEntitled,
            };
        }
        let authorization = match family {
            ProviderFamilyDomain::Zai => format!("Bearer {access_token}"),
            ProviderFamilyDomain::Bigmodel => access_token.to_string(),
        };
        let headers = vec![
            ("Authorization".to_string(), authorization),
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "bigmodel-organization".to_string(),
                organization_id.to_string(),
            ),
            ("bigmodel-project".to_string(), project_id.to_string()),
        ];
        let url = format!(
            "{host}/api/biz/team/subscribe/product/querySubscribeDetail"
        );
        let response = match self.http.get(&HttpRequest { url, headers }).await {
            Ok(response) => response,
            Err(_) => return CodingPlanAvailabilityResult::Unknown,
        };
        if matches!(response.status, 401 | 403) {
            return CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanAuthFailed,
            };
        }
        if !(200..300).contains(&response.status) {
            return CodingPlanAvailabilityResult::Unknown;
        }
        let Ok(payload) = serde_json::from_slice::<Value>(&response.body) else {
            return CodingPlanAvailabilityResult::Unknown;
        };
        match coding_plan_entitlement::parse_team_coding_plan_entitlement(&payload) {
            CodingPlanEntitlement::Available { .. } => {
                CodingPlanAvailabilityResult::Available { models: None }
            }
            CodingPlanEntitlement::Unavailable { .. } => {
                CodingPlanAvailabilityResult::Unavailable {
                    reason: CodingPlanUnavailableReason::CodingPlanNotEntitled,
                }
            }
            CodingPlanEntitlement::Unknown => CodingPlanAvailabilityResult::Unknown,
        }
    }

    /// 团队订阅详情（`/api/biz/team/subscribe/product/querySubscribeDetail`）。
    pub async fn team_entitlement(
        &self,
        host: &str,
        authorization: &str,
        organization_id: &str,
        project_id: &str,
    ) -> CodingPlanEntitlement<TeamCodingPlanSubscription> {
        let authorization = authorization.trim();
        if authorization.is_empty() {
            return CodingPlanEntitlement::Unknown;
        }
        let url = format!(
            "{}/api/biz/team/subscribe/product/querySubscribeDetail",
            host.trim_end_matches('/')
        );
        let response = match self
            .http
            .get(&HttpRequest {
                url,
                headers: vec![
                    ("Authorization".to_string(), authorization.to_string()),
                    (
                        "bigmodel-organization".to_string(),
                        organization_id.to_string(),
                    ),
                    ("bigmodel-project".to_string(), project_id.to_string()),
                ],
            })
            .await
        {
            Ok(response) => response,
            Err(_) => return CodingPlanEntitlement::Unknown,
        };
        if !(200..300).contains(&response.status) {
            return CodingPlanEntitlement::Unknown;
        }
        let Ok(payload) = serde_json::from_slice::<Value>(&response.body) else {
            return CodingPlanEntitlement::Unknown;
        };
        parse_team_coding_plan_entitlement(&payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coding_plan_entitlement::EntitlementUnavailableReason;
    use std::sync::{Arc, Mutex};

    struct FakePort {
        requests: Mutex<Vec<HttpRequest>>,
        response: Mutex<Result<HttpResponse, HttpTransportError>>,
    }

    impl FakePort {
        fn ok(body: Value) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(Ok(HttpResponse {
                    status: 200,
                    body: serde_json::to_vec(&body).unwrap(),
                })),
            }
        }

        fn status(status: u16) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(Ok(HttpResponse {
                    status,
                    body: Vec::new(),
                })),
            }
        }

        fn failure() -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(Err(HttpTransportError("boom".to_string()))),
            }
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl CodingPlanHttpPort for FakePort {
        fn get<'a>(&'a self, request: &'a HttpRequest) -> HttpFuture<'a> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request.clone());
                self.response.lock().unwrap().clone()
            })
        }
    }

    fn active_subscription() -> Value {
        serde_json::json!({
            "success": true,
            "data": [{
                "productId": "mode-coding-plan",
                "status": "VALID",
                "inCurrentPeriod": true
            }]
        })
    }

    #[tokio::test]
    async fn subscription_success_maps_to_available_and_sends_auth() {
        let http = CodingPlanAvailability::new(FakePort::ok(active_subscription()));
        let result = http
            .subscription_availability("https://example.test/list", " key ")
            .await;
        assert_eq!(
            result,
            CodingPlanAvailabilityResult::Available { models: None }
        );
        let requests = http.http.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, "https://example.test/list");
        assert_eq!(
            requests[0].headers,
            vec![("Authorization".to_string(), "key".to_string())]
        );
    }

    #[tokio::test]
    async fn subscription_auth_error_is_auth_failed_and_empty_key_is_unknown() {
        let http = CodingPlanAvailability::new(FakePort::status(401));
        assert_eq!(
            http.subscription_availability("https://example.test/list", "key").await,
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanAuthFailed
            }
        );

        let empty = CodingPlanAvailability::new(FakePort::ok(active_subscription()));
        assert_eq!(
            empty.subscription_availability("https://example.test/list", "  ").await,
            CodingPlanAvailabilityResult::Unknown
        );
        assert!(empty.http.requests().is_empty());
    }

    #[tokio::test]
    async fn transport_and_server_errors_are_unknown() {
        let transport = CodingPlanAvailability::new(FakePort::failure());
        assert_eq!(
            transport
                .subscription_availability("https://example.test/list", "key")
                .await,
            CodingPlanAvailabilityResult::Unknown
        );

        let server = CodingPlanAvailability::new(FakePort::status(500));
        assert_eq!(
            server
                .subscription_availability("https://example.test/list", "key")
                .await,
            CodingPlanAvailabilityResult::Unknown
        );
    }

    #[tokio::test]
    async fn start_plan_maps_payload_and_missing_auth() {
        let payload = serde_json::json!({
            "success": true,
            "data": {
                "plans": [{ "plan_id": "mode-v3-start-plan", "status": "active" }],
                "balances": [{ "capabilities": ["model:glm-4.6"] }]
            }
        });
        let http = CodingPlanAvailability::new(FakePort::ok(payload));
        assert_eq!(
            http.start_plan_availability("https://example.test/balance", "Bearer jwt", 1_000.0)
                .await,
            CodingPlanAvailabilityResult::Available {
                models: Some(vec!["GLM-4.6".to_string()])
            }
        );

        let missing = CodingPlanAvailability::new(FakePort::ok(active_subscription()));
        assert_eq!(
            missing
                .start_plan_availability("https://example.test/balance", "", 1_000.0)
                .await,
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotAuthenticated
            }
        );
    }

    #[tokio::test]
    async fn start_plan_auth_error_is_auth_failed() {
        let http = CodingPlanAvailability::new(FakePort::status(403));
        assert_eq!(
            http.start_plan_availability("https://example.test/balance", "Bearer jwt", 1_000.0)
                .await,
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanAuthFailed
            }
        );
    }

    #[tokio::test]
    async fn team_entitlement_builds_url_and_headers() {
        let payload = serde_json::json!({
            "data": {
                "hasSubscription": true,
                "status": "EFFECTIVE",
                "memberGrantStatus": "VALID"
            }
        });
        let http = CodingPlanAvailability::new(FakePort::ok(payload));
        let result = http
            .team_entitlement("https://bm.example/", "Bearer jwt", "org-1", "proj-1")
            .await;
        assert!(matches!(
            result,
            CodingPlanEntitlement::Available { .. }
        ));
        let requests = http.http.requests();
        assert_eq!(
            requests[0].url,
            "https://bm.example/api/biz/team/subscribe/product/querySubscribeDetail"
        );
        assert!(requests[0]
            .headers
            .contains(&("bigmodel-organization".to_string(), "org-1".to_string())));
        assert!(requests[0]
            .headers
            .contains(&("bigmodel-project".to_string(), "proj-1".to_string())));
    }

    struct QueueHttp {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<std::collections::VecDeque<HttpResponse>>,
    }

    impl QueueHttp {
        fn new(responses: Vec<(u16, Value)>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(
                    responses
                        .into_iter()
                        .map(|(status, body)| HttpResponse {
                            status,
                            body: serde_json::to_vec(&body).unwrap(),
                        })
                        .collect(),
                ),
            }
        }
    }

    struct SharedHttp(Arc<QueueHttp>);

    impl CodingPlanHttpPort for SharedHttp {
        fn get<'a>(&'a self, request: &'a HttpRequest) -> HttpFuture<'a> {
            self.0.get(request)
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

    #[tokio::test]
    async fn team_availability_checks_project_and_uses_bearer_for_zai() {
        let customer = serde_json::json!({
            "data": {
                "organizations": [{
                    "organizationId": "org-1",
                    "projects": [{ "projectId": "team-1", "projectType": 2 }]
                }]
            }
        });
        let entitlement = serde_json::json!({
            "data": {
                "hasSubscription": true,
                "status": "EFFECTIVE",
                "memberGrantStatus": "VALID"
            }
        });
        let http = Arc::new(QueueHttp::new(vec![(200, customer), (200, entitlement)]));
        let availability = CodingPlanAvailability::new(SharedHttp(Arc::clone(&http)));
        assert_eq!(
            availability
                .team_availability(
                    ProviderFamilyDomain::Zai,
                    "https://zai.test/",
                    "token",
                    "org-1",
                    "team-1"
                )
                .await,
            CodingPlanAvailabilityResult::Available { models: None }
        );
        let requests = http.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].headers[0],
            ("Authorization".to_string(), "Bearer token".to_string())
        );
        assert!(requests[1]
            .headers
            .contains(&("bigmodel-organization".to_string(), "org-1".to_string())));
    }

    #[tokio::test]
    async fn team_availability_requires_team_project_and_token() {
        let http = Arc::new(QueueHttp::new(vec![(
            200,
            serde_json::json!({
                "data": { "organizations": [{ "organizationId": "org-1",
                    "projects": [{ "projectId": "team-1", "projectType": 1 }] }] }
            }),
        )]));
        let availability = CodingPlanAvailability::new(SharedHttp(Arc::clone(&http)));
        assert_eq!(
            availability
                .team_availability(
                    ProviderFamilyDomain::Bigmodel,
                    "https://bm.test",
                    "token",
                    "org-1",
                    "team-1"
                )
                .await,
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotEntitled
            }
        );

        let missing = CodingPlanAvailability::new(SharedHttp(http));
        assert_eq!(
            missing
                .team_availability(
                    ProviderFamilyDomain::Bigmodel,
                    "https://bm.test",
                    " ",
                    "org-1",
                    "team-1"
                )
                .await,
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotConnected
            }
        );
    }

    #[tokio::test]
    async fn team_availability_maps_401_to_auth_failed() {
        let customer = serde_json::json!({
            "data": { "organizations": [{ "organizationId": "org-1",
                "projects": [{ "projectId": "team-1", "projectType": 2 }] }] }
        });
        let http = Arc::new(QueueHttp::new(vec![
            (200, customer),
            (403, serde_json::json!({})),
        ]));
        let availability = CodingPlanAvailability::new(SharedHttp(http));
        assert_eq!(
            availability
                .team_availability(
                    ProviderFamilyDomain::Bigmodel,
                    "https://bm.test",
                    "token",
                    "org-1",
                    "team-1"
                )
                .await,
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanAuthFailed
            }
        );
    }

    #[tokio::test]
    async fn team_expired_maps_to_unavailable_reason() {
        let payload = serde_json::json!({
            "data": { "hasSubscription": true, "status": "EXPIRED" }
        });
        let http = CodingPlanAvailability::new(FakePort::ok(payload));
        assert_eq!(
            http.team_entitlement("https://bm.example", "Bearer jwt", "o", "p")
                .await,
            CodingPlanEntitlement::Unavailable {
                reason: Some(EntitlementUnavailableReason::Expired)
            }
        );
    }
}

#[cfg(test)]
mod reqwest_tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reqwest_transport_sends_headers_and_reads_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 4096];
            let read = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            assert!(request.starts_with("GET /list HTTP/1.1"), "{request}");
            assert!(
                request.to_lowercase().contains("authorization: bearer sk-test"),
                "{request}"
            );
            let body = "{\"ok\":true}";
            let response = format!(
                "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let http = ReqwestCodingPlanHttp::new(Duration::from_secs(2)).unwrap();
        let response = http
            .get(&HttpRequest {
                url: format!("http://{address}/list"),
                headers: vec![("Authorization".to_string(), "Bearer sk-test".to_string())],
            })
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"ok":true}"#.to_vec());
        server.await.unwrap();
    }
}
