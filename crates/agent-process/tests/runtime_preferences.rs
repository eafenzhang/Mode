//! 运行时偏好反向请求：宿主侧应答、参数错误码与注册时机。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    default_runtime_preferences, default_runtime_preferences_handler, runtime_preferences_handler,
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
    let handler = runtime_preferences_handler(|scope| {
        assert_eq!(scope, RuntimePreferencesScope::UserExecution);
        Ok(default_runtime_preferences())
    });

    let reply = handler(&json!({
        "id": "server-1",
        "method": SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD,
        "params": { "sessionId": "sess-1", "scope": "user-execution" }
    }));
    match reply {
        RequestReply::Result(value) => {
            assert_eq!(value["nativeSearchEnhancementsEnabled"], true);
            assert_eq!(value["memoryEnabled"], false);
            assert_eq!(value["askUserQuestionAutoResolutionEnabled"], true);
        }
        other => panic!("unexpected reply: {other:?}"),
    }

    let reply = handler(&json!({
        "id": "server-1",
        "method": SESSION_REQUEST_RUNTIME_PREFERENCES_METHOD,
        "params": { "sessionId": "sess-1", "scope": "bogus" }
    }));
    match reply {
        RequestReply::Error { code, .. } => assert_eq!(code, -32602),
        other => panic!("unexpected reply: {other:?}"),
    }

    let reply = handler(&json!({ "id": "server-1", "method": "other/method", "params": {} }));
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
    let registry = Arc::new(registry("reverse-runtime-preferences-invalid", &log, spawns));
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
    process.client().request("test/trigger", None).await.unwrap();

    let reply = log.reply().expect("已有进程也必须挂上新处理器");
    assert_eq!(reply["result"]["nativeSearchEnhancementsEnabled"], true);
    registry.shutdown_all(Duration::from_secs(3)).await;
}
