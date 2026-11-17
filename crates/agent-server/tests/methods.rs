//! session 方法表集成测试（LT-1 C1 Task 7）：真实 `run_stdio` over duplex 流。
//!
//! 覆盖：`session/create`（严格参数 / 落行 / 快照形状）、`session/send`（立即回执 /
//! 事件流 / inputId 幂等 / `expectedRevision` 冲突 / 缺模型的 `CONFIGURATION_ERROR`）、
//! `session/subscribe`（replay+live 无重复无缺口）、`session/read`（冷读 + messageLimit）、
//! `session/messages`（after/limit）、`session/list`（directory / includeArchived / limit）、
//! `session/close`+`session/resume`（历史恢复、序号不撞）、三个 setter、
//! `session/compact`/`session/goal` 显式 `-32601`、启动存储门（checking→ready/failed）。
//!
//! 测试库 DDL 从 `scripts/sqlite-schema-baseline.json` 生成（与 session-store /
//! runtime 测试同一口径）；provider 配置走临时 builtin/personal 文件；模型端点
//! `127.0.0.1:9`（保留端口，**绝不**发起真实请求——provider 由脚本化 fake 装配）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
    ProviderError, ScriptStep, ScriptedProvider,
};
use agent_protocol::message::MessageWithParts;
use agent_protocol::snapshot::SessionStateSnapshot;
use agent_server::{
    run_stdio, AgentServer, Frame, ProviderConfigHub, ProviderSource, RequestId, ServerConfig,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::Notify;

// ---------------------------------------------------------------------------
// 基线测试库夹具（与 session-store/tests/common 同一口径）
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
            "agent-server-methods-{label}-{}-{}",
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

/// 按基线生成已迁移的测试库（表 + 索引 + 账本）。
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

fn read_column(path: &Path, table: &str, column: &str, where_sql: &str) -> String {
    raw_connection(path)
        .query_row(
            &format!("SELECT {column} FROM {table} WHERE {where_sql}"),
            [],
            |row| row.get(0),
        )
        .expect("读取列")
}

/// 直接种一行 session（list 的归档过滤用例；C1 没有归档方法）。
fn insert_session_row(
    db_path: &Path,
    id: &str,
    directory: &str,
    time_updated: i64,
    archived_at: Option<i64>,
) {
    let connection = Connection::open(db_path).expect("打开库写种子行");
    connection
        .execute(
            "INSERT INTO session (\
             id, project_id, workspace_id, parent_id, slug, directory, path, title, version, \
             share_url, summary_additions, summary_deletions, summary_files, summary_diffs, \
             revert, permission, time_created, time_updated, time_compacting, time_archived, \
             task_type, title_source, title_message_id, time_title_updated, trace_id) \
             VALUES (?1, 'proj_seed', NULL, NULL, ?1, ?2, ?2, 'seeded', '0.0.0', NULL, \
             NULL, NULL, NULL, NULL, NULL, '{\"mode\":\"build\"}', ?3 - 10, ?3, NULL, ?4, \
             'interactive', 'first_input', NULL, NULL, 'trace-seed')",
            rusqlite::params![id, directory, time_updated, archived_at],
        )
        .expect("种 session 行");
}

// ---------------------------------------------------------------------------
// provider 夹具：临时 builtin/personal 文件 + 脚本化 provider 装配源
// ---------------------------------------------------------------------------

/// 写最小 Personal provider 文件：`mock-chat` 的 `mock-1` / `mock-2` 两个模型，
/// api-key access，端点指向保留地址（脚本化 provider 不走真实 HTTP）。
fn write_personal_file(root: &Path) -> PathBuf {
    let path = root.join("provider-config.json");
    let model_rules = |model_id: &str| {
        json!({
            "providerId": "mock-chat",
            "modelId": model_id,
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
        })
    };
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
                        "personalModelIds": ["mock-1", "mock-2"],
                        "api": { "type": "openai-chat-completions", "baseUrl": "http://127.0.0.1:9/v1/" }
                    }
                }]
            },
            "modelConfigRules": {
                "providerModelRules": [model_rules("mock-1"), model_rules("mock-2")],
                "manualProviderModelRules": []
            }
        }
    });
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    path
}

/// 写最小 Built-in Release（无 provider 规则；模型全部来自 Personal 层）。
fn write_builtin_file(root: &Path) -> String {
    let path = root.join("mode-builtin.json");
    std::fs::write(
        &path,
        r#"{
          "schemaVersion": 1,
          "revision": 7,
          "config": {
            "providerConfigRules": {
              "templateRules": [],
              "providerRules": []
            },
            "modelConfigRules": {
              "modelRules": [],
              "modelApiRules": [],
              "providerSiteRules": [],
              "templateModelRules": [],
              "builtinProviderModelRules": []
            }
          }
        }"#,
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

/// 脚本化装配源：`mock-chat` 的模型解析成同一份剧本的 provider；其他 provider
/// 显式失败（与 hub 的「provider 不存在」口径一致，覆盖 create 宽容 / send 严格）。
struct ScriptedSource {
    steps: Vec<ScriptStep>,
}

impl ProviderSource for ScriptedSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        if provider_id != "mock-chat" {
            return Err(format!("Provider 不存在: {provider_id}"));
        }
        Ok(Arc::new(ScriptedProvider::new(self.steps.clone())))
    }
}

/// 门控 provider：每次模型请求都停在同一个 release 信号上（close-drain 用例需要
/// 「turn 1 还在跑、turn 2 已排队」的确定性窗口）。
struct GatedProvider {
    release: Arc<Notify>,
}

impl ModelProvider for GatedProvider {
    fn stream(
        &self,
        _request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            release.notified().await;
            deltas
                .send(ModelDelta::Text {
                    text: "gated answer".to_string(),
                })
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

/// 门控装配源：`mock-chat` 的解析产出共享同一 release 信号的 provider。
struct GatedSource {
    release: Arc<Notify>,
}

impl ProviderSource for GatedSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        if provider_id != "mock-chat" {
            return Err(format!("Provider 不存在: {provider_id}"));
        }
        Ok(Arc::new(GatedProvider {
            release: Arc::clone(&self.release),
        }))
    }
}

// ---------------------------------------------------------------------------
// stdio 测试连接（duplex；门通知在 connect 内消费）
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

/// 整套夹具：临时数据根 + 已迁移库 + provider 配置 + 配好的 server。
struct Fixture {
    server: Arc<AgentServer>,
    db_path: PathBuf,
    ws: PathBuf,
    _root: TempRoot,
}

fn fixture(label: &str, steps: Vec<ScriptStep>) -> Fixture {
    fixture_with_source(label, Arc::new(ScriptedSource { steps }))
}

/// 自定义 provider 装配源的夹具变体（close-drain 用例注入门控 provider）。
fn fixture_with_source(label: &str, source: Arc<dyn ProviderSource>) -> Fixture {
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
        _root: root,
    }
}

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("用例超时（10s）")
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

    /// Task 8：物化即发的 `session/requestRuntimePreferences` 反向请求由宿主
    /// （本测试）按 spec §1.3 的默认应答回掉；不回会让 worker 卡到 15s 超时。
    /// 其余帧原样透传（本文件不审批工具）。
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

    /// 等指定请求的 result；错误应答直接 panic（正路径用）。
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

    /// 订阅默认链路（oracle 生命周期：create 后立即 subscribe，事件才开始流动）。
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

    /// 等指定请求的错误应答 → `(code, message, data)`。
    async fn error(&mut self, id: u64) -> (i64, String, Option<Value>) {
        self.error_or_result(id)
            .await
            .expect_err("期望错误应答，实际返回 result")
    }

    /// 等指定请求的应答：`Ok(result)` 或 `Err((code, message, data))`。
    async fn error_or_result(&mut self, id: u64) -> Result<Value, (i64, String, Option<Value>)> {
        let expected = RequestId::Number(id as i64);
        loop {
            match self.next_frame().await {
                Frame::Response(response) if response.id == expected => return Ok(response.result),
                Frame::ErrorResponse(response) if response.id == expected => {
                    return Err((
                        response.error.code,
                        response.error.message,
                        response.error.data,
                    ));
                }
                _ => {}
            }
        }
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

    /// 断言短时间内没有新帧（负向验证：没有第二个 turn）。
    async fn assert_quiet(&mut self, ms: u64) {
        let mut line = String::new();
        let read =
            tokio::time::timeout(Duration::from_millis(ms), self.reader.read_line(&mut line)).await;
        assert!(read.is_err(), "期望无新帧，实际收到: {line}");
    }
}

