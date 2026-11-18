//! C3 T3：后台通知与终态收口。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §3 与 T0 夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c3/background-*.json`（只读），
//! 以及 C1 的后台通知时序（`docs/specs/tauri-agent-runtime.md` §3.4/§3.5）。
//!
//! 覆盖：
//! - 空闲完成 → 一个 `background_task` 通知 turn（XML/来源字段/DB 消息语义）；
//! - 忙时（模型流式等待）完成 → 合并延后，一个 turn 含两条通知；
//! - 前台工具执行期间完成 → 丢弃（C1 观察；C3 未重录，登记在契约 §3.5/§3.6）；
//! - goal 续跑 → 通知 turn 在 verifier 前，计入 target tokens；
//! - 终态恰好一次：工具错误 / provider 错误 / 超时 / 取消；已提交行保留；
//! - 取消占位（`finish:null`、无 part）与 apiRetry 清空（含 goal 失败）。
//!
//! 测试库 DDL 来自 `scripts/sqlite-schema-baseline.json`；provider 是进程内 fake，
//! 不发起真实网络请求。后台命令用短进程制造完成窗口，不把 sleep 当同步证明：
//! 完成性用输出文件内容/事件谓词验证。

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse,
    ModelUsage, ProviderError,
};
use agent_core::RetryPolicy;
use agent_protocol::session_event::{SessionEvent, SessionEventType};
use agent_server::{
    run_stdio, AgentServer, Frame, ProviderConfigHub, ProviderSource, RequestId, RuntimeConfig,
    ServerConfig, SessionInput, SessionRuntime,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use session_store::{SessionRecord, SessionStore};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

// ---------------------------------------------------------------------------
// 测试库 / 临时目录
// ---------------------------------------------------------------------------

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS schema_migration (
    id text primary key,
    checksum text not null,
    app_version text,
    time_applied integer not null
)";

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-background-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn db_path(&self) -> PathBuf {
        self.0.join("db.sqlite")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn baseline() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("sqlite-schema-baseline.json");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("读取 schema 基线失败：{error}"));
    serde_json::from_str(&text).expect("schema 基线必须是 JSON")
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn table_ddl(name: &str, table: &Value) -> String {
    let columns = table["columns"].as_array().expect("columns 数组");
    let mut definitions = Vec::with_capacity(columns.len() + 1);
    let mut primary_key: Vec<(i64, String)> = Vec::new();
    for column in columns {
        let column_name = column["name"].as_str().expect("列名");
        let column_type = column["type"].as_str().expect("列类型");
        let mut definition = format!("{} {}", quote_identifier(column_name), column_type);
        if column["notNull"].as_bool().unwrap_or(false) {
            definition.push_str(" NOT NULL");
        }
        if let Some(default) = column["defaultValue"].as_str() {
            definition.push_str(" DEFAULT ");
            definition.push_str(default);
        }
        definitions.push(definition);
        let position = column["primaryKey"].as_i64().unwrap_or(0);
        if position > 0 {
            primary_key.push((position, column_name.to_owned()));
        }
    }
    if !primary_key.is_empty() {
        primary_key.sort_by_key(|(position, _)| *position);
        let names = primary_key
            .iter()
            .map(|(_, column)| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        definitions.push(format!("PRIMARY KEY ({names})"));
    }
    format!(
        "CREATE TABLE {} (\n  {}\n)",
        quote_identifier(name),
        definitions.join(",\n  ")
    )
}

fn create_baseline_database(path: &Path) {
    let tables = baseline()["databases"]["session-store"]["tables"]
        .as_object()
        .expect("基线缺少 databases[session-store].tables")
        .clone();
    let connection = Connection::open(path).expect("创建测试库");
    for (name, table) in &tables {
        connection
            .execute_batch(&table_ddl(name, table))
            .unwrap_or_else(|error| panic!("创建表 {name} 失败：{error}"));
    }
    for (_name, table) in &tables {
        for index in table["indexes"].as_array().expect("indexes 数组") {
            let sql = index["sql"].as_str().expect("索引 SQL");
            connection
                .execute_batch(sql)
                .unwrap_or_else(|error| panic!("创建索引失败：{error}"));
        }
    }
    drop(connection);
    let ledger = Connection::open(path).expect("打开测试库写账本");
    ledger.execute_batch(LEDGER_DDL).expect("创建账本");
    ledger
        .execute(
            "INSERT INTO schema_migration (id, checksum, app_version, time_applied) \
             VALUES ('0001_fixture', 'fixture-checksum', '0.0.0', 1)",
            [],
        )
        .expect("写入账本行");
}

fn raw_connection(path: &Path) -> Connection {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("只读连接");
    connection
        .busy_timeout(Duration::from_secs(5))
        .expect("busy_timeout");
    connection
}

fn turn_usage_rows(path: &Path, session_id: &str) -> Vec<Value> {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare(
            "SELECT turn_id, status, model_request_count, computed_total_tokens, \
                    cancelled_by_user, error_type, error_code \
             FROM turn_usage WHERE session_id = ?1 ORDER BY started_at",
        )
        .expect("准备 turn_usage 查询");
    statement
        .query_map([session_id], |row| {
            Ok(json!({
                "turnId": row.get::<_, String>(0)?,
                "status": row.get::<_, String>(1)?,
                "modelRequestCount": row.get::<_, i64>(2)?,
                "computedTotalTokens": row.get::<_, i64>(3)?,
                "cancelledByUser": row.get::<_, i64>(4)?,
                "errorType": row.get::<_, Option<String>>(5)?,
                "errorCode": row.get::<_, Option<String>>(6)?,
            }))
        })
        .expect("查询 turn_usage")
        .map(|row| row.expect("读取 turn_usage"))
        .collect()
}

/// 指定 `semantics.kind` 的消息行数（C3 §3.1 的 `background_notification`）。
fn message_kind_count(path: &Path, session_id: &str, kind: &str) -> i64 {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare("SELECT data FROM message WHERE session_id = ?1")
        .expect("准备 message 查询");
    statement
        .query_map([session_id], |row| row.get::<_, String>(0))
        .expect("查询 message")
        .map(|row| {
            serde_json::from_str::<Value>(&row.expect("读取 message")).expect("message JSON")
        })
        .filter(|data| data["semantics"]["kind"] == json!(kind))
        .count() as i64
}

fn background_message_sources(path: &Path, session_id: &str) -> Vec<String> {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare("SELECT data FROM message WHERE session_id = ?1")
        .expect("准备 message 查询");
    statement
        .query_map([session_id], |row| row.get::<_, String>(0))
        .expect("查询 message")
        .map(|row| {
            serde_json::from_str::<Value>(&row.expect("读取 message")).expect("message JSON")
        })
        .filter(|data| data["semantics"]["kind"] == json!("background_notification"))
        .filter_map(|data| {
            data["semantics"]["source"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| Some(String::new()))
        })
        .collect()
}

/// `finish:null` 的 assistant 行及其 part 数（取消占位断言）。
fn finishless_assistant(path: &Path, session_id: &str) -> Vec<(String, i64)> {
    let connection = raw_connection(path);
    let mut messages = Vec::new();
    {
        let mut statement = connection
            .prepare("SELECT id, data FROM message WHERE session_id = ?1")
            .expect("准备 message 查询");
        let rows = statement
            .query_map([session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("查询 message");
        for row in rows {
            let (id, data) = row.expect("读取 message");
            let data: Value = serde_json::from_str(&data).expect("message JSON");
            if data["role"] == json!("assistant") && data["finish"].is_null() {
                messages.push(id);
            }
        }
    }
    messages
        .into_iter()
        .map(|id| {
            let parts: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM part WHERE message_id = ?1",
                    [id.clone()],
                    |row| row.get(0),
                )
                .expect("统计 part");
            (id, parts)
        })
        .collect()
}

fn assistant_rows(path: &Path, session_id: &str) -> i64 {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare("SELECT data FROM message WHERE session_id = ?1")
        .expect("准备 message 查询");
    statement
        .query_map([session_id], |row| row.get::<_, String>(0))
        .expect("查询 message")
        .map(|row| {
            serde_json::from_str::<Value>(&row.expect("读取 message")).expect("message JSON")
        })
        .filter(|data| data["role"] == json!("assistant"))
        .count() as i64
}

// ---------------------------------------------------------------------------
// provider（进程内 fake）
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ToolCallSpec {
    tool_call_id: String,
    tool_name: String,
    input: Value,
}

#[derive(Clone)]
enum MainStep {
    ToolCalls(Vec<ToolCallSpec>),
    Text {
        text: String,
        usage: (u64, u64),
    },
    GatedText {
        text: String,
        usage: (u64, u64),
        entered: Arc<tokio::sync::Notify>,
        gate: Arc<tokio::sync::Semaphore>,
    },
    Error(ProviderError),
}

struct BackgroundProvider {
    steps: Mutex<VecDeque<MainStep>>,
    calls: Arc<AtomicU64>,
    verifier: Mutex<VecDeque<String>>,
    notification_text: String,
    notification_usage: (u64, u64),
}

impl BackgroundProvider {
    fn new(steps: Vec<MainStep>) -> (Self, Arc<AtomicU64>) {
        let calls = Arc::new(AtomicU64::new(0));
        (
            Self {
                steps: Mutex::new(steps.into()),
                calls: Arc::clone(&calls),
                verifier: Mutex::new(VecDeque::new()),
                notification_text: "notification handled".to_string(),
                notification_usage: (10, 12),
            },
            calls,
        )
    }

    fn with_notification_text(mut self, text: &str) -> Self {
        self.notification_text = text.to_string();
        self
    }

    fn queue_verifier(&self, response: &str) {
        self.verifier
            .lock()
            .expect("verifier queue")
            .push_back(response.to_string());
    }
}

fn last_user(request: &ModelRequest) -> String {
    match request.messages.last() {
        Some(ModelMessage::User(text)) => text.clone(),
        Some(ModelMessage::System(text)) => text.clone(),
        _ => String::new(),
    }
}

fn response_usage((input, output): (u64, u64)) -> ModelUsage {
    ModelUsage {
        input_tokens: input,
        output_tokens: output,
        ..ModelUsage::default()
    }
}

impl ModelProvider for BackgroundProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        let last_user = last_user(&request);
        if last_user.contains(agent_core::goal::VERIFIER_MARKER) {
            let response = self
                .verifier
                .lock()
                .expect("verifier queue")
                .pop_front()
                .unwrap_or_else(|| {
                    r#"{"passed":true,"reason":"goal verified","nextAction":null}"#.to_string()
                });
            return Box::pin(async move {
                let _ = deltas.send(ModelDelta::Text { text: response });
                Ok(ModelResponse {
                    usage: response_usage((1, 1)),
                    stop_reason: "stop",
                })
            });
        }
        if last_user.starts_with("<task-notification>") {
            let text = self.notification_text.clone();
            let usage = self.notification_usage;
            return Box::pin(async move {
                let _ = deltas.send(ModelDelta::Text { text });
                Ok(ModelResponse {
                    usage: response_usage(usage),
                    stop_reason: "stop",
                })
            });
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .expect("steps lock")
            .pop_front()
            .unwrap_or(MainStep::Text {
                text: "done".to_string(),
                usage: (10, 12),
            });
        Box::pin(async move {
            match step {
                MainStep::ToolCalls(calls) => {
                    for call in calls {
                        let _ = deltas.send(ModelDelta::ToolCall {
                            tool_call_id: call.tool_call_id,
                            tool_name: call.tool_name,
                            input: call.input,
                        });
                    }
                    Ok(ModelResponse {
                        usage: response_usage((10, 20)),
                        stop_reason: "tool-calls",
                    })
                }
                MainStep::Text { text, usage } => {
                    let _ = deltas.send(ModelDelta::Text { text });
                    Ok(ModelResponse {
                        usage: response_usage(usage),
                        stop_reason: "stop",
                    })
                }
                MainStep::GatedText {
                    text,
                    usage,
                    entered,
                    gate,
                } => {
                    let _ = deltas.send(ModelDelta::Text {
                        text: "partial ".to_string(),
                    });
                    entered.notify_one();
                    let _permit = gate
                        .acquire()
                        .await
                        .map_err(|_| ProviderError::local("stream gate closed"))?;
                    let _ = deltas.send(ModelDelta::Text { text });
                    Ok(ModelResponse {
                        usage: response_usage(usage),
                        stop_reason: "stop",
                    })
                }
                MainStep::Error(error) => Err(error),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// 运行时夹具
// ---------------------------------------------------------------------------

struct Fixture {
    store: Arc<SessionStore>,
    record: SessionRecord,
    db_path: PathBuf,
    root: TempRoot,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = TempRoot::new(label);
        let db_path = root.db_path();
        create_baseline_database(&db_path);
        let store = Arc::new(SessionStore::open(&db_path).expect("打开测试库"));
        let cwd = root.path().join("ws");
        fs::create_dir_all(&cwd).expect("建工作目录");
        let record = SessionRecord {
            id: "sess_background".to_owned(),
            project_id: "proj_background".to_owned(),
            workspace_id: None,
            parent_id: None,
            slug: "sess_background".to_owned(),
            directory: cwd.to_string_lossy().to_string(),
            path: Some(cwd.to_string_lossy().to_string()),
            title: String::new(),
            version: "0.0.0".to_owned(),
            share_url: None,
            summary_additions: None,
            summary_deletions: None,
            summary_files: None,
            summary_diffs: None,
            revert: None,
            permission: Some("{\"mode\":\"build\"}".to_owned()),
            time_created: 1_000,
            time_updated: 1_000,
            time_compacting: None,
            time_archived: None,
            task_type: "interactive".to_owned(),
            title_source: "first_input".to_owned(),
            title_message_id: None,
            time_title_updated: None,
            trace_id: Some("trace-background".to_owned()),
        };
        store
            .create_session(record.clone())
            .expect("创建 session 行");
        Self {
            store,
            record,
            db_path,
            root,
        }
    }

    fn data_root(&self) -> PathBuf {
        self.root.path().join(".mode")
    }

    fn output_path(&self, tool_call_id: &str) -> PathBuf {
        self.data_root()
            .join("cli")
            .join("exec")
            .join(&self.record.id)
            .join(format!("{tool_call_id}-stdout.log"))
    }
}

struct FixedSource(Arc<dyn ModelProvider>);

impl ProviderSource for FixedSource {
    fn resolve(
        &self,
        _provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        Ok(Arc::clone(&self.0))
    }
}

fn runtime_config(data_root: PathBuf, retry: RetryPolicy) -> RuntimeConfig {
    RuntimeConfig {
        mode: "build".to_string(),
        provider_id: "fixture-provider".to_string(),
        model_id: "fixture-model".to_string(),
        reasoning_level: None,
        capabilities: agent_core::ModelCapabilities {
            context_window: Some(128_000),
            max_output_tokens: Some(8_192),
        },
        thought_level: None,
        model_catalog: None,
        data_root: Some(data_root),
        event_log_capacity: 4096,
        broadcast_capacity: 256,
        retry,
    }
}

fn spawn_runtime(
    fixture: &Fixture,
    provider: Arc<dyn ModelProvider>,
    retry: RetryPolicy,
) -> SessionRuntime {
    SessionRuntime::new(
        fixture.record.clone(),
        Arc::clone(&fixture.store),
        Arc::new(FixedSource(provider)),
        runtime_config(fixture.data_root(), retry),
        agent_server::RuntimeWiring::default(),
    )
    .expect("构造运行时")
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        base_delay_ms: 0,
        ..RetryPolicy::default()
    }
}

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(15), future)
        .await
        .expect("用例超时（15s）")
}

async fn drain_until<F>(
    receiver: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
    predicate: F,
) -> Vec<SessionEvent>
where
    F: Fn(&SessionEvent) -> bool,
{
    let mut events = Vec::new();
    loop {
        let event = within(receiver.recv()).await.expect("总线不应关闭");
        let hit = predicate(&event);
        events.push(event);
        if hit {
            return events;
        }
    }
}

fn is_terminal(event: &SessionEvent) -> bool {
    matches!(
        event.event_type,
        SessionEventType::TurnCompleted | SessionEventType::TurnFailed
    )
}

async fn next_terminal(
    receiver: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
) -> Vec<SessionEvent> {
    drain_until(receiver, is_terminal).await
}

fn terminals(events: &[SessionEvent]) -> Vec<&SessionEvent> {
    events.iter().filter(|event| is_terminal(event)).collect()
}

fn turn_starts(events: &[SessionEvent]) -> Vec<&SessionEvent> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::TurnStarted)
        .collect()
}

fn background_turn_starts(events: &[SessionEvent]) -> Vec<&SessionEvent> {
    events
        .iter()
        .filter(|event| {
            event.event_type == SessionEventType::TurnStarted
                && event
                    .payload
                    .as_ref()
                    .is_some_and(|payload| payload["inputSource"] == json!("background_task"))
        })
        .collect()
}

async fn wait_settled(runtime: &SessionRuntime) {
    for _ in 0..600 {
        let status = runtime.snapshot().projection.status;
        if !matches!(status, agent_protocol::session_read::SessionStatus::Running) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("终态收口未在 3s 内完成");
}

async fn wait_for_file(path: &Path, needle: &str) {
    for _ in 0..600 {
        if fs::read_to_string(path)
            .map(|text| text.contains(needle))
            .unwrap_or(false)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} 未在限期内出现 {:?}", path.display(), needle);
}

fn notification_xml(
    task_id: &str,
    tool_call_id: &str,
    output_path: &Path,
    summary: &str,
) -> String {
    format!(
        "<task-notification>\n<task-id>{task_id}</task-id>\n<tool-use-id>{tool_call_id}</tool-use-id>\n<output-file>{}</output-file>\n<status>completed</status>\n<summary>{summary}</summary>\n</task-notification>",
        output_path.display()
    )
}

fn bash_background(id: &str, command: &str, description: &str) -> ToolCallSpec {
    ToolCallSpec {
        tool_call_id: id.to_string(),
        tool_name: "Bash".to_string(),
        input: json!({
            "command": command,
            "run_in_background": true,
            "description": description,
        }),
    }
}

fn bash_foreground(id: &str, command: &str) -> ToolCallSpec {
    ToolCallSpec {
        tool_call_id: id.to_string(),
        tool_name: "Bash".to_string(),
        input: json!({ "command": command }),
    }
}

// ---------------------------------------------------------------------------
// 1. 空闲完成 → 一个通知 turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn idle_completion_opens_one_background_notification_turn() {
    let fixture = Fixture::new("idle");
    let (provider, _calls) = BackgroundProvider::new(vec![
        MainStep::ToolCalls(vec![bash_background(
            "toolu_bg_idle",
            "sleep 0.3; printf 'idle-bg-done\\n'",
            "idle background marker",
        )]),
        MainStep::Text {
            text: "background started".to_string(),
            usage: (10, 12),
        },
    ]);
    let provider = provider.with_notification_text("notification turn acknowledged");
    let runtime = spawn_runtime(&fixture, Arc::new(provider), fast_retry());
    let (_replay, mut receiver) = runtime.subscribe(None);
    let session_id = fixture.record.id.clone();

    runtime
        .submit(SessionInput::text("start background idle"))
        .expect("受理输入");
    let first = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&first).len(), 1, "普通 turn 终态恰好一次");
    assert_eq!(
        first
            .last()
            .expect("terminal")
            .payload
            .as_ref()
            .expect("payload")["resultType"],
        json!("success")
    );

    // 主 turn 结束后会话空闲；后台任务完成后应唤醒 worker 并开通知 turn。
    let second = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&second).len(), 1, "通知 turn 终态恰好一次");
    let starts = turn_starts(&second);
    assert_eq!(starts.len(), 1);
    let payload = starts[0].payload.as_ref().expect("payload");
    assert_eq!(payload["inputSource"], json!("background_task"));
    assert_eq!(payload["inputVisibility"], json!("model-only"));
    assert_eq!(payload["backgroundSource"], json!("bash"));
    assert_eq!(payload["turnNumber"], json!(1));
    assert_eq!(payload["foregroundExecutionId"], json!("runtime_command_2"));
    let work_id = payload["originMeta"]["workId"]
        .as_str()
        .expect("workId")
        .to_string();
    assert!(work_id.starts_with("exec_"), "后台 task id: {work_id}");
    assert_eq!(payload["originMeta"]["backgroundSource"], json!("bash"));
    assert_eq!(
        payload["originMeta"]["title"],
        json!("idle background marker")
    );
    assert!(payload.get("targetId").is_none(), "通知 turn 没有 targetId");
    let expected = notification_xml(
        &work_id,
        "toolu_bg_idle",
        &fixture.output_path("toolu_bg_idle"),
        "Background command \"idle background marker\" completed (exit code 0)",
    );
    assert_eq!(payload["input"], json!(expected));
    // turn.completed 带 inputId（通知 turn 的终态形状）。
    assert!(second
        .last()
        .expect("terminal")
        .payload
        .as_ref()
        .expect("payload")
        .get("inputId")
        .and_then(Value::as_str)
        .is_some());

    wait_settled(&runtime).await;
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.projection.turn_count, 2);
    assert_eq!(
        snapshot.projection.status,
        agent_protocol::session_read::SessionStatus::Idle
    );
    assert_eq!(
        message_kind_count(&fixture.db_path, &session_id, "background_notification"),
        1
    );
    assert_eq!(
        background_message_sources(&fixture.db_path, &session_id),
        vec!["background_task".to_string()]
    );
    assert_eq!(turn_usage_rows(&fixture.db_path, &session_id).len(), 2);
    assert!(fs::read_to_string(fixture.output_path("toolu_bg_idle"))
        .expect("读后台输出")
        .contains("idle-bg-done"));
}

