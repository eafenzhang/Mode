//! C3 T1：`session/goal` 全 action、续跑/停止条件、verifier、revision 冲突与冷恢复。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §1 与机器录制的 24 个 C3 夹具
//! （`packages/desktop/tests/agent-runtime-oracle/fixtures/c3/`，只读）。
//!
//! 覆盖（与 task-1-brief 的验收项一一对应）：
//! - show/set/replace/pause/resume/clear 的应答、事件、`session_target` 行与快照投影；
//!   无 goal 的 titleUpdated / cleared 副作用；
//! - prompt/continuation 运行中 show/set/replace/resume/clear → `-32010`，`pause` 例外；
//! - pause 在 provider 请求前（model_request_count 0）与 in-flight（1、cancelled 用量）；
//! - set-over-existing / replace 的目标 id 变化、revision 冲突零副作用；
//! - 续跑循环（无轮次上限）、user stop、clear、completion verifier 的合法/非法 JSON；
//! - 冷恢复保留 tokens/time、active→paused、无 idle turn。
//!
//! provider 是进程内 fake（记录请求、verifier 响应可排队、可选门控），绝不发起真实
//! 网络请求；测试库 DDL 来自 `scripts/sqlite-schema-baseline.json`。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse,
    ModelUsage, ProviderError,
};
use agent_server::{
    run_stdio, AgentServer, Frame, ProviderConfigHub, ProviderSource, RequestId, ServerConfig,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
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
            "agent-server-goal-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn baseline() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("sqlite-schema-baseline.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 schema 基线失败：{error}"));
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

fn count_rows(path: &Path, table: &str) -> i64 {
    raw_connection(path)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("统计行数")
}

fn target_row(path: &Path, session_id: &str) -> Option<Value> {
    let connection = raw_connection(path);
    connection
        .query_row(
            "SELECT session_id, target_id, objective, status, token_budget, tokens_used, \
                    time_used_seconds, summary_title, active_input_id, active_run_started_at, \
                    active_run_last_seen_at, time_created, time_updated \
             FROM session_target WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok(json!({
                    "sessionId": row.get::<_, String>(0)?,
                    "targetId": row.get::<_, String>(1)?,
                    "objective": row.get::<_, String>(2)?,
                    "status": row.get::<_, String>(3)?,
                    "tokenBudget": row.get::<_, Option<i64>>(4)?,
                    "tokensUsed": row.get::<_, i64>(5)?,
                    "timeUsedSeconds": row.get::<_, i64>(6)?,
                    "summaryTitle": row.get::<_, Option<String>>(7)?,
                    "activeInputId": row.get::<_, Option<String>>(8)?,
                    "activeRunStartedAt": row.get::<_, Option<i64>>(9)?,
                    "activeRunLastSeenAt": row.get::<_, Option<i64>>(10)?,
                    "timeCreated": row.get::<_, i64>(11)?,
                    "timeUpdated": row.get::<_, i64>(12)?,
                }))
            },
        )
        .ok()
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

fn model_usage_rows(path: &Path, session_id: &str) -> Vec<Value> {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare(
            "SELECT query_source, status, input_tokens, output_tokens, computed_total_tokens, \
                    error_type, finish_reason \
             FROM model_usage WHERE session_id = ?1 ORDER BY started_at",
        )
        .expect("准备 model_usage 查询");
    statement
        .query_map([session_id], |row| {
            Ok(json!({
                "querySource": row.get::<_, String>(0)?,
                "status": row.get::<_, String>(1)?,
                "inputTokens": row.get::<_, i64>(2)?,
                "outputTokens": row.get::<_, i64>(3)?,
                "computedTotalTokens": row.get::<_, i64>(4)?,
                "errorType": row.get::<_, Option<String>>(5)?,
                "finishReason": row.get::<_, Option<String>>(6)?,
            }))
        })
        .expect("查询 model_usage")
        .map(|row| row.expect("读取 model_usage"))
        .collect()
}

fn session_title(path: &Path, session_id: &str) -> String {
    raw_connection(path)
        .query_row(
            "SELECT title FROM session WHERE id = ?1",
            [session_id],
            |row| row.get::<_, String>(0),
        )
        .expect("读取 session.title")
}

fn assert_unique_monotonic(events: &[Value]) {
    let seqs: Vec<u64> = events
        .iter()
        .map(|event| event["seq"].as_u64().expect("seq"))
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "seq 必须严格递增：{seqs:?}"
    );
    let ids: std::collections::BTreeSet<&str> = events
        .iter()
        .map(|event| event["eventId"].as_str().expect("eventId"))
        .collect();
    assert_eq!(ids.len(), events.len(), "eventId 必须唯一");
}

fn messages_with_source(path: &Path, source: &str) -> Vec<Value> {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare("SELECT data FROM message ORDER BY COALESCE(sequence, -1), time_created, id")
        .expect("准备 message 查询");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("查询 message")
        .map(|row| {
            serde_json::from_str::<Value>(&row.expect("读取 message")).expect("message JSON")
        })
        .filter(|data| data["semantics"]["source"] == json!(source))
        .collect()
}

fn set_target_active_for_cold_kill(db_path: &Path, session_id: &str) {
    // 直接改库模拟「进程在 continuation 中被杀」：状态回 active、active run 字段非空。
    let connection = Connection::open(db_path).expect("打开库模拟 kill");
    connection
        .execute(
            "UPDATE session_target SET status = 'active', active_input_id = 'goal_killed', \
             active_run_started_at = 1000, active_run_last_seen_at = 1000 \
             WHERE session_id = ?1",
            [session_id],
        )
        .expect("模拟 active 目标");
}

// ---------------------------------------------------------------------------
// goal provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Continuation,
    Verifier,
}

#[derive(Debug, Clone)]
struct RequestRecord {
    kind: RequestKind,
    tool_count: usize,
    last_user: String,
}

struct GoalProvider {
    requests: Arc<Mutex<Vec<RequestRecord>>>,
    verifier_responses: Arc<Mutex<VecDeque<String>>>,
    continuation_text: String,
    /// provider 请求**发出前**的通知/闸门（pause 的 pre-request 窗口）。
    pre_entered: Arc<tokio::sync::Notify>,
    pre_gate: Option<Arc<tokio::sync::Semaphore>>,
    /// 请求已发出后的通知/闸门（in-flight 窗口）。
    entered: Arc<tokio::sync::Notify>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    /// 下一次 continuation 请求返回 provider 错误（未观察失败路径的落库一致性）。
    fail_next_continuation: Arc<std::sync::atomic::AtomicBool>,
    verifier_entered: Arc<tokio::sync::Notify>,
    verifier_gate: Option<Arc<tokio::sync::Semaphore>>,
    /// 连续流式 delta 的数量（0 = 关闭；用于 pause 与流式事件竞争 seq）。
    streaming_deltas: usize,
}

impl GoalProvider {
    fn new(continuation_text: &str) -> (Self, Arc<Mutex<Vec<RequestRecord>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                requests: Arc::clone(&requests),
                verifier_responses: Arc::new(Mutex::new(VecDeque::new())),
                continuation_text: continuation_text.to_string(),
                pre_entered: Arc::new(tokio::sync::Notify::new()),
                pre_gate: None,
                entered: Arc::new(tokio::sync::Notify::new()),
                gate: None,
                fail_next_continuation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                verifier_entered: Arc::new(tokio::sync::Notify::new()),
                verifier_gate: None,
                streaming_deltas: 0,
            },
            requests,
        )
    }

    fn with_gate(self, gate: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            gate: Some(gate),
            ..self
        }
    }

    fn with_pre_gate(self, gate: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            pre_gate: Some(gate),
            ..self
        }
    }

    fn with_verifier_gate(self, gate: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            verifier_gate: Some(gate),
            ..self
        }
    }

    fn verifier_entered(&self) -> &tokio::sync::Notify {
        &self.verifier_entered
    }

    fn with_streaming_deltas(mut self, count: usize) -> Self {
        self.streaming_deltas = count;
        self
    }

    fn queue_verifier(&self, response: &str) {
        self.verifier_responses
            .lock()
            .expect("verifier queue")
            .push_back(response.to_string());
    }

    fn fail_next_continuation(&self) {
        self.fail_next_continuation
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn entered(&self) -> &tokio::sync::Notify {
        &self.entered
    }

    fn pre_entered(&self) -> &tokio::sync::Notify {
        &self.pre_entered
    }

    fn requests_snapshot(&self) -> Vec<RequestRecord> {
        self.requests.lock().expect("requests").clone()
    }
}

fn is_verifier_request(request: &ModelRequest) -> bool {
    matches!(
        request.messages.last(),
        Some(ModelMessage::User(text))
            if text.contains(agent_core::goal::VERIFIER_MARKER)
    )
}

