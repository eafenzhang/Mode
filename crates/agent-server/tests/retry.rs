//! C3 T2：流恢复/重试在 `agent-server` 的投影与账务。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §2 与 T0 夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c3/retry-*.json`（只读）。
//!
//! 覆盖：
//! - HTTP 5xx 重试：`session.updated(model_retry_scheduled)` 的 apiRetry 状态、
//!   快照 `runtime.apiRetry`、`turn_usage.model_retry_count`；
//! - 流中断：4 条 `streamRecovery.updated`、被丢弃的 assistant 行
//!   （`finish:"stream_recovery_discarded"`、text 空）与最终消息；事件层
//!   `modelRequestCount` 与 DB `model_request_count` 的差异（契约 §2.2/§2.6）；
//! - 401：无重试、`turn.failed` 恰好一次、错误归属/账务；
//! - 退避期间取消：第二次尝试不发生、cancelled 终态一次、apiRetry 清空；
//! - 重试耗尽：按夹具观察的 `maxRetries:10` 停止（耗尽形状未观察到）。
//!
//! 所有用例使用临时库（`TempRoot`）与进程内 fake provider；不用 sleep 作同步证明
//! （等事件/快照谓词，bounded timeout）。重试退避由 `RetryPolicy` 注入，测试不
//! 等待真实退避时间。

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
    ProviderError,
};
use agent_core::RetryPolicy;
use agent_protocol::session_event::{SessionEvent, SessionEventType};
use agent_server::{RuntimeConfig, SessionInput, SessionRuntime};
use provider_http::{OpenAiChatConfig, OpenAiChatProvider};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use session_store::{SessionRecord, SessionStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------------------
// 临时库（与 runtime.rs 测试同口径）
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
            "mode-agent-server-retry-{label}-{}-{}",
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

fn turn_usage_row(path: &Path, session_id: &str) -> Value {
    raw_connection(path)
        .query_row(
            "SELECT status, model_request_count, model_retry_count, cancelled_by_user, \
                    error_type, error_code, input_tokens, output_tokens, computed_total_tokens \
             FROM turn_usage WHERE session_id = ?1 ORDER BY started_at DESC LIMIT 1",
            [session_id],
            |row| {
                Ok(json!({
                    "status": row.get::<_, String>(0)?,
                    "modelRequestCount": row.get::<_, i64>(1)?,
                    "modelRetryCount": row.get::<_, i64>(2)?,
                    "cancelledByUser": row.get::<_, i64>(3)?,
                    "errorType": row.get::<_, Option<String>>(4)?,
                    "errorCode": row.get::<_, Option<String>>(5)?,
                    "inputTokens": row.get::<_, i64>(6)?,
                    "outputTokens": row.get::<_, i64>(7)?,
                    "computedTotalTokens": row.get::<_, i64>(8)?,
                }))
            },
        )
        .expect("读取 turn_usage")
}

fn count_rows(path: &Path, table: &str) -> i64 {
    raw_connection(path)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("统计行数")
}

/// 会话的全部消息摘要（finish + 文本拼接），按 sequence 排序。
fn message_texts(path: &Path, session_id: &str) -> Vec<(Option<String>, String)> {
    let connection = raw_connection(path);
    let mut messages = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT id, data FROM message WHERE session_id = ?1 \
                 ORDER BY COALESCE(sequence, -1), time_created, id",
            )
            .expect("准备 message 查询");
        let rows = statement
            .query_map([session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("查询 message");
        for row in rows {
            let (id, data) = row.expect("读取 message");
            let data: Value = serde_json::from_str(&data).expect("message JSON");
            let finish = data["finish"].as_str().map(str::to_owned);
            messages.push((id, finish, String::new()));
        }
    }
    for (id, _finish, text) in &mut messages {
        let mut statement = connection
            .prepare("SELECT data FROM part WHERE message_id = ?1 ORDER BY COALESCE(sequence, -1)")
            .expect("准备 part 查询");
        let rows = statement
            .query_map([id.clone()], |row| row.get::<_, String>(0))
            .expect("查询 part");
        for row in rows {
            let data: Value = serde_json::from_str(&row.expect("读取 part")).expect("part JSON");
            if data["type"] == json!("text") {
                if let Some(chunk) = data["text"].as_str() {
                    text.push_str(chunk);
                }
            }
        }
    }
    messages
        .into_iter()
        .map(|(_, finish, text)| (finish, text))
        .collect()
}

// ---------------------------------------------------------------------------
// provider
// ---------------------------------------------------------------------------

enum Attempt {
    Http {
        status: u16,
        message: &'static str,
    },
    AbortAfter {
        deltas: Vec<String>,
        message: &'static str,
    },
    Success {
        text: String,
    },
    Auth {
        status: u16,
    },
}

struct RetryProvider {
    attempts: Mutex<VecDeque<Attempt>>,
    calls: Arc<AtomicU64>,
    fallback: Option<Attempt>,
}

impl RetryProvider {
    fn new(attempts: Vec<Attempt>) -> (Self, Arc<AtomicU64>) {
        let calls = Arc::new(AtomicU64::new(0));
        (
            Self {
                attempts: Mutex::new(attempts.into()),
                calls: Arc::clone(&calls),
                fallback: None,
            },
            calls,
        )
    }

    fn always_http(status: u16) -> (Self, Arc<AtomicU64>) {
        let calls = Arc::new(AtomicU64::new(0));
        (
            Self {
                attempts: Mutex::new(VecDeque::new()),
                calls: Arc::clone(&calls),
                fallback: Some(Attempt::Http {
                    status,
                    message: "Provider returned a server error.",
                }),
            },
            calls,
        )
    }
}

impl ModelProvider for RetryProvider {
    fn stream(
        &self,
        _request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let attempt = self
            .attempts
            .lock()
            .expect("attempts lock")
            .pop_front()
            .or_else(|| self.fallback.as_ref().and_then(Attempt::clone_kind))
            .unwrap_or(Attempt::Http {
                status: 503,
                message: "Provider returned a server error.",
            });
        Box::pin(async move {
            match attempt {
                Attempt::Http { status, message } => {
                    Err(ProviderError::http_status(status, message))
                }
                Attempt::AbortAfter {
                    deltas: emitted,
                    message,
                } => {
                    for text in emitted {
                        let _ = deltas.send(ModelDelta::Text { text });
                    }
                    Err(ProviderError::stream_abort(message))
                }
                Attempt::Success { text } => {
                    let _ = deltas.send(ModelDelta::Text { text });
                    Ok(ModelResponse {
                        usage: ModelUsage {
                            input_tokens: 10,
                            output_tokens: 12,
                            ..ModelUsage::default()
                        },
                        stop_reason: "stop",
                    })
                }
                Attempt::Auth { status } => Err(ProviderError::auth(status, "invalid api key")),
            }
        })
    }
}

/// 流中断后恢复：第一次发 partial 后中断；第二次发 recovered delta 后停在闸门上，
/// 供测试在「恢复进展已到、provider 尚未返回」的窗口读快照。
struct GatedRecoveryProvider {
    calls: Arc<AtomicU64>,
    release: Arc<tokio::sync::Semaphore>,
}

impl GatedRecoveryProvider {
    fn new() -> (Self, Arc<AtomicU64>, Arc<tokio::sync::Semaphore>) {
        let calls = Arc::new(AtomicU64::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        (
            Self {
                calls: Arc::clone(&calls),
                release: Arc::clone(&release),
            },
            calls,
            release,
        )
    }
}

impl ModelProvider for GatedRecoveryProvider {
    fn stream(
        &self,
        _request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            if call == 0 {
                let _ = deltas.send(ModelDelta::Text {
                    text: "partial text ".to_string(),
                });
                return Err(ProviderError::stream_abort("connection reset by peer"));
            }
            let _ = deltas.send(ModelDelta::Text {
                text: "recovered answer".to_string(),
            });
            let _permit = release
                .acquire()
                .await
                .map_err(|_| ProviderError::local("recovery gate closed"))?;
            Ok(ModelResponse {
                usage: ModelUsage {
                    input_tokens: 10,
                    output_tokens: 12,
                    ..ModelUsage::default()
                },
                stop_reason: "stop",
            })
        })
    }
}