// ---------------------------------------------------------------------------
// 2. 忙时（模型流式等待）完成 → 合并为一个通知 turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn busy_completions_coalesce_into_one_notification_turn() {
    let fixture = Fixture::new("busy");
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (provider, _calls) = BackgroundProvider::new(vec![
        MainStep::ToolCalls(vec![
            bash_background(
                "toolu_bg_one",
                "sleep 0.05; printf 'busy-one\\n'",
                "busy one",
            ),
            bash_background(
                "toolu_bg_two",
                "sleep 0.4; printf 'busy-two\\n'",
                "busy two",
            ),
        ]),
        MainStep::GatedText {
            text: "busy answer".to_string(),
            usage: (10, 12),
            entered: Arc::clone(&entered),
            gate: Arc::clone(&gate),
        },
    ]);
    let provider = provider.with_notification_text("notification handled");
    let runtime = spawn_runtime(&fixture, Arc::new(provider), fast_retry());
    let (_replay, mut receiver) = runtime.subscribe(None);
    let session_id = fixture.record.id.clone();

    runtime
        .submit(SessionInput::text("start background busy"))
        .expect("受理输入");
    // 第二个模型请求已发出并停住：这正是夹具的「模型流式等待窗口」。
    within(entered.notified()).await;
    // 两个后台任务在窗口内完成（输出文件内容即完成证据）。
    wait_for_file(&fixture.output_path("toolu_bg_one"), "busy-one").await;
    wait_for_file(&fixture.output_path("toolu_bg_two"), "busy-two").await;
    // 放行 busy turn。
    gate.add_permits(1);

    let first = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&first).len(), 1);
    let second = within(next_terminal(&mut receiver)).await;
    let mut events = first;
    let note_starts = background_turn_starts(&second);
    assert_eq!(note_starts.len(), 1, "合并后只有一个通知 turn");
    let payload = note_starts[0].payload.as_ref().expect("payload");
    assert_eq!(payload["turnNumber"], json!(1));
    let work_one = payload["originMeta"]["workId"].as_str().expect("workId");
    let input_one = notification_xml(
        work_one,
        "toolu_bg_one",
        &fixture.output_path("toolu_bg_one"),
        "Background command \"busy one\" completed (exit code 0)",
    );
    let input = payload["input"].as_str().expect("input");
    let first_at = input.find(&input_one).expect("第一条通知");
    let second_xml = "<summary>Background command \"busy two\" completed (exit code 0)</summary>";
    let second_at = input.find(second_xml).expect("第二条通知");
    assert!(first_at < second_at, "按完成顺序合并：{input}");
    assert_eq!(
        input.matches("<task-notification>").count(),
        2,
        "两条通知都在一个 turn 里"
    );
    assert_eq!(payload["originMeta"]["title"], json!("busy one · busy two"));
    events.extend(second);
    assert_eq!(
        background_turn_starts(&events).len(),
        1,
        "不得重复开通知 turn"
    );

    wait_settled(&runtime).await;
    assert_eq!(runtime.snapshot().projection.turn_count, 2);
    assert_eq!(
        message_kind_count(&fixture.db_path, &session_id, "background_notification"),
        1
    );
    assert_eq!(turn_usage_rows(&fixture.db_path, &session_id).len(), 2);
}