impl ModelProvider for GoalProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        let kind = if is_verifier_request(&request) {
            RequestKind::Verifier
        } else {
            RequestKind::Continuation
        };
        let last_user = match request.messages.last() {
            Some(ModelMessage::User(text)) => text.clone(),
            Some(ModelMessage::System(text)) => text.clone(),
            _ => String::new(),
        };
        let record = RequestRecord {
            kind,
            tool_count: request.tools.len(),
            last_user,
        };
        let requests = Arc::clone(&self.requests);
        let pre_entered = Arc::clone(&self.pre_entered);
        let pre_gate = self.pre_gate.clone();
        let entered = Arc::clone(&self.entered);
        let gate = self.gate.clone();
        let fail = Arc::clone(&self.fail_next_continuation);
        let streaming_deltas = self.streaming_deltas;
        if kind == RequestKind::Continuation {
            let text = self.continuation_text.clone();
            return Box::pin(async move {
                // 请求真正发出前的窗口：pause 命中时 provider 不应记到这次请求。
                if let Some(pre_gate) = pre_gate {
                    pre_entered.notify_one();
                    let _permit = pre_gate.acquire().await.map_err(|_| ProviderError {
                        message: "goal pre-gate closed".to_string(),
                    })?;
                }
                requests.lock().expect("requests").push(record);
                if fail.load(std::sync::atomic::Ordering::SeqCst) {
                    fail.store(false, std::sync::atomic::Ordering::SeqCst);
                    return Err(ProviderError {
                        message: "mock provider failure".to_string(),
                    });
                }
                if streaming_deltas > 0 {
                    for index in 0..streaming_deltas {
                        let _ = deltas.send(ModelDelta::Text {
                            text: format!("tick{index} "),
                        });
                        if index == 0 {
                            entered.notify_one();
                        }
                        tokio::task::yield_now().await;
                    }
                    return Ok(ModelResponse {
                        usage: ModelUsage {
                            input_tokens: 10,
                            output_tokens: 12,
                            ..ModelUsage::default()
                        },
                        stop_reason: "stop",
                    });
                }
                if let Some(gate) = gate {
                    // in-flight 窗口：请求已发出并产出了首个 delta，然后停住等 pause。
                    let _ = deltas.send(ModelDelta::Text {
                        text: "partial ".to_string(),
                    });
                    entered.notify_one();
                    let _permit = gate.acquire().await.map_err(|_| ProviderError {
                        message: "goal gate closed".to_string(),
                    })?;
                }
                let _ = deltas.send(ModelDelta::Text { text });
                Ok(ModelResponse {
                    usage: ModelUsage {
                        input_tokens: 10,
                        output_tokens: 12,
                        ..ModelUsage::default()
                    },
                    stop_reason: "stop",
                })
            });
        }
        let response = self
            .verifier_responses
            .lock()
            .expect("verifier queue")
            .pop_front()
            .unwrap_or_else(|| {
                r#"{"passed":true,"reason":"goal verified","nextAction":null}"#.to_string()
            });
        let verifier_entered = Arc::clone(&self.verifier_entered);
        let verifier_gate = self.verifier_gate.clone();
        Box::pin(async move {
            requests.lock().expect("requests").push(record);
            if let Some(gate) = verifier_gate {
                verifier_entered.notify_one();
                let _permit = gate.acquire().await.map_err(|_| ProviderError {
                    message: "verifier gate closed".to_string(),
                })?;
            }
            let _ = deltas.send(ModelDelta::Text {
                text: response.clone(),
            });
            Ok(ModelResponse {
                usage: ModelUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..ModelUsage::default()
                },
                stop_reason: "stop",
            })
        })
    }
}

struct StaticSource {
    provider: Arc<dyn ModelProvider>,
}

impl ProviderSource for StaticSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        if provider_id != "mock-chat" {
            return Err(format!("Provider 不存在: {provider_id}"));
        }
        Ok(Arc::clone(&self.provider))
    }
}

// ---------------------------------------------------------------------------
// stdio 测试连接 / 夹具
// ---------------------------------------------------------------------------