/// 本地 OpenAI Chat 端点：第一次 503（非 JSON 形态的正文），第二次成功 SSE。
/// 用来验证真实 `provider-http` 分类经 runtime 后的事件形状。
async fn spawn_chat_provider_server() -> (String, Arc<AtomicU64>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local server");
    let address = listener.local_addr().expect("local addr");
    let calls = Arc::new(AtomicU64::new(0));
    let calls_for_server = Arc::clone(&calls);
    tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().await.expect("accept");
            // 读完整请求（头 + Content-Length body）：留下未读 body 再 close 会让
            // Windows 发 RST，客户端读不到响应（误判 network_error）。
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            let mut header_end: Option<usize> = None;
            loop {
                if let Some(end) = header_end {
                    let header = String::from_utf8_lossy(&request[..end]);
                    let content_length = header
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + content_length {
                        break;
                    }
                }
                let read = socket.read(&mut buffer).await.expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if header_end.is_none() {
                    header_end = request.windows(4).position(|window| window == b"\r\n\r\n");
                }
            }
            calls_for_server.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                let body = br#"{"error":{"message":"upstream boom"}}"#;
                let head = format!(
                    "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(head.as_bytes()).await.expect("write head");
                socket.write_all(body).await.expect("write body");
            } else {
                let body = concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"answer after one retry\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":12}}\n\n",
                    "data: [DONE]\n\n",
                );
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(head.as_bytes()).await.expect("write head");
                socket.write_all(body.as_bytes()).await.expect("write body");
            }
            socket.flush().await.expect("flush");
        }
    });
    (format!("http://{address}/v1"), calls)
}