// ---------------------------------------------------------------------------
// 3. 前台工具执行期间完成 → 丢弃（C1 观察；登记 §3.5/§3.6）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn foreground_tool_busy_completion_is_dropped() {
    let fixture = Fixture::new("foreground");
    let (provider, _calls) = BackgroundProvider::new(vec![
        MainStep::ToolCalls(vec![
            bash_background(
                "toolu_bg_dropped",
                "sleep 0.1; printf 'dropped-bg\\n'",
                "foreground busy bg",
            ),
            bash_foreground("toolu_bg_fg", "sleep 0.9; printf 'foreground\\n'"),
        ]),
        MainStep::Text {
            text: "after tools".to_string(),
            usage: (10, 12),
        },
    ]);
    let runtime = spawn_runtime(&fixture, Arc::new(provider), fast_retry());
    let (_replay, mut receiver) = runtime.subscribe(None);
    let session_id = fixture.record.id.clone();

    runtime
        .submit(SessionInput::text("foreground busy"))
        .expect("受理输入");
    let events = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&events).len(), 1);
    // 后台任务确实完成了（不是慢到没完成）。
    wait_for_file(&fixture.output_path("toolu_bg_dropped"), "dropped-bg").await;
    wait_settled(&runtime).await;

    // 窗口内完成的通知被丢弃：等一个宽限期，不得出现 notification turn。
    let mut saw_notification = false;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(700);
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, receiver.recv()).await {
        if event.event_type == SessionEventType::TurnStarted
            && event
                .payload
                .as_ref()
                .is_some_and(|payload| payload["inputSource"] == json!("background_task"))
        {
            saw_notification = true;
            break;
        }
    }
    assert!(
        !saw_notification,
        "前台工具执行期间完成的通知按 C1 观察丢弃"
    );
    assert_eq!(
        message_kind_count(&fixture.db_path, &session_id, "background_notification"),
        0
    );
    assert_eq!(turn_usage_rows(&fixture.db_path, &session_id).len(), 1);
    assert_eq!(runtime.snapshot().projection.turn_count, 1);
}

