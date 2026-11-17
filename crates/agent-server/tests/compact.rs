//! C2 T2a：`session/compact` 正常路径（受理 / 单飞入口 / 摘要落库 / 成功应答）。
//!
//! 行为依据：`docs/specs/agent-c2-contract.md` §1.1/§1.3/§1.4 与机器录制的夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/compact-manual.json`、
//! `compact-no-instructions.json`（只读，不修改）。
//!
//! 覆盖：
//! - 带 instructions：受理应答形状、`turn.started` / `turn.completed`、
//!   `message`/`part`/`turn_usage`/`model_usage` 行、`stateRevision` 递增；
//! - 不带 instructions：`turn.started.input == "/compact"`、`customInstructions:false`；
//! - `expectedRevision` 冲突：`-32009`，零事件零 DB 写；
//! - 长历史 turn 不触发自动 compact（T0 未观察到该行为）。
//!
//! 并发第二个 / 失败形状属 T2b，这里只覆盖「单飞判定先于入队」的显式拒绝面。
//!
//! 测试库 DDL 从 `scripts/sqlite-schema-baseline.json` 生成（与 methods/runtime 测试
//! 同一口径）；provider 是脚本化 fake，端点指向保留端口，绝不发起真实网络请求。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse,
    ModelUsage, ProviderError, ScriptStep, ScriptedProvider,
};
use agent_server::{
    run_stdio, AgentServer, Frame, ProviderConfigHub, ProviderSource, RequestId, ServerConfig,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

// ---------------------------------------------------------------------------
// 测试库夹具（与 tests/methods.rs 同一口径）
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
            "agent-server-compact-{label}-{}-{}",
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

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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

fn count_where(path: &Path, table: &str, where_sql: &str) -> i64 {
    raw_connection(path)
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE {where_sql}"),
            [],
            |row| row.get(0),
        )
        .expect("统计条件行数")
}

/// 八张紧凑观察表的行数快照（契约 §1.1：revision 冲突要求逐表相等）。
fn table_counts(path: &Path) -> BTreeMap<&'static str, i64> {
    [
        "session",
        "message",
        "part",
        "session_input",
        "session_entry",
        "tool_usage",
        "turn_usage",
        "model_usage",
    ]
    .into_iter()
    .map(|table| (table, count_rows(path, table)))
    .collect()
}

fn message_rows(path: &Path) -> Vec<(String, Value)> {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare(
            "SELECT id, data FROM message \
             ORDER BY COALESCE(sequence, -1), time_created, id",
        )
        .expect("准备查询");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("查询 messages");
    rows.map(|row| {
        let (id, data) = row.expect("读取 message");
        (
            id,
            serde_json::from_str(&data).expect("message data 是 JSON 文本"),
        )
    })
    .collect()
}

fn part_rows(path: &Path) -> Vec<(String, Value)> {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare(
            "SELECT p.message_id, p.data FROM part p JOIN message m ON m.id = p.message_id \
             ORDER BY COALESCE(m.sequence, -1), m.time_created, m.id, \
                      COALESCE(p.sequence, -1), p.time_created, p.id",
        )
        .expect("准备查询");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("查询 parts");
    rows.map(|row| {
        let (message_id, data) = row.expect("读取 part");
        (
            message_id,
            serde_json::from_str(&data).expect("part data 是 JSON 文本"),
        )
    })
    .collect()
}