struct TestConn {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: u64,
    /// 已被 `result`/`error`/其他帧读取顺带消费的 session/event，供后续断言使用。
    pending_events: VecDeque<Value>,
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
    }
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
        self.auto_answer_reverse(&frame).await;
        frame
    }

    async fn auto_answer_reverse(&mut self, frame: &Frame) {
        if let Frame::Request(request) = frame {
            if request.method == "session/requestRuntimePreferences" {
                let reply = agent_protocol::Frame::Response(agent_protocol::ResponseFrame {
                    id: request.id.clone(),
                    result: json!({
                        "askUserQuestionAutoResolutionEnabled": true,
                        "nativeSearchEnhancementsEnabled": false,
                        "memoryEnabled": false,
                        "modelContextBudgetStrategy": "preflight-v1",
                    }),
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

    /// 等指定应答，同时把期间收到的 `session/event` 收进 `events`（不丢弃通知）。
    async fn result_collecting(&mut self, id: u64, events: &mut Vec<Value>) -> Value {
        let expected = RequestId::Number(id as i64);
        loop {
            let frame = self.next_frame().await;
            events.extend(self.pending_events.drain(..));
            match frame {
                Frame::Response(response) if response.id == expected => return response.result,
                Frame::ErrorResponse(response) if response.id == expected => panic!(
                    "请求 {id} 失败: {} ({})",
                    response.error.message, response.error.code
                ),
                Frame::Notification(notification) if notification.method == "session/event" => {
                    events.push(notification.params.expect("事件 params"));
                }
                _ => {}
            }
        }
    }

    async fn error(&mut self, id: u64) -> (i64, String, Option<Value>) {
        let expected = RequestId::Number(id as i64);
        loop {
            match self.next_frame().await {
                Frame::Response(response) if response.id == expected => {
                    panic!("请求 {id} 意外成功: {}", response.result)
                }
                Frame::ErrorResponse(response) if response.id == expected => {
                    return (
                        response.error.code,
                        response.error.message,
                        response.error.data,
                    );
                }
                _ => {}
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

    /// 收集事件直到谓词命中（含命中事件）。
    async fn drain_until(&mut self, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut events = Vec::new();
        loop {
            let event = self.next_event().await;
            let hit = predicate(&event);
            events.push(event);
            if hit {
                return events;
            }
        }
    }

    async fn drain_until_goal_status(&mut self, status: &str) -> Vec<Value> {
        self.drain_until(|event| {
            event["type"] == json!("session.updated")
                && event["payload"]["action"] == json!("status_updated")
                && event["payload"]["target"]["status"] == json!(status)
        })
        .await
    }

    async fn drain_until_terminal(&mut self) -> Vec<Value> {
        self.drain_until(|event| {
            matches!(
                event["type"].as_str(),
                Some("turn.completed") | Some("turn.failed")
            )
        })
        .await
    }

    async fn read_snapshot(&mut self, session_id: &str) -> Value {
        let id = self
            .send("session/read", json!({ "sessionId": session_id }))
            .await;
        self.result(id).await
    }
}

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

/// provider 解析闸门 + 固定 Err：制造「goal job 已受理、pause 先到、随后配置失败」。
struct GatedErrorSource {
    state: Arc<(Mutex<(bool, bool)>, Condvar)>,
}

impl GatedErrorSource {
    #[allow(clippy::type_complexity)]
    fn new() -> (Self, Arc<(Mutex<(bool, bool)>, Condvar)>) {
        let state = Arc::new((Mutex::new((false, false)), Condvar::new()));
        (
            Self {
                state: Arc::clone(&state),
            },
            state,
        )
    }
}

impl ProviderSource for GatedErrorSource {
    fn resolve(
        &self,
        _provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        let mut state = self.state.0.lock().expect("gate");
        state.0 = true;
        self.state.1.notify_all();
        while !state.1 {
            state = self.state.1.wait(state).expect("gate wait");
        }
        Err("mock configuration failure".to_string())
    }
}

struct Fixture {
    server: Arc<AgentServer>,
    db_path: PathBuf,
    ws: PathBuf,
    provider: Arc<GoalProvider>,
    _root: TempRoot,
}

fn fixture(label: &str, provider: GoalProvider) -> Fixture {
    let provider = Arc::new(provider);
    let source: Arc<dyn ProviderSource> = Arc::new(StaticSource {
        provider: provider.clone(),
    });
    fixture_build(label, source, provider)
}

fn fixture_with_source(label: &str, source: Arc<dyn ProviderSource>) -> Fixture {
    fixture_build(label, source, Arc::new(GoalProvider::new("unused").0))
}

fn fixture_build(
    label: &str,
    source: Arc<dyn ProviderSource>,
    provider: Arc<GoalProvider>,
) -> Fixture {
    let root = TempRoot::new(label);
    let mode_root = root.path().join(".mode");
    let db_dir = mode_root.join("cli").join("db");
    std::fs::create_dir_all(&db_dir).expect("建 cli/db 目录");
    let db_path = db_dir.join("db.sqlite");
    create_baseline_database(&db_path);
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).expect("建工作目录");
    std::fs::write(ws.join("hello.txt"), "hello\n").expect("写种子文件");

    let hub = ProviderConfigHub::with_paths(
        write_builtin_file(root.path()),
        write_personal_file(root.path()),
    );
    let config = ServerConfig::from_data_base_dir(root.path());
    let server = Arc::new(AgentServer::with_parts(config, hub, source));
    Fixture {
        server,
        db_path,
        ws,
        provider,
        _root: root,
    }
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
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    path
}

fn write_builtin_file(root: &Path) -> String {
    let path = root.join("mode-builtin.json");
    std::fs::write(
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

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("用例超时（10s）")
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
            params[key.as_str()] = value.clone();
        }
    }
    let id = conn.send("session/goal", params).await;
    conn.result(id).await
}

async fn goal_error(
    conn: &mut TestConn,
    session_id: &str,
    action: &str,
    extra: Value,
) -> (i64, String, Option<Value>) {
    let mut params = json!({ "sessionId": session_id, "action": action });
    if let Some(object) = extra.as_object() {
        for (key, value) in object {
            params[key.as_str()] = value.clone();
        }
    }
    let id = conn.send("session/goal", params).await;
    conn.error(id).await
}

async fn wait_idle(conn: &mut TestConn, session_id: &str) -> Value {
    for _ in 0..400 {
        let snapshot = conn.read_snapshot(session_id).await;
        if snapshot["session"]["status"] == json!("idle") {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("会话未在限期内回到 idle");
}

async fn close_session(conn: &mut TestConn, session_id: &str) {
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    assert_eq!(conn.result(id).await, json!({ "closed": true }));
}

async fn resume_session(conn: &mut TestConn, session_id: &str) -> Value {
    let id = conn
        .send("session/resume", json!({ "sessionId": session_id }))
        .await;
    let snapshot = conn.result(id).await;
    assert_eq!(snapshot["session"]["sessionId"], json!(session_id));
    conn.subscribe_default(session_id).await;
    snapshot
}

fn kinds(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap_or("").to_string())
        .collect()
}

fn turn_starts(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event["type"] == json!("turn.started"))
        .map(|event| event["payload"].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// 1. 全 action + 无 goal 副作用
// ---------------------------------------------------------------------------

#[tokio::test]
async fn goal_all_actions_events_db_and_snapshot() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let fixture = fixture("all-actions", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    // show（无 goal）
    let show = goal_action(&mut conn, &session_id, "show", json!({})).await;
    assert_eq!(
        show["response"],
        json!("No goal is set. Use /goal <objective> to set one.")
    );
    assert_eq!(show["startedTurn"], json!(false));
    assert_eq!(show["snapshot"]["session"]["target"], Value::Null);
    assert_eq!(show["snapshot"]["projection"]["target"], Value::Null);

    // pause（无 goal）→ titleUpdated "/goal paused"；resume/clear（无 goal）文案。
    let pause = goal_action(&mut conn, &session_id, "pause", json!({})).await;
    assert_eq!(pause["response"], json!("No goal to pause."));
    let title_events = conn
        .drain_until(|event| event["type"] == json!("session.titleUpdated"))
        .await;
    let title = title_events.last().expect("titleUpdated");
    assert_eq!(title["payload"]["title"], json!("/goal paused"));
    assert_eq!(title["payload"]["source"], json!("first_input"));
    assert_eq!(title["payload"]["previousTitle"], json!(""));

    let resume = goal_action(&mut conn, &session_id, "resume", json!({})).await;
    assert_eq!(resume["response"], json!("No goal to resume."));
    let clear = goal_action(&mut conn, &session_id, "clear", json!({})).await;
    assert_eq!(clear["response"], json!("No goal to clear."));
    let cleared = conn
        .drain_until(|event| {
            event["type"] == json!("session.updated")
                && event["payload"]["action"] == json!("cleared")
        })
        .await;
    assert_eq!(
        cleared.last().expect("cleared")["payload"]["previousTarget"],
        Value::Null
    );
    // 无 goal 的 pause/clear 都不写 message 行（夹具 messageSummary 为空）。
    assert_eq!(count_rows(&fixture.db_path, "message"), 0);

    // set → 控制 turn + set + 续跑 + verifier → complete。
    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Ship the C3 goal contract", "inputId": "goal_set_main" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let response = set["response"].as_str().expect("response");
    assert!(response.starts_with("Goal active\nObjective: Ship the C3 goal contract\n"));
    assert_eq!(
        set["snapshot"]["session"]["target"]["status"],
        json!("active")
    );
    assert_eq!(
        set["snapshot"]["session"]["target"]["summaryTitle"],
        json!("Ship the C3 goal contract")
    );
    let target_id = set["snapshot"]["session"]["target"]["targetId"]
        .as_str()
        .expect("targetId")
        .to_string();

    let events = conn.drain_until_goal_status("complete").await;
    let starts = turn_starts(&events);
    assert_eq!(starts.len(), 2, "控制 turn + continuation turn");
    assert_eq!(starts[0]["executionKind"], json!("controlOnly"));
    assert_eq!(starts[0]["turnNumber"], json!(0));
    assert_eq!(starts[0]["input"], json!("Ship the C3 goal contract"));
    assert_eq!(starts[1]["inputSource"], json!("goal-continuation"));
    assert_eq!(starts[1]["inputVisibility"], json!("model-only"));
    assert_eq!(starts[1]["targetId"], json!(target_id));
    assert!(starts[1]["input"]
        .as_str()
        .expect("continuation input")
        .contains("<untrusted_objective>\nShip the C3 goal contract\n</untrusted_objective>"));
    let event_kinds = kinds(&events);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == json!("session.titleUpdated"))
            .count(),
        0,
        "title 已由无 goal 的 pause 发出，set 不再重复"
    );
    assert!(event_kinds.iter().any(|kind| kind == "turn.completed"));
    assert_eq!(
        event_kinds
            .iter()
            .filter(|kind| kind.as_str() == "turn.completed")
            .count(),
        2
    );
    let actions: Vec<&str> = events
        .iter()
        .filter_map(|event| event["payload"]["action"].as_str())
        .collect();
    for expected in [
        "summary_updated",
        "set",
        "run_started",
        "run_finished",
        "status_updated",
    ] {
        assert!(
            actions.contains(&expected),
            "缺少 session.updated action={expected}: {actions:?}"
        );
    }
    let verification_statuses: Vec<&str> = events
        .iter()
        .filter(|event| event["type"] == json!("session.updated"))
        .filter_map(|event| event["payload"]["status"].as_str())
        .filter(|status| matches!(*status, "started" | "completed" | "cancelled"))
        .collect();
    assert_eq!(verification_statuses, vec!["started", "completed"]);

    // show complete
    let show_complete = goal_action(&mut conn, &session_id, "show", json!({})).await;
    assert!(show_complete["response"]
        .as_str()
        .expect("response")
        .starts_with("Goal complete\n"));
    assert_eq!(show_complete["startedTurn"], json!(false));

    // 快照与 DB 同源
    let read = conn.read_snapshot(&session_id).await;
    assert_eq!(read["session"]["target"]["status"], json!("complete"));
    assert_eq!(read["session"]["target"]["tokensUsed"], json!(22));
    assert!(
        read["session"]["target"]["timeUsedSeconds"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );
    assert_eq!(
        read["session"]["target"], read["projection"]["target"],
        "session.target 与 projection.target 逐字段相同"
    );
    let target = target_row(&fixture.db_path, &session_id).expect("session_target 行");
    assert_eq!(target["status"], json!("complete"));
    assert_eq!(target["objective"], json!("Ship the C3 goal contract"));
    assert_eq!(target["summaryTitle"], json!("Ship the C3 goal contract"));
    assert_eq!(target["tokensUsed"], json!(22));
    assert_eq!(target["activeInputId"], Value::Null);
    assert_eq!(count_rows(&fixture.db_path, "session_target"), 1);

    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1, "控制 turn 不写 turn_usage");
    assert_eq!(usage[0]["status"], json!("completed"));
    assert_eq!(usage[0]["computedTotalTokens"], json!(22));

    let model_usage = model_usage_rows(&fixture.db_path, &session_id);
    assert!(
        model_usage
            .iter()
            .any(|row| row["querySource"] == json!("main_turn")
                && row["status"] == json!("completed"))
    );
    assert!(model_usage.iter().any(|row| row["querySource"]
        == json!("target_completion_verification")
        && row["status"] == json!("completed")));

    // verifier 请求：0 工具、提示词要求 JSON 形状。
    let records = fixture.provider.requests_snapshot();
    let verifier = records
        .iter()
        .find(|record| record.kind == RequestKind::Verifier)
        .expect("verifier 请求");
    assert_eq!(verifier.tool_count, 0);
    assert!(verifier
        .last_user
        .contains("{\"passed\": boolean, \"reason\": string, \"nextAction\": string}"));
}

// ---------------------------------------------------------------------------
// 1b. 缺 objective 的 usage 文案（成功应答、零状态变化）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_objective_returns_usage_without_state_change() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let fixture = fixture("missing-objective", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let before = conn.read_snapshot(&session_id).await;
    let set = goal_action(&mut conn, &session_id, "set", json!({})).await;
    assert_eq!(set["response"], json!("Usage: /goal <objective>"));
    assert_eq!(set["startedTurn"], json!(false));
    assert_eq!(set["snapshot"]["session"]["target"], Value::Null);
    let replace = goal_action(&mut conn, &session_id, "replace", json!({})).await;
    assert_eq!(
        replace["response"],
        json!("Usage: /goal replace <objective>")
    );
    assert_eq!(replace["startedTurn"], json!(false));
    let after = conn.read_snapshot(&session_id).await;
    assert_eq!(
        after["runtime"]["stateRevision"],
        before["runtime"]["stateRevision"]
    );
    assert_eq!(after["runtime"]["eventSeq"], before["runtime"]["eventSeq"]);
    assert_eq!(count_rows(&fixture.db_path, "session_target"), 0);
}

// ---------------------------------------------------------------------------
// 1c. 首次 set：titleUpdated 在控制 turn 之前，应答快照是控制 turn 之后
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_set_emits_title_and_returns_post_control_snapshot() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let fixture = fixture("first-set-title", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Goal one", "inputId": "goal_s1" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    assert_eq!(set["snapshot"]["runtime"]["stateRevision"], json!(3));
    assert_eq!(set["snapshot"]["projection"]["turnCount"], json!(1));
    let control_turn_id = set["snapshot"]["projection"]["currentTurnId"]
        .as_str()
        .expect("currentTurnId")
        .to_string();
    assert!(set["snapshot"]["runtime"]["eventSeq"].as_u64().unwrap_or(0) >= 4);

    // 事件缓冲里 set 之前的部分：title → 控制 turn → summary → set。
    let prep = conn
        .drain_until(|event| event["payload"]["action"] == json!("set"))
        .await;
    assert_eq!(prep[0]["type"], json!("session.titleUpdated"));
    assert_eq!(prep[0]["payload"]["title"], json!("Goal one"));
    assert_eq!(prep[0]["payload"]["previousTitle"], json!(""));
    assert_eq!(prep[0]["payload"]["source"], json!("first_input"));
    let control = prep
        .iter()
        .find(|event| event["type"] == json!("turn.started"))
        .expect("控制 turn");
    assert_eq!(control["payload"]["turnNumber"], json!(0));
    assert_eq!(control["payload"]["executionKind"], json!("controlOnly"));
    assert_eq!(control["turnId"].as_str(), Some(control_turn_id.as_str()));
    assert_eq!(
        prep.last().expect("set event")["seq"],
        set["snapshot"]["runtime"]["eventSeq"],
        "应答快照的 eventSeq 是 set 事件之后的序号"
    );
    assert_eq!(session_title(&fixture.db_path, &session_id), "Goal one");

    let _ = conn.drain_until_goal_status("complete").await;
}

#[tokio::test]
async fn first_replace_also_emits_title() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let fixture = fixture("first-replace-title", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let replace = goal_action(
        &mut conn,
        &session_id,
        "replace",
        json!({ "objective": "Replace goal", "inputId": "goal_r1" }),
    )
    .await;
    assert_eq!(replace["startedTurn"], json!(true));
    let prep = conn
        .drain_until(|event| event["payload"]["action"] == json!("set"))
        .await;
    assert_eq!(prep[0]["type"], json!("session.titleUpdated"));
    assert_eq!(prep[0]["payload"]["title"], json!("Replace goal"));
    let _ = conn.drain_until_goal_status("complete").await;
}

// ---------------------------------------------------------------------------
// 2. 运行中限制 + pause 例外（in-flight）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn running_goal_management_rejected_but_pause_cancels_in_flight() {
    let (provider, requests) = GoalProvider::new("slow goal answer");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_gate(Arc::clone(&gate));
    let fixture = fixture("running", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Pause in-flight goal", "inputId": "goal_pif_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));

    let _ = conn
        .drain_until(|event| event["type"] == json!("model.streaming"))
        .await;
    fixture.provider.entered().notified().await;
    for (action, extra) in [
        ("show", json!({})),
        ("set", json!({ "objective": "other" })),
        ("replace", json!({ "objective": "other" })),
        ("resume", json!({})),
        ("clear", json!({})),
    ] {
        let (code, message, _) = goal_error(&mut conn, &session_id, action, extra).await;
        assert_eq!(code, -32010, "action={action}");
        assert_eq!(message, "Cannot manage goals while a prompt is running");
    }

    // pause 例外：置 paused、取消 in-flight 请求。
    let pause = goal_action(&mut conn, &session_id, "pause", json!({})).await;
    assert_eq!(pause["response"], json!(""));
    assert_eq!(pause["startedTurn"], json!(false));
    assert_eq!(
        pause["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );
    assert_eq!(
        pause["snapshot"]["session"]["target"]["activeInputId"],
        json!("goal_pif_set")
    );
    // set 3 + pause 1；快照已含 status_updated 的 seq，turnCount 含已开始的续跑。
    assert_eq!(pause["snapshot"]["runtime"]["stateRevision"], json!(4));
    assert_eq!(pause["snapshot"]["projection"]["turnCount"], json!(2));
    assert!(
        pause["snapshot"]["projection"]["currentTurnId"].is_string(),
        "pause 快照保留运行中的 continuation turnId"
    );
    assert_eq!(
        pause["snapshot"]["session"]["status"],
        json!("running"),
        "pause 应答是取消中的快照"
    );

    let events = conn.drain_until_terminal().await;
    assert_unique_monotonic(&events);
    let terminal = events.last().expect("终态");
    assert_eq!(terminal["type"], json!("turn.completed"));
    assert_eq!(terminal["payload"]["resultType"], json!("cancelled"));
    let status = events
        .iter()
        .find(|event| {
            event["payload"]["action"] == json!("status_updated")
                && event["payload"]["target"]["status"] == json!("paused")
        })
        .expect("status_updated(paused)");
    assert_eq!(
        status["payload"]["previousTarget"]["status"],
        json!("active")
    );
    let run_finished = events
        .iter()
        .find(|event| event["payload"]["action"] == json!("run_finished"))
        .expect("run_finished");
    assert_eq!(
        run_finished["payload"]["target"]["activeInputId"],
        Value::Null
    );
    assert_eq!(
        pause["snapshot"]["runtime"]["eventSeq"], status["seq"],
        "pause 应答快照的 eventSeq 已包含 status_updated"
    );
    // status_updated 在 run_finished 与终态之前（观察顺序）。
    let status_position = events
        .iter()
        .position(|event| event["payload"]["action"] == json!("status_updated"))
        .expect("status position");
    let finished_position = events
        .iter()
        .position(|event| event["payload"]["action"] == json!("run_finished"))
        .expect("finished position");
    assert!(status_position < finished_position);

    let snapshot = wait_idle(&mut conn, &session_id).await;
    assert_eq!(snapshot["session"]["target"]["status"], json!("paused"));
    assert_eq!(snapshot["session"]["target"]["tokensUsed"], json!(0));
    assert_eq!(snapshot["session"]["target"]["activeInputId"], Value::Null);

    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["status"], json!("cancelled"));
    assert_eq!(usage[0]["modelRequestCount"], json!(1));
    assert_eq!(usage[0]["cancelledByUser"], json!(1));
    assert_eq!(usage[0]["errorType"], json!("turn_cancelled"));
    assert_eq!(usage[0]["errorCode"], json!("TURN_CANCELLED"));
    let model_usage = model_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(model_usage.len(), 1);
    assert_eq!(model_usage[0]["status"], json!("cancelled"));
    assert_eq!(model_usage[0]["errorType"], json!("cancelled"));
    assert_eq!(model_usage[0]["computedTotalTokens"], json!(0));
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("paused"));
    assert_eq!(target["tokensUsed"], json!(0));
    assert_eq!(requests.lock().expect("requests").len(), 1);
    assert_eq!(
        messages_with_source(&fixture.db_path, "goal_state_change").len(),
        1,
        "pause 在续跑用户消息之后写 goal_state_change 行"
    );

    // resume：status_updated 同步进应答快照；续跑 foreground 序号 +1，goalIteration 从 1。
    gate.add_permits(1);
    let resume = goal_action(
        &mut conn,
        &session_id,
        "resume",
        json!({ "inputId": "goal_pif_resume" }),
    )
    .await;
    assert_eq!(resume["startedTurn"], json!(true));
    assert_eq!(resume["snapshot"]["runtime"]["stateRevision"], json!(6));
    assert_eq!(resume["snapshot"]["projection"]["turnCount"], json!(2));
    let resumed_events = conn.drain_until_goal_status("complete").await;
    let resumed_start = resumed_events
        .iter()
        .find(|event| event["type"] == json!("turn.started"))
        .expect("resumed continuation turn");
    assert_eq!(resumed_start["payload"]["turnNumber"], json!(1));
    assert_eq!(
        resumed_start["payload"]["foregroundExecutionId"],
        json!("runtime_command_2")
    );
    let verification = resumed_events
        .iter()
        .find(|event| event["payload"]["status"] == json!("started"))
        .expect("verification started");
    assert_eq!(verification["payload"]["goalIteration"], json!(1));
    assert_eq!(
        verification["payload"]["foregroundExecutionId"],
        json!("runtime_command_2")
    );
    assert_eq!(
        messages_with_source(&fixture.db_path, "goal_state_change").len(),
        2,
        "pause + resume 各一条 goal_state_change 行"
    );

    // clear：应答快照含 cleared；turnCount 3（control + 两次 continuation）。
    let clear = goal_action(&mut conn, &session_id, "clear", json!({})).await;
    assert_eq!(clear["response"], json!("Goal cleared."));
    assert_eq!(clear["snapshot"]["runtime"]["stateRevision"], json!(8));
    assert_eq!(clear["snapshot"]["projection"]["turnCount"], json!(3));
    assert_eq!(clear["snapshot"]["session"]["target"], Value::Null);
    let clear_events = conn
        .drain_until(|event| event["payload"]["action"] == json!("cleared"))
        .await;
    assert_eq!(
        clear["snapshot"]["runtime"]["eventSeq"],
        clear_events.last().expect("cleared")["seq"]
    );
    assert_eq!(count_rows(&fixture.db_path, "session_target"), 0);
}

#[tokio::test]
async fn repeated_pause_keeps_pending_events_unique_and_monotonic() {
    let (provider, _requests) = GoalProvider::new("slow answer");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_gate(Arc::clone(&gate));
    let fixture = fixture("double-pause", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Double pause goal", "inputId": "goal_dp_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["type"] == json!("model.streaming"))
        .await;
    fixture.provider.entered().notified().await;

    let first_id = conn
        .send(
            "session/goal",
            json!({ "sessionId": session_id, "action": "pause" }),
        )
        .await;
    let second_id = conn
        .send(
            "session/goal",
            json!({ "sessionId": session_id, "action": "pause" }),
        )
        .await;
    let mut events = Vec::new();
    let first = conn.result_collecting(first_id, &mut events).await;
    assert_eq!(
        first["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );
    let second = conn.result_collecting(second_id, &mut events).await;
    assert_eq!(
        second["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );
    let cancelled = |event: &Value| {
        event["type"] == json!("turn.completed")
            && event["payload"]["resultType"] == json!("cancelled")
    };
    if !events.iter().any(cancelled) {
        events.extend(conn.drain_until(cancelled).await);
    }
    assert_unique_monotonic(&events);
    let status_events: Vec<&Value> = events
        .iter()
        .filter(|event| event["payload"]["action"] == json!("status_updated"))
        .collect();
    assert_eq!(status_events.len(), 2, "两次 pause 都保留自己的状态事件");
    gate.add_permits(1);
}

#[tokio::test]
async fn verifier_cancellation_keeps_seq_unique_and_monotonic() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_verifier_gate(Arc::clone(&gate));
    let fixture = fixture("verifier-cancel", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Verifier cancel goal", "inputId": "goal_vc_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["payload"]["status"] == json!("started"))
        .await;
    fixture.provider.verifier_entered().notified().await;

    let pause = goal_action(&mut conn, &session_id, "pause", json!({})).await;
    assert_eq!(
        pause["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );
    let events = conn
        .drain_until(|event| event["payload"]["status"] == json!("cancelled"))
        .await;
    assert_unique_monotonic(&events);
    let status_position = events
        .iter()
        .position(|event| event["payload"]["action"] == json!("status_updated"))
        .expect("status_updated");
    let cancelled_position = events
        .iter()
        .position(|event| event["payload"]["status"] == json!("cancelled"))
        .expect("verification cancelled");
    assert!(status_position < cancelled_position);
    let cancelled = &events[cancelled_position];
    assert_eq!(cancelled["payload"]["goalIteration"], json!(1));
    assert_eq!(
        cancelled["payload"]["foregroundExecutionId"],
        json!("runtime_command_1")
    );
    let _ = wait_idle(&mut conn, &session_id).await;
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("paused"));
    gate.add_permits(1);
}

#[tokio::test]
async fn pause_during_streaming_race_keeps_seq_unique_and_no_stream_after_terminal() {
    let provider = GoalProvider::new("unused").0;
    let provider = provider.with_streaming_deltas(200);
    let fixture = fixture("stream-race", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Stream race goal", "inputId": "goal_sr_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["type"] == json!("model.streaming"))
        .await;
    fixture.provider.entered().notified().await;
    let pause = goal_action(&mut conn, &session_id, "pause", json!({})).await;
    assert_eq!(
        pause["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );

    let events = conn.drain_until_terminal().await;
    assert_unique_monotonic(&events);
    let terminal_position = events
        .iter()
        .position(|event| event["type"] == json!("turn.completed"))
        .expect("terminal");
    assert!(
        events[terminal_position..]
            .iter()
            .all(|event| event["type"] != json!("model.streaming")),
        "终态之后不得再发布 model.streaming"
    );
    // 终态之后短窗口内也不允许再出现流式事件（pause 竞争窗口）。
    if let Ok(event) = tokio::time::timeout(Duration::from_millis(200), conn.next_event()).await {
        assert_ne!(
            event["type"],
            json!("model.streaming"),
            "终态后仍收到 model.streaming"
        );
    }
    assert!(events.iter().any(|event| {
        event["payload"]["action"] == json!("status_updated")
            && event["payload"]["target"]["status"] == json!("paused")
    }));
    let _ = wait_idle(&mut conn, &session_id).await;
}

// ---------------------------------------------------------------------------
// 3. pause 在 provider 请求前（model_request_count 0）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_before_provider_request_records_zero_model_requests() {
    let (provider, requests) = GoalProvider::new("never requested");
    let pre_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_pre_gate(Arc::clone(&pre_gate));
    let fixture = fixture("pause-before", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Pause before request", "inputId": "goal_before_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));

    // provider.stream 已进入但请求还没发出 → pause 命中「请求前」窗口。
    let _ = conn
        .drain_until(|event| event["payload"]["action"] == json!("run_started"))
        .await;
    fixture.provider.pre_entered().notified().await;
    let pause = goal_action(&mut conn, &session_id, "pause", json!({})).await;
    assert_eq!(pause["response"], json!(""));
    assert_eq!(
        pause["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );
    pre_gate.add_permits(1);

    let events = conn.drain_until_terminal().await;
    assert_eq!(
        events.last().expect("terminal")["payload"]["resultType"],
        json!("cancelled")
    );
    let _ = wait_idle(&mut conn, &session_id).await;

    assert!(
        requests.lock().expect("requests").is_empty(),
        "不得发起 provider 请求"
    );
    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["status"], json!("cancelled"));
    assert_eq!(usage[0]["modelRequestCount"], json!(0));
    assert!(model_usage_rows(&fixture.db_path, &session_id).is_empty());
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("paused"));
    assert_eq!(target["tokensUsed"], json!(0));
}

// ---------------------------------------------------------------------------
// 3b. user stop 在 provider 请求前（model_request_count 0）+ goal_state_change 行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_stop_before_provider_request_records_zero_requests() {
    let (provider, requests) = GoalProvider::new("never requested");
    let pre_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_pre_gate(Arc::clone(&pre_gate));
    let fixture = fixture("stop-before", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Stop before request", "inputId": "goal_stop_before_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["payload"]["action"] == json!("run_started"))
        .await;
    fixture.provider.pre_entered().notified().await;

    let stop_id = conn
        .send("session/stop", json!({ "sessionId": session_id }))
        .await;
    assert_eq!(conn.result(stop_id).await, json!({}));
    pre_gate.add_permits(1);

    let events = conn.drain_until_terminal().await;
    assert_unique_monotonic(&events);
    assert_eq!(
        events.last().expect("terminal")["payload"]["resultType"],
        json!("cancelled")
    );
    assert!(events.iter().any(|event| {
        event["payload"]["action"] == json!("status_updated")
            && event["payload"]["target"]["status"] == json!("paused")
    }));
    let _ = wait_idle(&mut conn, &session_id).await;

    assert!(requests.lock().expect("requests").is_empty());
    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["status"], json!("cancelled"));
    assert_eq!(usage[0]["modelRequestCount"], json!(0));
    assert_eq!(usage[0]["cancelledByUser"], json!(1));
    assert!(model_usage_rows(&fixture.db_path, &session_id).is_empty());
    assert_eq!(
        messages_with_source(&fixture.db_path, "goal_state_change").len(),
        1,
        "stop 写 goal_state_change 行"
    );
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("paused"));
    assert_eq!(target["activeInputId"], Value::Null);
}

// ---------------------------------------------------------------------------
// 4. set-over-existing / replace 目标 id 变化
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_over_existing_and_replace_change_target_id_single_row() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_gate(Arc::clone(&gate));
    let fixture = fixture("set-over", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let first = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Goal one", "inputId": "goal_s1" }),
    )
    .await;
    assert_eq!(first["snapshot"]["runtime"]["stateRevision"], json!(3));
    let first_id = first["snapshot"]["session"]["target"]["targetId"]
        .as_str()
        .expect("target-1")
        .to_string();
    let first_prep = conn
        .drain_until(|event| event["payload"]["action"] == json!("set"))
        .await;
    assert_eq!(first_prep[0]["type"], json!("session.titleUpdated"));
    assert_eq!(first_prep[0]["payload"]["title"], json!("Goal one"));
    let first_control = first_prep
        .iter()
        .find(|event| event["type"] == json!("turn.started"))
        .expect("first control turn");
    assert_eq!(first_control["payload"]["turnNumber"], json!(0));

    // 夹具在第二个 set 前先 pause 第一个目标；pause + cancelled 各 +1 revision。
    let _ = conn
        .drain_until(|event| event["type"] == json!("model.streaming"))
        .await;
    fixture.provider.entered().notified().await;
    let pause = goal_action(&mut conn, &session_id, "pause", json!({})).await;
    assert_eq!(pause["snapshot"]["runtime"]["stateRevision"], json!(4));
    let _ = conn.drain_until_terminal().await;
    let _ = wait_idle(&mut conn, &session_id).await;
    assert_eq!(
        target_row(&fixture.db_path, &session_id).expect("paused row")["status"],
        json!("paused")
    );

    let second = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Goal two", "inputId": "goal_s2" }),
    )
    .await;
    // 替换已有目标：set 只 +1（paused 后 5 → 6）。
    assert_eq!(second["snapshot"]["runtime"]["stateRevision"], json!(6));
    let second_id = second["snapshot"]["session"]["target"]["targetId"]
        .as_str()
        .expect("target-2")
        .to_string();
    assert_ne!(first_id, second_id, "set 覆盖已有目标产生新 id");
    // 应答快照在控制 turn 之后：turnCount 3（两次控制 + 一次续跑），currentTurnId 有值。
    assert_eq!(second["snapshot"]["projection"]["turnCount"], json!(3));
    assert!(second["snapshot"]["projection"]["currentTurnId"].is_string());
    let second_prep = conn
        .drain_until(|event| event["payload"]["action"] == json!("set"))
        .await;
    assert!(
        !second_prep
            .iter()
            .any(|event| event["type"] == json!("session.titleUpdated")),
        "title 只由首次 set 发出"
    );
    let second_control = second_prep
        .iter()
        .find(|event| event["type"] == json!("turn.started"))
        .expect("second control turn");
    assert_eq!(second_control["payload"]["turnNumber"], json!(1));
    let set_event = second_prep.last().expect("set event");
    assert_eq!(
        set_event["payload"]["previousTarget"]["targetID"],
        json!(first_id)
    );
    assert_eq!(set_event["payload"]["target"]["targetID"], json!(second_id));
    assert_eq!(
        second["snapshot"]["runtime"]["eventSeq"], set_event["seq"],
        "second set 应答快照是控制/set 事件之后"
    );
    gate.add_permits(1);
    let events = conn.drain_until_goal_status("complete").await;
    let continuation = events
        .iter()
        .find(|event| event["type"] == json!("turn.started"))
        .expect("second target continuation");
    assert_eq!(continuation["payload"]["turnNumber"], json!(2));
    assert_eq!(
        continuation["payload"]["foregroundExecutionId"],
        json!("runtime_command_2")
    );
    let verification = events
        .iter()
        .find(|event| event["payload"]["status"] == json!("started"))
        .expect("second target verifier started");
    assert_eq!(verification["payload"]["goalIteration"], json!(1));
    assert_eq!(
        verification["payload"]["foregroundExecutionId"],
        json!("runtime_command_2")
    );

    // DB 的新目标 time_created 与快照一致（替换语义，不保留旧创建的 time_created）。
    let row_after_second = target_row(&fixture.db_path, &session_id).expect("second target row");
    assert_eq!(
        second["snapshot"]["session"]["target"]["createdAt"].as_u64(),
        row_after_second["timeCreated"].as_u64()
    );

    let replace = goal_action(
        &mut conn,
        &session_id,
        "replace",
        json!({ "objective": "Goal three", "inputId": "goal_r3" }),
    )
    .await;
    let replace_id = replace["snapshot"]["session"]["target"]["targetId"]
        .as_str()
        .expect("target-3")
        .to_string();
    assert_ne!(second_id, replace_id);
    let events = conn.drain_until_goal_status("complete").await;
    let replace_event = events
        .iter()
        .find(|event| event["payload"]["action"] == json!("set"))
        .expect("replace 的 set event");
    assert_eq!(
        replace_event["payload"]["previousTarget"]["targetID"],
        json!(second_id)
    );

    assert_eq!(count_rows(&fixture.db_path, "session_target"), 1);
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["objective"], json!("Goal three"));
    assert_eq!(target["targetId"], json!(replace_id));
}