// ---------------------------------------------------------------------------
// 4. 终态恰好一次：工具错误 / provider 错误 / 超时 / 取消
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_error_keeps_committed_rows_and_terminal_once() {
    let fixture = Fixture::new("tool-error");
    let (provider, _calls) = BackgroundProvider::new(vec![
        MainStep::ToolCalls(vec![ToolCallSpec {
            tool_call_id: "toolu_missing".to_string(),
            tool_name: "NoSuchTool".to_string(),
            input: json!({}),
        }]),
        MainStep::Text {
            text: "recovered answer".to_string(),
            usage: (10, 12),
        },
    ]);
    let runtime = spawn_runtime(&fixture, Arc::new(provider), fast_retry());
    let (_replay, mut receiver) = runtime.subscribe(None);
    let session_id = fixture.record.id.clone();
    runtime
        .submit(SessionInput::text("tool error turn"))
        .expect("受理输入");
    let events = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&events).len(), 1, "工具错误后仍只有一个终态");
    assert!(events.iter().any(|event| {
        event.event_type == SessionEventType::ToolUpdated
            && event
                .payload
                .as_ref()
                .is_some_and(|payload| payload["kind"] == json!("error"))
    }));
    wait_settled(&runtime).await;
    let rows = assistant_rows(&fixture.db_path, &session_id);
    assert!(rows >= 2, "工具步骤与收尾步骤都已提交（got {rows}）");
    assert!(
        finishless_assistant(&fixture.db_path, &session_id).is_empty(),
        "成功 turn 不留下半截消息"
    );
    assert_eq!(turn_usage_rows(&fixture.db_path, &session_id).len(), 1);
}