fn create_params(ws: &Path, with_model: bool) -> Value {
    let mut params = json!({
        "workspace": {
            "workspacePath": ws.to_string_lossy(),
            "workspaceKey": ws.to_string_lossy()
        },
        "thoughtLevel": "disabled",
        "titleGenerationEnabled": false
    });
    if with_model {
        params["model"] = json!({
            "providerId": "mock-chat",
            "modelId": "mock-1",
            "options": { "reasoningLevel": "disabled" }
        });
    }
    params
}

// ---------------------------------------------------------------------------
// 1. session/create：严格参数 / 落行 / 快照形状
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_strict_params_persists_row_and_snapshot_parses() {
    let fixture = fixture("create", vec![ScriptStep::Text("answer".to_string())]);
    let mut conn = connect(Arc::clone(&fixture.server)).await;

    // 未知键 → -32602（.strict() 语义）。
    let id = conn
        .send(
            "session/create",
            json!({ "workspace": {"workspacePath": "/ws/x", "workspaceKey": "/ws/x"}, "bogus": 1 }),
        )
        .await;
    let (code, message, _) = conn.error(id).await;
    assert_eq!(code, -32602);
    assert!(message.contains("参数不合法"), "实际：{message}");

    // 合法 create：快照可严格解析，store 行存在。
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let snapshot_value = conn.result(id).await;
    let snapshot: SessionStateSnapshot =
        serde_json::from_value(snapshot_value.clone()).expect("快照必须是严格协议形状");
    assert_eq!(snapshot.protocol.version, 1);
    assert!(!snapshot.session.session_id.is_empty());
    // create 时 title 是空串（Task 0 观察：首条 send 后才写标题）。
    assert_eq!(snapshot.session.title, "");
    let model = snapshot.session.model.as_ref().expect("有模型");
    assert_eq!(model.provider_id, "mock-chat");
    assert_eq!(model.model_id, "mock-1");
    assert_eq!(snapshot.runtime.state_revision, 0);
    assert!(snapshot.messages.is_empty());
    // 未知可选字段缺省省略。
    assert!(
        snapshot_value["session"].get("titleSource").is_none(),
        "titleSource 缺省省略"
    );
    assert!(
        snapshot_value["runtime"].get("deliveryKind").is_none(),
        "deliveryKind 缺省省略"
    );

    // store 行：id/slug/title/directory/project_id 形状。
    let session_id = snapshot.session.session_id.clone();
    let db = &fixture.db_path;
    assert_eq!(count_rows(db, "session"), 1);
    assert_eq!(
        read_column(db, "session", "title", &format!("id = '{session_id}'")),
        ""
    );
    assert_eq!(
        read_column(db, "session", "directory", &format!("id = '{session_id}'")),
        fixture.ws.to_string_lossy()
    );
    assert_eq!(
        read_column(db, "session", "slug", &format!("id = '{session_id}'")),
        session_id
    );
    assert!(
        read_column(db, "session", "project_id", &format!("id = '{session_id}'"))
            .starts_with("proj_"),
        "project_id 是 proj_ 前缀的派生 scope"
    );
    // runtime entries 已写（resume 恢复模型/模式的事实源；观察形状 §3.6）。
    assert_eq!(count_rows(db, "session_entry"), 2);
    assert_eq!(
        read_column(
            db,
            "session_entry",
            "type",
            &format!("session_id = '{session_id}' AND id LIKE '%runtime-model%'")
        ),
        "runtime/model_selection"
    );
    assert_eq!(
        read_column(
            db,
            "session_entry",
            "type",
            &format!("session_id = '{session_id}' AND id LIKE '%execution%'")
        ),
        "runtime/execution_state"
    );
}

// ---------------------------------------------------------------------------
// 2. session/send：立即回执 / 事件流 / 幂等 / revision 冲突
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_streams_events_idempotent_input_and_revision_conflict() {
    let fixture = fixture(
        "send",
        vec![ScriptStep::Text("first turn answer".to_string())],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let snapshot = conn.result(id).await;
    let session_id = snapshot["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    conn.subscribe_default(&session_id).await;

    // 非法参数不拉起 turn：automationId 与 offPeakTaskId 互斥（superRefine）。
    let id = conn
        .send(
            "session/send",
            json!({
                "sessionId": session_id,
                "content": "should not start",
                "automationId": "a-1",
                "offPeakTaskId": "t-1"
            }),
        )
        .await;
    let (code, message, _) = conn.error(id).await;
    assert_eq!(code, -32602);
    assert!(message.contains("mutually exclusive"), "实际：{message}");

    // 正常 send：accepted:true 立即返回。
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "first prompt", "inputId": "in-1" }),
        )
        .await;
    let receipt = conn.result(id).await;
    assert_eq!(receipt["accepted"], json!(true));
    assert_eq!(receipt["sessionId"], json!(session_id));
    assert!(
        receipt["stateRevision"].as_u64().expect("stateRevision") >= 1,
        "接受即推进版本"
    );

    // 事件流：turn.started → model.streaming → turn.completed。
    let events = conn.drain_until_terminal().await;
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().expect("type"))
        .collect();
    assert_eq!(
        kinds,
        vec!["turn.started", "model.streaming", "turn.completed"]
    );
    let seqs: Vec<u64> = events
        .iter()
        .map(|event| event["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(seqs, vec![1, 2, 3], "seq 从 1 连续");

    // 重复 inputId：幂等（同一回执），不再拉起 turn。
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "different content", "inputId": "in-1" }),
        )
        .await;
    let duplicate = conn.result(id).await;
    assert_eq!(duplicate, receipt, "同 inputId 返回首次回执");
    conn.assert_quiet(400).await;

    // expectedRevision 冲突：显式错误，不启动 turn。
    let id = conn
        .send(
            "session/send",
            json!({
                "sessionId": session_id,
                "content": "stale",
                "inputId": "in-2",
                "expectedRevision": 999
            }),
        )
        .await;
    let (code, message, _) = conn.error(id).await;
    assert_eq!(code, -32602);
    assert!(
        message.contains("999"),
        "冲突信息要带 expected 值：{message}"
    );
    conn.assert_quiet(400).await;

    // 数据面：只有一次 turn 的行。
    assert_eq!(count_rows(&fixture.db_path, "message"), 2);
    assert_eq!(count_rows(&fixture.db_path, "turn_usage"), 1);
}

