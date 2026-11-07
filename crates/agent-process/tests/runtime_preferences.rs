//! 运行时偏好反向请求：宿主侧应答、参数错误码与注册时机。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    default_runtime_preferences, default_runtime_preferences_handler,
    external_runtime_preferences_handler, runtime_preferences_reply,
    PendingSessionRuntimePreferences, RuntimePreferencesResolution,
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget,
    ListSessionsOptions, SpecFactory, SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD,
};
use agent_protocol::session::RuntimePreferencesScope;
use mode_protocol_client::RequestReply;
use serde_json::{json, Value};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-runtime-preferences-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn reply(&self) -> Option<Value> {
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if let Some(line) = text.lines().find(|line| line.starts_with("reply\t")) {
                    return serde_json::from_str(line.trim_start_matches("reply\t")).ok();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn registry(mode: &str, log: &FrameLog, spawns: Arc<AtomicUsize>) -> AgentRuntimeRegistry {
    let mode = mode.to_string();
    let log = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        if !mode.is_empty() {
            spec.args.push(mode.clone());
        }
        spec.env
            .push(("AGENT_FIXTURE_FRAME_LOG".to_string(), log.clone()));
        Ok(spec)
    });
    AgentRuntimeRegistry::new(factory)
}

#[test]
fn handler_answers_default_preferences_and_rejects_bad_params() {
    let resolve = |scope| {
        assert_eq!(scope, RuntimePreferencesScope::UserExecution);
        Ok(default_runtime_preferences())
    };

    let reply = runtime_preferences_reply(&json!({
        "id": "server-1",
        "method": SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD,
        "params": { "sessionId": "sess-1", "scope": "user-execution" }
    }), &resolve);
    match reply {
        RequestReply::Result(value) => {
            assert_eq!(value["nativeSearchEnhancementsEnabled"], true);
            assert_eq!(value["memoryEnabled"], false);
            assert_eq!(value["askUserQuestionAutoResolutionEnabled"], true);
        }
        other => panic!("unexpected reply: {other:?}"),
    }

    let reply = runtime_preferences_reply(&json!({
        "id": "server-1",
        "method": SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD,
        "params": { "sessionId": "sess-1", "scope": "bogus" }
    }), &resolve);
    match reply {
        RequestReply::Error { code, .. } => assert_eq!(code, -32602),
        other => panic!("unexpected reply: {other:?}"),
    }

    let reply = runtime_preferences_reply(
        &json!({ "id": "server-1", "method": "other/method", "params": {} }),
        &resolve,
    );
    assert_eq!(reply, RequestReply::NotHandled);
}

