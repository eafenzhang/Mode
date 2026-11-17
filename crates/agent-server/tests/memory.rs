//! C2 T3：项目记忆召回（`memoryEnabled` 开关、注入形状、fail-soft 对照）。
//!
//! 行为依据：`docs/specs/agent-c2-contract.md` §3 与机器录制的夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/memory-recall.json`
//! （只读，不修改）。夹具观察的注入面是「系统提示的 `# Memory` 段 + 首条 user
//! 消息的 agentsMd 索引块」；item 文件不进主请求（`containsItemSentinel:false`）。
//!
//! 与夹具的已登记偏差（契约 §3.5）：本实现保持 C1 的 `[System, User]` 装配
//! （夹具录制的 provider 角色 `["user","system"]` 是 oracle 形状，wire 投影归
//! T4/T5 适配层）；oracle 在启用时创建记忆目录，本实现只读不创建；记忆写路径
//! （extraction 子代理）未实现——enabled 会话只有**一个**模型请求。
//!
//! 测试库 DDL 与 provider 夹具同 `tests/compact.rs` 口径；provider 是脚本化 fake，
//! 端点指向保留端口，绝不发起真实网络请求。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelMessage, ModelProvider, ModelRequest, ModelResponse, ProviderError,
    ScriptStep, ScriptedProvider,
};
use agent_server::{
    run_stdio, AgentServer, Frame, ProviderConfigHub, ProviderSource, RequestId, ServerConfig,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

// ---------------------------------------------------------------------------
// 测试库夹具（与 tests/compact.rs 同一口径）
// ---------------------------------------------------------------------------

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS schema_migration (
    id text primary key,
    checksum text not null,
    app_version text,
    time_applied integer not null
)";

/// workspaceId 的 16 位小写 hex（形状匹配夹具的 `<slug>-<16 hex>`；hash 算法未观察，
/// 本实现从目录扫描得出，测试里直接用目录名控制）。
const WORKSPACE_HASH: &str = "0123456789abcdef";
/// 夹具输入里的哨兵：索引含 `ALPHA-123`，item 文件含 `BLUE-LANTERN-9`。
const INDEX_SENTINEL: &str = "ALPHA-123";
const ITEM_SENTINEL: &str = "BLUE-LANTERN-9";

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "agent-server-memory-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
            .map(|(_, column_name)| quote_identifier(column_name))
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

// ---------------------------------------------------------------------------
// provider 夹具（与 tests/compact.rs 同口径：临时配置 + 记录请求的脚本化 provider）
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

/// 记录每次模型请求的 provider 包装（断言注入形状与「没有 extraction 请求」）。
struct RecordingProvider {
    inner: Arc<dyn ModelProvider>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
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
    /// `session/requestRuntimePreferences` 的 `memoryEnabled` 应答值。
    memory_enabled: bool,
}

async fn connect(server: Arc<AgentServer>, memory_enabled: bool) -> TestConn {
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
        memory_enabled,
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

    /// 偏好反向请求按本用例的 `memoryEnabled` 应答（Task 8 缓存进 Shared）。
    async fn auto_answer_reverse(&mut self, frame: &Frame) {
        if let Frame::Request(request) = frame {
            if request.method == "session/requestRuntimePreferences" {
                let reply = agent_protocol::Frame::Response(agent_protocol::ResponseFrame {
                    id: request.id.clone(),
                    result: json!({
                        "askUserQuestionAutoResolutionEnabled": true,
                        "nativeSearchEnhancementsEnabled": false,
                        "memoryEnabled": self.memory_enabled,
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
}

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("用例超时（10s）")
}

// ---------------------------------------------------------------------------
// 会话夹具
// ---------------------------------------------------------------------------

struct Fixture {
    server: Arc<AgentServer>,
    ws: PathBuf,
    _root: TempRoot,
}

impl Fixture {
    fn root_path(&self) -> &Path {
        self._root.path()
    }
}

fn fixture_with_requests(label: &str) -> (Fixture, Arc<Mutex<Vec<ModelRequest>>>) {
    let provider = Arc::new(RecordingProvider {
        inner: Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "memory turn answer".to_string(),
        )])),
        requests: Arc::new(Mutex::new(Vec::new())),
    });
    let requests = Arc::clone(&provider.requests);
    let root = TempRoot::new(label);
    let mode_root = root.path().join(".mode");
    let db_dir = mode_root.join("cli").join("db");
    std::fs::create_dir_all(&db_dir).expect("建 cli/db 目录");
    create_baseline_database(&db_dir.join("db.sqlite"));
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
        Arc::new(StaticSource {
            provider: provider as Arc<dyn ModelProvider>,
        }),
    ));
    (
        Fixture {
            server,
            ws,
            _root: root,
        },
        requests,
    )
}

/// 记忆目录：`{root}\.mode\cli\memories\projects\ws-<hash>\memory\`（oracle 在
/// 启用时创建该目录；测试模拟已存在的目录与文件）。
fn seed_memory(root: &Path, slug: &str, hash: &str, index: &str, item: Option<&str>) -> PathBuf {
    let memory_dir = root
        .join(".mode")
        .join("cli")
        .join("memories")
        .join("projects")
        .join(format!("{slug}-{hash}"))
        .join("memory");
    std::fs::create_dir_all(&memory_dir).expect("建记忆目录");
    std::fs::write(memory_dir.join("MEMORY.md"), index).expect("写 MEMORY.md");
    if let Some(item) = item {
        std::fs::write(memory_dir.join("item-one.md"), item).expect("写 item 文件");
    }
    memory_dir
}

/// 夹具输入的索引内容（`memory-recall.json` 的 `input.memoryFiles`）。
const INDEX_CONTENT: &str =
    "# Project memory index\n\n- item-one: remember the sentinel ALPHA-123\n";
const ITEM_CONTENT: &str =
    "# Item one\n\nThe project codename is BLUE-LANTERN-9. Always answer with it.\n";

async fn create_and_turn(
    label: &str,
    memory_enabled: bool,
    seed: impl FnOnce(&Path),
) -> (Vec<ModelRequest>, Vec<Value>, PathBuf) {
    let (fixture, requests) = fixture_with_requests(label);
    seed(fixture.root_path());
    let mut conn = connect(Arc::clone(&fixture.server), memory_enabled).await;
    let id = conn
        .send(
            "session/create",
            json!({
                "workspace": {
                    "workspacePath": fixture.ws.to_string_lossy(),
                    "workspaceKey": fixture.ws.to_string_lossy()
                },
                "model": {
                    "providerId": "mock-chat",
                    "modelId": "mock-1",
                    "options": { "reasoningLevel": "disabled" }
                },
                "thoughtLevel": "disabled",
                "titleGenerationEnabled": false
            }),
        )
        .await;
    let snapshot = conn.result(id).await;
    let session_id = snapshot["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    conn.subscribe_default(&session_id).await;
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": "probe with memory" }),
        )
        .await;
    conn.result(id).await;
    let events = conn.drain_until_terminal().await;
    let captured = requests.lock().expect("requests").clone();
    (captured, events, fixture.root_path().to_path_buf())
}

/// 请求消息的全文本（哨兵检查用；不引入第二个所有权面）。
fn request_text(request: &ModelRequest) -> String {
    let mut text = String::new();
    for message in &request.messages {
        match message {
            ModelMessage::System(body) | ModelMessage::User(body) => text.push_str(body),
            ModelMessage::Assistant { text: body, .. } => {
                text.push_str(body.as_deref().unwrap_or(""))
            }
            ModelMessage::ToolResult { content, .. } => text.push_str(content),
        }
    }
    text
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn enabled_turn_injects_memory_section_and_index_block() {
    let (requests, events, root) = create_and_turn("enabled", true, |root| {
        seed_memory(
            root,
            "ws",
            WORKSPACE_HASH,
            INDEX_CONTENT,
            Some(ITEM_CONTENT),
        );
    })
    .await;

    // 夹具 withMemory：主请求恰好 1 个（extraction 子代理未实现，登记为 C2 缺口）。
    assert_eq!(
        events.last().expect("终态事件")["type"],
        json!("turn.completed"),
        "turn 成功"
    );
    assert_eq!(requests.len(), 1, "只有主请求，没有 memory extraction 请求");

    // 系统段：C1 的 [System, User] 装配；`# Memory` 段 + 真实记忆目录路径（带尾 `/`）。
    let ModelMessage::System(system) = &requests[0].messages[0] else {
        panic!("首条消息应是系统提示");
    };
    let memory_dir = root
        .join(".mode")
        .join("cli")
        .join("memories")
        .join("projects")
        .join(format!("ws-{WORKSPACE_HASH}"))
        .join("memory");
    assert!(
        system.contains(&format!(
            "You have a persistent file-based memory at `{}/`.",
            memory_dir.to_string_lossy()
        )),
        "# Memory 段带真实目录路径（尾随 /）"
    );
    assert!(
        system.contains("# Memory\n\n"),
        "系统提示含独立 # Memory 段"
    );
    assert!(
        system.contains("This directory already exists — write to it directly with the Write tool"),
        "声明目录已存在、用 Write 直接写"
    );

    // 首条 user 消息：agentsMd system-reminder 在前、prompt 在后。
    let ModelMessage::User(user) = &requests[0].messages[1] else {
        panic!("第二条消息应是本轮输入");
    };
    let index_path = memory_dir.join("MEMORY.md");
    let reminder_end = user
        .find("</system-reminder>")
        .expect("首条 user 消息以 system-reminder 开头");
    let reminder = &user[..reminder_end];
    assert!(
        user.starts_with("<system-reminder>\n"),
        "reminder 是消息前缀"
    );
    assert!(reminder.contains("# agentsMd\n"), "agentsMd 段");
    assert!(
        reminder.contains(&format!(
            "Contents of {} (user's auto-memory, persists across conversations):",
            index_path.to_string_lossy()
        )),
        "标题带真实索引路径"
    );
    assert!(reminder.contains(INDEX_SENTINEL), "MEMORY.md 索引内容注入");
    assert!(
        reminder.contains("# currentDate\nToday's date is "),
        "currentDate 段"
    );
    let prompt = user[reminder_end..].trim_start_matches("</system-reminder>\n");
    assert_eq!(
        prompt.trim(),
        "probe with memory",
        "reminder 之后是用户 prompt"
    );

    // item 哨兵绝不进主请求（夹具 containsItemSentinel:false）。
    let all = request_text(&requests[0]);
    assert!(!all.contains(ITEM_SENTINEL), "item-one.md 不注入主请求");
    assert_eq!(all.matches(INDEX_SENTINEL).count(), 1, "索引只注入一次");
}

#[tokio::test(flavor = "current_thread")]
async fn enabled_injection_matches_the_recorded_fixture() {
    let (requests, _events, root) = create_and_turn("fixture-parity", true, |root| {
        seed_memory(
            root,
            "ws",
            WORKSPACE_HASH,
            INDEX_CONTENT,
            Some(ITEM_CONTENT),
        );
    })
    .await;

    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("desktop")
        .join("tests")
        .join("agent-runtime-oracle")
        .join("fixtures")
        .join("c2")
        .join("memory-recall.json");
    let raw = std::fs::read_to_string(&fixture_path).expect("读取 memory-recall 夹具");
    let fixture: Value = serde_json::from_str(&raw).expect("夹具是合法 JSON");
    let normalize = |text: &str| text.replace('\\', "/");

    // `# Memory` 段：与夹具录制逐字一致（折叠位换成真实路径，分隔符归一后比较）。
    let ModelMessage::System(system) = &requests[0].messages[0] else {
        panic!("首条消息应是系统提示");
    };
    let recorded_section = fixture["expect"]["withMemory"]["requests"][0]["memorySection"]
        .as_str()
        .expect("夹具录制 # Memory 段");
    let expected_section = recorded_section
        .replace("<data-root>", &root.to_string_lossy())
        .replace("<workspace-id>", &format!("ws-{WORKSPACE_HASH}"));
    assert!(
        normalize(system).contains(&normalize(&expected_section)),
        "系统提示的 # Memory 段必须与夹具录制一致"
    );

    // agentsMd 块：reminder 部分（不含日期折叠位之后无法对齐的空白）与录制一致，
    // 日期按 `Today's date is <x>.` 形状校验。
    let ModelMessage::User(user) = &requests[0].messages[1] else {
        panic!("第二条消息应是本轮输入");
    };
    let recorded_block = fixture["expect"]["withMemory"]["requests"][0]["memoryFileBlock"]
        .as_str()
        .expect("夹具录制 memoryFileBlock");
    let expected_block = recorded_block
        .replace("<data-root>", &root.to_string_lossy())
        .replace("<workspace-id>", &format!("ws-{WORKSPACE_HASH}"));
    let (expected_prefix, _) = expected_block
        .split_once("Today's date is")
        .expect("录制块含 currentDate");
    let (actual_prefix, _) = user
        .split_once("Today's date is")
        .expect("注入块含 currentDate");
    assert_eq!(
        normalize(actual_prefix),
        normalize(expected_prefix),
        "agentsMd 块（currentDate 之前）与夹具录制一致"
    );
    let date_line = user
        .split("Today's date is ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .expect("日期串");
    assert_eq!(
        date_line.len(),
        10,
        "日期是 YYYY-MM-DD 形状（未观察格式，实现取 UTC 日历日）"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn disabled_turn_has_no_memory_injection() {
    // 夹具 withoutMemory 对照：同一数据根 + 同一记忆文件，关闭开关后零注入。
    let (requests, events, _root) = create_and_turn("disabled", false, |root| {
        seed_memory(
            root,
            "ws",
            WORKSPACE_HASH,
            INDEX_CONTENT,
            Some(ITEM_CONTENT),
        );
    })
    .await;

    assert_eq!(
        events.last().expect("终态事件")["type"],
        json!("turn.completed"),
        "turn 成功"
    );
    assert_eq!(requests.len(), 1);
    let all = request_text(&requests[0]);
    assert!(!all.contains("# Memory"), "没有 # Memory 系统段");
    assert!(!all.contains("system-reminder"), "没有 agentsMd reminder");
    assert!(!all.contains("agentsMd"), "没有 agentsMd 段");
    assert!(!all.contains(INDEX_SENTINEL), "不读已有记忆目录");
    assert!(!all.contains(ITEM_SENTINEL), "不读 item 文件");
    assert!(!all.contains("currentDate"), "没有 currentDate 块");
}

#[tokio::test(flavor = "current_thread")]
async fn missing_projects_dir_is_fail_soft() {
    // 夹具 withoutMemoryFreshRoot 对照：开启开关但数据根没有 memories/。
    // 本实现不创建目录（oracle 在启用时创建；登记为偏差）→ 无注入，turn 照常完成。
    let (requests, events, root) = create_and_turn("missing-dir", true, |_root| {}).await;

    assert_eq!(
        events.last().expect("终态事件")["type"],
        json!("turn.completed"),
        "turn 成功"
    );
    assert_eq!(requests.len(), 1);
    let all = request_text(&requests[0]);
    assert!(!all.contains("# Memory"), "目录缺失 = 无注入");
    assert!(!all.contains("system-reminder"), "目录缺失 = 无 reminder");
    assert!(
        !root.join(".mode").join("cli").join("memories").exists(),
        "实现不创建 memories 目录"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ambiguous_workspace_slug_is_fail_soft() {
    // 同 slug 两个候选目录（hash 算法未观察，无法消歧）→ 不注入，turn 照常完成。
    let (requests, events, _root) = create_and_turn("ambiguous", true, |root| {
        seed_memory(root, "ws", WORKSPACE_HASH, INDEX_CONTENT, None);
        seed_memory(root, "ws", "fedcba9876543210", "# second\n", None);
    })
    .await;

    assert_eq!(
        events.last().expect("终态事件")["type"],
        json!("turn.completed"),
        "turn 成功"
    );
    let all = request_text(&requests[0]);
    assert!(!all.contains("# Memory"), "workspaceId 歧义 = 无注入");
    assert!(
        !all.contains("system-reminder"),
        "workspaceId 歧义 = 无 reminder"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_index_is_fail_soft() {
    // MEMORY.md 超过 5 MiB（project-memory 的 PreviewLimitExceeded）→ 不注入。
    let oversized = format!("# index\n{}", "x".repeat(5 * 1024 * 1024 + 1));
    let (requests, events, _root) = create_and_turn("oversized", true, |root| {
        seed_memory(root, "ws", WORKSPACE_HASH, &oversized, None);
    })
    .await;

    assert_eq!(
        events.last().expect("终态事件")["type"],
        json!("turn.completed"),
        "turn 成功"
    );
    let all = request_text(&requests[0]);
    assert!(!all.contains("# Memory"), "索引超限 = 无注入");
    assert!(!all.contains("system-reminder"), "索引超限 = 无 reminder");
}