// ---------------------------------------------------------------------------
// 3. session/subscribe：replay + live 无重复无缺口
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscribe_replays_and_streams_live_without_gap() {
    let fixture = fixture(
        "subscribe",
        vec![ScriptStep::Text("subscribe answer".to_string())],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let snapshot = conn.result(id).await;
    let session_id = snapshot["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    // 先订阅（includeSnapshot），后 send：replay 为空、live 全量。
    let id = conn
        .send(
            "session/subscribe",
            json!({
                "sessionId": session_id,
                "deliveryKind": "desktop-continuous",
                "includeSnapshot": true
            }),
        )
        .await;
    let subscribed = conn.result(id).await;
    assert_eq!(subscribed["sessionId"], json!(session_id));
    assert_eq!(subscribed["eventSeq"], json!(0));
    assert_eq!(subscribed["events"], json!([]));
    // 订阅快照的 runtime.deliveryKind 是**订阅的**链路（Task 0 观察）。
    assert_eq!(
        subscribed["snapshot"]["runtime"]["deliveryKind"],
        json!("desktop-continuous")
    );
    serde_json::from_value::<SessionStateSnapshot>(subscribed["snapshot"].clone())
        .expect("订阅快照可严格解析");

    let send_id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "live probe" }),
        )
        .await;
    let _ = conn.result(send_id).await;
    let live = conn.drain_until_terminal().await;
    let live_seqs: Vec<u64> = live
        .iter()
        .map(|event| event["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(live_seqs, vec![1, 2, 3], "live 段无缺口");

    // afterSeq=1 的第二次订阅：replay 拿 seq>1 的两条，无重复。
    let id = conn
        .send(
            "session/subscribe",
            json!({
                "sessionId": session_id,
                "deliveryKind": "desktop-continuous",
                "includeSnapshot": false,
                "afterSeq": 1
            }),
        )
        .await;
    let replayed = conn.result(id).await;
    let replay_seqs: Vec<u64> = replayed["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(replay_seqs, vec![2, 3]);
    assert_eq!(replayed["eventSeq"], json!(3));
    assert!(
        replayed.get("snapshot").is_none(),
        "includeSnapshot:false 无快照键"
    );
    // 每条事件都能严格解析成会话事件信封。
    for event in replayed["events"].as_array().expect("events") {
        serde_json::from_value::<agent_protocol::session_event::SessionEvent>(event.clone())
            .expect("事件信封可严格解析");
    }
}

// ---------------------------------------------------------------------------
// 4. session/read：冷读 / 热读 / messageLimit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_cold_and_warm_with_message_limit() {
    let fixture = fixture(
        "read",
        vec![
            ScriptStep::Text("first answer".to_string()),
            ScriptStep::Text("second answer".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    conn.subscribe_default(&session_id).await;
    for prompt in ["first prompt", "second prompt"] {
        let id = conn
            .send(
                "session/send",
                json!({ "sessionId": session_id, "content": prompt }),
            )
            .await;
        let _ = conn.result(id).await;
        conn.drain_until_terminal().await;
    }

    // 热读：runtime.stateRevision 反映活跃事实。
    let id = conn
        .send("session/read", json!({ "sessionId": session_id }))
        .await;
    let warm = conn.result(id).await;
    serde_json::from_value::<SessionStateSnapshot>(warm.clone()).expect("热读快照可解析");
    assert!(warm["runtime"]["stateRevision"].as_u64().unwrap() > 0);
    assert_eq!(warm["messages"].as_array().expect("messages").len(), 4);

    // messageLimit=1：最新 1 条（Node store 的 latest-N 语义）。
    let id = conn
        .send(
            "session/read",
            json!({ "sessionId": session_id, "messageLimit": 1 }),
        )
        .await;
    let limited = conn.result(id).await;
    let limited_messages = limited["messages"].as_array().expect("messages");
    assert_eq!(limited_messages.len(), 1);
    assert_eq!(
        limited_messages[0]["info"]["role"],
        json!("assistant"),
        "最新一条是 assistant"
    );

    // messageLimit=0 归一为「不限」（Node store 语义；裁决 4）。
    let id = conn
        .send(
            "session/read",
            json!({ "sessionId": session_id, "messageLimit": 0 }),
        )
        .await;
    let unlimited = conn.result(id).await;
    assert_eq!(unlimited["messages"].as_array().expect("messages").len(), 4);

    // 冷读：close 后无活跃 runtime，直接走 store；投影计数不按历史重算（Task 0 观察）。
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    let _ = conn.result(id).await;
    let id = conn
        .send("session/read", json!({ "sessionId": session_id }))
        .await;
    let cold = conn.result(id).await;
    serde_json::from_value::<SessionStateSnapshot>(cold.clone()).expect("冷读快照可解析");
    assert_eq!(cold["runtime"]["stateRevision"], json!(0));
    assert_eq!(
        cold["projection"]["turnCount"],
        json!(0),
        "冷读不重算投影计数"
    );
    assert_eq!(cold["messages"].as_array().expect("messages").len(), 4);
    // 冷读行的 titleSource 按库行投影。
    assert_eq!(cold["session"]["titleSource"], json!("first_input"));

    // 不存在的会话：显式 -32602。
    let id = conn
        .send("session/read", json!({ "sessionId": "sess_absent" }))
        .await;
    let (code, _, _) = conn.error(id).await;
    assert_eq!(code, -32602);
}

// ---------------------------------------------------------------------------
// 5. session/messages：after / limit / 线格式投影
// ---------------------------------------------------------------------------

#[tokio::test]
async fn messages_pagination_after_and_limit() {
    let fixture = fixture(
        "messages",
        vec![
            ScriptStep::Text("answer one".to_string()),
            ScriptStep::Text("answer two".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    conn.subscribe_default(&session_id).await;
    for prompt in ["first prompt", "second prompt"] {
        let id = conn
            .send(
                "session/send",
                json!({ "sessionId": session_id, "content": prompt }),
            )
            .await;
        let _ = conn.result(id).await;
        conn.drain_until_terminal().await;
    }

    // 全量：4 条，每条可严格解析（wire 投影 round-trip）。
    let id = conn
        .send("session/messages", json!({ "sessionId": session_id }))
        .await;
    let all = conn.result(id).await;
    let all_messages = all["messages"].as_array().expect("messages");
    assert_eq!(all_messages.len(), 4);
    for message in all_messages {
        serde_json::from_value::<MessageWithParts>(message.clone()).expect("消息线格式可解析");
    }
    assert_eq!(all_messages[0]["info"]["role"], json!("user"));
    assert_eq!(all_messages[0]["info"]["sessionId"], json!(session_id));
    assert_eq!(
        all_messages[1]["parts"].as_array().expect("parts").len(),
        3,
        "assistant = step-start + text + step-finish"
    );

    // limit=1：最新 1 条。
    let id = conn
        .send(
            "session/messages",
            json!({ "sessionId": session_id, "limit": 1 }),
        )
        .await;
    let limited = conn.result(id).await;
    assert_eq!(limited["messages"].as_array().expect("messages").len(), 1);

    // limit=0 归一为「不限」。
    let id = conn
        .send(
            "session/messages",
            json!({ "sessionId": session_id, "limit": 0 }),
        )
        .await;
    let unlimited = conn.result(id).await;
    assert_eq!(unlimited["messages"].as_array().expect("messages").len(), 4);

    // afterMessageId：第一条 user 之后的 3 条，升序。
    let first_id = all_messages[0]["info"]["messageId"]
        .as_str()
        .unwrap()
        .to_string();
    let id = conn
        .send(
            "session/messages",
            json!({ "sessionId": session_id, "afterMessageId": first_id }),
        )
        .await;
    let after = conn.result(id).await;
    let after_messages = after["messages"].as_array().expect("messages");
    assert_eq!(after_messages.len(), 3);
    assert_eq!(after_messages[0]["info"]["role"], json!("assistant"));

    // 游标不存在：显式 -32602（不静默当「从头开始」）。
    let id = conn
        .send(
            "session/messages",
            json!({ "sessionId": session_id, "afterMessageId": "msg_absent" }),
        )
        .await;
    let (code, _, _) = conn.error(id).await;
    assert_eq!(code, -32602);
}

// ---------------------------------------------------------------------------
// 6. session/list：directory / includeArchived / limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_by_directory_include_archived_and_limit() {
    let fixture = fixture("list", vec![ScriptStep::Text("unused".to_string())]);
    // 种子行：同 workspace 一条归档行 + 其他 workspace 一条。
    insert_session_row(
        &fixture.db_path,
        "sess_archived",
        &fixture.ws.to_string_lossy(),
        50,
        Some(60),
    );
    insert_session_row(&fixture.db_path, "sess_other_ws", "D:\\other-ws", 40, None);

    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    // 不带 workspace：全部未归档会话（oracle 的 `session/list {includeArchived}` 语义），
    // 归档行不出现；其他 workspace 的未归档行照常列出。
    let id = conn
        .send("session/list", json!({ "includeArchived": false }))
        .await;
    let listed = conn.result(id).await;
    let ids: Vec<&str> = listed["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|session| session["sessionId"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec![session_id.as_str(), "sess_other_ws"]);

    // 按 workspace（directory）过滤，含归档。
    let id = conn
        .send(
            "session/list",
            json!({
                "workspace": {
                    "workspacePath": fixture.ws.to_string_lossy(),
                    "workspaceKey": fixture.ws.to_string_lossy()
                },
                "includeArchived": true
            }),
        )
        .await;
    let with_archived = conn.result(id).await;
    let ids: Vec<&str> = with_archived["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|session| session["sessionId"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec![session_id.as_str(), "sess_archived"]);

    // 归档行排除。
    let id = conn
        .send(
            "session/list",
            json!({
                "workspace": {
                    "workspacePath": fixture.ws.to_string_lossy(),
                    "workspaceKey": fixture.ws.to_string_lossy()
                },
                "includeArchived": false
            }),
        )
        .await;
    let without_archived = conn.result(id).await;
    assert_eq!(
        without_archived["sessions"]
            .as_array()
            .expect("sessions")
            .len(),
        1
    );

    // limit=1：最近更新的一条（session 行 time_updated 最新）。
    let id = conn
        .send(
            "session/list",
            json!({
                "workspace": {
                    "workspacePath": fixture.ws.to_string_lossy(),
                    "workspaceKey": fixture.ws.to_string_lossy()
                },
                "includeArchived": true,
                "limit": 1
            }),
        )
        .await;
    let limited = conn.result(id).await;
    let ids: Vec<&str> = limited["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|session| session["sessionId"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec![session_id.as_str()]);

    // sessionIds 过滤。
    let id = conn
        .send(
            "session/list",
            json!({
                "includeArchived": true,
                "sessionIds": ["sess_archived"]
            }),
        )
        .await;
    let filtered = conn.result(id).await;
    let ids: Vec<&str> = filtered["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|session| session["sessionId"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["sess_archived"]);

    // 行形状可严格解析。
    let id = conn
        .send("session/list", json!({ "includeArchived": true }))
        .await;
    let all = conn.result(id).await;
    serde_json::from_value::<agent_protocol::session_read::SessionListResult>(all)
        .expect("list 行可严格解析");
}

// ---------------------------------------------------------------------------
// 7. close → resume：历史恢复、序号不撞
// ---------------------------------------------------------------------------

#[tokio::test]
async fn close_then_resume_restores_history_and_sequences() {
    let fixture = fixture(
        "resume",
        vec![
            ScriptStep::Text("before close".to_string()),
            ScriptStep::Text("after resume".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    conn.subscribe_default(&session_id).await;

    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "first prompt" }),
        )
        .await;
    let _ = conn.result(id).await;
    conn.drain_until_terminal().await;

    // close：{closed:true}，无其他字段。
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    let closed = conn.result(id).await;
    assert_eq!(closed, json!({ "closed": true }));
    assert_eq!(count_rows(&fixture.db_path, "message"), 2, "close 不删事实");

    // close 幂等（再 close 仍 closed:true）。
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    assert_eq!(conn.result(id).await, json!({ "closed": true }));

    // resume：恢复历史（快照带 messages）与模型（entry 事实）。
    let id = conn
        .send("session/resume", json!({ "sessionId": session_id }))
        .await;
    let resumed = conn.result(id).await;
    serde_json::from_value::<SessionStateSnapshot>(resumed.clone()).expect("resume 快照可解析");
    let messages = resumed["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "resume 恢复历史");
    let model = resumed["session"]["model"].as_object().expect("有模型");
    assert_eq!(
        model["providerId"],
        json!("mock-chat"),
        "模型来自 model_selection entry"
    );

    // 新 runtime（新事件总线）：live 事件要重新订阅（close 后旧订阅随旧 runtime 终止）。
    conn.subscribe_default(&session_id).await;

    // 再 send：新一轮正常完成，message 序号接着历史（max+1），不撞。
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "second prompt" }),
        )
        .await;
    let receipt = conn.result(id).await;
    assert_eq!(receipt["accepted"], json!(true));
    conn.drain_until_terminal().await;

    assert_eq!(count_rows(&fixture.db_path, "message"), 4);
    let connection = raw_connection(&fixture.db_path);
    let sequences: Vec<i64> = {
        let mut statement = connection
            .prepare(&format!(
                "SELECT sequence FROM message WHERE session_id = '{session_id}' ORDER BY sequence"
            ))
            .expect("prepare");
        statement
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集")
    };
    drop(connection);
    assert_eq!(
        sequences,
        vec![0, 1, 2, 3],
        "resume 后新行序号接续，不与历史冲突"
    );

    // 不存在的会话 resume：显式 -32602。
    let id = conn
        .send("session/resume", json!({ "sessionId": "sess_absent" }))
        .await;
    let (code, _, _) = conn.error(id).await;
    assert_eq!(code, -32602);
}

// ---------------------------------------------------------------------------
// 8. setters：快照回执 + runtime/entry 事实更新
// ---------------------------------------------------------------------------

#[tokio::test]
async fn setters_update_runtime_and_persist_entries() {
    let fixture = fixture(
        "setters",
        vec![ScriptStep::Text("setter answer".to_string())],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = created["runtime"]["stateRevision"].as_u64().unwrap();
    conn.subscribe_default(&session_id).await;

    // setModel：经 hub 解析（mock-2 也在白名单），快照与 entry 都更新。
    let id = conn
        .send(
            "session/setModel",
            json!({
                "sessionId": session_id,
                "model": { "providerId": "mock-chat", "modelId": "mock-2" },
                "expectedRevision": revision
            }),
        )
        .await;
    let snapshot = conn.result(id).await;
    serde_json::from_value::<SessionStateSnapshot>(snapshot.clone()).expect("setter 快照可解析");
    assert_eq!(snapshot["session"]["model"]["modelId"], json!("mock-2"));
    assert_eq!(
        snapshot["settings"]["model"]["current"]["modelId"],
        json!("mock-2")
    );
    let entry = read_column(
        &fixture.db_path,
        "session_entry",
        "data",
        &format!("session_id = '{session_id}' AND type = 'runtime/model_selection'"),
    );
    let entry: Value = serde_json::from_str(&entry).unwrap();
    assert_eq!(entry["modelSelection"]["modelId"], json!("mock-2"));

    // setMode：快照 mode 更新 + execution_state entry。
    let id = conn
        .send(
            "session/setMode",
            json!({ "sessionId": session_id, "mode": "plan" }),
        )
        .await;
    let snapshot = conn.result(id).await;
    assert_eq!(snapshot["session"]["mode"], json!("plan"));
    assert_eq!(snapshot["settings"]["mode"]["current"], json!("plan"));
    let entry = read_column(
        &fixture.db_path,
        "session_entry",
        "data",
        &format!("session_id = '{session_id}' AND type = 'runtime/execution_state'"),
    );
    let entry: Value = serde_json::from_str(&entry).unwrap();
    assert_eq!(entry["mode"], json!("plan"));

    // setThoughtLevel：快照 settings.thoughtLevel.current 更新。
    let id = conn
        .send(
            "session/setThoughtLevel",
            json!({ "sessionId": session_id, "thoughtLevel": "enabled" }),
        )
        .await;
    let snapshot = conn.result(id).await;
    assert_eq!(
        snapshot["settings"]["thoughtLevel"]["current"],
        json!("enabled")
    );

    // 过期 expectedRevision：显式冲突。
    let id = conn
        .send(
            "session/setModel",
            json!({
                "sessionId": session_id,
                "model": { "providerId": "mock-chat", "modelId": "mock-1" },
                "expectedRevision": 1
            }),
        )
        .await;
    let (code, _, _) = conn.error(id).await;
    assert_eq!(code, -32602);

    // 不可解析的模型：显式 -32603，内存事实不变。
    let id = conn
        .send(
            "session/setModel",
            json!({
                "sessionId": session_id,
                "model": { "providerId": "no-such-provider", "modelId": "mock-1" }
            }),
        )
        .await;
    let (code, _, _) = conn.error(id).await;
    assert_eq!(code, -32603);
    let id = conn
        .send("session/read", json!({ "sessionId": session_id }))
        .await;
    let snapshot = conn.result(id).await;
    assert_eq!(
        snapshot["session"]["model"]["modelId"],
        json!("mock-2"),
        "失败的 setter 不改模型事实"
    );

    // 新模型事实生效后 send 正常完成。
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "with new model" }),
        )
        .await;
    let _ = conn.result(id).await;
    let events = conn.drain_until_terminal().await;
    let terminal = events.last().expect("终态");
    assert_eq!(terminal["type"], json!("turn.completed"));
}

// ---------------------------------------------------------------------------
// 9. goal：显式 -32601（`session/compact` 已在 C2 T2 接入，行为见 tests/compact.rs）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn goal_stays_method_not_found() {
    let fixture = fixture("goal", vec![ScriptStep::Text("x".to_string())]);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/goal", json!({ "sessionId": "sess-1" }))
        .await;
    let (code, message, _) = conn.error(id).await;
    assert_eq!(code, -32601);
    assert_eq!(message, "Method not found");
}

// ---------------------------------------------------------------------------
// 10. 启动存储门：fresh 27 帧 / 二次启动 5 帧 / fail-closed
// ---------------------------------------------------------------------------

/// 收集门通知直到 ready / failed。
async fn collect_storage_gate(conn: &mut TestConn) -> Vec<Value> {
    let mut states = Vec::new();
    loop {
        match conn.next_frame().await {
            Frame::Notification(notification) => {
                assert_eq!(notification.method, "startup/storageState");
                states.push(notification.params.expect("params"));
            }
            other => panic!("门通知之后才允许有其他帧，实际 {other:?}"),
        }
        let phase = states.last().expect("有帧")["phase"]
            .as_str()
            .expect("phase");
        if phase == "ready" || phase == "failed" {
            return states;
        }
    }
}

fn assert_gate_common_fields(states: &[Value]) {
    let mut previous_sequence = 0;
    for state in states {
        assert_eq!(state["schemaVersion"], json!(1));
        assert_eq!(state["databaseKind"], json!("session"));
        assert_eq!(state["attemptId"], json!("attempt-1"));
        assert_eq!(state["databaseId"], json!("db-1"));
        let sequence = state["sequence"].as_u64().expect("sequence");
        assert!(
            sequence > previous_sequence,
            "sequence 必须严格递增：{sequence} <= {previous_sequence}"
        );
        previous_sequence = sequence;
        assert!(state["elapsedMs"].as_f64().expect("elapsedMs") >= 0.0);
        assert!(state.get("errorCode").is_none(), "非 failed 帧无 errorCode");
    }
}

#[tokio::test]
async fn storage_gate_frames_match_c2_contract() {
    // fresh 根：checking ×3 → migrating ×22 → committing ×1 → ready ×1（27 帧）。
    let root = TempRoot::new("gate-fresh");
    let hub = ProviderConfigHub::with_paths(
        write_builtin_file(root.path()),
        write_personal_file(root.path()),
    );
    let server = Arc::new(AgentServer::with_parts(
        ServerConfig::from_data_base_dir(root.path()),
        hub,
        Arc::new(ScriptedSource {
            steps: vec![ScriptStep::Text("x".to_string())],
        }),
    ));
    let mut conn = connect(Arc::clone(&server)).await;
    let states = collect_storage_gate(&mut conn).await;
    assert_eq!(states.len(), 27, "fresh 启动必须发 27 帧");
    let phases: Vec<&str> = states
        .iter()
        .map(|state| state["phase"].as_str().expect("phase"))
        .collect();
    let mut expected_phases = vec!["checking", "checking", "checking"];
    expected_phases.extend(std::iter::repeat_n("migrating", 22));
    expected_phases.push("committing");
    expected_phases.push("ready");
    assert_eq!(phases, expected_phases, "phase 序列与 C2 契约 §5.1 一致");
    assert_gate_common_fields(&states);

    // 前两个 checking 无 migration 事实；第 3 个带预检事实（initialize，无基线键）。
    assert!(states[0].get("migration").is_none());
    assert!(states[1].get("migration").is_none());
    assert_eq!(
        states[2]["migration"],
        json!({ "kind": "initialize", "executedCount": 0, "committedCount": 0 })
    );

    // migrating 帧：completed 0..21、total 22、migrationId = 冻结清单顺序。
    let frozen_ids: Vec<&str> = session_store::migration_definitions()
        .iter()
        .map(|definition| definition.id)
        .collect();
    for (index, state) in states[3..25].iter().enumerate() {
        assert_eq!(state["phase"], json!("migrating"));
        assert_eq!(state["completed"], json!(index as u64), "执行前发出");
        assert_eq!(state["total"], json!(22));
        assert_eq!(state["migrationId"], json!(frozen_ids[index]));
        assert_eq!(
            state["migration"],
            json!({
                "kind": "initialize",
                "executedCount": index as u64,
                "committedCount": 0,
                "lastAppliedMigrationId": null,
            })
        );
    }
    // committing：22 步已执行、尚未提交；ready：committedCount 追平。
    assert_eq!(
        states[25]["migration"],
        json!({
            "kind": "initialize",
            "executedCount": 22,
            "committedCount": 0,
            "lastAppliedMigrationId": null,
        })
    );
    assert_eq!(
        states[26]["migration"],
        json!({
            "kind": "initialize",
            "executedCount": 22,
            "committedCount": 22,
            "lastAppliedMigrationId": null,
        })
    );

    // 空根被创建成 CLI 布局的库。
    assert!(
        root.path()
            .join(".mode")
            .join("cli")
            .join("db")
            .join("db.sqlite")
            .is_file(),
        "fresh 根必须创建 db.sqlite"
    );

    // 第二次启动（同一数据根）：checking ×3 → committing ×1 → ready ×1，无 migrating。
    let hub = ProviderConfigHub::with_paths(
        write_builtin_file(root.path()),
        write_personal_file(root.path()),
    );
    let server = Arc::new(AgentServer::with_parts(
        ServerConfig::from_data_base_dir(root.path()),
        hub,
        Arc::new(ScriptedSource {
            steps: vec![ScriptStep::Text("x".to_string())],
        }),
    ));
    let mut conn = connect(server).await;
    let states = collect_storage_gate(&mut conn).await;
    assert_eq!(states.len(), 5, "第二次启动必须发 5 帧");
    let phases: Vec<&str> = states
        .iter()
        .map(|state| state["phase"].as_str().expect("phase"))
        .collect();
    assert_eq!(
        phases,
        vec!["checking", "checking", "checking", "committing", "ready"]
    );
    assert_gate_common_fields(&states);
    assert_eq!(
        states[2]["migration"],
        json!({ "kind": "none", "executedCount": 0, "committedCount": 0 })
    );
    assert_eq!(
        states[3]["migration"],
        json!({
            "kind": "none",
            "executedCount": 0,
            "committedCount": 0,
            "lastAppliedMigrationId": "0022_backfilled_session_reasoning",
        })
    );
    assert_eq!(
        states[4]["migration"],
        json!({
            "kind": "none",
            "executedCount": 0,
            "committedCount": 0,
            "lastAppliedMigrationId": "0022_backfilled_session_reasoning",
        })
    );
}

#[tokio::test]
async fn storage_gate_fails_closed_on_checksum_mismatch() {
    // failed：账本 checksum 与冻结清单不一致（fail-closed 契约不变）。
    let root = TempRoot::new("gate-failed");
    let db_dir = root.path().join(".mode").join("cli").join("db");
    std::fs::create_dir_all(&db_dir).expect("建目录");
    let db_path = db_dir.join("db.sqlite");
    Connection::open(&db_path)
        .expect("空库文件")
        .execute_batch(
            "CREATE TABLE dummy (id text);
             CREATE TABLE schema_migration (
               id text primary key, checksum text not null, app_version text, time_applied integer not null
             );
             INSERT INTO schema_migration (id, checksum, app_version, time_applied)
             VALUES ('0001_base_session_store', 'checksum-from-another-impl', '0.2.0', 1);",
        )
        .unwrap();
    let hub = ProviderConfigHub::with_paths(
        write_builtin_file(root.path()),
        write_personal_file(root.path()),
    );
    let server = Arc::new(AgentServer::with_parts(
        ServerConfig::from_data_base_dir(root.path()),
        hub,
        Arc::new(ScriptedSource {
            steps: vec![ScriptStep::Text("x".to_string())],
        }),
    ));
    let mut conn = connect(server).await;
    let states = collect_storage_gate(&mut conn).await;
    let failed = states.last().expect("有帧");
    assert_eq!(failed["phase"], json!("failed"));
    assert_eq!(failed["schemaVersion"], json!(1));
    assert_eq!(failed["databaseKind"], json!("session"));
    // 稳定错误码来自 session_store::StoreError::code()（agent-core-parity.md §6），
    // agent-server 不另造取值。
    assert_eq!(failed["errorCode"], json!("storage_schema_unsupported"));

    // fail-closed：session 方法显式内部错误，不静默降级。
    let id = conn
        .send(
            "session/create",
            json!({ "workspace": { "workspacePath": "/ws/x", "workspaceKey": "/ws/x" } }),
        )
        .await;
    let (code, message, _) = conn.error(id).await;
    assert_eq!(code, -32603);
    assert!(message.contains("会话库不可用"), "实际：{message}");
}

// ---------------------------------------------------------------------------
// 11. 缺模型：create 允许，send 的 turn 以 CONFIGURATION_ERROR 失败
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_without_model_fails_turn_with_configuration_error() {
    let fixture = fixture("no-model", vec![ScriptStep::Text("never used".to_string())]);
    let mut conn = connect(Arc::clone(&fixture.server)).await;

    // create 不带 model：允许（快照无 model 键）。
    let id = conn
        .send("session/create", create_params(&fixture.ws, false))
        .await;
    let created = conn.result(id).await;
    serde_json::from_value::<SessionStateSnapshot>(created.clone()).expect("快照可解析");
    assert!(
        created["session"].get("model").is_none(),
        "无模型时 model 键省略"
    );
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    conn.subscribe_default(&session_id).await;

    // send 被接受，turn 立即失败：CONFIGURATION_ERROR / "Select a model before continuing"。
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "no model" }),
        )
        .await;
    assert_eq!(conn.result(id).await["accepted"], json!(true));
    let events = conn.drain_until_terminal().await;
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().expect("type"))
        .collect();
    assert_eq!(kinds, vec!["turn.started", "turn.failed"]);
    let failed = events.last().expect("终态");
    assert_eq!(
        failed["payload"]["error"]["type"],
        json!("CONFIGURATION_ERROR")
    );
    assert_eq!(
        failed["payload"]["error"]["message"],
        json!("Select a model before continuing")
    );

    // 不可解析的模型同样在 turn 上显式失败（create 宽容 / send 严格——裁决 7）。
    let mut params = create_params(&fixture.ws, false);
    params["model"] = json!({ "providerId": "ghost-provider", "modelId": "ghost-model" });
    let id = conn.send("session/create", params).await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    conn.subscribe_default(&session_id).await;
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "ghost" }),
        )
        .await;
    let _ = conn.result(id).await;
    let events = conn.drain_until_terminal().await;
    let failed = events.last().expect("终态");
    assert_eq!(failed["type"], json!("turn.failed"));
    assert_eq!(
        failed["payload"]["error"]["type"],
        json!("CONFIGURATION_ERROR")
    );
    assert!(
        failed["payload"]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("ghost-provider"),
        "错误信息带 provider 名：{failed}"
    );

    // setModel 到可解析模型后，send 恢复正常（ScriptedProvider 剧本兜底）。
    let id = conn
        .send(
            "session/setModel",
            json!({
                "sessionId": session_id,
                "model": { "providerId": "mock-chat", "modelId": "mock-1" }
            }),
        )
        .await;
    let _ = conn.result(id).await;
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "after fix" }),
        )
        .await;
    let _ = conn.result(id).await;
    let events = conn.drain_until_terminal().await;
    assert_eq!(
        events.last().expect("终态")["type"],
        json!("turn.completed")
    );
}