#[tokio::test]
async fn responds_to_reverse_request_sent_on_startup() {
    let log = FrameLog::new("startup");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("reverse-runtime-preferences", &log, spawns));
    registry.set_request_handler(Some(default_runtime_preferences_handler()));
    let control = AgentControlClient::new(Arc::clone(&registry));

    control
        .list_sessions(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListSessionsOptions::default(),
        )
        .await
        .unwrap();

    let reply = log.reply().expect("必须收到反向请求的应答");
    assert_eq!(reply["id"], "server-1");
    assert_eq!(reply["result"]["nativeSearchEnhancementsEnabled"], true);
    assert!(reply.get("error").is_none());
    control.dispose_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn invalid_scope_returns_invalid_params_error() {
    let log = FrameLog::new("invalid");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry(
        "reverse-runtime-preferences-invalid",
        &log,
        spawns,
    ));
    registry.set_request_handler(Some(default_runtime_preferences_handler()));
    let control = AgentControlClient::new(Arc::clone(&registry));

    control
        .list_sessions(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListSessionsOptions::default(),
        )
        .await
        .unwrap();

    let reply = log.reply().expect("必须收到错误应答");
    assert_eq!(reply["error"]["code"], -32602);
    control.dispose_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn handler_set_after_spawn_applies_to_existing_process() {
    let log = FrameLog::new("after-spawn");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("reverse-on-trigger", &log, spawns));
    let process = registry
        .get_or_spawn(&agent_process::WorkspaceTarget::new("/ws/demo"))
        .await
        .unwrap();

    registry.set_request_handler(Some(default_runtime_preferences_handler()));
    process
        .client()
        .request("test/trigger", None)
        .await
        .unwrap();

    let reply = log.reply().expect("已有进程也必须挂上新处理器");
    assert_eq!(reply["result"]["nativeSearchEnhancementsEnabled"], true);
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn external_handler_waits_for_respond() {
    let log = FrameLog::new("external");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("reverse-runtime-preferences", &log, spawns));
    let pending = PendingSessionRuntimePreferences::new();
    let (payload_tx, mut payload_rx) = tokio::sync::mpsc::unbounded_channel();
    registry.set_request_handler(Some(external_runtime_preferences_handler(
        std::sync::Arc::clone(&pending),
        move |payload| {
            let _ = payload_tx.send(payload);
            true
        },
    )));
    let control = AgentControlClient::new(std::sync::Arc::clone(&registry));
    let list = tokio::spawn({
        let control = std::sync::Arc::new(control);
        async move {
            control
                .list_sessions(
                    &AgentWorkspaceTarget::new("/ws/demo"),
                    ListSessionsOptions::default(),
                )
                .await
        }
    });

    let payload = payload_rx.recv().await.expect("renderer 必须收到请求");
    let request_id = payload["requestId"].as_str().expect("requestId").to_owned();
    pending
        .respond(
            &request_id,
            RuntimePreferencesResolution::Resolved(default_runtime_preferences()),
        )
        .unwrap();
    list.await.unwrap().unwrap();

    let reply = log.reply().expect("必须收到反向请求的应答");
    assert_eq!(reply["result"]["nativeSearchEnhancementsEnabled"], true);
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn responds_to_provider_runtime_headers_request() {
    use agent_process::{
        provider_runtime_headers_handler, ProviderRuntimeHeadersResolver,
        ProviderRuntimeHeadersFuture,
    };

    struct StaticResolver;
    impl ProviderRuntimeHeadersResolver for StaticResolver {
        fn resolve<'a>(
            &'a self,
            _params: &'a agent_protocol::provider_runtime_headers::ProviderRuntimeHeadersRequestParams,
        ) -> ProviderRuntimeHeadersFuture<'a> {
            Box::pin(async move { Some("jwt-1".to_string()) })
        }
    }

    let log = FrameLog::new("provider-headers");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("reverse-provider-runtime-headers", &log, spawns));
    registry.push_request_handler(provider_runtime_headers_handler(
        Arc::new(StaticResolver),
        tokio::runtime::Handle::current(),
    ));
    let control = AgentControlClient::new(Arc::clone(&registry));

    control
        .list_sessions(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListSessionsOptions {
                include_archived: false,
                limit: None,
                session_ids: None,
                runtime_policy: agent_process::RuntimePolicy::StartIfNeeded,
            },
        )
        .await
        .unwrap();

    for _ in 0..200 {
        if let Some(reply) = log.reply() {
            assert_eq!(reply["result"]["headersApplied"], true);
            assert_eq!(reply["result"]["requestAuth"]["apiKey"], "jwt-1");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("provider runtime headers reply not observed");
}

#[tokio::test]
async fn provider_runtime_headers_bad_params_are_rejected() {
    use agent_process::{
        provider_runtime_headers_handler, ProviderRuntimeHeadersResolver,
        ProviderRuntimeHeadersFuture,
    };

    struct RejectResolver;
    impl ProviderRuntimeHeadersResolver for RejectResolver {
        fn resolve<'a>(
            &'a self,
            _params: &'a agent_protocol::provider_runtime_headers::ProviderRuntimeHeadersRequestParams,
        ) -> ProviderRuntimeHeadersFuture<'a> {
            Box::pin(async move { Some("unused".to_string()) })
        }
    }

    let log = FrameLog::new("provider-headers-invalid");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry(
        "reverse-provider-runtime-headers-invalid",
        &log,
        spawns,
    ));
    registry.push_request_handler(provider_runtime_headers_handler(
        Arc::new(RejectResolver),
        tokio::runtime::Handle::current(),
    ));
    let control = AgentControlClient::new(Arc::clone(&registry));

    control
        .list_sessions(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListSessionsOptions {
                include_archived: false,
                limit: None,
                session_ids: None,
                runtime_policy: agent_process::RuntimePolicy::StartIfNeeded,
            },
        )
        .await
        .unwrap();

    for _ in 0..200 {
        if let Some(reply) = log.reply() {
            assert_eq!(reply["error"]["code"], -32602);
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("provider runtime headers error reply not observed");
}

#[tokio::test]
async fn responds_to_official_mcp_auth_headers_request() {
    use agent_process::{
        official_mcp_auth_headers_handler, OfficialMcpAuthHeadersFuture,
        OfficialMcpAuthHeadersResolver,
    };
    use std::collections::BTreeMap;

    struct StaticResolver;
    impl OfficialMcpAuthHeadersResolver for StaticResolver {
        fn resolve<'a>(
            &'a self,
            _params: &'a agent_protocol::official_mcp_auth::OfficialMcpAuthHeadersRequestParams,
        ) -> OfficialMcpAuthHeadersFuture<'a> {
            Box::pin(async move {
                let mut headers = BTreeMap::new();
                headers.insert("Authorization".to_string(), "Bearer jwt-1".to_string());
                Ok(headers)
            })
        }
    }

    let log = FrameLog::new("official-mcp");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("reverse-official-mcp-headers", &log, spawns));
    registry.push_request_handler(official_mcp_auth_headers_handler(
        Arc::new(StaticResolver),
        tokio::runtime::Handle::current(),
    ));
    let control = AgentControlClient::new(Arc::clone(&registry));

    control
        .list_sessions(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListSessionsOptions {
                include_archived: false,
                limit: None,
                session_ids: None,
                runtime_policy: agent_process::RuntimePolicy::StartIfNeeded,
            },
        )
        .await
        .unwrap();

    for _ in 0..200 {
        if let Some(reply) = log.reply() {
            assert_eq!(reply["result"]["ok"], true);
            assert_eq!(reply["result"]["headers"]["Authorization"], "Bearer jwt-1");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("official mcp auth headers reply not observed");
}

#[tokio::test]
async fn official_mcp_auth_headers_failure_reason_is_preserved() {
    use agent_process::{
        official_mcp_auth_headers_handler, OfficialMcpAuthHeadersFuture,
        OfficialMcpAuthHeadersResolver, OfficialMcpAuthPortFailureReason,
    };
    struct UntrustedResolver;
    impl OfficialMcpAuthHeadersResolver for UntrustedResolver {
        fn resolve<'a>(
            &'a self,
            _params: &'a agent_protocol::official_mcp_auth::OfficialMcpAuthHeadersRequestParams,
        ) -> OfficialMcpAuthHeadersFuture<'a> {
            Box::pin(async move {
                Err(OfficialMcpAuthPortFailureReason::OfficialMcpOriginUntrusted)
            })
        }
    }

    let log = FrameLog::new("official-mcp-untrusted");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("reverse-official-mcp-headers", &log, spawns));
    registry.push_request_handler(official_mcp_auth_headers_handler(
        Arc::new(UntrustedResolver),
        tokio::runtime::Handle::current(),
    ));
    let control = AgentControlClient::new(Arc::clone(&registry));

    control
        .list_sessions(
            &AgentWorkspaceTarget::new("/ws/demo"),
            ListSessionsOptions {
                include_archived: false,
                limit: None,
                session_ids: None,
                runtime_policy: agent_process::RuntimePolicy::StartIfNeeded,
            },
        )
        .await
        .unwrap();

    for _ in 0..200 {
        if let Some(reply) = log.reply() {
            assert_eq!(reply["result"]["ok"], false);
            assert_eq!(
                reply["result"]["reason"],
                "official_mcp_origin_untrusted"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("official mcp auth headers failure reply not observed");
}