// ---------------------------------------------------------------------------
// 4b. provider 错误：清 active 字段并同步落库（hot/cold 同源）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_error_clears_active_fields_in_db_and_snapshot() {
    let (provider, _requests) = GoalProvider::new("unused");
    provider.fail_next_continuation();
    let fixture = fixture("provider-error", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Provider error goal", "inputId": "goal_err_set" }),
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
    let snapshot = wait_idle(&mut conn, &session_id).await;
    assert_eq!(snapshot["session"]["target"]["status"], json!("active"));
    assert_eq!(
        snapshot["session"]["target"]["activeInputId"],
        Value::Null,
        "失败 turn 不能把 activeInputId 留在热状态"
    );
    let row = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(row["status"], json!("active"));
    assert_eq!(row["activeInputId"], Value::Null);
    assert_eq!(row["activeRunStartedAt"], Value::Null);
    assert_eq!(row["activeRunLastSeenAt"], Value::Null);
    assert_eq!(
        snapshot["session"]["target"]["activeInputId"],
        row["activeInputId"]
    );
}

#[tokio::test]
async fn stop_during_ordinary_prompt_with_active_target_drains_pending_event() {
    let (provider, _requests) = GoalProvider::new("ordinary answer");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_gate(Arc::clone(&gate));
    provider.fail_next_continuation();
    let fixture = fixture("stop-ordinary", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Ordinary stop goal", "inputId": "goal_ord_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["type"] == json!("turn.failed"))
        .await;
    let _ = wait_idle(&mut conn, &session_id).await;
    let row = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(row["status"], json!("active"));

    let prompt_id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "ordinary prompt" }),
        )
        .await;
    let _ = conn.result(prompt_id).await;
    let _ = conn
        .drain_until(|event| event["type"] == json!("model.streaming"))
        .await;
    fixture.provider.entered().notified().await;
    let stop_id = conn
        .send("session/stop", json!({ "sessionId": session_id }))
        .await;
    let mut events = Vec::new();
    assert_eq!(
        conn.result_collecting(stop_id, &mut events).await,
        json!({})
    );
    let cancelled = |event: &Value| {
        event["type"] == json!("turn.completed")
            && event["payload"]["resultType"] == json!("cancelled")
    };
    let paused = |event: &Value| {
        event["payload"]["action"] == json!("status_updated")
            && event["payload"]["target"]["status"] == json!("paused")
    };
    let mut saw_terminal = events.iter().any(cancelled);
    let mut saw_paused = events.iter().any(paused);
    while !(saw_terminal && saw_paused) {
        let event = conn.next_event().await;
        saw_terminal |= cancelled(&event);
        saw_paused |= paused(&event);
        events.push(event);
    }
    assert_unique_monotonic(&events);
    assert!(saw_paused);
    let _ = wait_idle(&mut conn, &session_id).await;
    // 事件序号没有缺号：后续普通 prompt 仍能继续发布更大的 seq。
    gate.add_permits(1);
    let next_id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "next prompt" }),
        )
        .await;
    let _ = conn.result(next_id).await;
    let next_events = conn.drain_until_terminal().await;
    let mut combined = events;
    combined.extend(next_events);
    assert_unique_monotonic(&combined);
}

