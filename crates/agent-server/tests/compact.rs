//! C2 T2a/T2b：`session/compact` 的受理、单飞、成功/跳过/失败落库与 resume 边界。
//!
//! 行为依据：`docs/specs/agent-c2-contract.md` §1.1–§1.9 与机器录制的夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/`（只读，不修改）。
//!
//! 覆盖：
//! - 带 instructions：受理应答形状、`turn.started` / `turn.completed`、
//!   `message`/`part`/`turn_usage`/`model_usage` 行、`stateRevision` 递增；
//! - 不带 instructions：`turn.started.input == "/compact"`、`customInstructions:false`；
//! - `expectedRevision` 冲突：`-32009`，零事件零 DB 写；
//! - 并发：执行中第二个 → `already_running`；排队中第二个 → `accepted` 后 `skipped`；
//! - 失败：provider tool-call / 空文本 → `turn.failed` 与夹具失败行；
//! - 无模型 compact：`CONFIGURATION_ERROR` 终态且状态不留在 Running；
//! - 长历史 turn 不触发自动 compact（T0 的 15 × 100k 字符规模）；
//! - resume 边界：只恢复最近一次成功 compact 的摘要续接 + 之后的消息。
//!
//! 测试库 DDL 从 `scripts/sqlite-schema-baseline.json` 生成（与 methods/runtime 测试
//! 同一口径）；provider 是脚本化 fake，端点指向保留端口，绝不发起真实网络请求。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse,
    ModelUsage, ProviderError, ScriptStep, ScriptToolCall, ScriptedProvider,
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

/// compact 摘要指令的固定开头（识别 compact 请求，不依赖脚本下标）。
const COMPACT_PROMPT_PREFIX: &str = "CRITICAL: Respond with TEXT ONLY.";

fn is_compact_request(request: &ModelRequest) -> bool {
    matches!(
        request.messages.last(),
        Some(ModelMessage::User(text)) if text.starts_with(COMPACT_PROMPT_PREFIX)
    )
}

/// 门控 provider：compact 请求在 `gate` 上等待许可，普通 turn 直接透传。
///
/// 用在 `already_running` 用例：先收到 compact 的 `turn.started`（相位已切 Running），
/// 再发第二个 compact，保证单飞判定不依赖调度竞态。
struct GatedCompactProvider {
    inner: Arc<dyn ModelProvider>,
    gate: Arc<tokio::sync::Semaphore>,
}

impl ModelProvider for GatedCompactProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        if !is_compact_request(&request) {
            return self.inner.stream(request, deltas);
        }
        let gate = Arc::clone(&self.gate);
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let _permit = gate.acquire().await.map_err(|_| ProviderError {
                message: "compact gate closed".to_string(),
            })?;
            inner.stream(request, deltas).await
        })
    }
}

/// 门控 provider：**prompt** 请求在 `gate` 上等待许可（compact 请求不会到达 provider——
/// 运行中受理即拒绝），用于把 prompt turn 钉在 Running 相位
/// （`-32010 Cannot compact while a prompt is running` 的判定窗口，契约 §1.6；
/// fixture `compact-while-turn-running`）。
struct GatedPromptProvider {
    inner: Arc<dyn ModelProvider>,
    gate: Arc<tokio::sync::Semaphore>,
}

impl ModelProvider for GatedPromptProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        if is_compact_request(&request) {
            return self.inner.stream(request, deltas);
        }
        let gate = Arc::clone(&self.gate);
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let _permit = gate.acquire().await.map_err(|_| ProviderError {
                message: "prompt gate closed".to_string(),
            })?;
            inner.stream(request, deltas).await
        })
    }
}

/// provider **解析**闸门：阻塞 `ProviderSource::resolve`，把 worker 停在
/// 「已取到 compact、相位仍是 Queued」的窗口里（`already_running` 判定尚未发生）。
///
/// `armed` 只应在主 turn 完成后打开；`cache_key` 的代际切换强制 compact 重新解析
/// provider（默认实现会复用已缓存的 provider，窗口不可观察）。
struct ResolveGate {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: Mutex<bool>,
    condvar: Condvar,
    armed: AtomicBool,
}

impl ResolveGate {
    fn new() -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        (
            Self {
                entered: Mutex::new(Some(sender)),
                released: Mutex::new(false),
                condvar: Condvar::new(),
                armed: AtomicBool::new(false),
            },
            receiver,
        )
    }

    fn wait(&self) {
        let mut released = self.released.lock().expect("gate released");
        while !*released {
            released = self.condvar.wait(released).expect("gate wait");
        }
    }

    fn release(&self) {
        *self.released.lock().expect("gate released") = true;
        self.condvar.notify_all();
    }
}

struct ResolveGateSource {
    provider: Arc<dyn ModelProvider>,
    generation: Mutex<String>,
    gate: Arc<ResolveGate>,
}