impl Attempt {
    fn clone_kind(&self) -> Option<Attempt> {
        match self {
            Attempt::Http { status, message } => Some(Attempt::Http {
                status: *status,
                message,
            }),
            Attempt::AbortAfter { deltas, message } => Some(Attempt::AbortAfter {
                deltas: deltas.clone(),
                message,
            }),
            Attempt::Success { text } => Some(Attempt::Success { text: text.clone() }),
            Attempt::Auth { status } => Some(Attempt::Auth { status: *status }),
        }
    }
}

// ---------------------------------------------------------------------------
// 运行时夹具
// ---------------------------------------------------------------------------

struct Fixture {
    store: Arc<SessionStore>,
    record: SessionRecord,
    db_path: PathBuf,
    _cwd: PathBuf,
    _root: TempRoot,
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
            id: "sess_retry".to_owned(),
            project_id: "proj_retry".to_owned(),
            workspace_id: None,
            parent_id: None,
            slug: "sess_retry".to_owned(),
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
            trace_id: Some("trace-retry".to_owned()),
        };
        store
            .create_session(record.clone())
            .expect("创建 session 行");
        Self {
            store,
            record,
            db_path,
            _cwd: cwd,
            _root: root,
        }
    }
}

fn runtime_config(retry: RetryPolicy) -> RuntimeConfig {
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
        data_root: None,
        event_log_capacity: 4096,
        broadcast_capacity: 256,
        retry,
    }
}

struct FixedSource(Arc<dyn ModelProvider>);

impl agent_server::ProviderSource for FixedSource {
    fn resolve(
        &self,
        _provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        Ok(Arc::clone(&self.0))
    }
}

fn spawn_runtime(
    fixture: &Fixture,
    provider: Arc<dyn ModelProvider>,
    retry: RetryPolicy,
) -> SessionRuntime {
    spawn_runtime_with_config(fixture, provider, runtime_config(retry))
}

fn spawn_runtime_with_config(
    fixture: &Fixture,
    provider: Arc<dyn ModelProvider>,
    config: RuntimeConfig,
) -> SessionRuntime {
    SessionRuntime::new(
        fixture.record.clone(),
        Arc::clone(&fixture.store),
        Arc::new(FixedSource(provider)),
        config,
        agent_server::RuntimeWiring::default(),
    )
    .expect("构造运行时")
}

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("用例超时（10s）")
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

async fn drain_until_terminal(
    receiver: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
) -> Vec<SessionEvent> {
    drain_until(receiver, is_terminal).await
}