#[tokio::test]
async fn provider_error_terminal_is_once_and_placeholder_has_no_parts() {
    let fixture = Fixture::new("provider-error");
    let (provider, calls) = BackgroundProvider::new(vec![MainStep::Error(ProviderError::auth(
        401,
        "invalid api key",
    ))]);
    let runtime = spawn_runtime(&fixture, Arc::new(provider), fast_retry());
    let (_replay, mut receiver) = runtime.subscribe(None);
    let session_id = fixture.record.id.clone();
    runtime
        .submit(SessionInput::text("provider error turn"))
        .expect("受理输入");
    let events = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&events).len(), 1, "turn.failed 恰好一次");
    assert_eq!(
        events.last().expect("terminal").event_type,
        SessionEventType::TurnFailed
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "401 不重试");
    wait_settled(&runtime).await;
    let placeholders = finishless_assistant(&fixture.db_path, &session_id);
    assert_eq!(placeholders.len(), 1);
    assert_eq!(placeholders[0].1, 0, "占位没有 part");
    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["status"], json!("failed"));
}

#[tokio::test]
async fn timeout_failure_terminal_is_once() {
    let fixture = Fixture::new("timeout");
    let (provider, calls) = BackgroundProvider::new(vec![MainStep::Error(
        ProviderError::http_status(408, "request timed out"),
    )]);
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(provider),
        RetryPolicy {
            max_retries: 0,
            base_delay_ms: 0,
            ..RetryPolicy::default()
        },
    );
    let (_replay, mut receiver) = runtime.subscribe(None);
    let session_id = fixture.record.id.clone();
    runtime
        .submit(SessionInput::text("timeout turn"))
        .expect("受理输入");
    let events = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&events).len(), 1, "超时失败只有一个终态");
    assert_eq!(
        events.last().expect("terminal").event_type,
        SessionEventType::TurnFailed
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "maxRetries:0 不再尝试");
    wait_settled(&runtime).await;
    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["status"], json!("failed"));
}