// ---------------------------------------------------------------------------
// 12. close 的 drain 窗口：resume / 同 id create 显式拒绝，drain 后恢复（复评裁决 2）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_during_close_drain_rejects_then_recovers() {
    let release = Arc::new(Notify::new());
    let fixture = fixture_with_source(
        "close-drain",
        Arc::new(GatedSource {
            release: Arc::clone(&release),
        }),
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    conn.subscribe_default(&session_id).await;

    // in-1 立即 running（被 gate 卡住）；in-2 在 close 前已排队（CommandInbox 的
    // FIFO 准入与 worker 进度无关：in-1 恒为 running，in-2 恒为 queued，因此
    // close 时 drain 窗口是确定的，不依赖事件观察）。
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "will be cancelled", "inputId": "in-1" }),
        )
        .await;
    let _ = conn.result(id).await;
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "queued behind", "inputId": "in-2" }),
        )
        .await;
    assert_eq!(conn.result(id).await["accepted"], json!(true));

    // close：runtime 进 drain 窗口（in-1 取消、in-2 照常 drain）。
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    assert_eq!(conn.result(id).await, json!({ "closed": true }));

    // drain 窗口内 resume：显式 -32603（不另起第二个 runtime owner）。
    let id = conn
        .send("session/resume", json!({ "sessionId": session_id }))
        .await;
    let (code, message, _) = conn.error(id).await;
    assert_eq!(code, -32603);
    assert!(message.contains("正在关闭"), "实际：{message}");

    // 同 id create 同守卫。
    let mut params = create_params(&fixture.ws, true);
    params["sessionId"] = json!(session_id);
    let id = conn.send("session/create", params).await;
    let (code, message, _) = conn.error(id).await;
    assert_eq!(code, -32603);
    assert!(message.contains("正在关闭"), "实际：{message}");

    // 放行 → in-1 取消收口、in-2 正常完成 → worker 退出 → drain 完成、台账清理。
    release.notify_one();
    // 轮询直到 drain 窗口关闭（旧订阅还能收到 in-2 的终态；resume 可用即台账已清）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let resumed = loop {
        let id = conn
            .send("session/resume", json!({ "sessionId": session_id }))
            .await;
        match tokio::time::timeout(Duration::from_secs(5), conn.error_or_result(id)).await {
            Ok(Ok(value)) => break value,
            Ok(Err((_, message, _))) if message.contains("正在关闭") => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "drain 窗口未在期限内关闭"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(Err((code, message, _))) => panic!("resume 失败 ({code}): {message}"),
            Err(_) => panic!("resume 轮询超时"),
        }
    };
    serde_json::from_value::<SessionStateSnapshot>(resumed.clone()).expect("resume 快照可解析");
    // drain 期间的行都在：in-1 只落 user（取消），in-2 user+assistant → 3 条。
    assert_eq!(resumed["messages"].as_array().expect("messages").len(), 3);

    // 恢复后的会话只有一个 runtime owner：继续 send 正常完成，序号接续不撞。
    // （门控 provider 是一次性 permit：in-3 的模型请求需要再放行一次。）
    conn.subscribe_default(&session_id).await;
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "after recovery", "inputId": "in-3" }),
        )
        .await;
    let _ = conn.result(id).await;
    release.notify_one();
    conn.drain_until_terminal().await;

    let connection = raw_connection(&fixture.db_path);
    let mut sequences: Vec<i64> = {
        let mut statement = connection
            .prepare(&format!(
                "SELECT COALESCE(sequence, -1) FROM message \
                 WHERE session_id = '{session_id}' ORDER BY COALESCE(sequence, -1)"
            ))
            .expect("prepare");
        statement
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集")
    };
    let duplicate_count = sequences.len() - {
        sequences.sort();
        sequences.dedup();
        sequences.len()
    };
    drop(connection);
    assert_eq!(sequences, vec![0, 1, 2, 3, 4], "序号接续");
    assert_eq!(duplicate_count, 0, "无重复 message.sequence");
}