async fn wait_settled(runtime: &SessionRuntime) {
    for _ in 0..200 {
        let status = runtime.snapshot().projection.status;
        if !matches!(status, agent_protocol::session_read::SessionStatus::Running) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("终态收口未在 1s 内完成");
}

fn session_updated<'a>(events: &'a [SessionEvent], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::SessionUpdated)
        .filter_map(|event| event.payload.as_ref())
        .filter(|payload| payload["type"] == json!(kind))
        .collect()
}

fn text_deltas(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ModelStreaming)
        .filter_map(|event| event.payload.as_ref())
        .filter(|payload| payload["kind"] == json!("text_delta"))
        .filter_map(|payload| payload["delta"].as_str().map(str::to_owned))
        .collect()
}

fn terminal_events(events: &[SessionEvent]) -> Vec<&SessionEvent> {
    events.iter().filter(|event| is_terminal(event)).collect()
}

// ---------------------------------------------------------------------------
// 1. HTTP 5xx 重试 → apiRetry 投影 + DB retry 计数
// ---------------------------------------------------------------------------

#[tokio::test]
async fn five_xx_retry_projects_api_retry_and_db_retry_count() {
    let fixture = Fixture::new("5xx");
    let (provider, calls) = RetryProvider::new(vec![
        Attempt::Http {
            status: 503,
            message: "HTTP 503: upstream boom",
        },
        Attempt::Success {
            text: "answer after one retry".to_string(),
        },
    ]);
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(provider),
        RetryPolicy {
            base_delay_ms: 0,
            ..RetryPolicy::default()
        },
    );
    let (_replay, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("retry scenario"))
        .expect("受理输入");
    let events = drain_until_terminal(&mut receiver).await;

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(terminal_events(&events).len(), 1, "终态恰好一次");
    let scheduled = session_updated(&events, "model_retry_scheduled");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0]["attempt"], json!(1));
    assert_eq!(scheduled[0]["maxAttempts"], json!(11));
    assert_eq!(scheduled[0]["statusCode"], json!(503));
    assert_eq!(
        scheduled[0]["_meta"]["mode"]["apiRetry"]["attempt"],
        json!(1)
    );
    assert_eq!(
        scheduled[0]["_meta"]["mode"]["apiRetry"]["maxRetries"],
        json!(10)
    );
    // 夹具 §2.3：事件面用规范文案，不透传适配器的 `HTTP 503: …` 前缀。
    assert_eq!(
        scheduled[0]["message"],
        json!("Provider returned a server error.")
    );
    assert_eq!(
        scheduled[0]["_meta"]["mode"]["apiRetry"]["error"],
        json!("Provider returned a server error.")
    );
    // 宿主补齐的会话/追踪事实（夹具 §2.3 的键集合）。
    assert_eq!(scheduled[0]["providerId"], json!("fixture-provider"));
    assert_eq!(scheduled[0]["modelId"], json!("fixture-model"));
    assert_eq!(scheduled[0]["sessionId"], json!("sess_retry"));
    assert_eq!(scheduled[0]["querySource"], json!("main_turn"));
    assert_eq!(scheduled[0]["traceId"], json!("trace-retry"));
    assert!(scheduled[0]["turnId"].as_str().is_some());
    assert!(scheduled[0]["queryId"].as_str().is_some());
    assert!(scheduled[0]["requestId"].as_str().is_some());
    assert!(scheduled[0]["spanId"].as_str().is_some());
    assert!(scheduled[0]["parentSpanId"].as_str().is_some());
    assert_eq!(scheduled[0]["modelCall"]["operation"], json!("agent_step"));
    assert_eq!(scheduled[0]["modelCall"]["actorKind"], json!("main"));
    assert!(scheduled[0]["modelCall"]["stepIndex"].is_number());
    let completed = session_updated(&events, "model_request_completed");
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["_meta"]["mode"]["apiRetry"], Value::Null);
    assert_eq!(
        completed[0]["queryId"], scheduled[0]["queryId"],
        "同一 turn 的重试事件共享 queryId"
    );
    assert!(
        completed[0]["modelCall"].get("callCause").is_none(),
        "HTTP 重试不是 recovery（无 callCause）"
    );

    let terminal = terminal_events(&events)[0];
    assert_eq!(terminal.event_type, SessionEventType::TurnCompleted);
    let payload = terminal.payload.as_ref().expect("payload");
    assert_eq!(payload["resultType"], json!("success"));
    assert_eq!(payload["usage"]["modelRequestCount"], json!(1));
    assert_eq!(payload["historyRoundCount"], json!(1));

    wait_settled(&runtime).await;
    let usage = turn_usage_row(&fixture.db_path, "sess_retry");
    assert_eq!(usage["status"], json!("completed"));
    assert_eq!(usage["modelRequestCount"], json!(1));
    assert_eq!(
        usage["modelRetryCount"],
        json!(1),
        "DB 层必须记录 adapter 重试次数"
    );

    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime.api_retry,
        Some(None),
        "成功收口后 apiRetry 必须清空（键仍在、值为 null）"
    );
}