#[tokio::test]
async fn configuration_failure_drains_pending_pause_event() {
    let (source, gate) = GatedErrorSource::new();
    let fixture = fixture_with_source("config-drain", Arc::new(source));
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Config failure goal", "inputId": "goal_cf_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    // 读一次快照：stdio 循环顺带应答 worker 启动时的偏好反向请求，worker 才会
    // 进入 goal 任务并调用 provider resolve。
    let _ = conn.read_snapshot(&session_id).await;

    // provider 解析被闸门挡住 → pause 先入 pending 队列（异步轮询，不阻塞 runtime）。
    for _ in 0..400 {
        if gate.0.lock().expect("gate").0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(gate.0.lock().expect("gate").0, "provider 解析未进入闸门");
    let mut events = Vec::new();
    let pause_id = conn
        .send(
            "session/goal",
            json!({ "sessionId": session_id, "action": "pause" }),
        )
        .await;
    let pause = conn.result_collecting(pause_id, &mut events).await;
    assert_eq!(
        pause["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );

    // 放行解析 → 配置失败；finish 必须 drain pending 事件而不是留缺口。
    {
        let mut state = gate.0.lock().expect("gate");
        state.1 = true;
        gate.1.notify_all();
    }
    let paused = |event: &Value| {
        event["payload"]["action"] == json!("status_updated")
            && event["payload"]["target"]["status"] == json!("paused")
    };
    if !events.iter().any(paused) {
        events.extend(conn.drain_until(paused).await);
    }
    assert_unique_monotonic(&events);
    let snapshot = conn.read_snapshot(&session_id).await;
    assert_ne!(
        snapshot["session"]["status"],
        json!("running"),
        "配置失败后不会停在 running"
    );
    let row = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(row["status"], json!("paused"));

    // 后续输入仍能发布更大的 seq（没有 pending 缺号）。
    let next_id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "after config failure" }),
        )
        .await;
    let _ = conn.result(next_id).await;
    let next_events = conn
        .drain_until(|event| event["type"] == json!("turn.failed"))
        .await;
    let mut combined = events;
    combined.extend(next_events);
    assert_unique_monotonic(&combined);
}