// ---------------------------------------------------------------------------
// 12. 最终评审修复波：共享库 / data_root / setMode / identity / 陈旧游标 / DB 形状
// ---------------------------------------------------------------------------

/// 两个合法 workspace 的 AgentServer 可以同时通过同一会话库的存储门并各自写会话。
#[tokio::test]
async fn two_workspaces_share_one_database_gate() {
    let root = TempRoot::new("multi-workspace");
    let mode_root = root.path().join(".mode");
    let db_dir = mode_root.join("cli").join("db");
    std::fs::create_dir_all(&db_dir).expect("建 cli/db");
    let db_path = db_dir.join("db.sqlite");
    create_baseline_database(&db_path);

    let ws_a = root.path().join("ws-a");
    let ws_b = root.path().join("ws-b");
    std::fs::create_dir_all(&ws_a).expect("ws-a");
    std::fs::create_dir_all(&ws_b).expect("ws-b");

    let source_a: Arc<dyn ProviderSource> = Arc::new(ScriptedSource {
        steps: vec![ScriptStep::Text("a".to_string())],
    });
    let source_b: Arc<dyn ProviderSource> = Arc::new(ScriptedSource {
        steps: vec![ScriptStep::Text("b".to_string())],
    });
    let hub_a = ProviderConfigHub::with_paths(
        write_builtin_file(root.path()),
        write_personal_file(root.path()),
    );
    let hub_b = ProviderConfigHub::with_paths(
        write_builtin_file(root.path()),
        write_personal_file(root.path()),
    );
    let server_a = Arc::new(AgentServer::with_parts(
        ServerConfig::from_data_base_dir(root.path()),
        hub_a,
        source_a,
    ));
    let server_b = Arc::new(AgentServer::with_parts(
        ServerConfig::from_data_base_dir(root.path()),
        hub_b,
        source_b,
    ));

    let mut conn_a = connect(server_a).await;
    let mut conn_b = connect(server_b).await;
    let id = conn_a
        .send("session/create", create_params(&ws_a, true))
        .await;
    let created_a = conn_a.result(id).await;
    let id = conn_b
        .send("session/create", create_params(&ws_b, true))
        .await;
    let created_b = conn_b.result(id).await;
    assert_eq!(created_a["session"]["status"], json!("idle"));
    assert_eq!(created_b["session"]["status"], json!("idle"));
    assert_eq!(
        count_rows(&db_path, "session"),
        2,
        "两个 workspace 各写一条"
    );
    assert!(
        !db_path.with_extension("sqlite.agent-write.lock").exists(),
        "不得再创建进程独占锁"
    );
}