// ---------------------------------------------------------------------------
// 1b. 真实 provider-http 503：分类 → runtime 事件规范化/补齐
// ---------------------------------------------------------------------------

#[tokio::test]
async fn real_provider_http_5xx_is_canonicalized_and_enriched() {
    let fixture = Fixture::new("real-5xx");
    let (base_url, server_calls) = spawn_chat_provider_server().await;
    let provider = OpenAiChatProvider::new(OpenAiChatConfig {
        base_url,
        api_key: "test-key".to_string(),
        model: "mock-1".to_string(),
        max_output_tokens: None,
        extra_headers: Vec::new(),
    })
    .expect("构造 provider");
    let mut config = runtime_config(RetryPolicy {
        base_delay_ms: 0,
        ..RetryPolicy::default()
    });
    config.reasoning_level = Some("disabled".to_string());
    let runtime = spawn_runtime_with_config(&fixture, Arc::new(provider), config);
    let (_replay, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("retry scenario"))
        .expect("受理输入");
    let events = drain_until_terminal(&mut receiver).await;

    assert_eq!(server_calls.load(Ordering::SeqCst), 2, "503 后重试一次");
    let terminals = terminal_events(&events);
    assert_eq!(terminals.len(), 1);
    assert_eq!(
        terminals[0].event_type,
        SessionEventType::TurnCompleted,
        "terminal={:?}",
        terminals[0].payload
    );
    let scheduled = session_updated(&events, "model_retry_scheduled");
    assert_eq!(scheduled.len(), 1);
    // provider-http 的原始文案是 `HTTP 503: upstream boom`；事件面必须用规范文案。
    assert_eq!(
        scheduled[0]["message"],
        json!("Provider returned a server error.")
    );
    assert_eq!(scheduled[0]["statusCode"], json!(503));
    assert_eq!(
        scheduled[0]["_meta"]["mode"]["apiRetry"]["error"],
        json!("Provider returned a server error.")
    );
    assert!(!scheduled[0]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("upstream boom"));
    // 宿主补齐的字段（夹具 §2.3）。
    assert_eq!(scheduled[0]["providerId"], json!("fixture-provider"));
    assert_eq!(scheduled[0]["modelId"], json!("fixture-model"));
    assert_eq!(scheduled[0]["querySource"], json!("main_turn"));
    assert_eq!(scheduled[0]["traceId"], json!("trace-retry"));
    assert_eq!(
        scheduled[0]["modelCall"]["reasoning"]["requestedLevel"],
        json!("disabled")
    );
    assert_eq!(scheduled[0]["modelRequestSessionType"], json!("main"));
    assert!(scheduled[0]["queryId"].as_str().is_some());
    assert!(scheduled[0]["turnId"].as_str().is_some());

    wait_settled(&runtime).await;
    let usage = turn_usage_row(&fixture.db_path, "sess_retry");
    assert_eq!(usage["status"], json!("completed"));
    assert_eq!(usage["modelRetryCount"], json!(1));
}