impl ProviderSource for ResolveGateSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        if self.gate.armed.load(Ordering::SeqCst) {
            if let Some(sender) = self.gate.entered.lock().expect("gate entered").take() {
                let _ = sender.send(());
            }
            self.gate.wait();
        }
        if provider_id != "mock-chat" {
            return Err(format!("Provider 不存在: {provider_id}"));
        }
        let provider: Arc<dyn ModelProvider> = Arc::clone(&self.provider);
        Ok(provider)
    }

    fn cache_key(&self) -> String {
        self.generation.lock().expect("generation").clone()
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

/// 无模型会话：create 允许，compact 的 turn 走 `CONFIGURATION_ERROR`。
fn create_params_without_model(ws: &Path) -> Value {
    let mut params = create_params(ws);
    params
        .as_object_mut()
        .expect("create params 对象")
        .remove("model");
    params
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
    fixture_with_source(label, Arc::new(StaticSource { provider }), requests)
}

fn fixture_with_source(
    label: &str,
    source: Arc<dyn ProviderSource>,
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
    let server = Arc::new(AgentServer::with_parts(config, hub, source));
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

async fn close_session(conn: &mut TestConn, session_id: &str) {
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    assert_eq!(conn.result(id).await, json!({ "closed": true }));
}

async fn resume_session(conn: &mut TestConn, session_id: &str) {
    let id = conn
        .send("session/resume", json!({ "sessionId": session_id }))
        .await;
    let snapshot = conn.result(id).await;
    assert_eq!(snapshot["session"]["sessionId"], json!(session_id));
    // close 后旧 runtime 的订阅已终止：resume 必须重新订阅。
    conn.subscribe_default(session_id).await;
}

/// 逐条读事件直到谓词命中（已消费的事件不回放）。
async fn next_event_until(conn: &mut TestConn, predicate: impl Fn(&Value) -> bool) -> Value {
    loop {
        let event = conn.next_event().await;
        if predicate(&event) {
            return event;
        }
    }
}

/// 收集事件直到第 `count` 个终态（多 compact 排队时用）。
async fn drain_until_terminals(conn: &mut TestConn, count: usize) -> Vec<Value> {
    let mut events = Vec::new();
    let mut terminals = 0;
    loop {
        let event = conn.next_event().await;
        let terminal = matches!(
            event["type"].as_str(),
            Some("turn.completed") | Some("turn.failed")
        );
        events.push(event);
        if terminal {
            terminals += 1;
            if terminals == count {
                return events;
            }
        }
    }
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

fn messages_with_kind(path: &Path, kind: &str) -> Vec<Value> {
    message_rows(path)
        .into_iter()
        .filter(|(_, data)| data["semantics"]["kind"] == json!(kind))
        .map(|(_, data)| data)
        .collect()
}

fn parts_with_type(path: &Path, part_type: &str) -> Vec<Value> {
    part_rows(path)
        .into_iter()
        .filter(|(_, data)| data["type"] == json!(part_type))
        .map(|(_, data)| data)
        .collect()
}

/// 等 worker 的终态收口完成（终态事件先于状态收口发布，跨线程窗口见 C1 注释）。
async fn wait_session_status(conn: &mut TestConn, session_id: &str, expected: &str) -> Value {
    for _ in 0..200 {
        let snapshot = conn.read_snapshot(session_id).await;
        if snapshot["session"]["status"] == json!(expected) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("session 状态未在 1s 内变为 {expected}");
}

/// 等 stateRevision 达到期望值（受理 +1 / 终态 +1 在同一临界区推进）。
async fn wait_state_revision(conn: &mut TestConn, session_id: &str, expected: u64) -> Value {
    for _ in 0..200 {
        let snapshot = conn.read_snapshot(session_id).await;
        if snapshot["runtime"]["stateRevision"] == json!(expected) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("stateRevision 未在 1s 内达到 {expected}");
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
    let after = wait_state_revision(&mut conn, &session_id, revision_before + 2).await;
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
    let after = wait_state_revision(&mut conn, &session_id, revision_before + 2).await;
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
        answer: "x".repeat(100_000),
        requests: Arc::clone(&requests),
    });
    let fixture = fixture_with_model("long-history", provider, Arc::clone(&requests));
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    // 对齐 T0 的观察规模（`auto-compact-long-history`）：15 轮 × 100k 字符 answer
    // ≈ 1.5M 字符历史；按 4 字符≈1 token 估算约 375k tokens，已超过 200k 窗口。
    // 该规模下 oracle 未触发自动 compact，Rust 同样不得自行启用阈值。
    for turn in 0..15 {
        let prompt = format!("prompt {turn}: {}", "p".repeat(1_000));
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
        15,
        "15 轮 turn = 15 次模型请求"
    );

    // 未观察到自动 compact（契约 §2）：0 个 compaction part / 摘要消息。
    assert_eq!(
        parts_with_type(&fixture.db_path, "compaction").len(),
        0,
        "长历史不产生 compaction part"
    );
    assert_eq!(
        messages_with_kind(&fixture.db_path, "compact_summary").len(),
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

// ---------------------------------------------------------------------------
// 5. 执行中的第二个 compact：already_running，不产生事件/行/revision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compact_while_running_returns_already_running_without_side_effects() {
    let scripted = ScriptedProvider::new(vec![
        ScriptStep::Text("normal answer".to_string()),
        ScriptStep::Text("summary while running".to_string()),
    ]);
    let recording = Arc::new(RecordingProvider::new(Arc::new(scripted)));
    let requests = Arc::clone(&recording.requests);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let inner: Arc<dyn ModelProvider> = Arc::clone(&recording) as Arc<dyn ModelProvider>;
    let provider = Arc::new(GatedCompactProvider {
        inner,
        gate: Arc::clone(&gate),
    });
    let fixture = fixture_with_source(
        "already-running",
        Arc::new(StaticSource { provider }),
        requests,
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;
    send_turn(&mut conn, &session_id, "first prompt").await;

    let before = conn.read_snapshot(&session_id).await;
    let revision_before = before["runtime"]["stateRevision"].as_u64().expect("rev");

    let first_input = "input_compact_running_1";
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": first_input,
                "expectedRevision": revision_before,
            }),
        )
        .await;
    let first = conn.result(id).await;
    assert_eq!(first["compact"]["state"], json!("accepted"));
    assert_eq!(first["compact"]["inputId"], json!(first_input));

    // compact 的 turn.started 在相位切 Running 之后发布：此时第二个请求必须
    // already_running（不依赖调度竞态）。
    let started = next_event_until(&mut conn, |event| event["type"] == json!("turn.started")).await;
    assert_eq!(started["payload"]["input"], json!("/compact"));
    let running = conn.read_snapshot(&session_id).await;
    assert_eq!(
        running["runtime"]["stateRevision"],
        json!(revision_before + 1)
    );
    assert!(
        running["runtime"]["activeTurnId"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "执行中的 compact 快照带 activeTurnId"
    );
    assert_eq!(running["runtime"]["activeTurnKind"], json!("compact"));
    assert_eq!(running["session"]["status"], json!("running"));

    let second_input = "input_compact_running_2";
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": second_input,
            }),
        )
        .await;
    let second = conn.result(id).await;
    assert_eq!(
        sorted_keys(&second["compact"]),
        vec!["inputId", "state"],
        "already_running 同样没有 operationId"
    );
    assert_eq!(second["compact"]["state"], json!("already_running"));
    assert_eq!(second["compact"]["inputId"], json!(second_input));
    assert_eq!(
        second["snapshot"]["runtime"]["stateRevision"],
        json!(revision_before + 1),
        "already_running 不推进 revision"
    );
    assert_eq!(
        second["snapshot"]["runtime"]["activeTurnKind"],
        json!("compact")
    );

    // 放行第一个 compact；第二个没有入队，只能看到第一个的终态。
    gate.add_permits(1);
    let rest = conn.drain_until_terminal().await;
    assert_eq!(event_types(&rest), vec!["turn.completed"]);
    assert_eq!(rest[0]["payload"]["response"], json!("Compacted"));

    let after = wait_state_revision(&mut conn, &session_id, revision_before + 2).await;
    assert_eq!(after["session"]["status"], json!("idle"));
    assert!(after["runtime"].get("activeTurnId").is_none());

    assert_eq!(
        requested_requests(&fixture).len(),
        2,
        "普通 turn + 一次 compact = 2 次模型请求；already_running 不发起第二次"
    );
    assert_eq!(
        messages_with_kind(&fixture.db_path, "compact_summary").len(),
        1
    );
    assert_eq!(parts_with_type(&fixture.db_path, "compaction").len(), 2);
    assert_eq!(count_rows(&fixture.db_path, "turn_usage"), 2);
    // 主 turn 与 skipped 都不写 model_usage；只有 compact 请求一行（C1 未持久化主 turn
    // 的 model_usage，登记在 C1 契约）。
    assert_eq!(count_rows(&fixture.db_path, "model_usage"), 1);
}