#[tokio::test]
async fn cancel_terminal_is_once_with_placeholder_and_api_retry_cleared() {
    let fixture = Fixture::new("cancel");
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (provider, _calls) = BackgroundProvider::new(vec![MainStep::GatedText {
        text: "never delivered".to_string(),
        usage: (10, 12),
        entered: Arc::clone(&entered),
        gate: Arc::clone(&gate),
    }]);
    let runtime = spawn_runtime(&fixture, Arc::new(provider), fast_retry());
    let (_replay, mut receiver) = runtime.subscribe(None);
    let session_id = fixture.record.id.clone();
    runtime
        .submit(SessionInput::text("cancel turn"))
        .expect("受理输入");
    within(entered.notified()).await;
    runtime.cancel_turn().expect("取消当前 turn");
    gate.add_permits(1);

    let events = within(next_terminal(&mut receiver)).await;
    assert_eq!(terminals(&events).len(), 1, "取消终态恰好一次");
    let terminal = events.last().expect("terminal");
    assert_eq!(terminal.event_type, SessionEventType::TurnCompleted);
    assert_eq!(
        terminal.payload.as_ref().expect("payload")["resultType"],
        json!("cancelled")
    );
    wait_settled(&runtime).await;
    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["status"], json!("cancelled"));
    assert_eq!(usage[0]["cancelledByUser"], json!(1));
    assert_eq!(usage[0]["modelRequestCount"], json!(1));
    let placeholders = finishless_assistant(&fixture.db_path, &session_id);
    assert_eq!(placeholders.len(), 1, "取消补 finish:null 占位");
    assert_eq!(placeholders[0].1, 0);
    let snapshot = runtime.snapshot();
    let projected = serde_json::to_value(&snapshot).expect("snapshot JSON");
    assert!(
        projected["runtime"]["apiRetry"].is_null(),
        "取消终态后 apiRetry 必须清空: {}",
        projected["runtime"]["apiRetry"]
    );
}

// ---------------------------------------------------------------------------
// 5. goal 集成（stdio）：通知 turn 在 verifier 前并计入目标
// ---------------------------------------------------------------------------

fn create_params(ws: &Path) -> Value {
    json!({
        "workspace": {
            "workspacePath": ws.to_string_lossy(),
            "workspaceKey": ws.to_string_lossy()
        },
        "model": {
            "providerId": "mock-chat",
            "modelId": "mock-1",
            "options": { "reasoningLevel": "disabled" }
        },
        "thoughtLevel": "disabled",
        "titleGenerationEnabled": false
    })
}

fn write_personal_file(root: &Path) -> PathBuf {
    let path = root.join("provider-config.json");
    let document = json!({
        "schemaVersion": 1,
        "config": {
            "providerConfigRules": {
                "providerRules": [{
                    "providerId": "mock-chat",
                    "providerName": "Mock Chat",
                    "enabled": true,
                    "config": {
                        "group": "standard-personal",
                        "visibility": "visible",
                        "access": { "type": "api-key", "apiKey": "mock-key-abc" },
                        "personalModelIds": ["mock-1"],
                        "api": { "type": "openai-chat-completions", "baseUrl": "http://127.0.0.1:9/v1/" }
                    }
                }]
            },
            "modelConfigRules": {
                "providerModelRules": [{
                    "providerId": "mock-chat",
                    "modelId": "mock-1",
                    "config": {
                        "enabled": true,
                        "properties": {
                            "contextWindow": 200000,
                            "supportsToolCall": true,
                            "inputFormat": { "supportsText": true },
                            "outputFormat": { "supportsText": true }
                        },
                        "optionSpecs": {
                            "reasoningLevel": { "values": ["disabled", "enabled"] },
                            "maxOutputTokens": { "max": 8192 }
                        }
                    }
                }],
                "manualProviderModelRules": []
            }
        }
    });
    fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    path
}