/// 大 Bash 输出必须落到 `<dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log`。
#[tokio::test]
async fn large_bash_output_lands_under_the_retained_data_root() {
    let fixture = fixture(
        "artifact-root",
        vec![
            ScriptStep::ToolCalls(vec![agent_core::provider::ScriptToolCall {
                name: "Bash".to_string(),
                input: json!({ "command": "printf 'a%.0s' {1..40000}" }),
            }]),
            ScriptStep::Text("done".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    conn.subscribe_default(&session_id).await;
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "big output" }),
        )
        .await;
    let _ = conn.result(id).await;
    let events = conn.drain_until_terminal().await;
    assert!(
        events
            .iter()
            .any(|event| event["type"] == json!("turn.completed")),
        "turn 正常完成"
    );

    let artifact = fixture
        ._root
        .path()
        .join(".mode")
        .join("cli")
        .join("exec")
        .join(&session_id)
        .join("toolu_mock_0_0-stdout.log");
    let metadata = std::fs::metadata(&artifact)
        .unwrap_or_else(|error| panic!("artifact 必须落在 {artifact:?}（实际错误 {error}）"));
    assert_eq!(metadata.len(), 40_000, "完整输出落盘");
}

/// `setMode` 必须同时更新热快照、`session/list` 与冷读三个口径。
#[tokio::test]
async fn set_mode_updates_hot_list_and_cold_views() {
    let fixture = fixture(
        "set-mode-consistency",
        vec![ScriptStep::Text("answer".to_string())],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    conn.subscribe_default(&session_id).await;

    let id = conn
        .send(
            "session/setMode",
            json!({ "sessionId": session_id, "mode": "plan" }),
        )
        .await;
    let hot = conn.result(id).await;
    assert_eq!(hot["session"]["mode"], json!("plan"));
    assert_eq!(hot["projection"]["mode"], json!("plan"));

    let id = conn
        .send(
            "session/list",
            json!({ "workspace": { "workspacePath": fixture.ws.to_string_lossy(), "workspaceKey": fixture.ws.to_string_lossy() }, "includeArchived": true }),
        )
        .await;
    let listed = conn.result(id).await;
    assert_eq!(
        listed["sessions"][0]["mode"],
        json!("plan"),
        "list 必须从 session.permission 读到新 mode"
    );

    // DB 的 permission 列同步更新。
    let permission = read_column(&fixture.db_path, "session", "permission", "1=1");
    assert_eq!(permission, r#"{"mode":"plan"}"#);

    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    conn.result(id).await;
    // 冷读不拉起 runtime，直接投影 DB 行。
    let id = conn
        .send("session/read", json!({ "sessionId": session_id }))
        .await;
    let cold = conn.result(id).await;
    assert_eq!(cold["session"]["mode"], json!("plan"));
    assert_eq!(cold["settings"]["mode"]["current"], json!("plan"));
}

/// `workspaceIdentity` 在热快照、list 与冷读里都必须保留，key 采用 identity。
#[tokio::test]
async fn workspace_identity_survives_hot_list_and_cold_views() {
    let fixture = fixture(
        "workspace-identity",
        vec![ScriptStep::Text("answer".to_string())],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let mut params = create_params(&fixture.ws, true);
    params["workspace"]["workspaceIdentity"] = json!("ws-identity-9");
    let id = conn.send("session/create", params).await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    assert_eq!(
        created["session"]["workspace"]["workspaceIdentity"],
        json!("ws-identity-9")
    );
    assert_eq!(
        created["session"]["workspace"]["workspaceKey"],
        json!("ws-identity-9")
    );

    let id = conn
        .send(
            "session/list",
            json!({ "workspace": { "workspacePath": fixture.ws.to_string_lossy(), "workspaceKey": fixture.ws.to_string_lossy() }, "includeArchived": true }),
        )
        .await;
    let listed = conn.result(id).await;
    assert_eq!(
        listed["sessions"][0]["workspace"]["workspaceIdentity"],
        json!("ws-identity-9")
    );
    assert_eq!(
        listed["sessions"][0]["workspace"]["workspaceKey"],
        json!("ws-identity-9")
    );

    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    conn.result(id).await;
    let id = conn
        .send("session/read", json!({ "sessionId": session_id }))
        .await;
    let cold = conn.result(id).await;
    assert_eq!(
        cold["session"]["workspace"]["workspaceIdentity"],
        json!("ws-identity-9")
    );
    assert_eq!(
        cold["session"]["workspace"]["workspaceKey"],
        json!("ws-identity-9")
    );
}

/// resume 后的订阅带着旧 runtime 的 afterSeq：不能过滤掉新 runtime 的事件。
#[tokio::test]
async fn subscribe_with_old_cursor_after_resume_still_forwards_live_events() {
    let fixture = fixture(
        "resume-old-cursor",
        vec![
            ScriptStep::Text("before close".to_string()),
            ScriptStep::Text("after resume".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    conn.subscribe_default(&session_id).await;
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "first" }),
        )
        .await;
    let _ = conn.result(id).await;
    conn.drain_until_terminal().await;
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    conn.result(id).await;
    let id = conn
        .send("session/resume", json!({ "sessionId": session_id }))
        .await;
    let _ = conn.result(id).await;

    // 旧 runtime 的事件序号（例如 12）在新 runtime 里不存在；按 reset 处理。
    let id = conn
        .send(
            "session/subscribe",
            json!({
                "sessionId": session_id,
                "deliveryKind": "desktop-continuous",
                "includeSnapshot": false,
                "afterSeq": 999
            }),
        )
        .await;
    let subscribed = conn.result(id).await;
    assert_eq!(subscribed["events"], json!([]), "旧游标没有可回放事件");
    assert!(
        subscribed["eventSeq"].as_u64().unwrap_or(u64::MAX) < 999,
        "新 runtime 的 eventSeq 低于旧游标"
    );

    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "second" }),
        )
        .await;
    let _ = conn.result(id).await;
    let events = conn.drain_until_terminal().await;
    assert!(
        events
            .iter()
            .any(|event| event["type"] == json!("turn.started")),
        "旧游标不能丢掉新 runtime 的 live 事件"
    );
}