fn session_column(path: &Path, session_id: &str, column: &str) -> Option<i64> {
    raw_connection(path)
        .query_row(
            &format!("SELECT {column} FROM session WHERE id = ?1"),
            [session_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .expect("读取 session 列")
}

fn turn_usage_row(path: &Path, turn_id: &str) -> Value {
    raw_connection(path)
        .query_row(
            "SELECT status, user_message_id, model_request_count, input_tokens, \
                    output_tokens, computed_total_tokens, error_type, error_code, trace_id \
             FROM turn_usage WHERE turn_id = ?1",
            [turn_id],
            |row| {
                Ok(json!({
                    "status": row.get::<_, String>(0)?,
                    "userMessageId": row.get::<_, Option<String>>(1)?,
                    "modelRequestCount": row.get::<_, i64>(2)?,
                    "inputTokens": row.get::<_, i64>(3)?,
                    "outputTokens": row.get::<_, i64>(4)?,
                    "computedTotalTokens": row.get::<_, i64>(5)?,
                    "errorType": row.get::<_, Option<String>>(6)?,
                    "errorCode": row.get::<_, Option<String>>(7)?,
                    "traceId": row.get::<_, Option<String>>(8)?,
                }))
            },
        )
        .expect("读取 compact turn_usage")
}

fn compact_model_usage_row(path: &Path) -> Value {
    raw_connection(path)
        .query_row(
            "SELECT id, query_source, assistant_message_id, parent_user_message_id, \
                    finish_reason, status, input_tokens, output_tokens, computed_total_tokens, \
                    provider_total_tokens, tool_call_count, raw_usage_json, provider_metadata_json, \
                    variant, agent, mode, task_type, attempt_index \
             FROM model_usage WHERE query_source = 'compact'",
            [],
            |row| {
                let raw_usage: Option<String> = row.get(11)?;
                let metadata: Option<String> = row.get(12)?;
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "querySource": row.get::<_, String>(1)?,
                    "assistantMessageId": row.get::<_, Option<String>>(2)?,
                    "parentUserMessageId": row.get::<_, Option<String>>(3)?,
                    "finishReason": row.get::<_, Option<String>>(4)?,
                    "status": row.get::<_, String>(5)?,
                    "inputTokens": row.get::<_, i64>(6)?,
                    "outputTokens": row.get::<_, i64>(7)?,
                    "computedTotalTokens": row.get::<_, i64>(8)?,
                    "providerTotalTokens": row.get::<_, Option<i64>>(9)?,
                    "toolCallCount": row.get::<_, i64>(10)?,
                    "rawUsage": raw_usage.map(|text| serde_json::from_str::<Value>(&text).expect("raw_usage_json")),
                    "providerMetadata": metadata.map(|text| serde_json::from_str::<Value>(&text).expect("provider_metadata_json")),
                    "variant": row.get::<_, Option<String>>(13)?,
                    "agent": row.get::<_, Option<String>>(14)?,
                    "mode": row.get::<_, Option<String>>(15)?,
                    "taskType": row.get::<_, Option<String>>(16)?,
                    "attemptIndex": row.get::<_, i64>(17)?,
                }))
            },
        )
        .expect("读取 compact model_usage")
}

// ---------------------------------------------------------------------------
// provider 夹具：临时配置 + 记录请求的脚本化 provider
// ---------------------------------------------------------------------------

