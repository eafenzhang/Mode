//! 权限与反向请求集成测试（LT-1 C1 · Task 8）。
//!
//! 本文件**扮演 stdio 对端的宿主**：真实 `run_stdio` over duplex，读反向请求帧并
//! 应答（审批三态、偏好默认值、`-32601`）。覆盖：
//!
//! 1. `allow_once`：审批反向请求 → 应答 → 工具执行 → `permission.resolved` +
//!    `tool.updated(result)`；
//! 2. `deny`：只留 `scheduled` + `batch(errorCount=1)`，无 started/result/error；
//! 3. `allow_project`：`permissionUpdates` 写 `local_setting` 行，第二次同规则请求
//!    直接放行（不发反向请求）；新运行时（同库）经加载的规则集同样放行；
//! 4. 运行时偏好：物化即发一次、应答缓存；`-32601` 回退登记保守默认；活跃运行时
//!    resume 不重发；
//! 5. provider 运行时头：账号型 provider 解析触发
//!    `interaction/requestProviderRuntimeHeaders`，`headersApplied:true` 用
//!    `requestAuth.apiKey` 构造可用配置（key 只进 Authorization 路径、不进 Debug）；
//!    `false` / `-32601` / 无钩子 → 显式缺 key 错误；
//! 6. 反向请求生命周期：宿主关闭 stdio → pending 审批 deny + 终态恰好一次，不悬挂；
//!    审批超时同样 deny。
//!
//! 测试库 DDL 从 `scripts/sqlite-schema-baseline.json` 生成（含 `local_setting`，
//! 与 session-store / methods / runtime 测试同一口径；helper 因跨测试目标不能
//! 直接引用，在此按同一实现复制）。模型端点是测试自写的 mock 值，绝不发起真实
//! 网络请求；凭据 `tok-*` 是测试内造的假值。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::provider::{ModelProvider, ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_protocol::frame::{Frame, RequestId, ResponseFrame};
use agent_protocol::permission::PermissionRequest;
use agent_protocol::session_event::{SessionEvent, SessionEventType};
use agent_server::{
    AgentServer, HubProviderSource, ProviderConfigHub, ProviderRequestContext, ProviderSource,
    ReverseHub, RuntimeConfig, RuntimePreferences, RuntimeWiring, SessionInput, SessionRuntime,
};
use rusqlite::{Connection as SqliteConnection, OpenFlags};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// 基线测试库夹具（与 session-store/tests/common、tests/runtime.rs 同一口径）
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
            "agent-server-permissions-{label}-{}-{}",
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