/// DB 形状：与 `session-lifecycle-db.json` 的 message/part 行逐字段对齐
///（合成 timeline 行除外；其余未对齐字段登记在 `tauri-agent-runtime.md` §C1.3-6）。
#[tokio::test]
async fn db_message_and_part_shape_matches_the_lifecycle_fixture_contract() {
    let fixture = fixture("db-shape", Vec::new());
    // 读的是绝对路径（夹具录制值）；steps 需要 fixture 的 ws，因此在 create 前换源。
    fixture.server.set_provider_source(Arc::new(ScriptedSource {
        steps: vec![
            ScriptStep::Text("first turn answer".to_string()),
            ScriptStep::ToolCalls(vec![agent_core::provider::ScriptToolCall {
                name: "Read".to_string(),
                input: json!({ "file_path": fixture.ws.join("hello.txt").to_string_lossy() }),
            }]),
            ScriptStep::Text("second turn answer".to_string()),
        ],
    }));
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    conn.subscribe_default(&session_id).await;
    for prompt in ["first prompt", "Read hello.txt and summarize"] {
        let id = conn
            .send(
                "session/send",
                json!({ "sessionId": session_id, "content": prompt }),
            )
            .await;
        let _ = conn.result(id).await;
        conn.drain_until_terminal().await;
    }

    let oracle_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("desktop")
        .join("tests")
        .join("agent-runtime-oracle")
        .join("fixtures")
        .join("session-lifecycle")
        .join("session-lifecycle-db.json");
    let oracle: Value =
        serde_json::from_str(&std::fs::read_to_string(&oracle_path).expect("读取 oracle DB 夹具"))
            .expect("oracle DB 夹具是 JSON");
    // 行 0 是 C1 不写的合成 timeline 消息（§C1.3-5）；其余逐行对拍。
    let oracle_messages: Vec<Value> = oracle["expect"]["tables"]["message"]["rows"]
        .as_array()
        .expect("message rows")
        .iter()
        .skip(1)
        .cloned()
        .collect();
    let oracle_parts: Vec<Value> = oracle["expect"]["tables"]["part"]["rows"]
        .as_array()
        .expect("part rows")
        .iter()
        .skip(1)
        .cloned()
        .collect();

    let connection = raw_connection(&fixture.db_path);
    let mut statement = connection
        .prepare(
            "SELECT id, data FROM message WHERE session_id = ?1 \
             ORDER BY COALESCE(sequence, -1), time_created, id",
        )
        .expect("prepare message");
    let messages: Vec<(String, Value)> = statement
        .query_map([session_id.as_str()], |row| {
            let id: String = row.get(0)?;
            let data: String = row.get(1)?;
            Ok((
                id,
                serde_json::from_str(&data).expect("message.data 是 JSON"),
            ))
        })
        .expect("query message")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect message");
    assert_eq!(
        messages.len(),
        oracle_messages.len(),
        "message 行数（oracle 6 行减去合成 timeline 1 行）"
    );
    for ((_id, ours), expected_row) in messages.iter().zip(oracle_messages.iter()) {
        let expected = expected_row.get("data").expect("oracle message.data");
        assert_eq!(ours.get("role"), expected.get("role"), "role");
        assert_eq!(ours.get("finish"), expected.get("finish"), "finish");
        assert_eq!(
            ours.get("tokens"),
            expected.get("tokens"),
            "每个 assistant 行的 tokens 必须是该步骤模型请求的 usage"
        );
        assert_eq!(
            ours.get("semantics"),
            expected.get("semantics"),
            "semantics"
        );
    }

    let mut part_statement = connection
        .prepare(
            "SELECT data FROM part WHERE message_id = ?1 \
             ORDER BY COALESCE(sequence, -1), time_created, id",
        )
        .expect("prepare part");
    let mut ours_parts: Vec<Value> = Vec::new();
    for (message_id, _) in &messages {
        let parts = part_statement
            .query_map([message_id.as_str()], |row| {
                let data: String = row.get(0)?;
                Ok(serde_json::from_str::<Value>(&data).expect("part.data 是 JSON"))
            })
            .expect("query part")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect part");
        ours_parts.extend(parts);
    }
    assert_eq!(
        ours_parts.len(),
        oracle_parts.len(),
        "part 行数（oracle 12 行减去 timeline part 1 行）"
    );
    for (ours, expected_row) in ours_parts.iter().zip(oracle_parts.iter()) {
        let expected = expected_row.get("data").expect("oracle part.data");
        assert_eq!(ours.get("type"), expected.get("type"), "part type");
        match ours.get("type").and_then(Value::as_str) {
            Some("text") => assert_eq!(ours.get("text"), expected.get("text"), "text part"),
            Some("step-finish") => {
                assert_eq!(
                    ours.get("reason"),
                    expected.get("reason"),
                    "step-finish reason"
                );
                assert_eq!(
                    ours.get("tokens"),
                    expected.get("tokens"),
                    "step-finish tokens"
                );
                assert_eq!(ours.get("cost"), expected.get("cost"), "step-finish cost");
            }
            Some("tool") => {
                assert_eq!(ours.get("tool"), expected.get("tool"), "tool name");
                let ours_state = ours.get("state").expect("state");
                let expected_state = expected.get("state").expect("oracle state");
                assert_eq!(ours_state.get("status"), expected_state.get("status"));
                assert_eq!(
                    ours_state
                        .get("input")
                        .map(|value| normalize_workspace(value, &fixture.ws)),
                    expected_state.get("input").cloned(),
                    "tool state.input（路径归一成 <cwd>）"
                );
                assert_eq!(ours_state.get("output"), expected_state.get("output"));
                assert_eq!(ours_state.get("title"), expected_state.get("title"));
                // DB 保留 readFileState；C1 的 DB 尚未写它（§C1.3-6），比较时两侧剔除。
                assert_eq!(
                    prune_private_metadata(ours_state.get("metadata")),
                    prune_private_metadata(expected_state.get("metadata")),
                    "tool state.metadata（readFileState 两侧剔除）"
                );
            }
            _ => {}
        }
    }
}