fn write_builtin_file(root: &Path) -> String {
    let path = root.join("mode-builtin.json");
    fs::write(
        &path,
        r#"{
          "schemaVersion": 1,
          "revision": 7,
          "config": {
            "providerConfigRules": { "templateRules": [], "providerRules": [] },
            "modelConfigRules": {
              "modelRules": [], "modelApiRules": [], "providerSiteRules": [],
              "templateModelRules": [], "builtinProviderModelRules": []
            }
          }
        }"#,
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

struct GoalFixture {
    server: Arc<AgentServer>,
    db_path: PathBuf,
    ws: PathBuf,
    root: TempRoot,
}

fn goal_fixture(label: &str, provider: Arc<BackgroundProvider>) -> GoalFixture {
    let root = TempRoot::new(label);
    let mode_root = root.path().join(".mode");
    let db_dir = mode_root.join("cli").join("db");
    fs::create_dir_all(&db_dir).expect("建 cli/db 目录");
    let db_path = db_dir.join("db.sqlite");
    create_baseline_database(&db_path);
    let ws = root.path().join("ws");
    fs::create_dir_all(&ws).expect("建工作目录");
    let hub = ProviderConfigHub::with_paths(
        write_builtin_file(root.path()),
        write_personal_file(root.path()),
    );
    let config = ServerConfig::from_data_base_dir(root.path());
    let source: Arc<dyn ProviderSource> = Arc::new(FixedSource(provider));
    let server = Arc::new(AgentServer::with_parts(config, hub, source));
    GoalFixture {
        server,
        db_path,
        ws,
        root,
    }
}

struct TestConn {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: u64,
    pending_events: VecDeque<Value>,
    /// 已自动应答的 `session/requestRuntimePreferences` 数（worker 的用户执行偏好）。
    preferences_answered: u64,
}

impl TestConn {
    async fn send(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let line = serde_json::to_string(&json!({ "id": id, "method": method, "params": params }))
            .expect("请求帧可序列化");
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("写请求");
        self.writer.write_all(b"\n").await.expect("写换行");
        self.writer.flush().await.expect("flush");
        id
    }

    async fn next_frame(&mut self) -> Frame {
        let mut line = String::new();
        within(self.reader.read_line(&mut line))
            .await
            .expect("读取失败：连接意外关闭或超时");
        let frame: Frame = serde_json::from_str(line.trim_end()).expect("出站帧是合法 JSON");
        if let Frame::Notification(notification) = &frame {
            if notification.method == "session/event" {
                if let Some(params) = &notification.params {
                    self.pending_events.push_back(params.clone());
                }
            }
        }
        if let Frame::Request(request) = &frame {
            let result = if request.method == "session/requestRuntimePreferences" {
                self.preferences_answered += 1;
                Some(json!({
                    "askUserQuestionAutoResolutionEnabled": true,
                    "nativeSearchEnhancementsEnabled": false,
                    "memoryEnabled": false,
                    "modelContextBudgetStrategy": "preflight-v1",
                }))
            } else {
                None
            };
            if let Some(result) = result {
                let reply = agent_protocol::Frame::Response(agent_protocol::ResponseFrame {
                    id: request.id.clone(),
                    result,
                });
                let line = serde_json::to_string(&reply).expect("应答帧可序列化");
                self.writer
                    .write_all(line.as_bytes())
                    .await
                    .expect("写应答");
                self.writer.write_all(b"\n").await.expect("写换行");
                self.writer.flush().await.expect("flush");
            }
        }
        frame
    }

    async fn result(&mut self, id: u64) -> Value {
        let expected = RequestId::Number(id as i64);
        loop {
            match self.next_frame().await {
                Frame::Response(response) if response.id == expected => return response.result,
                Frame::ErrorResponse(response) if response.id == expected => panic!(
                    "请求 {id} 失败: {} ({})",
                    response.error.message, response.error.code
                ),
                _ => {}
            }
        }
    }

    async fn next_event(&mut self) -> Value {
        if let Some(event) = self.pending_events.pop_front() {
            return event;
        }
        loop {
            if let Frame::Notification(notification) = self.next_frame().await {
                assert_eq!(notification.method, "session/event");
                if let Some(event) = self.pending_events.pop_front() {
                    return event;
                }
                return notification.params.expect("事件通知必须带 params");
            }
        }
    }

    async fn drain_until(&mut self, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut events = Vec::new();
        loop {
            let event = within(self.next_event()).await;
            let hit = predicate(&event);
            events.push(event);
            if hit {
                return events;
            }
        }
    }

    async fn subscribe_default(&mut self, session_id: &str) {
        let id = self
            .send(
                "session/subscribe",
                json!({
                    "sessionId": session_id,
                    "deliveryKind": "desktop-continuous",
                    "includeSnapshot": false
                }),
            )
            .await;
        let result = self.result(id).await;
        assert_eq!(result["sessionId"], json!(session_id));
    }

    async fn read_snapshot(&mut self, session_id: &str) -> Value {
        let id = self
            .send("session/read", json!({ "sessionId": session_id }))
            .await;
        self.result(id).await
    }
}

async fn connect(server: Arc<AgentServer>) -> TestConn {
    let (client, server_stream) = tokio::io::duplex(1 << 16);
    let (server_read, server_write) = tokio::io::split(server_stream);
    tokio::spawn(async move {
        run_stdio(server_read, server_write, server)
            .await
            .expect("stdio 循环不应失败");
    });
    let (client_read, client_write) = tokio::io::split(client);
    TestConn {
        reader: BufReader::new(client_read),
        writer: client_write,
        next_id: 0,
        pending_events: VecDeque::new(),
        preferences_answered: 0,
    }
}

async fn create_session(conn: &mut TestConn, ws: &Path) -> String {
    let id = conn.send("session/create", create_params(ws)).await;
    let snapshot = conn.result(id).await;
    let session_id = snapshot["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    conn.subscribe_default(&session_id).await;
    session_id
}

async fn goal_action(conn: &mut TestConn, session_id: &str, action: &str, extra: Value) -> Value {
    let mut params = json!({ "sessionId": session_id, "action": action });
    if let Some(object) = extra.as_object() {
        for (key, value) in object {
            params[key] = value.clone();
        }
    }
    let id = conn.send("session/goal", params).await;
    conn.result(id).await
}

fn target_row(path: &Path, session_id: &str) -> Option<Value> {
    let connection = raw_connection(path);
    connection
        .query_row(
            "SELECT target_id, objective, status, tokens_used, summary_title, active_input_id \
             FROM session_target WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok(json!({
                    "targetId": row.get::<_, String>(0)?,
                    "objective": row.get::<_, String>(1)?,
                    "status": row.get::<_, String>(2)?,
                    "tokensUsed": row.get::<_, i64>(3)?,
                    "summaryTitle": row.get::<_, Option<String>>(4)?,
                    "activeInputId": row.get::<_, Option<String>>(5)?,
                }))
            },
        )
        .ok()
}