// ---------------------------------------------------------------------------
// 5. revision 冲突零副作用
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revision_conflict_has_zero_side_effects() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let fixture = fixture("revision", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Goal before replace", "inputId": "goal_rev" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn.drain_until_goal_status("complete").await;
    let _ = wait_idle(&mut conn, &session_id).await;
    let before = conn.read_snapshot(&session_id).await;
    let revision = before["runtime"]["stateRevision"]
        .as_u64()
        .expect("revision");
    let event_seq = before["runtime"]["eventSeq"].clone();
    let counts_before = (
        count_rows(&fixture.db_path, "session_target"),
        count_rows(&fixture.db_path, "message"),
        count_rows(&fixture.db_path, "part"),
        count_rows(&fixture.db_path, "turn_usage"),
        count_rows(&fixture.db_path, "model_usage"),
    );

    let (code, message, data) = goal_error(
        &mut conn,
        &session_id,
        "replace",
        json!({
            "objective": "should not apply",
            "inputId": "goal_conflict",
            "expectedRevision": revision - 1,
        }),
    )
    .await;
    assert_eq!(code, -32009);
    assert_eq!(message, "Session state revision mismatch");
    let data = data.expect("conflict data");
    assert_eq!(data["actualRevision"], json!(revision));
    assert_eq!(data["expectedRevision"], json!(revision - 1));

    let after = conn.read_snapshot(&session_id).await;
    assert_eq!(after["runtime"]["stateRevision"], json!(revision));
    assert_eq!(after["runtime"]["eventSeq"], event_seq);
    assert_eq!(
        after["session"]["target"]["objective"],
        json!("Goal before replace")
    );
    let counts_after = (
        count_rows(&fixture.db_path, "session_target"),
        count_rows(&fixture.db_path, "message"),
        count_rows(&fixture.db_path, "part"),
        count_rows(&fixture.db_path, "turn_usage"),
        count_rows(&fixture.db_path, "model_usage"),
    );
    assert_eq!(counts_before, counts_after, "冲突不得有 DB 写");

    let replace = goal_action(
        &mut conn,
        &session_id,
        "replace",
        json!({
            "objective": "Goal after replace",
            "inputId": "goal_replace_ok",
            "expectedRevision": revision,
        }),
    )
    .await;
    assert_eq!(replace["startedTurn"], json!(true));
    let _ = conn.drain_until_goal_status("complete").await;
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["objective"], json!("Goal after replace"));
    assert_eq!(target["status"], json!("complete"));
}