// ---------------------------------------------------------------------------
// 2. 流中断：4 条恢复事件 + discarded 消息 + 事件/DB 计数差异
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_abort_recovers_and_splits_event_vs_db_accounting() {
    let fixture = Fixture::new("abort");
    let (provider, calls) = RetryProvider::new(vec![
        Attempt::AbortAfter {
            deltas: vec!["partial text ".to_string()],
            message: "connection reset by peer",
        },
        Attempt::Success {
            text: "complete answer after recovery".to_string(),
        },
    ]);
    let runtime = spawn_runtime(&fixture, Arc::new(provider), RetryPolicy::default());
    let (_replay, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("retry scenario"))
        .expect("受理输入");
    let events = drain_until_terminal(&mut receiver).await;

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(terminal_events(&events).len(), 1);

    let recovery: Vec<&Value> = events
        .iter()
        .filter(|event| event.event_type == SessionEventType::StreamRecoveryUpdated)
        .filter_map(|event| event.payload.as_ref())
        .collect();
    assert_eq!(recovery.len(), 4);
    assert_eq!(recovery[0]["failureKind"], json!("provider_network_error"));
    assert_eq!(recovery[2]["discardedTextBytes"], json!(13));
    assert_eq!(recovery[3]["streamMode"], json!("sse"));
    assert_eq!(
        text_deltas(&events),
        vec!["partial text ", "complete answer after recovery"]
    );

    let terminal = terminal_events(&events)[0];
    let payload = terminal.payload.as_ref().expect("payload");
    assert_eq!(payload["resultType"], json!("success"));
    assert_eq!(payload["response"], json!("complete answer after recovery"));
    // 事件层只计成功完成的模型请求（1），DB 层计尝试数（2）——契约 §2.2/§2.6。
    assert_eq!(payload["usage"]["modelRequestCount"], json!(1));
    assert_eq!(payload["historyRoundCount"], json!(2));

    // 恢复收口事件带回 streamRecovery 摘要与 recovery 的 modelCall 形状。
    let completed = session_updated(&events, "model_request_completed");
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["attempt"], json!(1), "恢复请求 attempt 仍为 1");
    assert_eq!(
        completed[0]["streamRecovery"]["attemptId"],
        recovery[0]["attemptId"]
    );
    assert_eq!(
        completed[0]["streamRecovery"]["recoveredFromRequestId"],
        recovery[0]["failedRequestId"]
    );
    assert_eq!(completed[0]["streamRecovery"]["retryNumber"], json!(1));
    assert_eq!(completed[0]["modelCall"]["callCause"], json!("recovery"));
    assert_eq!(
        completed[0]["modelCall"]["attributes"]["streamRecoveryNumber"],
        json!(1)
    );

    wait_settled(&runtime).await;
    let usage = turn_usage_row(&fixture.db_path, "sess_retry");
    assert_eq!(usage["status"], json!("completed"));
    assert_eq!(usage["modelRequestCount"], json!(2));
    assert_eq!(
        usage["modelRetryCount"],
        json!(0),
        "stream recovery 不计 adapter 重试次数"
    );

    let messages = message_texts(&fixture.db_path, "sess_retry");
    assert!(
        messages.iter().any(|(finish, text)| finish.as_deref()
            == Some("stream_recovery_discarded")
            && text.is_empty()),
        "被丢弃的 assistant 行必须保留（finish=stream_recovery_discarded、text 空）：{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|(finish, text)| finish.as_deref() == Some("stop")
                && text == "complete answer after recovery"),
        "最终消息只有恢复后的文本：{messages:?}"
    );
    assert!(
        !messages
            .iter()
            .any(|(_, text)| text.contains("partial text")),
        "被丢弃的 partial delta 不能进入最终消息：{messages:?}"
    );
}