// ---------------------------------------------------------------------------
// 6. 排队中的第二个 compact：accepted → skipped（不产生第二份摘要）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn queued_second_compact_is_accepted_then_skipped() {
    let scripted = ScriptedProvider::new(vec![
        ScriptStep::Text("normal answer".to_string()),
        ScriptStep::Text("first compact summary".to_string()),
    ]);
    let recording = Arc::new(RecordingProvider::new(Arc::new(scripted)));
    let requests = Arc::clone(&recording.requests);
    let (gate, entered) = ResolveGate::new();
    let gate = Arc::new(gate);
    let provider_concrete = Arc::clone(&recording);
    let provider_dyn: Arc<dyn ModelProvider> = provider_concrete;
    let source = Arc::new(ResolveGateSource {
        provider: provider_dyn,
        generation: Mutex::new("generation-a".to_string()),
        gate: Arc::clone(&gate),
    });
    let source_dyn: Arc<dyn ProviderSource> = Arc::clone(&source) as Arc<dyn ProviderSource>;
    let fixture = fixture_with_source("skipped", source_dyn, requests);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;
    send_turn(&mut conn, &session_id, "first prompt").await;

    let before = conn.read_snapshot(&session_id).await;
    let revision_before = before["runtime"]["stateRevision"].as_u64().expect("rev");

    // 强制 compact 重新解析 provider，并把 worker 停在「已取到、相位仍 Queued」窗口。
    *source.generation.lock().expect("generation") = "generation-b".to_string();
    gate.armed.store(true, Ordering::SeqCst);

    let first_input = "input_compact_queued_1";
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": first_input,
                "expectedRevision": revision_before,
            }),
        )
        .await;
    let first = conn.result(id).await;
    assert_eq!(first["compact"]["state"], json!("accepted"));
    within(entered)
        .await
        .expect("worker 应进入 provider 解析窗口");

    let second_input = "input_compact_queued_2";
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": second_input,
            }),
        )
        .await;
    let second = conn.result(id).await;
    assert_eq!(sorted_keys(&second["compact"]), vec!["inputId", "state"]);
    assert_eq!(
        second["compact"]["state"],
        json!("accepted"),
        "相位 Queued 时第二个 compact 被受理"
    );
    assert_eq!(second["compact"]["inputId"], json!(second_input));
    assert_eq!(
        second["snapshot"]["runtime"]["stateRevision"],
        json!(revision_before + 2),
        "第二个受理同样推进 revision"
    );
    assert_eq!(
        requested_requests(&fixture).len(),
        1,
        "compact 尚未执行：只有主 turn 的请求"
    );

    gate.release();
    let events = drain_until_terminals(&mut conn, 2).await;
    assert_eq!(
        event_types(&events),
        vec![
            "turn.started",
            "turn.completed",
            "turn.started",
            "turn.completed"
        ],
        "第一个成功，第二个以 skipped 终态收口"
    );
    assert_eq!(events[0]["payload"]["input"], json!("/compact"));
    assert_eq!(events[0]["payload"]["inputId"], json!(first_input));
    assert_eq!(events[1]["payload"]["response"], json!("Compacted"));
    assert_eq!(events[2]["payload"]["input"], json!("/compact"));
    assert_eq!(events[2]["payload"]["inputId"], json!(second_input));

    assert_eq!(
        events[3]["payload"]["response"],
        json!("Context is up to date; no compression needed")
    );
    assert_eq!(events[3]["payload"]["resultType"], json!("success"));
    assert_eq!(events[3]["payload"]["inputId"], json!(second_input));
    assert!(events[3]["payload"]["tokenCount"].as_u64().unwrap_or(0) > 0);
    assert_eq!(
        events[3]["payload"]["cacheStats"],
        json!({ "totalMessages": 6, "cachedMessages": 5, "lastCacheHit": false })
    );

    // 第二个不产生新摘要/模型请求；skipped 行按夹具形状落库。
    let after = wait_state_revision(&mut conn, &session_id, revision_before + 4).await;
    assert_eq!(after["session"]["status"], json!("idle"));
    assert_eq!(
        requested_requests(&fixture).len(),
        2,
        "主 turn + 第一个 compact = 2 次模型请求"
    );
    assert_eq!(
        messages_with_kind(&fixture.db_path, "compact_summary").len(),
        1,
        "只有一份摘要"
    );

    let skipped_message_id = message_rows(&fixture.db_path)
        .into_iter()
        .find(|(_, data)| data["finish"] == json!("skipped"))
        .map(|(message_id, _)| message_id)
        .expect("skipped timeline 消息");
    let skipped = message_rows(&fixture.db_path)
        .into_iter()
        .find(|(message_id, _)| message_id == &skipped_message_id)
        .map(|(_, data)| data)
        .expect("skipped timeline data");
    assert_eq!(skipped["role"], json!("assistant"));
    assert_eq!(skipped["semantics"]["kind"], json!("timeline_event"));
    assert_eq!(
        skipped["tokens"],
        json!({
            "input": 0,
            "output": 0,
            "reasoning": 0,
            "cache": { "read": 0, "write": 0 }
        })
    );
    let skipped_parts: Vec<Value> = part_rows(&fixture.db_path)
        .into_iter()
        .filter(|(message_id, _)| message_id == &skipped_message_id)
        .map(|(_, data)| data)
        .collect();
    assert_eq!(skipped_parts.len(), 2, "timeline + compaction 两个 part");
    assert_eq!(skipped_parts[0]["type"], json!("timeline"));
    assert_eq!(skipped_parts[0]["status"], json!("skipped"));
    assert_eq!(skipped_parts[0]["sourceCommandId"], json!(second_input));
    assert_eq!(skipped_parts[0]["compactReason"], json!("user_requested"));
    assert_eq!(skipped_parts[1]["type"], json!("compaction"));
    assert_eq!(skipped_parts[1]["timelineStatus"], json!("skipped"));
    assert_eq!(skipped_parts[1]["timelineDisplay"], json!("separator"));
    assert_eq!(skipped_parts[1]["replace"], json!(true));
    assert!(
        skipped_parts[1]["preCompactTokenCount"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(
        skipped_parts[1].get("boundaryId").is_none(),
        "skipped 不建立边界"
    );
    assert!(skipped_parts[1].get("summaryMessageId").is_none());

    let compaction_parts = parts_with_type(&fixture.db_path, "compaction");
    assert_eq!(
        compaction_parts.len(),
        3,
        "成功 compact 的 timeline/summary compaction part + skipped 1"
    );
    assert_eq!(
        compaction_parts
            .iter()
            .filter(|data| data["timelineStatus"] == json!("completed"))
            .count(),
        1,
        "只有 timeline compaction part 带 timelineStatus:completed"
    );
    assert_eq!(
        compaction_parts
            .iter()
            .filter(|data| data["timelineStatus"] == json!("skipped"))
            .count(),
        1
    );
    assert_eq!(
        compaction_parts
            .iter()
            .filter(|data| data.get("compactBoundary").is_some())
            .count(),
        1,
        "摘要 compaction part 携带 compactBoundary"
    );

    // 夹具的 dbSkippedRows 没有 skipped turn_usage：只保留主 turn 与成功 compact 两行。
    assert_eq!(
        count_rows(&fixture.db_path, "turn_usage"),
        2,
        "skipped 不写 turn_usage 行（夹具未录该行）"
    );
    assert_eq!(
        count_rows(&fixture.db_path, "model_usage"),
        1,
        "只有成功 compact 的 model_usage；skipped 不发起模型请求"
    );
    assert_eq!(
        session_column(&fixture.db_path, &session_id, "time_compacting"),
        None
    );
}

// ---------------------------------------------------------------------------
// 6b. prompt 运行中的 compact：`-32010`（fixture `compact-while-turn-running`）
// ---------------------------------------------------------------------------

/// prompt 仍在执行（provider 闸门挂起）时 `session/compact` 必须显式拒绝：
/// `-32010 Cannot compact while a prompt is running`，零事件、零落库、revision 不动，
/// 且 prompt turn 之后照常完成（契约 §1.1/§1.6）。
#[tokio::test]
async fn compact_while_prompt_running_is_rejected_with_zero_side_effects() {
    let scripted = ScriptedProvider::new(vec![ScriptStep::Text(
        "prompt answer after gate".to_string(),
    )]);
    let recording = Arc::new(RecordingProvider::new(Arc::new(scripted)));
    let requests = Arc::clone(&recording.requests);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let inner: Arc<dyn ModelProvider> = Arc::clone(&recording) as Arc<dyn ModelProvider>;
    let provider = Arc::new(GatedPromptProvider {
        inner,
        gate: Arc::clone(&gate),
    });
    let fixture = fixture_with_source(
        "while-turn-running",
        Arc::new(StaticSource { provider }),
        requests,
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    // prompt turn：受理后在 provider 闸门上挂起（turn.started 已发布 → 相位 Running，
    // `current_kind == Prompt` 是 `request_compact` 的判定点，与发布同一临界区）。
    let send_id = conn
        .send(
            "session/send",
            json!({
                "sessionId": session_id,
                "content": "turn that is still running",
            }),
        )
        .await;
    let sent = conn.result(send_id).await;
    assert_eq!(sent["accepted"], json!(true));
    let started = next_event_until(&mut conn, |event| event["type"] == json!("turn.started")).await;
    assert_eq!(
        started["payload"]["input"],
        json!("turn that is still running")
    );

    let counts_before = table_counts(&fixture.db_path);
    let revision_before = conn.read_snapshot(&session_id).await["runtime"]["stateRevision"]
        .as_u64()
        .expect("rev");

    // prompt 仍在执行的同一窗口里 compact：显式 JSON-RPC 拒绝（fixture 观察形状：
    // `{"code":-32010,"message":"Cannot compact while a prompt is running"}`，无 data）。
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": "input_compact_running_1",
            }),
        )
        .await;
    let (code, message, data) = conn.error(id).await;
    assert_eq!(code, -32010);
    assert_eq!(message, "Cannot compact while a prompt is running");
    assert!(data.is_none(), "拒绝不带 data（fixture 观察形状）");

    // 零副作用：revision 不动、八张观察表行数逐表不变（fixture `dbCountsDuring`）。
    let snapshot_after = conn.read_snapshot(&session_id).await;
    assert_eq!(
        snapshot_after["runtime"]["stateRevision"],
        json!(revision_before),
        "拒绝路径不推进 revision"
    );
    let counts_after = table_counts(&fixture.db_path);
    assert_eq!(counts_after, counts_before, "拒绝路径零落库");

    // 放行 prompt：正常完成（没有 compact 的 turn.started；被拒 compact 不发起模型请求）。
    gate.add_permits(1);
    let rest = conn.drain_until_terminal().await;
    assert_eq!(
        event_types(&rest),
        vec!["model.streaming", "turn.completed"],
        "终态前只有本 prompt 的流式事件；没有 compact 事件"
    );
    assert_eq!(
        rest[0]["payload"]["delta"],
        json!("prompt answer after gate")
    );
    assert_eq!(
        rest.last().expect("终态")["payload"]["response"],
        json!("prompt answer after gate")
    );
    assert_eq!(
        requested_requests(&fixture).len(),
        1,
        "只有 prompt 一次模型请求"
    );
    assert_eq!(
        session_column(&fixture.db_path, &session_id, "time_compacting"),
        None
    );
}