// ---------------------------------------------------------------------------
// 6. 续跑循环（verifier false ×N → true；无轮次上限）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continuation_loop_runs_until_verifier_passes() {
    let (provider, _requests) = GoalProvider::new("working toward goal");
    provider.queue_verifier(r#"{"passed":false,"reason":"not yet 1","nextAction":"continue"}"#);
    provider.queue_verifier(r#"{"passed":false,"reason":"not yet 2","nextAction":"continue"}"#);
    provider.queue_verifier(r#"{"passed":true,"reason":"goal verified","nextAction":null}"#);
    let fixture = fixture("loop", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Loop goal", "inputId": "goal_loop_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let events = conn.drain_until_goal_status("complete").await;
    let starts = turn_starts(&events);
    assert_eq!(starts.len(), 4, "1 个控制 turn + 3 个 continuation turn");
    assert_eq!(
        starts
            .iter()
            .filter(|payload| payload["inputSource"] == json!("goal-continuation"))
            .count(),
        3
    );
    let verification_started = events
        .iter()
        .filter(|event| event["payload"]["status"] == json!("started"))
        .count();
    assert_eq!(verification_started, 3);

    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 3);
    assert!(usage.iter().all(|row| row["status"] == json!("completed")));
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["tokensUsed"], json!(66));
    assert_eq!(target["status"], json!("complete"));

    let read = conn.read_snapshot(&session_id).await;
    assert_eq!(
        read["runtime"]["goalVerificationTimeline"]
            .as_array()
            .expect("timeline")
            .len(),
        3
    );
    let verifications = read["runtime"]["goalVerifications"]
        .as_array()
        .expect("goalVerifications");
    assert_eq!(verifications.len(), 2, "最近验证窗口 length:2");
    assert_eq!(verifications[1]["reason"], json!("goal verified"));
}

// ---------------------------------------------------------------------------
// 7. user stop（session/stop）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_stop_cancels_continuation_and_pauses_target() {
    let (provider, requests) = GoalProvider::new("slow stop answer");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_gate(Arc::clone(&gate));
    let fixture = fixture("stop", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Stop goal", "inputId": "goal_stop_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["type"] == json!("model.streaming"))
        .await;
    fixture.provider.entered().notified().await;

    let stop_id = conn
        .send("session/stop", json!({ "sessionId": session_id }))
        .await;
    assert_eq!(conn.result(stop_id).await, json!({}));

    let events = conn.drain_until_terminal().await;
    assert_unique_monotonic(&events);
    assert_eq!(
        events.last().expect("terminal")["payload"]["resultType"],
        json!("cancelled")
    );
    assert!(events.iter().any(|event| {
        event["payload"]["action"] == json!("status_updated")
            && event["payload"]["target"]["status"] == json!("paused")
    }));
    let _ = wait_idle(&mut conn, &session_id).await;

    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("paused"));
    assert_eq!(target["tokensUsed"], json!(0));
    let usage = turn_usage_rows(&fixture.db_path, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["status"], json!("cancelled"));
    assert_eq!(usage[0]["cancelledByUser"], json!(1));
    assert_eq!(requests.lock().expect("requests").len(), 1);
    assert_eq!(
        messages_with_source(&fixture.db_path, "goal_state_change").len(),
        1,
        "stop 在续跑用户消息之后写 goal_state_change 行"
    );
    gate.add_permits(1);
}

// ---------------------------------------------------------------------------
// 8. verifier 非法 JSON fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_verifier_json_falls_back_to_passed_true() {
    let (provider, _requests) = GoalProvider::new("answer before invalid verifier");
    provider.queue_verifier("oracle-mock");
    let fixture = fixture("invalid-verifier", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Verifier invalid JSON goal", "inputId": "goal_invalid" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let events = conn.drain_until_goal_status("complete").await;
    let completed = events
        .iter()
        .find(|event| {
            event["payload"]["status"] == json!("completed")
                && event["payload"]["verification"]["passed"] == json!(true)
        })
        .expect("completed verification");
    assert_eq!(
        completed["payload"]["verification"]["reason"],
        json!("The completion verifier did not return valid JSON.")
    );

    let read = conn.read_snapshot(&session_id).await;
    assert_eq!(read["session"]["target"]["status"], json!("complete"));
    assert_eq!(
        read["runtime"]["goalVerifications"][0]["reason"],
        json!("The completion verifier did not return valid JSON.")
    );
    assert_eq!(
        read["runtime"]["goalVerificationTimeline"][0]["status"],
        json!("completed")
    );
}