// ---------------------------------------------------------------------------
// 2b. 恢复进展清空 apiRetry（首个恢复 delta 到达时）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recovery_progress_clears_api_retry_before_completion() {
    let fixture = Fixture::new("progress-clear");
    let (provider, calls, release) = GatedRecoveryProvider::new();
    let runtime = spawn_runtime(&fixture, Arc::new(provider), RetryPolicy::default());
    let (_replay, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("retry scenario"))
        .expect("受理输入");

    // 收集到第一个恢复文本 delta（provider 仍在闸门上，未返回 Ok）。
    let mut events = Vec::new();
    loop {
        let event = within(receiver.recv()).await.expect("总线不应关闭");
        let recovered_text = event.event_type == SessionEventType::ModelStreaming
            && event
                .payload
                .as_ref()
                .is_some_and(|payload| payload["delta"] == json!("recovered answer"));
        events.push(event);
        if recovered_text {
            break;
        }
    }
    // 恢复事件先建立 apiRetry，首个进展 delta 按 crate 规则清空。
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == SessionEventType::StreamRecoveryUpdated)
            .count(),
        4
    );
    let snapshot = runtime.snapshot();
    assert!(
        snapshot.runtime.api_retry.is_none() || snapshot.runtime.api_retry == Some(None),
        "恢复进展后 apiRetry 必须清空：{:?}",
        snapshot.runtime.api_retry
    );

    release.add_permits(1);
    let mut tail = drain_until_terminal(&mut receiver).await;
    events.append(&mut tail);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let terminal = terminal_events(&events);
    assert_eq!(terminal.len(), 1);
    assert_eq!(
        terminal[0].payload.as_ref().expect("payload")["response"],
        json!("recovered answer")
    );
}