/// 按基线生成完整测试库（全部表 + 索引 + 账本），与 session-store 测试同一实现。
fn create_baseline_database(path: &Path) {
    let tables = baseline()["databases"]["session-store"]["tables"]
        .as_object()
        .expect("基线缺少 databases[session-store].tables")
        .clone();
    let connection = SqliteConnection::open(path).expect("创建测试库");
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
    let ledger = SqliteConnection::open(path).expect("打开测试库写账本");
    ledger.execute_batch(LEDGER_DDL).expect("创建账本");
    ledger
        .execute(
            "INSERT INTO schema_migration (id, checksum, app_version, time_applied) \
             VALUES ('0001_fixture', 'fixture-checksum', '0.0.0', 1)",
            [],
        )
        .expect("写入账本行");
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
            .map(|(_, name)| quote_identifier(name))
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

fn raw_connection(path: &Path) -> SqliteConnection {
    let connection = SqliteConnection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("只读连接");
    connection
        .busy_timeout(Duration::from_secs(5))
        .expect("busy_timeout");
    connection
}

/// `local_setting` 里当前 ruleset 行的 value（不存在 panic，由用例先断言存在性）。
fn ruleset_value(path: &Path, scope_id: &str) -> Value {
    let text: String = raw_connection(path)
        .query_row(
            "SELECT value FROM local_setting \
             WHERE scope='project' AND namespace='permission' AND key='ruleset' AND scope_id=?1",
            [scope_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("读取 ruleset 行失败：{error}"));
    serde_json::from_str(&text).expect("ruleset 行是 JSON")
}

fn count_rows(path: &Path, table: &str) -> i64 {
    raw_connection(path)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|error| panic!("统计 {table} 行数失败：{error}"))
}

// ---------------------------------------------------------------------------
// 夹具：数据根 + provider 装配 + stdio 双工连接
// ---------------------------------------------------------------------------

/// 脚本化装配源：第 n 次 resolve 产出第 n 份剧本的新 provider 实例（超出按最后
/// 一份循环）。会话 1 / 会话 2 需要不同剧本（后者从 Read+Write 批次开始）时用。
struct ScriptedSource {
    scripts: Vec<Vec<ScriptStep>>,
    call: std::sync::atomic::AtomicUsize,
}

impl ScriptedSource {
    fn single(steps: Vec<ScriptStep>) -> Self {
        Self {
            scripts: vec![steps],
            call: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl ProviderSource for ScriptedSource {
    fn resolve(
        &self,
        _provider_id: &str,
        _model_id: &str,
        _context: Option<&ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        let index = self.call.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let steps = self
            .scripts
            .get(index.min(self.scripts.len() - 1))
            .expect("剧本非空");
        Ok(Arc::new(ScriptedProvider::new(steps.clone())))
    }
}

/// 运行时级夹具（生命周期/超时用例直接构造 SessionRuntime）。
struct RuntimeFixture {
    runtime: SessionRuntime,
    db_path: PathBuf,
    ws: PathBuf,
    _store: Arc<session_store::SessionStore>,
    _root: TempRoot,
}

fn runtime_fixture(
    label: &str,
    make_steps: impl FnOnce(&Path) -> Vec<ScriptStep>,
    wiring: RuntimeWiring,
) -> RuntimeFixture {
    let root = TempRoot::new(label);
    let mode_root = root.path().join(".mode");
    let db_dir = mode_root.join("cli").join("db");
    std::fs::create_dir_all(&db_dir).expect("建 cli/db 目录");
    let db_path = db_dir.join("db.sqlite");
    create_baseline_database(&db_path);
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).expect("建工作目录");
    let store = Arc::new(session_store::SessionStore::open(&db_path).expect("打开测试库"));
    let record = session_store::SessionRecord {
        id: "sess_runtime".to_owned(),
        project_id: agent_core::scope_id_for(&ws.to_string_lossy()),
        workspace_id: None,
        parent_id: None,
        slug: "sess_runtime".to_owned(),
        directory: ws.to_string_lossy().to_string(),
        path: Some(ws.to_string_lossy().to_string()),
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
        trace_id: Some("trace-permissions".to_owned()),
    };
    store
        .create_session(record.clone())
        .expect("创建 session 行");
    let config = RuntimeConfig {
        mode: "build".to_string(),
        provider_id: "mock-chat".to_string(),
        model_id: "mock-1".to_string(),
        reasoning_level: None,
        capabilities: agent_core::ModelCapabilities::default(),
        model_catalog: None,
        thought_level: None,
        data_root: None,
        event_log_capacity: 4096,
        broadcast_capacity: 256,
        retry: agent_core::RetryPolicy::default(),
    };
    let runtime = SessionRuntime::new(
        record,
        Arc::clone(&store),
        Arc::new(ScriptedSource::single(make_steps(&ws))),
        config,
        wiring,
    )
    .expect("构造运行时");
    RuntimeFixture {
        runtime,
        db_path,
        ws,
        _store: store,
        _root: root,
    }
}

struct Fixture {
    server: Arc<AgentServer>,
    db_path: PathBuf,
    ws: PathBuf,
    _root: TempRoot,
}

/// stdio 夹具：`make_steps` 收到 ws 路径后产出剧本（Write 工具用例需要具体文件
/// 路径），其余与 methods.rs 的 fixture 同构。
fn fixture(label: &str, make_steps: impl FnOnce(&Path) -> Vec<ScriptStep>) -> Fixture {
    let root = TempRoot::new(label);
    let mode_root = root.path().join(".mode");
    let db_dir = mode_root.join("cli").join("db");
    std::fs::create_dir_all(&db_dir).expect("建 cli/db 目录");
    let db_path = db_dir.join("db.sqlite");
    create_baseline_database(&db_path);
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).expect("建工作目录");

    // 文件层路径指到不存在的位置：能力解析失败按缺省能力落（create 宽容），
    // provider 由注入的脚本化装配源供给。账号端点用例自建 builtin 文件。
    let hub = ProviderConfigHub::with_paths(
        root.path()
            .join("missing-builtin.json")
            .to_string_lossy()
            .into_owned(),
        root.path().join("missing-personal.json"),
    );
    let config = agent_server::ServerConfig::from_data_base_dir(root.path());
    let server = Arc::new(AgentServer::with_parts(
        config,
        hub,
        Arc::new(ScriptedSource::single(make_steps(&ws))),
    ));
    Fixture {
        server,
        db_path,
        ws,
        _root: root,
    }
}

/// `Write` 工具 + 收尾文本的剧本（Write 是 medium ask，触发审批反向请求）。
fn write_script(file_path: &Path) -> Vec<ScriptStep> {
    vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Write".to_string(),
            input: json!({
                "file_path": file_path.to_string_lossy(),
                "content": "written-by-tool\n"
            }),
        }]),
        ScriptStep::Text("done".to_string()),
    ]
}

