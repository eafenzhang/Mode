//! OrcaRouter OAuth 2.0 + PKCE（OOB code）连接控制器。
//!
//! 规范见 `docs/specs/orcarouter-service.md`。每次尝试新建 verifier/state；
//! verifier 只存在进程内存；generation 守卫生效，旧异步响应不得覆盖新登录。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::credential::{OrcaCredential, OrcaCredentialSource, OrcaCredentialStore};
use crate::origins::{
    build_orca_authorize_url, OrcaAuthorizeUrlInput, OrcaOrigins, ORCAROUTER_CODE_CHALLENGE_METHOD,
    ORCAROUTER_EXCHANGE_PATH, ORCAROUTER_REQUESTED_SCOPE,
};

/// 随机源：返回 `None` 表示系统随机数不可用（授权必须失败，不能降级）。
pub type RandomSource = Arc<dyn Fn(usize) -> Option<Vec<u8>> + Send + Sync>;
pub type ConnectLog = Arc<dyn Fn(&str, &str) + Send + Sync>;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn default_random(length: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0_u8; length];
    getrandom::getrandom(&mut buffer).ok()?;
    Some(buffer)
}

/// 宿主默认随机源（OS CSPRNG）。
pub fn default_random_source() -> RandomSource {
    Arc::new(default_random)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrcaPkceMaterial {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

/// 每次尝试都必须用加密随机数新建 verifier 与 state。
pub fn create_pkce_material(generate: &RandomSource) -> Result<OrcaPkceMaterial, OrcaConnectError> {
    let verifier = B64.encode(generate(32).ok_or_else(random_unavailable)?);
    let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
    let state = B64.encode(generate(16).ok_or_else(random_unavailable)?);
    Ok(OrcaPkceMaterial {
        verifier,
        challenge,
        state,
    })
}

/// 恒定时间比较，避免用早退比较泄露 state 前缀。
pub fn safe_state_equals(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrcaConnectPhase {
    Idle,
    Waiting,
    Exchanging,
    Connected,
    Error,
}

impl OrcaConnectPhase {
    fn busy(self) -> bool {
        matches!(self, Self::Waiting | Self::Exchanging)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaConnectState {
    pub phase: OrcaConnectPhase,
    pub session_id: Option<String>,
    pub authorize_url: Option<String>,
    pub hint: Option<String>,
    pub error: Option<String>,
    pub busy: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrcaConnectErrorKind {
    Denied,
    StateMismatch,
    ExchangeRejected,
    ScopeDowngraded,
    RateLimited,
    Timeout,
    Cancelled,
    Network,
}

impl OrcaConnectErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::StateMismatch => "state-mismatch",
            Self::ExchangeRejected => "exchange-rejected",
            Self::ScopeDowngraded => "scope-downgraded",
            Self::RateLimited => "rate-limited",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Network => "network",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrcaConnectError {
    pub kind: OrcaConnectErrorKind,
    pub message: String,
}

impl OrcaConnectError {
    fn new(kind: OrcaConnectErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OrcaConnectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OrcaConnectError {}

fn random_unavailable() -> OrcaConnectError {
    OrcaConnectError::new(OrcaConnectErrorKind::Network, "无法生成授权参数")
}

#[derive(Clone)]
struct ActiveAttempt {
    session_id: String,
    generation: u64,
    material: OrcaPkceMaterial,
    authorize_url: String,
    #[allow(dead_code)]
    started_at: i64,
}

#[derive(Default)]
struct ConnectStateInner {
    attempt: Option<ActiveAttempt>,
    phase: Option<OrcaConnectPhase>,
    error: Option<String>,
    hint: Option<String>,
    generation: u64,
}

pub struct OrcaConnectOptions {
    pub credential_store: Arc<OrcaCredentialStore>,
    pub origins: OrcaOrigins,
    pub app_name: String,
    pub timeout_ms: u64,
    pub generate: RandomSource,
    pub now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub log: Option<ConnectLog>,
}

impl OrcaConnectOptions {
    pub fn new(
        credential_store: Arc<OrcaCredentialStore>,
        origins: OrcaOrigins,
        app_name: impl Into<String>,
    ) -> Self {
        Self {
            credential_store,
            origins,
            app_name: app_name.into(),
            timeout_ms: 30_000,
            generate: Arc::new(default_random),
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

pub struct OrcaConnectController {
    store: Arc<OrcaCredentialStore>,
    origins: OrcaOrigins,
    app_name: String,
    client: reqwest::Client,
    timeout: Duration,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    generate: RandomSource,
    log: Option<ConnectLog>,
    state: Mutex<ConnectStateInner>,
}

impl OrcaConnectController {
    pub fn new(options: OrcaConnectOptions) -> Result<Self, String> {
        // reqwest 以 rustls-no-provider 编译：必须在建 Client 前安装 provider。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(options.timeout_ms.max(1)))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            store: options.credential_store,
            origins: options.origins,
            app_name: options.app_name,
            client,
            timeout: Duration::from_millis(options.timeout_ms.max(1)),
            now_ms: options.now_ms,
            generate: options.generate,
            log: options.log,
            state: Mutex::new(ConnectStateInner::default()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ConnectStateInner> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn log(&self, level: &str, message: &str) {
        if let Some(log) = &self.log {
            log(level, message);
        }
    }

    pub fn get_state(&self) -> OrcaConnectState {
        let state = self.lock();
        let phase = state.phase.unwrap_or(OrcaConnectPhase::Idle);
        OrcaConnectState {
            phase,
            session_id: state
                .attempt
                .as_ref()
                .map(|attempt| attempt.session_id.clone()),
            authorize_url: state
                .attempt
                .as_ref()
                .map(|attempt| attempt.authorize_url.clone()),
            hint: state.hint.clone(),
            error: state.error.clone(),
            busy: phase.busy(),
            generation: state.generation,
        }
    }

    /// 开始一次登录：新建 verifier/state，返回用户需要打开的授权 URL。
    pub fn begin(&self) -> Result<OrcaConnectState, OrcaConnectError> {
        let generation = {
            let mut state = self.lock();
            state.generation += 1;
            state.generation
        };
        let material = match create_pkce_material(&self.generate) {
            Ok(material) => material,
            Err(error) => {
                let mut state = self.lock();
                state.phase = Some(OrcaConnectPhase::Error);
                state.error = Some(error.message.clone());
                return Err(error);
            }
        };
        let authorize_url = build_orca_authorize_url(OrcaAuthorizeUrlInput {
            auth_base: &self.origins.auth_base,
            callback_url: "oob",
            code_challenge: &material.challenge,
            state: &material.state,
            app_name: &self.app_name,
            scope: None,
        })
        .map_err(|message| OrcaConnectError::new(OrcaConnectErrorKind::Network, message))?;
        let session_bytes = self.generate.as_ref()(8).ok_or_else(random_unavailable)?;
        let session_id = B64.encode(session_bytes);
        let now = (self.now_ms)();
        {
            let mut state = self.lock();
            if state.generation != generation {
                return Err(OrcaConnectError::new(
                    OrcaConnectErrorKind::Cancelled,
                    "OrcaRouter 授权已被新的尝试取代",
                ));
            }
            state.attempt = Some(ActiveAttempt {
                session_id: session_id.clone(),
                generation,
                material,
                authorize_url: authorize_url.clone(),
                started_at: now,
            });
            state.phase = Some(OrcaConnectPhase::Waiting);
            state.error = None;
            state.hint = Some(authorize_url);
        }
        self.log(
            "info",
            &format!("OrcaRouter 授权已开始（session {session_id}）"),
        );
        Ok(self.get_state())
    }

    /// 用户粘贴 code 后完成交换并持久化。
    pub async fn submit_code(&self, code: &str) -> Result<OrcaCredential, OrcaConnectError> {
        let attempt = self.lock().attempt.clone().ok_or_else(|| {
            OrcaConnectError::new(
                OrcaConnectErrorKind::Cancelled,
                "OrcaRouter 授权已取消或尚未开始",
            )
        })?;
        let normalized = code.trim();
        if normalized.is_empty() {
            return Err(OrcaConnectError::new(
                OrcaConnectErrorKind::ExchangeRejected,
                "授权码不能为空",
            ));
        }
        {
            let mut state = self.lock();
            state.phase = Some(OrcaConnectPhase::Exchanging);
            state.hint = None;
            state.error = None;
        }
        let body = json!({
            "code": normalized,
            "code_verifier": attempt.material.verifier,
            "code_challenge_method": ORCAROUTER_CODE_CHALLENGE_METHOD,
        });
        let url = format!("{}{}", self.origins.auth_base, ORCAROUTER_EXCHANGE_PATH);
        let response = match self.client.post(url).json(&body).send().await {
            Ok(response) => response,
            Err(error) => {
                if self.current_generation() != attempt.generation {
                    return Err(OrcaConnectError::new(
                        OrcaConnectErrorKind::Cancelled,
                        "OrcaRouter 授权已被新的尝试取代",
                    ));
                }
                let timed_out = error.is_timeout();
                let kind = if timed_out {
                    OrcaConnectErrorKind::Timeout
                } else {
                    OrcaConnectErrorKind::Network
                };
                let message = if timed_out {
                    "OrcaRouter 授权超时，请重试"
                } else {
                    "无法连接 OrcaRouter 授权服务"
                };
                let mut state = self.lock();
                state.phase = Some(OrcaConnectPhase::Error);
                state.error = Some(message.to_string());
                drop(state);
                self.log("warn", &format!("OrcaRouter 交换失败：{}", kind.as_str()));
                return Err(OrcaConnectError::new(kind, message));
            }
        };
        if self.current_generation() != attempt.generation {
            return Err(OrcaConnectError::new(
                OrcaConnectErrorKind::Cancelled,
                "OrcaRouter 授权已被新的尝试取代",
            ));
        }
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let (kind, message) = match status {
                403 | 400 => (
                    OrcaConnectErrorKind::ExchangeRejected,
                    if status == 403 {
                        "授权码无效、已过期或已被使用，请重新发起授权"
                    } else {
                        "OrcaRouter 拒绝了本次授权请求，请重新发起"
                    },
                ),
                429 => (
                    OrcaConnectErrorKind::RateLimited,
                    "OrcaRouter 授权请求过于频繁（每 24 小时最多 10 次），请稍后再试",
                ),
                _ => (
                    OrcaConnectErrorKind::Network,
                    "OrcaRouter 授权服务暂时不可用",
                ),
            };
            let mut state = self.lock();
            state.phase = Some(OrcaConnectPhase::Error);
            state.error = Some(message.to_string());
            drop(state);
            self.log(
                "warn",
                &format!("OrcaRouter 交换返回 {status}（{}）", kind.as_str()),
            );
            return Err(OrcaConnectError::new(kind, message));
        }
        let payload = match response.json::<Value>().await {
            Ok(payload) => payload,
            Err(_) => {
                if self.current_generation() != attempt.generation {
                    return Err(OrcaConnectError::new(
                        OrcaConnectErrorKind::Cancelled,
                        "OrcaRouter 授权已被新的尝试取代",
                    ));
                }
                let mut state = self.lock();
                state.phase = Some(OrcaConnectPhase::Error);
                state.error = Some("OrcaRouter 返回的授权结果无法解析".to_string());
                return Err(OrcaConnectError::new(
                    OrcaConnectErrorKind::Network,
                    "OrcaRouter 返回的授权结果无法解析",
                ));
            }
        };
        if self.current_generation() != attempt.generation {
            return Err(OrcaConnectError::new(
                OrcaConnectErrorKind::Cancelled,
                "OrcaRouter 授权已被新的尝试取代",
            ));
        }
        let object = payload.as_object();
        let key = object
            .and_then(|object| object.get("key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if key.is_empty() {
            return Err(OrcaConnectError::new(
                OrcaConnectErrorKind::ExchangeRejected,
                "OrcaRouter 未返回 API key",
            ));
        }
        let scope = object
            .and_then(|object| object.get("scope"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if scope != ORCAROUTER_REQUESTED_SCOPE {
            let message = format!(
                "OrcaRouter 只授予了 \"{}\" 范围，无法用于本用途",
                if scope.is_empty() { "(empty)" } else { &scope }
            );
            let mut state = self.lock();
            state.phase = Some(OrcaConnectPhase::Error);
            state.error = Some(message.clone());
            return Err(OrcaConnectError::new(
                OrcaConnectErrorKind::ScopeDowngraded,
                message,
            ));
        }
        let user_id = object
            .and_then(|object| object.get("user_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if self.current_generation() != attempt.generation {
            return Err(OrcaConnectError::new(
                OrcaConnectErrorKind::Cancelled,
                "OrcaRouter 授权已被新的尝试取代",
            ));
        }
        let account_id = (!user_id.is_empty()).then_some(user_id);
        let credential = self
            .store
            .save(&key, OrcaCredentialSource::Pkce, Some(scope), account_id)
            .map_err(|message| OrcaConnectError::new(OrcaConnectErrorKind::Network, message))?;
        {
            let mut state = self.lock();
            state.phase = Some(OrcaConnectPhase::Connected);
            state.hint = None;
            state.error = None;
            state.attempt = None;
        }
        self.log("info", "OrcaRouter 授权完成，密钥已保存到本地凭据存储");
        Ok(credential)
    }

    /// 显式取消：立即失效当前尝试并清空等待态。
    pub fn cancel(&self, _reason: &str) -> OrcaConnectState {
        {
            let mut state = self.lock();
            state.generation += 1;
            state.attempt = None;
            state.phase = Some(OrcaConnectPhase::Idle);
            state.hint = None;
            state.error = None;
        }
        self.get_state()
    }

    /// 失败后由 UI 主动复位（不改 generation）。
    pub fn reset(&self) -> OrcaConnectState {
        {
            let mut state = self.lock();
            state.attempt = None;
            state.phase = Some(OrcaConnectPhase::Idle);
            state.hint = None;
            state.error = None;
        }
        self.get_state()
    }

    /// `pagehide` / 卸载同步清理：清 busy 并递增 generation。
    pub fn invalidate_for_page_hide(&self) -> OrcaConnectState {
        {
            let mut state = self.lock();
            state.generation += 1;
            state.attempt = None;
            state.phase = Some(OrcaConnectPhase::Idle);
            state.hint = None;
            state.error = None;
        }
        self.get_state()
    }

    pub fn current_generation(&self) -> u64 {
        self.lock().generation
    }

    /// Flow A 回调路径复用：state 是否仍匹配当前尝试。
    pub fn matches_state(&self, state: &str) -> bool {
        let current = self.lock();
        current
            .attempt
            .as_ref()
            .is_some_and(|attempt| safe_state_equals(state, &attempt.material.state))
    }

    /// 供 RPC 层展示的 timeout 值（毫秒）。
    pub fn timeout_ms(&self) -> u64 {
        self.timeout.as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use credential_store::CredentialStore;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "orca-connect-{label}-{}-{}",
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

    fn store(dir: &TempDir) -> Arc<OrcaCredentialStore> {
        Arc::new(OrcaCredentialStore::new(Arc::new(
            CredentialStore::with_secret(&dir.0, "orca-connect-test-secret"),
        )))
    }

    fn make_controller(dir: &TempDir, auth_base: &str) -> OrcaConnectController {
        let counter = AtomicU64::new(0);
        let generate: RandomSource = Arc::new(move |length| {
            let seed = counter.fetch_add(1, Ordering::SeqCst);
            Some(
                (0..length)
                    .map(|index| (seed as usize + index) as u8)
                    .collect(),
            )
        });
        OrcaConnectController::new(OrcaConnectOptions {
            credential_store: store(dir),
            origins: OrcaOrigins {
                auth_base: auth_base.to_string(),
                api_base: "https://api.example".to_string(),
            },
            app_name: "Mode".to_string(),
            timeout_ms: 5_000,
            generate,
            now_ms: Arc::new(|| 1_700_000_000_000),
            log: None,
        })
        .expect("controller")
    }

    async fn spawn_http(
        status: &'static str,
        body: String,
    ) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut data = Vec::new();
            let mut buffer = [0_u8; 4096];
            let mut header_end = None;
            let mut content_length = 0_usize;
            loop {
                let read = socket.read(&mut buffer).await.expect("read");
                if read == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..read]);
                if header_end.is_none() {
                    if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                        header_end = Some(index + 4);
                        let head = String::from_utf8_lossy(&data[..index]).to_lowercase();
                        content_length = head
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                    }
                }
                if let Some(end) = header_end {
                    if data.len() >= end + content_length {
                        break;
                    }
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
    fn pkce_material_is_deterministic_for_injected_random() {
        let generate: RandomSource =
            Arc::new(|length| Some((0..length).map(|index| index as u8).collect()));
        let material = create_pkce_material(&generate).expect("material");
        assert_eq!(
            material.verifier,
            B64.encode((0..32).map(|i| i as u8).collect::<Vec<_>>())
        );
        assert_eq!(
            material.challenge,
            B64.encode(Sha256::digest(material.verifier.as_bytes()))
        );
        assert_eq!(
            material.state,
            B64.encode((0..16).map(|i| i as u8).collect::<Vec<_>>())
        );
    }

    #[test]
    fn state_comparison_is_length_and_content_safe() {
        assert!(safe_state_equals("abc", "abc"));
        assert!(!safe_state_equals("abc", "abd"));
        assert!(!safe_state_equals("abc", "abcd"));
    }

    #[test]
    fn begin_and_cancel_advance_generation() {
        let dir = TempDir::new("lifecycle");
        let controller = make_controller(&dir, "https://auth.example");
        let state = controller.begin().expect("begin");
        assert_eq!(state.phase, OrcaConnectPhase::Waiting);
        assert!(state.busy);
        assert!(state
            .authorize_url
            .as_deref()
            .expect("url")
            .contains("code_challenge="));
        let generation = state.generation;
        let attempt_state = controller
            .lock()
            .attempt
            .as_ref()
            .map(|attempt| attempt.material.state.clone())
            .expect("attempt");
        assert!(controller.matches_state(&attempt_state));

        let cancelled = controller.cancel("test");
        assert_eq!(cancelled.phase, OrcaConnectPhase::Idle);
        assert!(!cancelled.busy);
        assert!(cancelled.generation > generation);
        assert!(!controller.matches_state(&attempt_state));
    }

    #[tokio::test]
    async fn submit_code_persists_pkce_credential() {
        let dir = TempDir::new("exchange");
        let (port, server) = spawn_http(
            "200 OK",
            json!({ "key": "sk-orca-abcdef123456", "scope": "api", "user_id": "u-1" }).to_string(),
        )
        .await;
        let controller = make_controller(&dir, &format!("http://127.0.0.1:{port}"));
        controller.begin().expect("begin");
        let credential = controller.submit_code(" code-1 ").await.expect("exchange");
        assert_eq!(credential.api_key, "sk-orca-abcdef123456");
        assert_eq!(credential.source, OrcaCredentialSource::Pkce);
        assert_eq!(credential.account_id.as_deref(), Some("u-1"));
        assert_eq!(credential.granted_scope.as_deref(), Some("api"));
        assert_eq!(controller.get_state().phase, OrcaConnectPhase::Connected);
        let request = server.await.expect("server");
        assert!(request.starts_with("POST /api/v1/auth/keys"), "{request}");
        assert!(request.contains("\"code\":\"code-1\""), "{request}");
        assert!(
            request.contains("\"code_challenge_method\":\"S256\""),
            "{request}"
        );
        assert!(store(&dir).load_usable().expect("load").is_some());
    }

    #[tokio::test]
    async fn submit_code_classifies_rejections() {
        let dir = TempDir::new("reject");
        let (port, server) =
            spawn_http("403 Forbidden", json!({ "error": "bad" }).to_string()).await;
        let controller = make_controller(&dir, &format!("http://127.0.0.1:{port}"));
        controller.begin().expect("begin");
        let error = controller.submit_code("code").await.expect_err("403");
        assert_eq!(error.kind, OrcaConnectErrorKind::ExchangeRejected);
        assert_eq!(controller.get_state().phase, OrcaConnectPhase::Error);
        let _ = server.await;

        let (port, server) = spawn_http(
            "200 OK",
            json!({ "key": "sk-orca-abcdef123456", "scope": "read" }).to_string(),
        )
        .await;
        let controller = make_controller(&dir, &format!("http://127.0.0.1:{port}"));
        controller.begin().expect("begin");
        let error = controller.submit_code("code").await.expect_err("scope");
        assert_eq!(error.kind, OrcaConnectErrorKind::ScopeDowngraded);
        assert!(error.message.contains("\"read\""), "{}", error.message);
        let _ = server.await;
    }

    #[tokio::test]
    async fn cancel_supersedes_in_flight_exchange() {
        let dir = TempDir::new("supersede");
        let controller = make_controller(&dir, "http://127.0.0.1:1");
        controller.begin().expect("begin");
        controller.cancel("user");
        let error = controller.submit_code("code").await.expect_err("cancelled");
        assert_eq!(error.kind, OrcaConnectErrorKind::Cancelled);
    }
}