/// goal 场景 provider：
/// 第 1 个 continuation 请求返回后台 Bash 工具调用；第 2 个请求停闸门，
/// 测试等后台输出完成后放行；通知 turn 返回普通文本（usage 22）。
fn goal_background_provider(
    notification_text: &str,
) -> (
    BackgroundProvider,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Semaphore>,
) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (provider, _calls) = BackgroundProvider::new(vec![
        MainStep::ToolCalls(vec![bash_background(
            "toolu_bg_goal",
            "sleep 0.2; printf 'goal-bg\\n'",
            "goal background marker",
        )]),
        MainStep::GatedText {
            text: "continuation answer".to_string(),
            usage: (10, 12),
            entered: Arc::clone(&entered),
            gate: Arc::clone(&gate),
        },
    ]);
    let provider = provider.with_notification_text(notification_text);
    provider.queue_verifier(r#"{"passed":true,"reason":"goal verified","nextAction":null}"#);
    (provider, entered, gate)
}

#[tokio::test]
async fn goal_notification_turn_runs_before_verifier_and_counts_tokens() {
    let (provider, entered, gate) = goal_background_provider("notification handled");
    let provider = Arc::new(provider);
    let fixture = goal_fixture("goal-interaction", Arc::clone(&provider));
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Goal with a background task", "inputId": "goal_bg_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));

    // worker 在首个 turn 前发 `user-execution` 偏好请求；测试必须继续读帧并应答，
    // 否则 worker 阻塞在反向请求上，provider 永远不会进入第二次请求。
    while conn.preferences_answered < 2 {
        let _ = within(conn.next_frame()).await;
    }
    // 等 continuation 的第二次模型请求进入并停住。
    within(entered.notified()).await;
    let data_root = fixture.root.path().join(".mode");
    let output_path = data_root
        .join("cli")
        .join("exec")
        .join(&session_id)
        .join("toolu_bg_goal-stdout.log");
    wait_for_file(&output_path, "goal-bg").await;
    gate.add_permits(1);

    // continuation turn 完成 → notification turn 完成；之后 verifier 完成、目标 complete。
    let events = conn
        .drain_until(|event| {
            event["type"] == json!("session.updated")
                && event["payload"]["action"] == json!("status_updated")
                && event["payload"]["target"]["status"] == json!("complete")
        })
        .await;

    let starts: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == json!("turn.started"))
        .collect();
    assert_eq!(starts.len(), 3, "control + continuation + notification");
    assert_eq!(
        starts[2]["payload"]["inputSource"],
        json!("background_task")
    );
    assert!(starts[2]["payload"].get("targetId").is_none());
    assert!(starts[2]["payload"]["input"]
        .as_str()
        .expect("input")
        .starts_with("<task-notification>"));
    let notification_turn_number = starts[2]["payload"]["turnNumber"]
        .as_u64()
        .expect("turnNumber");
    assert_eq!(notification_turn_number, 2);

    // 顺序：notification turn 的终态在 verifier started 之前。
    let notification_completed = events
        .iter()
        .position(|event| {
            event["type"] == json!("turn.completed")
                && event["payload"]["response"] == json!("notification handled")
        })
        .expect("notification turn terminal");
    let verifier_started = events
        .iter()
        .position(|event| {
            event["type"] == json!("session.updated")
                && event["payload"]["status"] == json!("started")
                && event["payload"].get("targetId").is_some()
        })
        .expect("verifier started");
    assert!(
        notification_completed < verifier_started,
        "通知 turn 必须在 verifier 之前"
    );

    // 通知 turn 包在目标 run_started/run_finished 中（fixture §3.3）。
    let run_started_count = events
        .iter()
        .filter(|event| {
            event["type"] == json!("session.updated")
                && event["payload"]["action"] == json!("run_started")
        })
        .count();
    let run_finished_count = events
        .iter()
        .filter(|event| {
            event["type"] == json!("session.updated")
                && event["payload"]["action"] == json!("run_finished")
        })
        .count();
    assert_eq!(run_started_count, 2, "续跑 + 通知各一次");
    assert_eq!(run_finished_count, 2);

    // tokens = 30 + 22（continuation 两个请求）+ 22（通知 turn）= 74。
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("complete"));
    assert_eq!(target["tokensUsed"], json!(74));
    let read = conn.read_snapshot(&session_id).await;
    assert_eq!(read["session"]["target"]["tokensUsed"], json!(74));
    assert_eq!(
        message_kind_count(&fixture.db_path, &session_id, "background_notification"),
        1
    );
    // turn_usage：continuation 与 notification 各一行（控制 turn 不写）。
    assert_eq!(turn_usage_rows(&fixture.db_path, &session_id).len(), 2);
}

#[tokio::test]
async fn goal_failure_clears_api_retry_in_snapshot() {
    let provider = BackgroundProvider::new(vec![
        MainStep::Error(ProviderError::http_status(503, "upstream overloaded")),
        MainStep::Error(ProviderError::auth(401, "invalid api key")),
    ])
    .0;
    let provider = Arc::new(provider);
    let fixture = goal_fixture("goal-api-retry", Arc::clone(&provider));
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Goal that fails", "inputId": "goal_fail_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let events = conn
        .drain_until(|event| event["type"] == json!("turn.failed"))
        .await;
    assert_eq!(
        events.last().expect("terminal")["type"],
        json!("turn.failed")
    );

    // 失败后 worker 收口回 idle/error；等状态离开 running 再读快照。
    for _ in 0..600 {
        let status = conn.read_snapshot(&session_id).await["session"]["status"].clone();
        if status != json!("running") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let read = conn.read_snapshot(&session_id).await;
    assert!(
        read["runtime"]["apiRetry"].is_null(),
        "goal 失败后不得残留上一 turn 的 apiRetry: {}",
        read["runtime"]["apiRetry"]
    );
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("active"), "失败不完成目标");
}