// ---------------------------------------------------------------------------
// 9. clear：删行、保留时间线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clear_deletes_target_row_and_keeps_verification_timeline() {
    let (provider, _requests) = GoalProvider::new("goal answer");
    let fixture = fixture("clear", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Clear goal", "inputId": "goal_clear_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn.drain_until_goal_status("complete").await;
    let before = conn.read_snapshot(&session_id).await;
    assert_eq!(
        before["runtime"]["goalVerificationTimeline"]
            .as_array()
            .expect("timeline")
            .len(),
        1
    );

    let clear = goal_action(&mut conn, &session_id, "clear", json!({})).await;
    assert_eq!(clear["response"], json!("Goal cleared."));
    assert_eq!(clear["startedTurn"], json!(false));
    assert_eq!(clear["snapshot"]["session"]["target"], Value::Null);
    let events = conn
        .drain_until(|event| event["payload"]["action"] == json!("cleared"))
        .await;
    let cleared = events.last().expect("cleared");
    assert_eq!(cleared["payload"]["target"], Value::Null);
    assert!(cleared["payload"]["previousTarget"]["targetID"].is_string());

    assert_eq!(count_rows(&fixture.db_path, "session_target"), 0);
    let read = conn.read_snapshot(&session_id).await;
    assert_eq!(read["session"]["target"], Value::Null);
    assert_eq!(read["projection"]["target"], Value::Null);
    assert_eq!(
        read["runtime"]["goalVerificationTimeline"]
            .as_array()
            .expect("timeline")
            .len(),
        1,
        "clear 不删除 runtime 验证历史"
    );
    let show = goal_action(&mut conn, &session_id, "show", json!({})).await;
    assert_eq!(
        show["response"],
        json!("No goal is set. Use /goal <objective> to set one.")
    );
}

// ---------------------------------------------------------------------------
// 10. 冷恢复：complete 保留账务/时间线，无 idle turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cold_restart_complete_preserves_accounting_without_idle_turn() {
    let (provider, requests) = GoalProvider::new("goal answer");
    let fixture = fixture("cold-complete", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Complete restart goal", "inputId": "goal_cr_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn.drain_until_goal_status("complete").await;
    let before = conn.read_snapshot(&session_id).await;
    let tokens_before = before["session"]["target"]["tokensUsed"].clone();
    let time_before = before["session"]["target"]["timeUsedSeconds"].clone();
    assert_eq!(tokens_before, json!(22));
    assert!(time_before.as_u64().unwrap_or(0) >= 1);
    let request_count_before = requests.lock().expect("requests").len();

    close_session(&mut conn, &session_id).await;
    let resume = resume_session(&mut conn, &session_id).await;
    assert_eq!(resume["session"]["target"]["status"], json!("complete"));
    assert_eq!(resume["session"]["target"]["tokensUsed"], tokens_before);
    assert_eq!(
        resume["session"]["target"]["timeUsedSeconds"], time_before,
        "冷恢复保留 timeUsedSeconds"
    );
    assert_eq!(resume["goalStats"]["tokensUsed"], tokens_before);
    assert_eq!(resume["goalStats"]["timeUsedSeconds"], time_before);
    assert_eq!(resume["goalStats"]["iterationCount"], json!(1));
    assert_eq!(
        resume["runtime"]["goalVerificationTimeline"]
            .as_array()
            .expect("timeline")
            .len(),
        1
    );
    // 登记差异（同 C2 snapshot 测试）：oracle 的 resume 后 eventSeq 是 2（物化产生
    // 事件），Rust runtime 冷恢复不产出事件 → 0（agent-c3-contract.md §1.11）。
    assert_eq!(resume["runtime"]["eventSeq"], json!(0));
    assert_eq!(resume["runtime"]["stateRevision"], json!(0));

    // 无自动 turn：等待窗口内没有事件、provider 请求数不变。
    let no_event = tokio::time::timeout(Duration::from_millis(200), conn.next_frame()).await;
    assert!(no_event.is_err(), "冷恢复后不得自动开 turn");
    assert_eq!(
        requests.lock().expect("requests").len(),
        request_count_before,
        "冷恢复后不得重放已消费 turn"
    );
    let show = goal_action(&mut conn, &session_id, "show", json!({})).await;
    assert!(show["response"]
        .as_str()
        .expect("response")
        .starts_with("Goal complete\n"));
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("complete"));
    assert_eq!(target["tokensUsed"], json!(22));
}

// ---------------------------------------------------------------------------
// 11. 冷恢复：active（进程被杀）→ paused、清 active 字段、无 idle turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cold_restart_active_target_becomes_paused_without_idle_turn() {
    let (provider, requests) = GoalProvider::new("slow active restart");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_gate(Arc::clone(&gate));
    let fixture = fixture("cold-active", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;
    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Active restart goal", "inputId": "goal_active_restart_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["type"] == json!("model.streaming"))
        .await;
    fixture.provider.entered().notified().await;
    close_session(&mut conn, &session_id).await;
    gate.add_permits(1);
    let request_count_before = requests.lock().expect("requests").len();

    // 模拟进程在 continuation 中被杀：DB 行仍是 active + active run 字段。
    set_target_active_for_cold_kill(&fixture.db_path, &session_id);
    // 冷读（无 runtime）也直接从 DB 投影，并把 active→paused 落库。
    let cold_read = conn.read_snapshot(&session_id).await;
    assert_eq!(cold_read["session"]["target"]["status"], json!("paused"));
    assert_eq!(cold_read["projection"]["target"]["status"], json!("paused"));
    assert_eq!(cold_read["session"]["target"]["activeInputId"], Value::Null);
    let cold_row = target_row(&fixture.db_path, &session_id).expect("cold target row");
    assert_eq!(
        cold_read["session"]["target"]["updatedAt"].as_u64(),
        cold_row["timeUpdated"].as_u64(),
        "冷读返回的是落库后的 time_updated"
    );
    let resume = resume_session(&mut conn, &session_id).await;
    assert_eq!(resume["session"]["target"]["status"], json!("paused"));
    assert_eq!(resume["session"]["target"]["activeInputId"], Value::Null);
    assert_eq!(
        resume["session"]["target"]["activeRunStartedAtMs"],
        Value::Null
    );
    assert_eq!(
        resume["session"]["target"]["activeRunLastSeenAtMs"],
        Value::Null
    );
    assert_eq!(resume["goalStats"]["tokensUsed"], json!(0));

    let no_event = tokio::time::timeout(Duration::from_millis(200), conn.next_frame()).await;
    assert!(no_event.is_err(), "active→paused 冷恢复不得自动续跑");
    assert_eq!(
        requests.lock().expect("requests").len(),
        request_count_before
    );
    let target = target_row(&fixture.db_path, &session_id).expect("target row");
    assert_eq!(target["status"], json!("paused"));
    assert_eq!(target["activeInputId"], Value::Null);
    let show = goal_action(&mut conn, &session_id, "show", json!({})).await;
    assert!(show["response"]
        .as_str()
        .expect("response")
        .starts_with("Goal paused\n"));
}

// ---------------------------------------------------------------------------
// 12. 冷恢复 paused → 显式 resume 后完成
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cold_restart_paused_resumes_explicitly_and_completes() {
    let (provider, _requests) = GoalProvider::new("never requested");
    let pre_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = provider.with_pre_gate(Arc::clone(&pre_gate));
    let fixture = fixture("cold-paused", provider);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let set = goal_action(
        &mut conn,
        &session_id,
        "set",
        json!({ "objective": "Restart goal", "inputId": "goal_restart_set" }),
    )
    .await;
    assert_eq!(set["startedTurn"], json!(true));
    let _ = conn
        .drain_until(|event| event["payload"]["action"] == json!("run_started"))
        .await;
    fixture.provider.pre_entered().notified().await;
    let pause = goal_action(&mut conn, &session_id, "pause", json!({})).await;
    assert_eq!(
        pause["snapshot"]["session"]["target"]["status"],
        json!("paused")
    );
    pre_gate.add_permits(1);
    let _ = conn.drain_until_terminal().await;
    let _ = wait_idle(&mut conn, &session_id).await;

    close_session(&mut conn, &session_id).await;
    let resume = resume_session(&mut conn, &session_id).await;
    assert_eq!(resume["session"]["target"]["status"], json!("paused"));
    assert_eq!(resume["goalStats"]["iterationCount"], json!(1));
    let show = goal_action(&mut conn, &session_id, "show", json!({})).await;
    assert!(show["response"]
        .as_str()
        .expect("response")
        .starts_with("Goal paused\n"));

    let resumed = goal_action(
        &mut conn,
        &session_id,
        "resume",
        json!({ "inputId": "goal_resume_after_restart" }),
    )
    .await;
    assert_eq!(resumed["startedTurn"], json!(true));
    let response = resumed["response"].as_str().expect("response");
    assert!(response.starts_with("Goal resumed\nObjective: Restart goal\n"));
    let _ = conn.drain_until_goal_status("complete").await;
    let read = conn.read_snapshot(&session_id).await;
    assert_eq!(read["session"]["target"]["status"], json!("complete"));
    assert_eq!(read["session"]["target"]["tokensUsed"], json!(22));
    assert_eq!(
        read["runtime"]["goalVerificationTimeline"]
            .as_array()
            .expect("timeline")
            .len(),
        1
    );
}