// ---------------------------------------------------------------------------
// 3. 401：非 retryable、turn.failed 一次
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_401_fails_once_without_retry() {
    let fixture = Fixture::new("401");
    let (provider, calls) = RetryProvider::new(vec![Attempt::Auth { status: 401 }]);
    let runtime = spawn_runtime(&fixture, Arc::new(provider), RetryPolicy::default());
    let (_replay, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("retry scenario"))
        .expect("受理输入");
    let events = drain_until_terminal(&mut receiver).await;

    assert_eq!(calls.load(Ordering::SeqCst), 1, "401 不能重试");
    let terminals = terminal_events(&events);
    assert_eq!(terminals.len(), 1, "turn.failed 恰好一次");
    assert_eq!(terminals[0].event_type, SessionEventType::TurnFailed);
    let payload = terminals[0].payload.as_ref().expect("payload");
    // 夹具 §2.4：retryable 只在 attribution 里（顶层没有该键）。
    assert!(
        payload["error"].get("retryable").is_none(),
        "顶层 error.retryable 不是夹具形状：{payload}"
    );
    assert_eq!(payload["error"]["type"], json!("unknown_error"));
    assert_eq!(payload["error"]["code"], json!("provider_not_configured"));
    assert_eq!(
        payload["error"]["message"],
        json!("Provider authentication failed.")
    );
    assert_eq!(
        payload["error"]["underlyingErrorMessage"],
        json!("invalid api key")
    );
    assert_eq!(payload["turnPhase"], json!("processing_input"));
    assert_eq!(payload["error"]["attribution"]["retryable"], json!(false));
    assert_eq!(payload["error"]["attribution"]["statusCode"], json!(401));
    assert_eq!(payload["error"]["attribution"]["source"], json!("provider"));
    assert_eq!(
        payload["error"]["attribution"]["errorPhase"],
        json!("stream")
    );
    let detail = payload["error"]["detail"].as_str().expect("detail");
    assert!(detail.contains("reason=auth_failed"), "{detail}");
    assert!(detail.contains("status=401"), "{detail}");
    assert!(detail.contains("retryable=false"), "{detail}");
    assert!(detail.contains("invalid api key"), "{detail}");
    assert_eq!(
        payload["error"]["attribution"]["reason"],
        json!("auth_failed")
    );
    assert!(session_updated(&events, "model_retry_scheduled").is_empty());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == SessionEventType::StreamRecoveryUpdated)
            .count(),
        0
    );

    wait_settled(&runtime).await;
    let usage = turn_usage_row(&fixture.db_path, "sess_retry");
    assert_eq!(usage["status"], json!("failed"));
    assert_eq!(usage["modelRequestCount"], json!(1));
    assert_eq!(usage["modelRetryCount"], json!(0));
    assert_eq!(usage["errorType"], json!("unknown_error"));
    assert_eq!(usage["errorCode"], json!("UNKNOWN_ERROR"));
    assert_eq!(count_rows(&fixture.db_path, "model_usage"), 0);

    // 夹具 §2.4：失败会话留下一个 finish:null 的 assistant 占位（无文本 part）。
    let messages = message_texts(&fixture.db_path, "sess_retry");
    assert!(
        messages
            .iter()
            .any(|(finish, text)| finish.is_none() && text.is_empty()),
        "401 必须留下 finish:null 的空 assistant 占位：{messages:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. 退避期间取消
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_during_backoff_stops_retry_and_terminal_is_once() {
    let fixture = Fixture::new("cancel");
    let (provider, calls) = RetryProvider::new(vec![
        Attempt::Http {
            status: 503,
            message: "Provider returned a server error.",
        },
        Attempt::Success {
            text: "should not be reached".to_string(),
        },
    ]);
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(provider),
        RetryPolicy {
            // 长退避：给测试留出「已调度、未重试」的可观察窗口。
            base_delay_ms: 60_000,
            ..RetryPolicy::default()
        },
    );
    let (_replay, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("cancel during retry"))
        .expect("受理输入");
    // 等到 model_retry_scheduled 进入总线（apiRetry 状态已投影）。
    drain_until(&mut receiver, |event| {
        event.event_type == SessionEventType::SessionUpdated
            && event
                .payload
                .as_ref()
                .is_some_and(|payload| payload["type"] == json!("model_retry_scheduled"))
    })
    .await;

    let during_backoff = runtime.snapshot();
    let api_retry = during_backoff
        .runtime
        .api_retry
        .clone()
        .flatten()
        .expect("退避期间快照必须带 apiRetry");
    assert_eq!(api_retry.attempt, 1);
    assert_eq!(api_retry.max_retries, 10);
    assert_eq!(api_retry.error_status, Some(503));
    assert!(api_retry.retry_delay_ms > 0);

    runtime.cancel_turn().expect("取消当前 turn");
    let events = drain_until_terminal(&mut receiver).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "取消后不能再发起重试");
    let terminals = terminal_events(&events);
    assert_eq!(terminals.len(), 1);
    let payload = terminals[0].payload.as_ref().expect("payload");
    assert_eq!(payload["resultType"], json!("cancelled"));
    // 夹具 §2.5：取消终态没有 usage、historyRoundCount:0、tokenCount:0；
    // DB 的 model_request_count 仍记实际尝试数（1）。
    assert!(
        payload.get("usage").is_none(),
        "取消终态不应带 usage：{payload}"
    );
    assert_eq!(payload["historyRoundCount"], json!(0));
    assert_eq!(payload["tokenCount"], json!(0));
    let cancelled = session_updated(&events, "model_request_failed");
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0]["reason"], json!("cancelled"));
    assert_eq!(cancelled[0]["retryable"], json!(false));

    wait_settled(&runtime).await;
    let usage = turn_usage_row(&fixture.db_path, "sess_retry");
    assert_eq!(usage["status"], json!("cancelled"));
    assert_eq!(usage["cancelledByUser"], json!(1));
    assert_eq!(usage["modelRequestCount"], json!(1));
    assert_eq!(usage["modelRetryCount"], json!(1));
    assert_eq!(usage["errorType"], json!("turn_cancelled"));
    assert_eq!(usage["errorCode"], json!("TURN_CANCELLED"));

    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime.api_retry,
        Some(None),
        "取消收口后 apiRetry 必须清空"
    );
}

// ---------------------------------------------------------------------------
// 5. 重试耗尽（形状未观察到）：按 maxRetries=10 停止
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_exhaustion_fails_once_at_observed_attempt_count() {
    let fixture = Fixture::new("exhaustion");
    let (provider, calls) = RetryProvider::always_http(503);
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(provider),
        RetryPolicy {
            base_delay_ms: 0,
            ..RetryPolicy::default()
        },
    );
    let (_replay, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("retry scenario"))
        .expect("受理输入");
    let events = drain_until_terminal(&mut receiver).await;

    assert_eq!(calls.load(Ordering::SeqCst), 11, "1 次初始 + 10 次重试");
    assert_eq!(session_updated(&events, "model_retry_scheduled").len(), 10);
    let terminals = terminal_events(&events);
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].event_type, SessionEventType::TurnFailed);

    wait_settled(&runtime).await;
    let usage = turn_usage_row(&fixture.db_path, "sess_retry");
    assert_eq!(usage["status"], json!("failed"));
    assert_eq!(usage["modelRequestCount"], json!(1));
    assert_eq!(usage["modelRetryCount"], json!(10));
}