fn write_personal_file(root: &Path) -> PathBuf {
    let path = root.join("provider-config.json");
    let model_rules = json!({
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
    });
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
                "providerModelRules": [model_rules],
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

/// 记录每次模型请求的 provider 包装（断言 compact 请求体与「没有额外请求」）。
struct RecordingProvider {
    inner: Arc<dyn ModelProvider>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingProvider {
    fn new(inner: Arc<dyn ModelProvider>) -> Self {
        Self {
            inner,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ModelProvider for RecordingProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        self.inner.stream(request, deltas)
    }
}

/// 长历史用例的 provider：每次请求返回固定长正文，并记录请求。
struct LongTextProvider {
    answer: String,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ModelProvider for LongTextProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        let answer = self.answer.clone();
        Box::pin(async move {
            deltas
                .send(ModelDelta::Text { text: answer })
                .map_err(|_| ProviderError {
                    message: "delta sink closed".to_string(),
                })?;
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
// stdio 测试连接（duplex）
// ---------------------------------------------------------------------------

struct TestConn {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: u64,
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

    /// 等下一帧 `session/event` 通知（跳过其他帧）。
    async fn next_event(&mut self) -> Value {
        loop {
            if let Frame::Notification(notification) = self.next_frame().await {
                assert_eq!(notification.method, "session/event");
                return notification.params.expect("事件通知必须带 params");
            }
        }
    }

    /// 收集事件直到 `turn.completed` / `turn.failed`（按 seq 单调）。
    async fn drain_until_terminal(&mut self) -> Vec<Value> {
        let mut events = Vec::new();
        loop {
            let event = self.next_event().await;
            let terminal = matches!(
                event["type"].as_str(),
                Some("turn.completed") | Some("turn.failed")
            );
            events.push(event);
            if terminal {
                return events;
            }
        }
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

struct Fixture {
    server: Arc<AgentServer>,
    db_path: PathBuf,
    ws: PathBuf,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    _root: TempRoot,
}

fn fixture(label: &str, steps: Vec<ScriptStep>) -> Fixture {
    let provider = Arc::new(RecordingProvider::new(Arc::new(ScriptedProvider::new(
        steps,
    ))));
    let requests = Arc::clone(&provider.requests);
    fixture_with_model(label, provider, requests)
}

/// 构造夹具：`provider` 已经记录自己的请求，`requests` 是同一份记录。
fn fixture_with_model(
    label: &str,
    provider: Arc<dyn ModelProvider>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
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
    let server = Arc::new(AgentServer::with_parts(
        config,
        hub,
        Arc::new(StaticSource { provider }),
    ));
    Fixture {
        server,
        db_path,
        ws,
        requests,
        _root: root,
    }
}

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("用例超时（10s）")
}

/// create + subscribe，返回 sessionId（oracle 生命周期：create 后立即 subscribe）。
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

async fn send_turn(conn: &mut TestConn, session_id: &str, content: &str) -> Vec<Value> {
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": content }),
        )
        .await;
    conn.result(id).await;
    conn.drain_until_terminal().await
}

fn event_types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["type"].as_str().expect("type"))
        .collect()
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().expect("对象").keys().cloned().collect();
    keys.sort();
    keys
}

fn requested_requests(fixture: &Fixture) -> Vec<ModelRequest> {
    fixture.requests.lock().expect("requests").clone()
}

// 观察值：摘要续接文本（契约 §1.3；落库数据契约，逐字来自夹具）。
const SUMMARY_PREFIX: &str = "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n";
const SUMMARY_SUFFIX: &str = "\nContinue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, do not preface with \"I'll continue\" or similar. Pick up the last task as if the break never happened.";

// ---------------------------------------------------------------------------
// 1. 正常 compact（带 instructions）：应答 / 事件 / DB / stateRevision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compact_with_instructions_accepts_persists_and_advances_revision() {
    let summary = "the model summary body";
    let instructions = "Focus on decisions and open questions; keep identifiers verbatim.";
    let fixture = fixture(
        "manual",
        vec![
            ScriptStep::Text("first turn answer".to_string()),
            ScriptStep::Text("second turn answer".to_string()),
            ScriptStep::Text(summary.to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    // 两个 turn 作为 compact 的历史。
    let first = send_turn(&mut conn, &session_id, "first prompt").await;
    assert_eq!(
        event_types(&first),
        vec!["turn.started", "model.streaming", "turn.completed"]
    );
    let second = send_turn(&mut conn, &session_id, "second prompt").await;
    assert_eq!(
        event_types(&second),
        vec!["turn.started", "model.streaming", "turn.completed"]
    );

    let before = conn.read_snapshot(&session_id).await;
    let revision_before = before["runtime"]["stateRevision"]
        .as_u64()
        .expect("stateRevision");
    let event_seq_before = before["runtime"]["eventSeq"].as_u64().expect("eventSeq");
    let turn_count_before = before["projection"]["turnCount"]
        .as_u64()
        .expect("turnCount");
    let total_tokens_before = before["projection"]["totalTokenCount"]
        .as_u64()
        .expect("totalTokenCount");
    let session_updated_before =
        session_column(&fixture.db_path, &session_id, "time_updated").expect("time_updated");
    let counts_before = table_counts(&fixture.db_path);
    assert_eq!(counts_before["session_input"], 0);
    assert_eq!(counts_before["session_entry"], 2);

    let input_id = "input_compact_manual_1";
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": input_id,
                "instructions": instructions,
                "expectedRevision": revision_before,
            }),
        )
        .await;
    let result = conn.result(id).await;

    // —— 受理应答形状（契约 §1.1）——
    assert_eq!(result["response"], json!(""));
    assert_eq!(
        sorted_keys(&result["compact"]),
        vec!["inputId", "state"],
        "compact.operationId 未观察到，必须省略"
    );
    assert_eq!(result["compact"]["state"], json!("accepted"));
    assert_eq!(result["compact"]["inputId"], json!(input_id));
    assert_eq!(
        sorted_keys(&result),
        vec!["compact", "response", "snapshot"],
        "顶层固定 response/snapshot/compact"
    );
    // 受理即推进 revision；快照是受理时刻（compact 尚未执行）。
    assert_eq!(
        result["snapshot"]["runtime"]["stateRevision"],
        json!(revision_before + 1)
    );
    assert_eq!(
        result["snapshot"]["projection"]["turnCount"],
        json!(turn_count_before)
    );
    assert!(
        result["snapshot"]["runtime"].get("activeTurnId").is_none(),
        "accepted 应答不应带执行中的 activeTurnId"
    );

    // —— 事件：compact 的 Rust 可产出子集（session.updated 类事件见契约注记）——
    let events = conn.drain_until_terminal().await;
    assert_eq!(event_types(&events), vec!["turn.started", "turn.completed"]);
    let started = &events[0]["payload"];
    assert_eq!(
        sorted_keys(started),
        vec!["input", "inputId", "inputVisibility", "turnNumber"],
        "compact 的 turn.started 载荷与夹具逐键一致"
    );
    assert_eq!(started["turnNumber"], json!(2));
    assert_eq!(started["input"], json!(format!("/compact {instructions}")));
    assert_eq!(started["inputId"], json!(input_id));
    assert_eq!(started["inputVisibility"], json!("model-only"));
    let turn_id = events[0]["turnId"].as_str().expect("turnId").to_string();

    let completed = &events[1]["payload"];
    assert_eq!(completed["response"], json!("Compacted"));
    assert_eq!(completed["tokenCount"], json!(22));
    assert_eq!(completed["resultType"], json!("success"));
    assert_eq!(completed["historyRoundCount"], json!(1));
    assert_eq!(completed["toolCallCount"], json!(0));
    assert_eq!(completed["inputId"], json!(input_id));
    assert_eq!(
        completed["usage"],
        json!({
            "source": "provider",
            "modelRequestCount": 1,
            "inputTokens": 10,
            "outputTokens": 12,
            "totalTokens": 22,
            "cacheReadTokens": 0,
            "cacheWriteTokens": 0,
            "reasoningTokens": 0,
            "webFetchRequests": 0,
            "webSearchRequests": 0,
        })
    );
    assert_eq!(
        completed["cacheStats"],
        json!({ "totalMessages": 6, "cachedMessages": 5, "lastCacheHit": false })
    );
    assert_eq!(
        events[1]["seq"].as_u64().expect("seq"),
        events[0]["seq"].as_u64().expect("seq") + 1,
        "turn.started 后只有一条终态事件"
    );

    // —— stateRevision：受理 +1，终态再 +1（每次接受/终态各推进一次）——
    let after = conn.read_snapshot(&session_id).await;
    assert_eq!(
        after["runtime"]["stateRevision"],
        json!(revision_before + 2)
    );
    assert_eq!(after["runtime"]["eventSeq"], json!(event_seq_before + 2));
    // contextUsage 的 used 来自 compact 请求的 provider total（最近一次请求）。
    assert_eq!(after["runtime"]["contextUsage"]["used"], json!(22));
    assert_eq!(
        after["projection"]["totalTokenCount"],
        json!(total_tokens_before + 22),
        "compact 的 tokenCount 同样进入投影累计"
    );

    // —— DB：message +2 / part +4 / turn_usage +1 / model_usage +1 ——
    let counts_after = table_counts(&fixture.db_path);
    for table in ["session", "session_entry", "session_input", "tool_usage"] {
        assert_eq!(
            counts_after[table], counts_before[table],
            "{table} 行数不应变化"
        );
    }
    assert_eq!(counts_after["message"], counts_before["message"] + 2);
    assert_eq!(counts_after["part"], counts_before["part"] + 4);
    assert_eq!(counts_after["turn_usage"], counts_before["turn_usage"] + 1);
    assert_eq!(
        counts_after["model_usage"],
        counts_before["model_usage"] + 1
    );

    // session.time_compacting 观察值恒 null；time_updated 前进。
    assert_eq!(
        session_column(&fixture.db_path, &session_id, "time_compacting"),
        None,
        "time_compacting 的观察值窗口内恒 null"
    );
    let session_updated_after =
        session_column(&fixture.db_path, &session_id, "time_updated").expect("time_updated");
    assert!(
        session_updated_after >= session_updated_before,
        "compact 终态必须 touch session"
    );

    // —— message 行：timeline 消息 + 摘要消息（契约 §1.3）——
    let messages = message_rows(&fixture.db_path);
    assert_eq!(messages.len(), 6, "两轮 4 行 + timeline + 摘要");
    let (timeline_id, timeline) = &messages[messages.len() - 2];
    let (summary_message_id, summary_message) = &messages[messages.len() - 1];

    assert_eq!(timeline["role"], json!("assistant"));
    assert_eq!(timeline["finish"], json!("completed"));
    assert_eq!(
        timeline["semantics"],
        json!({
            "origin": "system",
            "kind": "timeline_event",
            "uiVisibility": "visible",
            "providerVisibility": "hidden",
            "transcriptVisibility": "visible"
        })
    );
    assert_eq!(
        timeline["tokens"],
        json!({
            "input": 0,
            "output": 0,
            "reasoning": 0,
            "cache": { "read": 0, "write": 0 }
        }),
        "timeline 行 tokens 全 0 且没有 total"
    );
    assert!(timeline.get("messageId").is_none(), "data 里没有 messageId");
    assert!(timeline.get("sequence").is_none(), "sequence 只在列上");
    assert_eq!(timeline["planEnabled"], json!(false));
    assert_eq!(timeline["cost"], json!(0));
    assert!(
        timeline["parentID"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "timeline 行 parentID 指向最后一条被摘要消息"
    );

    assert_eq!(summary_message["role"], json!("user"));
    assert_eq!(
        summary_message["summary"],
        json!({ "title": "Compact summary", "body": summary, "diffs": [] })
    );
    assert_eq!(
        summary_message["semantics"],
        json!({
            "origin": "agent_runtime",
            "kind": "compact_summary",
            "uiVisibility": "hidden",
            "providerVisibility": "visible",
            "transcriptVisibility": "hidden"
        })
    );
    assert_eq!(
        summary_message["modelSelection"],
        json!({ "providerId": "mock-chat", "modelId": "mock-1" })
    );
    assert_eq!(
        summary_message["model"],
        json!({ "providerID": "mock-chat", "modelID": "mock-1" })
    );
    assert!(
        summary_message["tools"].is_object(),
        "夹具的 tools 是对象（内容折叠为 <tool-flags>）"
    );
    assert_eq!(
        summary_message["time"]
            .as_object()
            .expect("time")
            .keys()
            .collect::<Vec<_>>(),
        vec!["created"],
        "摘要消息没有 time.completed"
    );

    // —— part 行：timeline 消息 2 行 + 摘要消息 2 行 ——
    let parts = part_rows(&fixture.db_path);
    let new_parts: Vec<(&String, &Value)> = parts[parts.len() - 4..]
        .iter()
        .map(|(message_id, data)| (message_id, data))
        .collect();
    assert_eq!(new_parts[0].0, timeline_id);
    assert_eq!(new_parts[1].0, timeline_id);
    assert_eq!(new_parts[2].0, summary_message_id);
    assert_eq!(new_parts[3].0, summary_message_id);
    let timeline_part = new_parts[0].1;
    let compaction_part = new_parts[1].1;
    let summary_text_part = new_parts[2].1;
    let summary_compaction_part = new_parts[3].1;

    assert_eq!(timeline_part["type"], json!("timeline"));
    assert_eq!(timeline_part["timelineType"], json!("context_compaction"));
    assert_eq!(timeline_part["display"], json!("separator"));
    assert_eq!(timeline_part["status"], json!("completed"));
    assert_eq!(timeline_part["trigger"], json!("manual"));
    assert_eq!(timeline_part["phase"], json!("standalone_turn"));
    assert_eq!(timeline_part["compactReason"], json!("user_requested"));
    assert_eq!(timeline_part["sourceCommandId"], json!(input_id));
    assert!(timeline_part["operationId"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(timeline_part["boundaryId"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(timeline_part["summaryMessageId"], json!(summary_message_id));
    assert_eq!(timeline_part["postCompactTokenCount"], json!(22));
    assert!(
        timeline_part["preCompactTokenCount"].as_u64().unwrap_or(0) > 0,
        "preCompactTokenCount 是本地历史估算"
    );
    assert!(
        timeline_part["truePostCompactTokenCount"]
            .as_u64()
            .unwrap_or(0)
            > 22
    );

    assert_eq!(compaction_part["type"], json!("compaction"));
    assert_eq!(compaction_part["auto"], json!(false));
    assert_eq!(compaction_part["trigger"], json!("manual"));
    assert_eq!(compaction_part["phase"], json!("standalone_turn"));
    assert_eq!(compaction_part["compactReason"], json!("user_requested"));
    assert_eq!(compaction_part["timelineStatus"], json!("completed"));
    assert_eq!(compaction_part["timelineDisplay"], json!("separator"));
    assert_eq!(compaction_part["replace"], json!(true));
    assert_eq!(compaction_part["boundaryId"], timeline_part["boundaryId"]);
    assert_eq!(
        compaction_part["summaryMessageId"],
        json!(summary_message_id)
    );
    assert_eq!(compaction_part["tail_start_id"], json!(summary_message_id));
    assert_eq!(
        compaction_part["operationId"], timeline_part["operationId"],
        "timeline / compaction part 共用同一 operationId"
    );

    assert_eq!(summary_text_part["type"], json!("text"));
    assert_eq!(summary_text_part["synthetic"], json!(true));
    assert_eq!(
        summary_text_part["text"],
        json!(format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}")),
        "摘要续接文本 = 固定前缀 + 模型正文 + 固定后缀"
    );

    assert_eq!(summary_compaction_part["type"], json!("compaction"));
    assert_eq!(summary_compaction_part["auto"], json!(false));
    assert_eq!(
        summary_compaction_part["tail_start_id"],
        json!(summary_message_id)
    );
    let boundary = summary_compaction_part["compactBoundary"]
        .as_object()
        .expect("compactBoundary");
    assert_eq!(boundary["boundaryId"], timeline_part["boundaryId"]);
    assert_eq!(boundary["trigger"], json!("manual"));
    assert_eq!(boundary["phase"], json!("standalone_turn"));
    assert_eq!(boundary["compactReason"], json!("user_requested"));
    assert_eq!(boundary["summarySource"], json!("model"));
    assert_eq!(
        boundary["preCompactTokenCount"],
        timeline_part["preCompactTokenCount"]
    );
    assert_eq!(boundary["postCompactTokenCount"], json!(22));
    assert_eq!(boundary["summarizedMessageCount"], json!(4));
    assert_eq!(boundary["keptMessageCount"], json!(0));
    assert_eq!(boundary["lastSummarizedMessageId"], timeline["parentID"]);
    assert_eq!(boundary["summaryMessageIds"], json!([summary_message_id]));
    assert_eq!(boundary["customInstructions"], json!(true));
    assert!(boundary["traceId"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(boundary["turnId"], json!(turn_id));

    // —— turn_usage / model_usage ——
    let usage = turn_usage_row(&fixture.db_path, &turn_id);
    assert_eq!(usage["status"], json!("completed"));
    assert_eq!(usage["userMessageId"], Value::Null);
    assert_eq!(usage["modelRequestCount"], json!(1));
    assert_eq!(usage["inputTokens"], json!(10));
    assert_eq!(usage["outputTokens"], json!(12));
    assert_eq!(usage["computedTotalTokens"], json!(22));
    assert_eq!(usage["errorType"], Value::Null);
    assert_eq!(usage["errorCode"], Value::Null);
    assert!(
        usage["traceId"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "turn_usage.trace_id 非空（夹具观察）"
    );

    let model_usage = compact_model_usage_row(&fixture.db_path);
    assert_eq!(model_usage["querySource"], json!("compact"));
    assert!(
        model_usage["id"]
            .as_str()
            .is_some_and(|value| value.starts_with("usage_model_compact_")),
        "model_usage.id 前缀 usage_model_compact_"
    );
    assert_eq!(model_usage["assistantMessageId"], Value::Null);
    assert_eq!(model_usage["parentUserMessageId"], Value::Null);
    assert_eq!(model_usage["finishReason"], json!("stop"));
    assert_eq!(model_usage["status"], json!("completed"));
    assert_eq!(model_usage["inputTokens"], json!(10));
    assert_eq!(model_usage["outputTokens"], json!(12));
    assert_eq!(model_usage["computedTotalTokens"], json!(22));
    assert_eq!(model_usage["providerTotalTokens"], json!(22));
    assert_eq!(model_usage["toolCallCount"], json!(0));
    assert_eq!(
        model_usage["rawUsage"],
        json!({
            "inputTokens": 10,
            "outputTokens": 12,
            "totalTokens": 22,
            "cacheReadTokens": 0,
            "cacheWriteTokens": 0,
        })
    );
    assert_eq!(
        model_usage["providerMetadata"],
        json!({ "rawFinishReason": "end_turn" })
    );
    assert_eq!(model_usage["variant"], json!("disabled"));
    assert_eq!(model_usage["agent"], json!("mode-agent"));
    assert_eq!(model_usage["mode"], json!("build"));
    assert_eq!(model_usage["taskType"], json!("interactive"));
    assert_eq!(model_usage["attemptIndex"], json!(0));

    // —— provider 可见的 compact 请求（契约 §1.4）——
    let requests = requested_requests(&fixture);
    assert_eq!(requests.len(), 3, "两轮 turn + 一次 compact = 3 次模型请求");
    let compact_request = &requests[2];
    assert_eq!(
        compact_request.messages.len(),
        6,
        "请求 = 系统提示 + 4 条历史 + 末尾摘要指令"
    );
    assert!(matches!(
        &compact_request.messages[0],
        ModelMessage::System(_)
    ));
    assert!(
        matches!(&compact_request.messages[1], ModelMessage::User(text) if text == "first prompt")
    );
    let ModelMessage::User(prompt) = &compact_request.messages[5] else {
        panic!("compact 请求最后一条必须是 user 摘要指令");
    };
    assert!(
        prompt.starts_with("CRITICAL: Respond with TEXT ONLY. Do NOT call any tools."),
        "摘要指令以观察到的接口性片段开头"
    );
    assert!(prompt.contains("<analysis>") && prompt.contains("<summary>"));
    assert!(
        prompt.contains(&format!("Additional Instructions:\n{instructions}")),
        "instructions 进入 compact 请求"
    );
    assert!(prompt.contains("REMINDER: Do NOT call any tools."));
    assert!(!compact_request.tools.is_empty(), "compact 复用会话工具面");
}

// ---------------------------------------------------------------------------
// 2. 不带 instructions：`/compact` + customInstructions:false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compact_without_instructions_uses_bare_command() {
    let summary = "plain summary body";
    let fixture = fixture(
        "plain",
        vec![
            ScriptStep::Text("first turn answer".to_string()),
            ScriptStep::Text(summary.to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;
    send_turn(&mut conn, &session_id, "first prompt").await;

    let before = conn.read_snapshot(&session_id).await;
    let revision_before = before["runtime"]["stateRevision"].as_u64().expect("rev");

    let input_id = "input_compact_plain_1";
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": input_id,
                "expectedRevision": revision_before,
            }),
        )
        .await;
    let result = conn.result(id).await;
    assert_eq!(result["response"], json!(""));
    assert_eq!(result["compact"]["state"], json!("accepted"));
    assert_eq!(result["compact"]["inputId"], json!(input_id));
    assert!(result["compact"].get("operationId").is_none());

    let events = conn.drain_until_terminal().await;
    assert_eq!(event_types(&events), vec!["turn.started", "turn.completed"]);
    assert_eq!(events[0]["payload"]["input"], json!("/compact"));
    assert_eq!(events[0]["payload"]["inputVisibility"], json!("model-only"));
    assert_eq!(events[1]["payload"]["response"], json!("Compacted"));
    assert_eq!(events[1]["payload"]["inputId"], json!(input_id));

    let messages = message_rows(&fixture.db_path);
    let summary_message = &messages.last().expect("摘要消息").1;
    assert_eq!(summary_message["summary"]["body"], json!(summary));

    let parts = part_rows(&fixture.db_path);
    let last_boundary = parts
        .iter()
        .rev()
        .find_map(|(_, data)| data.get("compactBoundary"))
        .expect("摘要 compaction part");
    assert_eq!(
        last_boundary["customInstructions"],
        json!(false),
        "无 instructions 时 customInstructions:false"
    );
    assert_eq!(last_boundary["summarizedMessageCount"], json!(2));

    let requests = requested_requests(&fixture);
    assert_eq!(requests.len(), 2, "一轮 turn + 一次 compact");
    let ModelMessage::User(prompt) = requests[1].messages.last().expect("摘要指令") else {
        panic!("compact 请求最后一条必须是 user 摘要指令");
    };
    assert!(!prompt.contains("Additional Instructions:"));
    assert!(prompt.starts_with("CRITICAL: Respond with TEXT ONLY. Do NOT call any tools."));

    // 终态再推进一次 revision。
    let after = conn.read_snapshot(&session_id).await;
    assert_eq!(
        after["runtime"]["stateRevision"],
        json!(revision_before + 2)
    );
}

// ---------------------------------------------------------------------------
// 3. expectedRevision 冲突：-32009，零事件零 DB 写
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compact_revision_conflict_has_zero_side_effects() {
    let fixture = fixture(
        "revision",
        vec![ScriptStep::Text("first turn answer".to_string())],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;
    send_turn(&mut conn, &session_id, "first prompt").await;

    let before = conn.read_snapshot(&session_id).await;
    let revision_before = before["runtime"]["stateRevision"].as_u64().expect("rev");
    let event_seq_before = before["runtime"]["eventSeq"].as_u64().expect("seq");
    let counts_before = table_counts(&fixture.db_path);

    // 未知键先撞严格解析（-32602），同样不应有任何副作用。
    let id = conn
        .send(
            "session/compact",
            json!({ "sessionId": session_id, "bogus": true }),
        )
        .await;
    let (code, message, data) = conn.error(id).await;
    assert_eq!(code, -32602);
    assert!(message.contains("参数不合法"), "实际：{message}");
    assert_eq!(data, None);

    // 过期 revision：错误应答 + 结构化 data（契约 §1.1）。
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": "input_compact_stale",
                "expectedRevision": revision_before + 1,
            }),
        )
        .await;
    let (code, message, data) = conn.error(id).await;
    assert_eq!(code, -32009);
    assert_eq!(message, "Session state revision mismatch");
    assert_eq!(
        data,
        Some(json!({
            "actualRevision": revision_before,
            "expectedRevision": revision_before + 1,
        }))
    );

    // 零事件、零 DB 写、revision 不动。
    let after = conn.read_snapshot(&session_id).await;
    assert_eq!(after["runtime"]["stateRevision"], json!(revision_before));
    assert_eq!(after["runtime"]["eventSeq"], json!(event_seq_before));
    assert_eq!(
        table_counts(&fixture.db_path),
        counts_before,
        "八张观察表行数逐表不变"
    );
    assert_eq!(
        session_column(&fixture.db_path, &session_id, "time_compacting"),
        None
    );
    assert_eq!(requested_requests(&fixture).len(), 1, "冲突不发起模型请求");
}

// ---------------------------------------------------------------------------
// 4. 长历史 turn 不自动 compact（T0 未观察到自动触发）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn long_history_turn_does_not_auto_compact() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(LongTextProvider {
        answer: "x".repeat(40_000),
        requests: Arc::clone(&requests),
    });
    let fixture = fixture_with_model("long-history", provider, Arc::clone(&requests));
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    // 5 轮 × (10k prompt + 40k answer) ≈ 250k 字符历史（远大于任何合理阈值观察）。
    for turn in 0..5 {
        let prompt = format!("prompt {turn}: {}", "p".repeat(10_000));
        let events = send_turn(&mut conn, &session_id, &prompt).await;
        assert_eq!(
            event_types(&events),
            vec!["turn.started", "model.streaming", "turn.completed"],
            "turn {turn} 只有普通事件，没有 compact 信号"
        );
        assert!(
            events
                .iter()
                .all(|event| event["payload"]["input"].as_str() != Some("/compact")),
            "没有 compact turn 开始"
        );
    }

    // 每个 turn 恰好一次模型请求：没有隐藏的自动 compact 请求。
    assert_eq!(
        requests.lock().expect("requests").len(),
        5,
        "5 轮 turn = 5 次模型请求"
    );

    // 未观察到自动 compact（契约 §2）：0 个 compaction part / 摘要消息。
    let parts = part_rows(&fixture.db_path);
    assert_eq!(
        parts
            .iter()
            .filter(|(_, data)| data["type"] == json!("compaction"))
            .count(),
        0,
        "长历史不产生 compaction part"
    );
    let messages = message_rows(&fixture.db_path);
    assert_eq!(
        messages
            .iter()
            .filter(|(_, data)| data["semantics"]["kind"] == json!("compact_summary"))
            .count(),
        0,
        "长历史不产生 compact_summary 消息"
    );
    assert_eq!(
        count_where(
            &fixture.db_path,
            "model_usage",
            "query_source != 'main_turn'"
        ),
        0,
        "没有 compact 模型的 usage 行"
    );
    assert_eq!(
        count_where(
            &fixture.db_path,
            "message",
            "data LIKE '%context_compaction%'"
        ),
        0,
        "没有 context_compaction timeline"
    );
    assert_eq!(
        session_column(&fixture.db_path, &session_id, "time_compacting"),
        None
    );
}