/// 把工作区绝对路径归一成夹具的 `<cwd>` 占位符（反斜杠统一成 `/`）。
fn normalize_workspace(value: &Value, workspace: &Path) -> Value {
    let prefix = workspace.to_string_lossy();
    match value {
        Value::String(text) => Value::String(
            text.replace(prefix.as_ref(), "<cwd>")
                .replace(std::path::MAIN_SEPARATOR, "/"),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_workspace(item, workspace))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, inner)| (key.clone(), normalize_workspace(inner, workspace)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// 递归剔除工具私有 metadata 键（`readFileState`）；返回处理后的对象/值。
fn prune_private_metadata(value: Option<&Value>) -> Option<Value> {
    let mut value = value?.clone();
    fn prune(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.retain(|key, _| !key.starts_with("readFileState"));
                for inner in map.values_mut() {
                    prune(inner);
                }
            }
            Value::Array(items) => {
                for item in items {
                    prune(item);
                }
            }
            _ => {}
        }
    }
    prune(&mut value);
    Some(value)
}

/// `sessionIds` 过滤必须先于 `limit` 截断：先 SQL LIMIT 会把目标会话切掉。
#[tokio::test]
async fn list_filters_session_ids_before_applying_limit() {
    let fixture = fixture("list-ids-limit", Vec::new());
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let mut ids = Vec::new();
    for index in 0..3 {
        let mut params = create_params(&fixture.ws, true);
        params["sessionId"] = json!(format!("sess_list_{index}"));
        let id = conn.send("session/create", params).await;
        let _ = conn.result(id).await;
        ids.push(format!("sess_list_{index}"));
    }
    // 固定 time_updated，让排序可预测：sess_list_2 最新、sess_list_0 最旧。
    let writer = Connection::open(&fixture.db_path).expect("写连接");
    for (index, session_id) in ids.iter().enumerate() {
        writer
            .execute(
                "UPDATE session SET time_updated = ?2 WHERE id = ?1",
                rusqlite::params![session_id, index as i64 + 1],
            )
            .expect("更新时间");
    }
    drop(writer);

    let id = conn
        .send(
            "session/list",
            json!({
                "includeArchived": true,
                "limit": 1,
                "sessionIds": [ids[0]]
            }),
        )
        .await;
    let listed = conn.result(id).await;
    let sessions = listed["sessions"].as_array().expect("sessions");
    assert_eq!(sessions.len(), 1, "过滤后仍应返回目标会话");
    assert_eq!(sessions[0]["sessionId"], json!(ids[0]));
}

/// `session/messages` 是投影：`semantics` 保留，工具私有 `readFileState` 不进 wire，
/// 即使 DB 行里有它（最终评审 I2 / contract §3.7）。
#[tokio::test]
async fn messages_projection_keeps_semantics_and_strips_private_tool_metadata() {
    let fixture = fixture("wire-projection", Vec::new());
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    let session_id = created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    let writer = Connection::open(&fixture.db_path).expect("写连接");
    writer
        .execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence) \
             VALUES ('msg_proj', ?1, 10, 11, ?2, 999)",
            rusqlite::params![
                session_id,
                json!({
                    "role": "assistant",
                    "messageId": "msg_proj",
                    "providerId": "mock-chat",
                    "modelId": "mock-1",
                    "mode": "build",
                    "finish": "stop",
                    "time": { "created": 10, "completed": 11 },
                    "semantics": {
                        "origin": "agent_runtime",
                        "kind": "assistant_response",
                        "uiVisibility": "visible",
                        "providerVisibility": "visible",
                        "transcriptVisibility": "visible"
                    }
                })
                .to_string()
            ],
        )
        .expect("写 message");
    writer
        .execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data, sequence) \
             VALUES ('part_proj', 'msg_proj', ?1, 10, 11, ?2, 0)",
            rusqlite::params![
                session_id,
                json!({
                    "type": "tool",
                    "callID": "call_proj",
                    "tool": "Read",
                    "state": {
                        "status": "completed",
                        "input": { "file_path": "hello.txt" },
                        "output": "1\thello\n2\t",
                        "title": "Read",
                        "metadata": {
                            "schemaVersion": 1,
                            "readFileState": { "path": "hello.txt", "content": "hello\n" }
                        },
                        "time": { "start": 10, "end": 11 }
                    }
                })
                .to_string()
            ],
        )
        .expect("写 part");
    drop(writer);

    let id = conn
        .send("session/messages", json!({ "sessionId": session_id }))
        .await;
    let result = conn.result(id).await;
    let messages = result["messages"].as_array().expect("messages");
    let projected = messages
        .iter()
        .find(|message| message["info"]["messageId"] == json!("msg_proj"))
        .expect("投影包含手写行");
    assert_eq!(
        projected["info"]["semantics"]["origin"],
        json!("agent_runtime"),
        "semantics 必须进 wire"
    );
    let tool = &projected["parts"][0];
    assert_eq!(tool["type"], json!("tool"));
    assert!(
        tool["state"]["metadata"].get("readFileState").is_none(),
        "readFileState 是运行时内部状态，不进 wire：{}",
        tool["state"]["metadata"]
    );
    assert_eq!(
        tool["state"]["metadata"]["schemaVersion"],
        json!(1),
        "非私有 metadata 保留"
    );
}