fn create_params(ws: &Path, session_id: &str) -> Value {
    json!({
        "sessionId": session_id,
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

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("用例超时（10s）")
}

/// 宿主侧对 `session/requestRuntimePreferences` 的应答策略。
enum PreferencesReply {
    /// spec §1.3 的宿主默认应答。
    Default,
    /// 自定义应答（原样回给运行时）。
    Custom(Value),
    /// 错误应答（`-32601` 等）。
    Error { code: i64, message: String },
}

/// 宿主连接：读 server 出站帧，按策略应答反向请求，并记录看到的反向请求。
struct TestConn {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: u64,
    preferences: PreferencesReply,
    /// 看到的反向请求方法（帧序；审批用例据此断言「只发过一次」）。
    seen_reverse: Vec<String>,
    /// 等待审批反向请求期间收到的本会话事件（scheduled 现在先于审批请求发出；
    /// 若在 `next_permission_request` 里丢掉，后续 `drain_events_until_terminal`
    /// 就会少帧）。drain 时先取这里。
    buffered_events: Vec<SessionEvent>,
}

/// 连接与 run_stdio 的 JoinHandle（生命周期用例等待循环退出）。
struct Connection {
    conn: TestConn,
    loop_handle: JoinHandle<Result<(), agent_server::ServerError>>,
}

async fn connect(server: Arc<AgentServer>) -> Connection {
    let (client, server_stream) = tokio::io::duplex(1 << 16);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let loop_handle = tokio::spawn(async move {
        // 宿主主动关闭（生命周期用例）时连接以 EOF/写失败收场：不算测试失败。
        agent_server::run_stdio(server_read, server_write, server).await
    });
    let (client_read, client_write) = tokio::io::split(client);
    Connection {
        conn: TestConn {
            reader: BufReader::new(client_read),
            writer: client_write,
            next_id: 0,
            preferences: PreferencesReply::Default,
            seen_reverse: Vec::new(),
            buffered_events: Vec::new(),
        },
        loop_handle,
    }
}

impl TestConn {
    async fn send(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let line = serde_json::to_string(&json!({ "id": id, "method": method, "params": params }))
            .expect("请求帧可序列化");
        self.write_line(&line).await;
        id
    }

    async fn write_line(&mut self, line: &str) {
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("写请求");
        self.writer.write_all(b"\n").await.expect("写换行");
        self.writer.flush().await.expect("flush");
    }

    async fn write_frame(&mut self, frame: &Frame) {
        let line = serde_json::to_string(frame).expect("帧可序列化");
        self.write_line(&line).await;
    }

    /// 读下一帧；`session/requestRuntimePreferences` 按策略自动应答（审批帧
    /// 原样上交，由用例决定三态）。
    async fn next_frame(&mut self) -> Frame {
        loop {
            let mut line = String::new();
            within(self.reader.read_line(&mut line))
                .await
                .expect("读取失败：连接意外关闭或超时");
            let frame: Frame = serde_json::from_str(line.trim_end()).expect("出站帧是合法 JSON");
            if let Some(frame) = self.auto_answer(frame).await {
                return frame;
            }
        }
    }

    /// 有界读帧：超时/无帧返回 `None`（偏好缓存等待用——自动应答发生在泵帧时）。
    async fn try_next_frame(&mut self, timeout: Duration) -> Option<Frame> {
        let mut line = String::new();
        match tokio::time::timeout(timeout, self.reader.read_line(&mut line)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => None,
            Ok(Ok(_)) => {
                let frame: Frame =
                    serde_json::from_str(line.trim_end()).expect("出站帧是合法 JSON");
                self.auto_answer(frame).await
            }
        }
    }

    /// 偏好请求的自动应答；返回 `None` 表示帧已被消费（继续读），`Some(frame)`
    /// 表示应上交给用例。
    async fn auto_answer(&mut self, frame: Frame) -> Option<Frame> {
        if let Frame::Request(request) = &frame {
            self.seen_reverse.push(request.method.clone());
            if request.method == "session/requestRuntimePreferences" {
                match &self.preferences {
                    PreferencesReply::Default => {
                        let result = json!({
                            "askUserQuestionAutoResolutionEnabled": true,
                            "nativeSearchEnhancementsEnabled": false,
                            "memoryEnabled": false,
                            "modelContextBudgetStrategy": "preflight-v1",
                        });
                        self.write_frame(&Frame::Response(ResponseFrame {
                            id: request.id.clone(),
                            result,
                        }))
                        .await;
                    }
                    PreferencesReply::Custom(value) => {
                        self.write_frame(&Frame::Response(ResponseFrame {
                            id: request.id.clone(),
                            result: value.clone(),
                        }))
                        .await;
                    }
                    PreferencesReply::Error { code, message } => {
                        let reply = Frame::ErrorResponse(agent_protocol::ErrorResponseFrame {
                            id: request.id.clone(),
                            error: agent_protocol::ErrorObject {
                                code: *code,
                                message: message.clone(),
                                data: None,
                            },
                        });
                        self.write_frame(&reply).await;
                    }
                }
                return None;
            }
        }
        Some(frame)
    }

    /// 等指定请求的 result；错误应答直接 panic（正路径用）。
    async fn result(&mut self, id: u64) -> Value {
        match self.try_result(id).await {
            Ok(result) => result,
            Err((code, message)) => panic!("请求 {id} 失败: {message} ({code})"),
        }
    }

    /// 等指定请求的 result；错误应答按 `(code, message)` 返回（重试窗口用）。
    async fn try_result(&mut self, id: u64) -> Result<Value, (i64, String)> {
        let expected = RequestId::Number(id as i64);
        loop {
            match self.next_frame().await {
                Frame::Response(response) if response.id == expected => return Ok(response.result),
                Frame::ErrorResponse(response) if response.id == expected => {
                    return Err((response.error.code, response.error.message));
                }
                _ => {}
            }
        }
    }

    /// 等下一帧 `session/event` 通知。
    async fn next_event(&mut self) -> SessionEvent {
        loop {
            if let Frame::Notification(notification) = self.next_frame().await {
                assert_eq!(notification.method, "session/event");
                let params = notification.params.expect("事件通知带 params");
                return serde_json::from_value(params).expect("事件信封可解码");
            }
        }
    }

    /// 读一帧审批反向请求（`interaction/requestPermission`）。
    async fn next_permission_request(&mut self) -> (RequestId, PermissionRequest) {
        loop {
            match self.next_frame().await {
                Frame::Request(request) => {
                    assert_eq!(
                        request.method, "interaction/requestPermission",
                        "期待审批反向请求，实际 {}",
                        request.method
                    );
                    let params = request.params.expect("审批请求带 params");
                    let parsed: PermissionRequest =
                        serde_json::from_value(params).expect("审批请求可解码");
                    return (request.id, parsed);
                }
                Frame::Notification(notification) if notification.method == "session/event" => {
                    if let Some(params) = notification.params {
                        if let Ok(event) = serde_json::from_value::<SessionEvent>(params) {
                            self.buffered_events.push(event);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 以 `options[index]` 的预置应答回审批（allow_once=0 / allow_project=1 / deny=2）。
    async fn reply_permission(&mut self, id: RequestId, request: &PermissionRequest, index: usize) {
        let response = &request.options[index].response;
        let result = serde_json::to_value(response).expect("选项应答可序列化");
        self.write_frame(&Frame::Response(ResponseFrame { id, result }))
            .await;
    }

    async fn create_and_subscribe(&mut self, ws: &Path, session_id: &str) {
        let id = self
            .send("session/create", create_params(ws, session_id))
            .await;
        let snapshot = self.result(id).await;
        assert_eq!(snapshot["session"]["sessionId"], json!(session_id));
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
        self.result(id).await;
    }

    /// 收集事件直到终态（turn.completed / turn.failed）。
    async fn drain_events_until_terminal(&mut self) -> Vec<SessionEvent> {
        let mut events = std::mem::take(&mut self.buffered_events);
        loop {
            let event = self.next_event().await;
            let terminal = matches!(
                event.event_type,
                SessionEventType::TurnCompleted | SessionEventType::TurnFailed
            );
            events.push(event);
            if terminal {
                return events;
            }
        }
    }
}

fn event_types(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

/// `tool.updated` 事件的 kind 序列（scheduled/started/result/error/batch）。
fn tool_kinds(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ToolUpdated)
        .map(|event| {
            event
                .payload_as::<Value>()
                .expect("能解码")
                .expect("有载荷")["kind"]
                .as_str()
                .expect("kind 是字符串")
                .to_string()
        })
        .collect()
}

fn permission_resolutions(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::PermissionResolved)
        .map(|event| {
            event
                .payload_as::<Value>()
                .expect("能解码")
                .expect("有载荷")["decision"]
                .as_str()
                .expect("decision 是字符串")
                .to_string()
        })
        .collect()
}

fn terminals(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                SessionEventType::TurnCompleted | SessionEventType::TurnFailed
            )
        })
        .map(|event| event.event_type.as_str())
        .collect()
}

// ---------------------------------------------------------------------------
// 1. allow_once：审批反向请求 → 应答 → 工具执行 → permission.resolved + result
// ---------------------------------------------------------------------------

#[tokio::test]
async fn allow_once_reply_executes_tool_and_emits_permission_events() {
    // 不预置文件：Write 写**新文件**不需要先读（「先读再写」只挡覆盖未读的
    // 已存在文件），工具在 allow 后创建它。
    let fixture = fixture("allow-once", |ws| write_script(&ws.join("hello-allow.txt")));
    let target = fixture.ws.join("hello-allow.txt");

    let mut connection = connect(Arc::clone(&fixture.server)).await;
    connection
        .conn
        .create_and_subscribe(&fixture.ws, "sess_perm_allow")
        .await;

    let id = connection
        .conn
        .send(
            "session/send",
            json!({ "sessionId": "sess_perm_allow", "content": "write it" }),
        )
        .await;
    let sent = connection.conn.result(id).await;
    assert_eq!(sent["accepted"], json!(true));

    // 审批反向请求：medium / Write / 三条选项；回 allow_once。
    let (id, request) = connection.conn.next_permission_request().await;
    if let RequestId::String(text) = &id {
        assert!(
            text.starts_with("server-"),
            "反向请求 id 是 server-<n> 形状，实际 {text}"
        );
    } else {
        panic!("反向请求 id 是字符串形状，实际 {id:?}");
    }
    assert_eq!(request.tool_name, "Write");
    assert_eq!(request.risk_level, "medium");
    assert_eq!(request.session_id, "sess_perm_allow");
    let option_ids: Vec<String> = request
        .options
        .iter()
        .map(|option| option.option_id.clone())
        .collect();
    assert_eq!(option_ids, vec!["allow_once", "allow_project", "deny"]);
    connection.conn.reply_permission(id, &request, 0).await;

    let events = connection.conn.drain_events_until_terminal().await;

    // 事件顺序：scheduled → permission.requested → permission.resolved →
    // started → result → batch（工具三态 + 审批事件都在）。
    assert_eq!(
        tool_kinds(&events),
        vec!["scheduled", "started", "result", "batch"]
    );
    assert!(event_types(&events).contains(&"permission.requested"));
    assert_eq!(permission_resolutions(&events), vec!["allow"]);
    assert_eq!(terminals(&events), vec!["turn.completed"]);
    // allow_once 放行：文件确实被工具创建。
    let content = std::fs::read_to_string(&target).expect("读回文件");
    assert_eq!(content, "written-by-tool\n");

    // 偏好请求每运行时一对（物化时 `runtime-materialization` + 首个 turn 前
    // `user-execution`，Task 10 对齐 oracle 的 §2.2 观察），审批发过一次。
    let preferences = connection
        .conn
        .seen_reverse
        .iter()
        .filter(|method| *method == "session/requestRuntimePreferences")
        .count();
    assert_eq!(
        preferences, 2,
        "偏好反向请求每运行时一对（两个 scope 各一次）"
    );
    let permissions = connection
        .conn
        .seen_reverse
        .iter()
        .filter(|method| *method == "interaction/requestPermission")
        .count();
    assert_eq!(permissions, 1);
}

// ---------------------------------------------------------------------------
// 2. deny：只留 scheduled + batch(errorCount=1)，无 started/result/error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deny_reply_keeps_only_scheduled_and_batch() {
    let fixture = fixture("deny", |ws| write_script(&ws.join("hello-deny.txt")));
    let target = fixture.ws.join("hello-deny.txt");

    let mut connection = connect(Arc::clone(&fixture.server)).await;
    connection
        .conn
        .create_and_subscribe(&fixture.ws, "sess_perm_deny")
        .await;
    let id = connection
        .conn
        .send(
            "session/send",
            json!({ "sessionId": "sess_perm_deny", "content": "write it" }),
        )
        .await;
    let _ = connection.conn.result(id).await;

    let (reply_id, request) = connection.conn.next_permission_request().await;
    connection
        .conn
        .reply_permission(reply_id, &request, 2)
        .await;
    let events = connection.conn.drain_events_until_terminal().await;

    // 观察（spec §3.3）：deny 只留 scheduled + batch，没有 started/result/error；
    // batch 的 errorCount 是 1。审批事件仍按会话循环发出。
    assert_eq!(tool_kinds(&events), vec!["scheduled", "batch"]);
    assert_eq!(permission_resolutions(&events), vec!["deny"]);
    assert_eq!(terminals(&events), vec!["turn.completed"], "终态恰好一次");
    let error_count = events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ToolUpdated)
        .filter_map(|event| event.payload_as::<Value>().expect("能解码"))
        .find(|payload| payload["kind"] == json!("batch"))
        .and_then(|payload| payload["errorCount"].as_u64())
        .expect("batch 载荷带 errorCount");
    assert_eq!(error_count, 1);
    // deny：工具没有执行。
    assert!(!target.exists(), "deny 后不应创建文件");
}

// ---------------------------------------------------------------------------
// 3. allow_project：落盘 local_setting，第二次同规则直接放行；新运行时同库放行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn allow_project_persists_rule_and_replays_allow_without_request() {
    // 三个 turn：写（审批→allow_project）、写（规则命中）、Read+写（新运行时命中；
    // Read 让新会话的「先读再写」前置通过，规则集让 Write 不再审批）。
    // 会话 2 的剧本经 set_provider_source 在 create 之前换上（同 server）。
    let fixture = fixture("allow-project", |ws| {
        let target = ws.join("hello-project.txt");
        [write_script(&target), write_script(&target)].concat()
    });
    let target = fixture.ws.join("hello-project.txt");
    let server = Arc::clone(&fixture.server);

    let mut connection = connect(Arc::clone(&server)).await;
    connection
        .conn
        .create_and_subscribe(&fixture.ws, "sess_perm_project")
        .await;

    // Turn 1：审批 → allow_project。
    let id = connection
        .conn
        .send(
            "session/send",
            json!({ "sessionId": "sess_perm_project", "content": "write 1" }),
        )
        .await;
    let _ = connection.conn.result(id).await;
    let (id, request) = connection.conn.next_permission_request().await;
    connection.conn.reply_permission(id, &request, 1).await;
    let events = connection.conn.drain_events_until_terminal().await;
    assert_eq!(permission_resolutions(&events), vec!["allow"]);
    assert_eq!(
        tool_kinds(&events),
        vec!["scheduled", "started", "result", "batch"]
    );

    // local_setting 行：scope='project'、namespace='permission'、key='ruleset'，
    // scope_id 由 cwd 归一化派生；value 是 {"version":1,"allow":[{toolName,ruleContent}]}。
    let scope_id = agent_core::scope_id_for(&fixture.ws.to_string_lossy());
    let ruleset = ruleset_value(&fixture.db_path, &scope_id);
    assert_eq!(ruleset["version"], json!(1));
    let allow = ruleset["allow"].as_array().expect("allow 数组");
    assert_eq!(allow.len(), 1);
    assert_eq!(allow[0]["toolName"], json!("Write"));
    assert_eq!(allow[0]["ruleContent"], json!(target.to_string_lossy()));

    // Turn 2（同一运行时）：同规则请求直接放行——不发审批反向请求、
    // 没有 permission.requested 事件，工具照常执行。
    let id = connection
        .conn
        .send(
            "session/send",
            json!({ "sessionId": "sess_perm_project", "content": "write 2" }),
        )
        .await;
    let _ = connection.conn.result(id).await;
    let events = connection.conn.drain_events_until_terminal().await;
    assert!(
        !event_types(&events).contains(&"permission.requested"),
        "规则命中不应再发审批事件，实际 {:?}",
        event_types(&events)
    );
    assert_eq!(
        tool_kinds(&events),
        vec!["scheduled", "started", "result", "batch"]
    );
    assert_eq!(
        connection
            .conn
            .seen_reverse
            .iter()
            .filter(|method| *method == "interaction/requestPermission")
            .count(),
        1,
        "整条连接只应发过一次审批反向请求"
    );

    // Turn 3：新运行时（同库）——关闭会话等 drain 完成，再 create 同 workspace 的
    // 新会话；规则集在 Session 构造时从 local_setting 加载，同样直接放行。
    let id = connection
        .conn
        .send("session/close", json!({ "sessionId": "sess_perm_project" }))
        .await;
    let _ = connection.conn.result(id).await;
    for _ in 0..100 {
        if server.find_runtime("sess_perm_project").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        server.find_runtime("sess_perm_project").is_none(),
        "close 后运行时应在 drain 后退出注册表"
    );
    // 会话 2 的剧本：Read + Write 同批（Read 满足「先读再写」，Write 命中规则）。
    server.set_provider_source(Arc::new(ScriptedSource::single(vec![
        ScriptStep::ToolCalls(vec![
            ScriptToolCall {
                name: "Read".to_string(),
                input: json!({ "file_path": target.to_string_lossy() }),
            },
            ScriptToolCall {
                name: "Write".to_string(),
                input: json!({
                    "file_path": target.to_string_lossy(),
                    "content": "written-again-by-tool\n"
                }),
            },
        ]),
        ScriptStep::Text("done".to_string()),
    ])));
    connection
        .conn
        .create_and_subscribe(&fixture.ws, "sess_perm_project2")
        .await;
    connection
        .conn
        .send(
            "session/send",
            json!({ "sessionId": "sess_perm_project2", "content": "write 3" }),
        )
        .await;
    let _ = connection.conn.result(connection.conn.next_id).await;
    let events = connection.conn.drain_events_until_terminal().await;
    assert!(
        !event_types(&events).contains(&"permission.requested"),
        "新运行时经加载的规则集也应直接放行"
    );
    assert_eq!(
        tool_kinds(&events),
        vec![
            "scheduled",
            "scheduled",
            "started",
            "result",
            "started",
            "result",
            "batch"
        ],
        "Read+Write 同批：整批先 scheduled，再串行 started/result"
    );
    let content = std::fs::read_to_string(&target).expect("读回文件");
    assert_eq!(content, "written-again-by-tool\n");
}

// ---------------------------------------------------------------------------
// 4. 运行时偏好：物化即发一次、应答缓存；-32601 回退；活跃 resume 不重发
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_preferences_cached_on_answer_and_fallback_on_error() {
    let fixture = fixture("preferences", |_ws| {
        vec![ScriptStep::Text("ok".to_string())]
    });
    let mut connection = connect(Arc::clone(&fixture.server)).await;

    // Create 会话 A：宿主回自定义应答（缺 modelContextBudgetStrategy —— 未列出
    // 字段按回退默认补）。
    connection.conn.preferences = PreferencesReply::Custom(json!({
        "askUserQuestionAutoResolutionEnabled": false,
        "memoryEnabled": true,
    }));
    let id = connection
        .conn
        .send("session/create", create_params(&fixture.ws, "sess_pref_a"))
        .await;
    let _ = connection.conn.result(id).await;
    // 作用域内取缓存：Arc<SessionRuntime> 不能带出作用域——它持有队列发送端，
    // 不释放会让 close 后的 drain 永远完不成（closing 台账不清理）。
    let cached = {
        let runtime = server_runtime(&fixture.server, "sess_pref_a").expect("运行时已物化");
        wait_preferences(&mut connection.conn, &runtime).await
    };
    assert_eq!(
        cached,
        RuntimePreferences {
            ask_user_question_auto_resolution_enabled: false,
            // 缺席字段按回退默认补（nativeSearchEnhancementsEnabled 默认 false）。
            native_search_enhancements_enabled: false,
            memory_enabled: true,
            model_context_budget_strategy: Some("preflight-v1".to_string()),
            raw: json!({ "askUserQuestionAutoResolutionEnabled": false, "memoryEnabled": true }),
        },
        "宿主应答按字段缓存，缺席字段按默认补"
    );

    // 活跃运行时的 resume：复用运行时，不重发偏好请求。
    let before = connection
        .conn
        .seen_reverse
        .iter()
        .filter(|method| *method == "session/requestRuntimePreferences")
        .count();
    let id = connection
        .conn
        .send("session/resume", json!({ "sessionId": "sess_pref_a" }))
        .await;
    let _ = connection.conn.result(id).await;
    assert_eq!(
        connection
            .conn
            .seen_reverse
            .iter()
            .filter(|method| *method == "session/requestRuntimePreferences")
            .count(),
        before,
        "活跃运行时 resume 不重发偏好请求"
    );

    // 关闭 A 等 drain，然后冷 resume 同 id：新运行时 → 新偏好请求 → 宿主回
    // `-32601` → 保守默认被登记。drain 窗口内 resume 显式 `-32603`（Task 7 复评
    // 裁决 2）——重试到窗口关闭为止。
    let id = connection
        .conn
        .send("session/close", json!({ "sessionId": "sess_pref_a" }))
        .await;
    let _ = connection.conn.result(id).await;
    connection.conn.preferences = PreferencesReply::Error {
        code: -32601,
        message: "Method not found".to_string(),
    };
    let mut resumed = None;
    for _ in 0..50 {
        let id = connection
            .conn
            .send("session/resume", json!({ "sessionId": "sess_pref_a" }))
            .await;
        match connection.conn.try_result(id).await {
            Ok(result) => {
                resumed = Some(result);
                break;
            }
            Err((-32603, _)) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err((code, message)) => panic!("resume 失败: {message} ({code})"),
        }
    }
    assert!(resumed.is_some(), "drain 完成后 resume 应在 ~2.5s 内成功");
    let runtime = server_runtime(&fixture.server, "sess_pref_a").expect("冷 resume 拉起运行时");
    let cached = wait_preferences(&mut connection.conn, &runtime).await;
    assert_eq!(
        cached,
        RuntimePreferences::fallback(),
        "`-32601` 应答登记保守默认（spec §1.3 的宿主默认应答）"
    );
    assert_eq!(
        cached.raw,
        RuntimePreferences::fallback().raw,
        "回退应答以默认形状登记（可原样回放）"
    );
}

fn server_runtime(server: &Arc<AgentServer>, session_id: &str) -> Option<Arc<SessionRuntime>> {
    server.find_runtime(session_id)
}

/// 等偏好请求完成（worker 物化后异步发请求；应答/回退登记进 Shared）。
///
/// 等待期间持续泵帧：宿主侧的自动应答发生在读帧时，不泵帧偏好请求就没有人回，
/// worker 会一直等到超时。
async fn wait_preferences(
    conn: &mut TestConn,
    runtime: &Arc<SessionRuntime>,
) -> RuntimePreferences {
    for _ in 0..100 {
        if let Some(preferences) = runtime.runtime_preferences() {
            return preferences;
        }
        if conn
            .try_next_frame(Duration::from_millis(50))
            .await
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    panic!("偏好应答未在 ~5s 内缓存");
}

// ---------------------------------------------------------------------------
// 5. provider 运行时头：账号型 provider → 宿主钩子；false / -32601 / 无钩子 → 缺 key
// ---------------------------------------------------------------------------

/// 账号型 provider 的 builtin Release（`zhipu-account` access + mock 端点）。
fn write_account_builtin(root: &Path) -> String {
    let path = root.join("mode-builtin.json");
    std::fs::write(
        &path,
        r#"{
          "schemaVersion": 1,
          "revision": 7,
          "config": {
            "providerConfigRules": {
              "templateRules": [],
              "providerRules": [
                {
                  "providerId": "acct-chat",
                  "config": {
                    "group": "zai-family",
                    "access": { "type": "zhipu-account", "accountType": "bigmodel", "mode": "start-plan" },
                    "api": { "type": "openai-chat-completions", "baseUrl": "https://acct.example/api" }
                  }
                }
              ]
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
    .expect("写 builtin 文件");
    path.to_string_lossy().into_owned()
}

/// 账号快照交付（与 provider_config.rs 测试同一形状：builtinModelIds 下发模型白名单）。
fn account_delivery() -> Value {
    json!({
        "revision": "acct-rev-1",
        "basedOnModeBuiltinRevision": "7",
        "providers": {
            "acct-chat": {
                "access": { "type": "zhipu-account", "entitled": true },
                "builtinModelIds": ["acct-model"]
            }
        },
        "states": {
            "acct-chat": {
                "availability": "available",
                "entitled": true,
                "current": true
            }
        }
    })
}

struct HeadersFixture {
    source: Arc<HubProviderSource>,
    reverse: Arc<ReverseHub>,
    outbound_rx: tokio::sync::mpsc::UnboundedReceiver<Frame>,
    hub: Arc<ProviderConfigHub>,
    _root: TempRoot,
}

fn headers_fixture(reply_timeout: Duration) -> HeadersFixture {
    let root = TempRoot::new("headers");
    let builtin = write_account_builtin(root.path());
    let hub = Arc::new(ProviderConfigHub::with_paths(
        builtin,
        root.path().join("missing-personal.json"),
    ));
    hub.handle_account_config(account_delivery())
        .expect("账号快照被接受");
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let reverse = ReverseHub::with_timeout(outbound_tx, reply_timeout);
    let slot = Arc::new(std::sync::RwLock::new(Some(Arc::clone(&reverse))));
    let source = Arc::new(HubProviderSource::new(Arc::clone(&hub), slot));
    HeadersFixture {
        source,
        reverse,
        outbound_rx,
        hub,
        _root: root,
    }
}

fn headers_context(ws: &Path) -> ProviderRequestContext {
    ProviderRequestContext {
        session_id: "sess_headers".to_string(),
        workspace_path: ws.to_string_lossy().to_string(),
        workspace_identity: None,
    }
}

#[tokio::test]
async fn provider_runtime_headers_build_config_with_host_key() {
    let mut fixture = headers_fixture(Duration::from_secs(5));
    let context = headers_context(fixture._root.path());

    // 装配在阻塞线程上进行（与 worker 同形态）：钩子会阻塞等待宿主应答。
    let source = Arc::clone(&fixture.source);
    let context_for_task = context.clone();
    let task = tokio::task::spawn_blocking(move || {
        source.resolve_openai_config("acct-chat", "acct-model", Some(&context_for_task))
    });

    // 反向请求帧：方法与 params 严格形状（accountAccess 判别联合）。
    let frame = within(fixture.outbound_rx.recv())
        .await
        .expect("钩子发出反向请求");
    let Frame::Request(request) = frame else {
        panic!("应是请求帧");
    };
    assert_eq!(
        request.method, "interaction/requestProviderRuntimeHeaders",
        "账号型 provider 触发宿主凭据钩子"
    );
    let params = request.params.expect("带 params");
    assert_eq!(params["sessionId"], json!("sess_headers"));
    assert_eq!(params["providerId"], json!("acct-chat"));
    assert_eq!(params["reason"], json!("model-request"));
    assert_eq!(
        params["workspace"]["workspacePath"],
        json!(fixture._root.path().to_string_lossy())
    );
    assert_eq!(params["modelSelection"]["providerId"], json!("acct-chat"));
    assert_eq!(params["modelSelection"]["modelId"], json!("acct-model"));
    let access = &params["accountAccess"];
    assert_eq!(access["type"], json!("zhipu-account"));
    assert_eq!(access["accountType"], json!("bigmodel"));
    assert_eq!(access["mode"], json!("start-plan"));
    assert_eq!(access["entitled"], json!(true));
    assert!(
        !params.to_string().contains("tok-hdr"),
        "请求里不携带凭据（凭据只在宿主应答里）"
    );

    // 宿主回 headersApplied:true + requestAuth.apiKey。
    fixture.reverse.deliver_response(
        &request.id,
        json!({ "headersApplied": true, "requestAuth": { "apiKey": "tok-hdr-key-123" } }),
    );
    let config = within(task).await.expect("解析成功").expect("拿到配置");
    // key 只进 Authorization 头路径：config.api_key 是唯一载体，Debug 恒为掩码。
    assert_eq!(config.api_key, "tok-hdr-key-123");
    assert_eq!(config.base_url, "https://acct.example/api");
    assert_eq!(config.model, "acct-model");
    assert!(
        !format!("{config:?}").contains("tok-hdr-key-123"),
        "Debug 不打印 key"
    );
    assert!(
        !format!("{:?}", fixture.hub).contains("tok-hdr-key-123"),
        "hub Debug 不打印 key"
    );

    // 同一配置能构建可执行 provider（不发起任何网络请求）。
    let provider = provider_http::OpenAiChatProvider::new(config);
    assert!(provider.is_ok(), "配置可构建 OpenAI Chat 适配器");
}

#[tokio::test]
async fn provider_runtime_headers_not_applied_falls_back_to_missing_key_error() {
    let mut fixture = headers_fixture(Duration::from_secs(5));
    let context = headers_context(fixture._root.path());

    // headersApplied:false → 显式缺 key 错误（既有口径）。
    let source = Arc::clone(&fixture.source);
    let context_for_task = context.clone();
    let task = tokio::task::spawn_blocking(move || {
        source.resolve_openai_config("acct-chat", "acct-model", Some(&context_for_task))
    });
    let frame = within(fixture.outbound_rx.recv())
        .await
        .expect("钩子发出反向请求");
    let Frame::Request(request) = frame else {
        panic!("应是请求帧");
    };
    fixture
        .reverse
        .deliver_response(&request.id, json!({ "headersApplied": false }));
    let error = within(task)
        .await
        .expect("解析结束")
        .expect_err("缺 key 显式报错");
    assert!(
        error.contains("access.apiKey"),
        "错误指名缺失字段，实际: {error}"
    );

    // `-32601`（宿主不支持钩子）→ 同一显式错误，不重试。
    let source = Arc::clone(&fixture.source);
    let context_for_task = context.clone();
    let task = tokio::task::spawn_blocking(move || {
        source.resolve_openai_config("acct-chat", "acct-model", Some(&context_for_task))
    });
    let frame = within(fixture.outbound_rx.recv())
        .await
        .expect("钩子发出反向请求");
    let Frame::Request(request) = frame else {
        panic!("应是请求帧");
    };
    fixture
        .reverse
        .deliver_error(&request.id, -32601, "Method not found");
    let error = within(task)
        .await
        .expect("解析结束")
        .expect_err("缺 key 显式报错");
    assert!(error.contains("access.apiKey"), "实际: {error}");

    // 没有连接级 hub（不经 stdio 的解析）→ 同一显式错误，且不发反向请求。
    let root = TempRoot::new("headers-no-hub");
    let hub = Arc::new(ProviderConfigHub::with_paths(
        write_account_builtin(root.path()),
        root.path().join("missing-personal.json"),
    ));
    hub.handle_account_config(account_delivery())
        .expect("账号快照被接受");
    let empty_slot = Arc::new(std::sync::RwLock::new(None));
    let source = HubProviderSource::new(Arc::clone(&hub), empty_slot);
    let error = source
        .resolve_openai_config("acct-chat", "acct-model", Some(&context))
        .expect_err("无钩子 → 缺 key");
    assert!(error.contains("access.apiKey"), "实际: {error}");
}

// ---------------------------------------------------------------------------
// 6. 反向请求生命周期：超时 deny；宿主关闭 stdio → deny + 终态恰好一次
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_reply_timeout_denies_with_batch_error() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let reverse = ReverseHub::with_timeout(outbound_tx, Duration::from_millis(300));
    let wiring = RuntimeWiring {
        permission_handler: Some(Box::new(agent_server::ProtocolPermissionHandler::new(
            Arc::clone(&reverse),
        ))),
        rule_db_path: None,
        reverse: Some(Arc::clone(&reverse)),
        mcp: None,
        mcp_hub: Default::default(),
        subagent_runner: None,
    };
    let fixture = runtime_fixture(
        "perm-timeout",
        |ws| write_script(&ws.join("hello-timeout.txt")),
        wiring,
    );
    let target = fixture.ws.join("hello-timeout.txt");
    let (_replay, mut receiver) = fixture.runtime.subscribe(None);
    fixture
        .runtime
        .submit(SessionInput::text("write it"))
        .expect("提交");

    // 审批反向请求发出，宿主不应答 → 300ms 超时 → deny（fail-closed）。
    // 偏好请求先到（worker 物化即发 + 首个 turn 前的 user-execution，Task 10）：
    // 均不应答，等各自 300ms 超时回退后继续。
    let request = loop {
        let frame = within(outbound_rx.recv()).await.expect("出站帧");
        let Frame::Request(request) = frame else {
            panic!("应是请求帧");
        };
        if request.method == "interaction/requestPermission" {
            break request;
        }
        assert_eq!(
            request.method, "session/requestRuntimePreferences",
            "意外的反向请求: {}",
            request.method
        );
    };
    let _ = request.id; // 不应答。

    let mut events = Vec::new();
    loop {
        let event = within(receiver.recv()).await.expect("事件总线不应关闭");
        let terminal = matches!(
            event.event_type,
            SessionEventType::TurnCompleted | SessionEventType::TurnFailed
        );
        events.push(event);
        if terminal {
            break;
        }
    }
    assert_eq!(
        tool_kinds(&events),
        vec!["scheduled", "batch"],
        "超时 deny 走 oracle 形状"
    );
    assert_eq!(permission_resolutions(&events), vec!["deny"]);
    assert_eq!(terminals(&events), vec!["turn.completed"]);
    assert!(!target.exists(), "超时 deny：工具未执行");
    // 没有规则库接线（内存规则集），deny 也不产生 permissionUpdates → 无规则行。
    assert_eq!(
        count_rows(&fixture.db_path, "local_setting"),
        0,
        "deny 路径不落盘规则"
    );
}

#[tokio::test]
async fn stdio_close_while_permission_pending_denies_once_without_hang() {
    let fixture = fixture("close-pending", |ws| {
        write_script(&ws.join("hello-close.txt"))
    });
    let target = fixture.ws.join("hello-close.txt");
    let server = Arc::clone(&fixture.server);

    let Connection {
        mut conn,
        loop_handle,
    } = connect(Arc::clone(&server)).await;
    conn.create_and_subscribe(&fixture.ws, "sess_perm_close")
        .await;

    // 进程内预挂事件订阅（连接关闭后 stdio 不再转发，终态断言走这条订阅）。
    let runtime = server
        .find_runtime("sess_perm_close")
        .expect("运行时已物化");
    let (_replay, mut runtime_events) = runtime.subscribe(None);

    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": "sess_perm_close", "content": "write it" }),
        )
        .await;
    let _ = conn.result(id).await;
    let _pending = conn.next_permission_request().await; // 宿主不应答。

    // 宿主关闭 stdio（双工两端一起丢）→ run_stdio 必须收场（不悬挂），并失败
    // 所有 pending 反向请求。
    drop(conn);
    let _ = tokio::time::timeout(Duration::from_secs(5), loop_handle)
        .await
        .expect("run_stdio 在宿主关闭后 5s 内退出（不悬挂）");

    // pending 被失败 → deny → 工具不执行 → batch → 下一模型步 → 终态恰好一次。
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), runtime_events.recv())
            .await
            .expect("收口在 5s 内完成（不悬挂）")
            .expect("事件总线不应关闭");
        let terminal = matches!(
            event.event_type,
            SessionEventType::TurnCompleted | SessionEventType::TurnFailed
        );
        events.push(event);
        if terminal {
            assert_eq!(
                tool_kinds(&events),
                vec!["scheduled", "batch"],
                "deny 走 oracle 形状"
            );
            assert_eq!(permission_resolutions(&events), vec!["deny"]);
            break;
        }
    }
    assert!(!target.exists(), "deny：文件未被创建");
}