// ---------------------------------------------------------------------------
// 7. provider 失败：tool-call / 空文本（compact-provider-* 夹具）
// ---------------------------------------------------------------------------

struct FailureCase {
    label: &'static str,
    step: ScriptStep,
    message: &'static str,
    retryable: bool,
    tool_calls: u64,
}

#[tokio::test]
async fn compact_provider_failures_persist_observed_failure_rows() {
    let cases = [
        FailureCase {
            label: "failure-tool-call",
            step: ScriptStep::ToolCalls(vec![ScriptToolCall {
                name: "Read".to_string(),
                input: json!({ "file_path": "hello.txt" }),
            }]),
            message: "Tool use is not allowed during compaction",
            retryable: false,
            tool_calls: 1,
        },
        FailureCase {
            label: "failure-empty",
            // T0 的 empty 剧步同样是 `toolCalls:[]`：finishReason=tool-calls、零文本。
            step: ScriptStep::ToolCalls(Vec::new()),
            message: "Failed to generate compact summary",
            retryable: true,
            tool_calls: 0,
        },
    ];

    for case in cases {
        let fixture = fixture(
            case.label,
            vec![
                ScriptStep::Text("normal turn answer".to_string()),
                ScriptStep::Text("normal turn answer".to_string()),
                case.step.clone(),
            ],
        );
        let mut conn = connect(Arc::clone(&fixture.server)).await;
        let session_id = create_session(&mut conn, &fixture.ws).await;
        send_turn(&mut conn, &session_id, "first prompt").await;
        send_turn(&mut conn, &session_id, "second prompt").await;

        let before = conn.read_snapshot(&session_id).await;
        let revision = before["runtime"]["stateRevision"].as_u64().expect("rev");
        let counts_before = table_counts(&fixture.db_path);

        let input_id = "input_compact_failure";
        let id = conn
            .send(
                "session/compact",
                json!({
                    "sessionId": session_id,
                    "inputId": input_id,
                    "expectedRevision": revision,
                }),
            )
            .await;
        let accepted = conn.result(id).await;
        assert_eq!(accepted["compact"]["state"], json!("accepted"));

        let events = conn.drain_until_terminal().await;
        assert_eq!(
            event_types(&events),
            vec!["turn.started", "turn.failed"],
            "{}",
            case.label
        );
        let started = &events[0]["payload"];
        assert_eq!(started["input"], json!("/compact"));
        assert_eq!(started["inputId"], json!(input_id));
        let turn_id = events[0]["turnId"].as_str().expect("turnId").to_string();
        let failed = &events[1]["payload"];
        assert_eq!(failed["error"]["type"], json!("unknown_error"));
        assert_eq!(failed["error"]["code"], json!("MODEL_ERROR"));
        assert_eq!(failed["error"]["message"], json!(case.message));
        assert_eq!(failed["error"]["detail"], json!("Compact failed"));
        assert_eq!(
            failed["error"]["underlyingErrorMessage"],
            json!(case.message)
        );
        assert_eq!(
            failed["error"]["attribution"],
            json!({ "retryable": case.retryable })
        );
        assert_eq!(failed["turnPhase"], json!("compact"));
        assert_eq!(failed["inputId"], json!(input_id));

        let counts_after = table_counts(&fixture.db_path);
        for table in ["session", "session_entry", "session_input", "tool_usage"] {
            assert_eq!(
                counts_after[table], counts_before[table],
                "{}: {table} 行数不变",
                case.label
            );
        }
        assert_eq!(counts_after["message"], counts_before["message"] + 1);
        assert_eq!(counts_after["part"], counts_before["part"] + 2);
        assert_eq!(counts_after["turn_usage"], counts_before["turn_usage"] + 1);
        assert_eq!(
            counts_after["model_usage"],
            counts_before["model_usage"] + 1
        );
        assert_eq!(
            messages_with_kind(&fixture.db_path, "compact_summary").len(),
            0,
            "{}: 失败不写摘要",
            case.label
        );

        let messages = message_rows(&fixture.db_path);
        let (failed_message_id, failed_message) = messages.last().expect("failed timeline");
        assert_eq!(failed_message["finish"], json!("failed"));
        assert_eq!(failed_message["semantics"]["kind"], json!("timeline_event"));
        assert_eq!(
            failed_message["tokens"],
            json!({
                "input": 0,
                "output": 0,
                "reasoning": 0,
                "cache": { "read": 0, "write": 0 }
            })
        );

        let failed_parts: Vec<Value> = part_rows(&fixture.db_path)
            .into_iter()
            .filter(|(message_id, _)| message_id == failed_message_id)
            .map(|(_, data)| data)
            .collect();
        assert_eq!(failed_parts.len(), 2);
        assert_eq!(failed_parts[0]["type"], json!("timeline"));
        assert_eq!(failed_parts[0]["status"], json!("failed"));
        assert_eq!(failed_parts[0]["reason"], json!("model_error"));
        assert_eq!(failed_parts[0]["attempt"], json!(1));
        assert_eq!(failed_parts[0]["sourceCommandId"], json!(input_id));
        assert!(
            failed_parts[0]["preCompactTokenCount"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );
        assert!(failed_parts[0].get("boundaryId").is_none());
        assert!(failed_parts[0].get("summaryMessageId").is_none());
        assert_eq!(failed_parts[1]["type"], json!("compaction"));
        assert_eq!(failed_parts[1]["timelineStatus"], json!("failed"));
        assert_eq!(failed_parts[1]["reason"], json!("model_error"));
        assert_eq!(failed_parts[1]["attempt"], json!(1));
        assert_eq!(failed_parts[1]["replace"], json!(true));
        assert!(failed_parts[1].get("boundaryId").is_none());
        assert!(failed_parts[1].get("summaryMessageId").is_none());

        let usage = turn_usage_row(&fixture.db_path, &turn_id);
        assert_eq!(usage["status"], json!("error"));
        assert_eq!(usage["userMessageId"], Value::Null);
        assert_eq!(usage["modelRequestCount"], json!(1));
        assert_eq!(usage["inputTokens"], json!(0));
        assert_eq!(usage["outputTokens"], json!(0));
        assert_eq!(usage["computedTotalTokens"], json!(0));
        assert_eq!(usage["errorType"], json!("unknown_error"));
        assert_eq!(usage["errorCode"], json!("UNKNOWN_ERROR"));
        assert!(
            usage["traceId"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "{}: turn_usage.trace_id 非空",
            case.label
        );

        let model_usage = compact_model_usage_row(&fixture.db_path);
        assert_eq!(model_usage["querySource"], json!("compact"));
        assert_eq!(model_usage["status"], json!("completed"));
        assert_eq!(model_usage["assistantMessageId"], Value::Null);
        assert_eq!(model_usage["parentUserMessageId"], Value::Null);
        assert_eq!(model_usage["finishReason"], json!("tool-calls"));
        assert_eq!(model_usage["toolCallCount"], json!(case.tool_calls));
        assert_eq!(model_usage["inputTokens"], json!(10));
        assert_eq!(model_usage["outputTokens"], json!(20));
        assert_eq!(model_usage["computedTotalTokens"], json!(30));
        assert_eq!(model_usage["providerTotalTokens"], json!(30));
        assert_eq!(
            model_usage["rawUsage"],
            json!({
                "inputTokens": 10,
                "outputTokens": 20,
                "totalTokens": 30,
                "cacheReadTokens": 0,
                "cacheWriteTokens": 0,
            })
        );
        assert_eq!(
            model_usage["providerMetadata"],
            json!({ "rawFinishReason": "tool_use" })
        );

        // 失败不破坏会话：状态离开 Running（Error），历史可继续使用。
        let settled = wait_session_status(&mut conn, &session_id, "error").await;
        assert!(settled["runtime"].get("activeTurnId").is_none());
        assert_eq!(
            session_column(&fixture.db_path, &session_id, "time_compacting"),
            None
        );
        assert_eq!(
            requested_requests(&fixture).len(),
            3,
            "{}: 两个正常 turn + 一次失败的 compact",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// 8. 无模型 compact：CONFIGURATION_ERROR 终态，状态不得卡在 Running
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compact_without_model_fails_configuration_and_leaves_running() {
    let fixture = fixture("config-failure", Vec::new());
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params_without_model(&fixture.ws))
        .await;
    let created = conn.result(id).await;
    assert!(created["session"].get("model").is_none());
    let session_id = created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    conn.subscribe_default(&session_id).await;
    let revision = created["runtime"]["stateRevision"].as_u64().expect("rev");
    let counts_before = table_counts(&fixture.db_path);

    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": "input_compact_no_model",
                "expectedRevision": revision,
            }),
        )
        .await;
    let accepted = conn.result(id).await;
    assert_eq!(accepted["compact"]["state"], json!("accepted"));

    let events = conn.drain_until_terminal().await;
    assert_eq!(event_types(&events), vec!["turn.started", "turn.failed"]);
    let failed = &events[1]["payload"];
    assert_eq!(failed["error"]["type"], json!("CONFIGURATION_ERROR"));
    assert_eq!(
        failed["error"]["message"],
        json!("Select a model before continuing")
    );
    assert_eq!(failed["turnPhase"], json!("configuration"));
    let turn_id = events[0]["turnId"].as_str().expect("turnId").to_string();

    // 回归点：compact 从未进入 CommandInbox；旧收口按 inbox.complete 的 Stale
    // 保持 Running。现在必须显式落 Error（不是 Running/Waiting）。
    let settled = wait_session_status(&mut conn, &session_id, "error").await;
    assert!(settled["runtime"].get("activeTurnId").is_none());
    assert_ne!(settled["session"]["status"], json!("running"));

    let counts_after = table_counts(&fixture.db_path);
    assert_eq!(counts_after["message"], counts_before["message"]);
    assert_eq!(counts_after["part"], counts_before["part"]);
    assert_eq!(counts_after["model_usage"], counts_before["model_usage"]);
    assert_eq!(counts_after["turn_usage"], counts_before["turn_usage"] + 1);

    let usage = turn_usage_row(&fixture.db_path, &turn_id);
    assert_eq!(usage["status"], json!("failed"));
    assert_eq!(usage["userMessageId"], Value::Null);
    assert_eq!(usage["modelRequestCount"], json!(0));
    assert_eq!(usage["inputTokens"], json!(0));
    assert_eq!(usage["computedTotalTokens"], json!(0));
    assert_eq!(usage["errorType"], Value::Null);
    assert_eq!(usage["errorCode"], Value::Null);

    assert_eq!(
        session_column(&fixture.db_path, &session_id, "time_compacting"),
        None
    );
    assert!(
        requested_requests(&fixture).is_empty(),
        "没有模型：不发起模型请求"
    );
}

// ---------------------------------------------------------------------------
// 9. resume 边界：只恢复最近一次成功 compact 的摘要 + 之后的消息
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_after_compaction_restores_summary_boundary_only() {
    let fixture = fixture(
        "resume-boundary",
        vec![
            // turn 1：先工具调用（形成 compact 边界前的 tool pair），再文本收尾。
            ScriptStep::ToolCalls(vec![ScriptToolCall {
                name: "Read".to_string(),
                input: json!({ "file_path": "hello.txt" }),
            }]),
            ScriptStep::Text("turn one follow-up".to_string()),
            // turn 2。
            ScriptStep::Text("second turn answer".to_string()),
            // compact 1（assistant 计数 = 3）。
            ScriptStep::Text("summary one".to_string()),
            // compact 2（resume 后 2 条 assistant 历史时取到这一格）。
            ScriptStep::Text("summary two".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&mut conn, &fixture.ws).await;

    let turn_one = send_turn(&mut conn, &session_id, "first prompt").await;
    assert_eq!(
        turn_one.last().expect("terminal")["type"],
        json!("turn.completed")
    );
    send_turn(&mut conn, &session_id, "second prompt").await;
    let settled = wait_session_status(&mut conn, &session_id, "idle").await;
    let revision = settled["runtime"]["stateRevision"]
        .as_u64()
        .expect("stateRevision");

    // 第一次 compact。
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": "input_compact_resume_1",
                "expectedRevision": revision,
            }),
        )
        .await;
    assert_eq!(conn.result(id).await["compact"]["state"], json!("accepted"));
    assert_eq!(
        event_types(&conn.drain_until_terminal().await),
        vec!["turn.started", "turn.completed"]
    );
    wait_state_revision(&mut conn, &session_id, revision + 2).await;

    // close + resume：新 runtime 的模型历史应从 compact 边界开始。
    close_session(&mut conn, &session_id).await;
    resume_session(&mut conn, &session_id).await;
    let resumed = conn.read_snapshot(&session_id).await;
    assert_eq!(resumed["runtime"]["stateRevision"], json!(0));

    send_turn(&mut conn, &session_id, "after resume").await;
    let resumed_settled = wait_session_status(&mut conn, &session_id, "idle").await;
    let resumed_revision = resumed_settled["runtime"]["stateRevision"]
        .as_u64()
        .expect("stateRevision");

    let requests = requested_requests(&fixture);
    assert_eq!(
        requests.len(),
        6,
        "resume 前后请求累计：turn1 2 + turn2 1 + compact 1 + resume 后 turn 2"
    );
    let resumed_request = &requests[4];
    assert_eq!(
        resumed_request.messages.len(),
        3,
        "系统提示 + 摘要续接 + 新 prompt"
    );
    assert!(matches!(
        &resumed_request.messages[0],
        ModelMessage::System(_)
    ));
    let ModelMessage::User(continuation) = &resumed_request.messages[1] else {
        panic!("resume 的第一条历史必须是摘要续接消息");
    };
    assert!(
        continuation.starts_with(SUMMARY_PREFIX),
        "续接消息带固定前缀"
    );
    assert!(continuation.contains("summary one"));
    assert_eq!(
        resumed_request.messages[2],
        ModelMessage::User("after resume".to_string())
    );
    assert!(
        !resumed_request
            .messages
            .iter()
            .any(|message| matches!(message, ModelMessage::ToolResult { .. })),
        "边界前的 tool pair 不得进入恢复后的请求"
    );
    for forbidden in [
        "first prompt",
        "second prompt",
        "turn one follow-up",
        "second turn answer",
        "hello",
    ] {
        assert!(
            !resumed_request.messages.iter().any(|message| {
                message_text(message).is_some_and(|text| text.contains(forbidden))
            }),
            "边界前内容 {forbidden:?} 不得进入恢复后的请求"
        );
    }

    // 第二次 compact：边界应移动到最新一条 completed compaction。
    let id = conn
        .send(
            "session/compact",
            json!({
                "sessionId": session_id,
                "inputId": "input_compact_resume_2",
                "expectedRevision": resumed_revision,
            }),
        )
        .await;
    assert_eq!(conn.result(id).await["compact"]["state"], json!("accepted"));
    assert_eq!(
        event_types(&conn.drain_until_terminal().await),
        vec!["turn.started", "turn.completed"]
    );
    wait_state_revision(&mut conn, &session_id, resumed_revision + 2).await;
    assert_eq!(
        parts_with_type(&fixture.db_path, "compaction")
            .iter()
            .filter(|data| data["timelineStatus"] == json!("completed"))
            .count(),
        2,
        "库里保留两个 completed compact 边界"
    );

    close_session(&mut conn, &session_id).await;
    resume_session(&mut conn, &session_id).await;
    send_turn(&mut conn, &session_id, "after second resume").await;
    wait_session_status(&mut conn, &session_id, "idle").await;

    let requests = requested_requests(&fixture);
    assert_eq!(
        requests.len(),
        9,
        "第二次 resume 后新增 2 个请求（tool + follow-up）"
    );
    let latest_request = &requests[7];
    assert_eq!(latest_request.messages.len(), 3);
    let ModelMessage::User(continuation) = &latest_request.messages[1] else {
        panic!("最新边界读取摘要续接消息");
    };
    assert!(continuation.starts_with(SUMMARY_PREFIX));
    assert!(
        continuation.contains("second turn answer"),
        "第二次 compact 的摘要正文（脚本步）替换了旧摘要"
    );
    assert!(
        !continuation.contains("summary one"),
        "旧摘要不再出现在请求里"
    );
    assert_eq!(
        latest_request.messages[2],
        ModelMessage::User("after second resume".to_string())
    );
}

fn message_text(message: &ModelMessage) -> Option<&str> {
    match message {
        ModelMessage::System(text)
        | ModelMessage::User(text)
        | ModelMessage::ToolResult { content: text, .. } => Some(text),
        ModelMessage::Assistant { text, .. } => text.as_deref(),
    }
}
