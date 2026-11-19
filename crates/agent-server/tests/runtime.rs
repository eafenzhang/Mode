//! `SessionRuntime` 的集成测试（LT-1 C1 Task 6）。
//!
//! 覆盖：串行准入 / inputId 幂等 / revision 冲突、事件顺序与持久化行、
//! provider 失败的回滚语义、取消、订阅（replay+live 无重复无缺口）、
//! worker future 的 `Send`（`tokio::spawn` 编译期验证）与快照形状。
//!
//! 测试库 DDL 从 `scripts/sqlite-schema-baseline.json` 生成，与
//! `session-store` 测试的基线夹具同一口径（helper 因跨 crate 不能直接引用，
//! 在此按同一实现复制，改字段必须两边同步）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::provider::{
    BoxFuture, DeltaSink, ModelDelta, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
    ProviderError, ScriptStep, ScriptToolCall, ScriptedProvider,
};
use agent_core::Admission;
use agent_protocol::session_event::{
    ModelStreamingPayload, TurnCompletedPayload, TurnFailedPayload,
};
use agent_protocol::session_event::{SessionEvent, SessionEventType};
use agent_protocol::snapshot::SessionStateSnapshot;
use agent_server::{Accepted, RuntimeConfig, RuntimeError, SessionInput, SessionRuntime};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use session_store::{SessionRecord, SessionStore};
use tokio::sync::Notify;

// ---------------------------------------------------------------------------
// 基线测试库夹具（与 session-store/tests/common/mod.rs 同一口径）
// ---------------------------------------------------------------------------

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS schema_migration (
    id text primary key,
    checksum text not null,
    app_version text,
    time_applied integer not null
)";

/// 临时目录；Drop 时递归删除。store/runtime 必须先于它释放。
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-runtime-{label}-{}-{}",
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

/// 按基线生成完整测试库（全部表 + 索引 + 账本），与 `session-store` 测试同一实现。
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

fn count_rows(path: &Path, table: &str) -> i64 {
    raw_connection(path)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|error| panic!("统计 {table} 行数失败：{error}"))
}

fn read_column(path: &Path, table: &str, column: &str, where_sql: &str) -> String {
    raw_connection(path)
        .query_row(
            &format!("SELECT {column} FROM {table} WHERE {where_sql}"),
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("读取 {table}.{column} 失败：{error}"))
}

fn raw_connection(path: &Path) -> Connection {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("只读连接");
    connection
        .busy_timeout(Duration::from_secs(5))
        .expect("busy_timeout");
    connection
}

// ---------------------------------------------------------------------------
// 运行时夹具
// ---------------------------------------------------------------------------

struct Fixture {
    store: Arc<SessionStore>,
    record: SessionRecord,
    db_path: PathBuf,
    cwd: PathBuf,
    /// 最后释放（删除目录）——字段顺序即 Drop 顺序。
    _root: TempRoot,
}

fn fixture(label: &str) -> Fixture {
    let root = TempRoot::new(label);
    let db_path = root.db_path();
    create_baseline_database(&db_path);
    let store = Arc::new(SessionStore::open(&db_path).expect("打开测试库"));
    let cwd = root.path().join("ws");
    fs::create_dir_all(&cwd).expect("建工作目录");
    fs::write(cwd.join("hello.txt"), "hello\n").expect("写种子文件");
    let record = SessionRecord {
        id: "sess_runtime".to_owned(),
        project_id: "proj_runtime".to_owned(),
        workspace_id: None,
        parent_id: None,
        slug: "sess_runtime".to_owned(),
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
        trace_id: Some("trace-runtime".to_owned()),
    };
    store
        .create_session(record.clone())
        .expect("创建 session 行");
    Fixture {
        store,
        record,
        db_path,
        cwd,
        _root: root,
    }
}

fn runtime_config() -> RuntimeConfig {
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
        retry: agent_core::RetryPolicy::default(),
    }
}

/// 固定 provider 的装配源（Task 7 的 `ProviderSource` 注入口）。
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

fn spawn_runtime(fixture: &Fixture, provider: Arc<dyn ModelProvider>) -> SessionRuntime {
    spawn_runtime_with_capabilities(fixture, provider, runtime_config().capabilities)
}

fn spawn_runtime_with_capabilities(
    fixture: &Fixture,
    provider: Arc<dyn ModelProvider>,
    capabilities: agent_core::ModelCapabilities,
) -> SessionRuntime {
    let mut config = runtime_config();
    config.capabilities = capabilities;
    SessionRuntime::new(
        fixture.record.clone(),
        Arc::clone(&fixture.store),
        Arc::new(FixedSource(provider)),
        config,
        agent_server::RuntimeWiring::default(),
    )
    .expect("构造运行时")
}

/// 让 worker 在 runtime Drop 后把队列收尾、释放 store Arc，再删临时目录。
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// 等 turn 的终态收口完成（Task 8：turn 跑在 `spawn_blocking` 线程上，终态事件
/// 先于 worker 的状态收口可被订阅者观察到——「terminal 发布后快照短暂仍是
/// Running」是跨线程执行的可观察窗口，收口随后完成）。bounded 轮询到 Idle/Error。
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

async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("用例超时（10s）")
}

/// 从 live receiver 收集事件直到终态（`turn.completed` / `turn.failed`）。
async fn drain_until_terminal(
    receiver: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let event = within(receiver.recv()).await.expect("总线不应关闭");
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

fn event_kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

fn terminals(events: &[SessionEvent]) -> Vec<SessionEventType> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                SessionEventType::TurnCompleted | SessionEventType::TurnFailed
            )
        })
        .map(|event| event.event_type)
        .collect()
}

fn streaming_kinds(events: &[SessionEvent]) -> Vec<ModelStreamingPayload> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ModelStreaming)
        .filter_map(|event| event.payload_as::<ModelStreamingPayload>().expect("能解码"))
        .collect()
}

/// 发一条 delta 后停在 `release` 信号上的 provider（准入门控用）。
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
            // 信号到达前不发任何 delta：订阅时序测试依赖「turn.started 之后没有新事件」。
            release.notified().await;
            deltas
                .send(ModelDelta::Text {
                    text: "gated answer".to_string(),
                })
                .expect("会话侧应保持 delta 通道打开");
            Ok(ModelResponse {
                usage: ModelUsage {
                    input_tokens: 11,
                    output_tokens: 22,
                    ..ModelUsage::default()
                },
                stop_reason: "stop",
            })
        })
    }
}

/// 第一步产出 `Read` 工具调用，之后的所有请求直接报错（committed-before-failure 场景）。
struct ToolThenErrorProvider {
    file_path: String,
}

impl ModelProvider for ToolThenErrorProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        let file_path = self.file_path.clone();
        Box::pin(async move {
            let assistants = request
                .messages
                .iter()
                .filter(|message| matches!(message, agent_core::ModelMessage::Assistant { .. }))
                .count();
            if assistants > 0 {
                return Err(ProviderError::new("第二次模型请求失败"));
            }
            let tool_call_id = "toolu_error_0_0".to_string();
            for delta in [
                ModelDelta::ToolInputStart {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "Read".to_string(),
                },
                ModelDelta::ToolInputDelta {
                    tool_call_id: tool_call_id.clone(),
                    delta: serde_json::to_string(&json!({ "file_path": file_path }))
                        .expect("可序列化"),
                },
                ModelDelta::ToolInputEnd {
                    tool_call_id: tool_call_id.clone(),
                },
                ModelDelta::ToolCall {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "Read".to_string(),
                    input: json!({ "file_path": file_path }),
                },
            ] {
                deltas.send(delta).expect("通道应打开");
            }
            Ok(ModelResponse {
                usage: ModelUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                    ..ModelUsage::default()
                },
                stop_reason: "tool-calls",
            })
        })
    }
}

// ---------------------------------------------------------------------------
// 1. 串行准入
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_submits_run_serially_and_second_starts_after_first_terminal() {
    let fixture = fixture("serial");
    let gate = Arc::new(Notify::new());
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(GatedProvider {
            release: Arc::clone(&gate),
        }),
    );
    let (replay, mut receiver) = runtime.subscribe(None);
    assert!(replay.is_empty(), "新运行时没有历史事件");

    let first = runtime
        .submit(SessionInput {
            input_id: Some("in-1".to_string()),
            content: "first prompt".to_string(),
            expected_revision: None,
        })
        .expect("第一次提交");
    assert_eq!(first.input_id, "in-1");
    assert_eq!(
        first.admission,
        Admission::Started,
        "第一个输入立即 running"
    );

    // 等 turn 1 真正开始（turn.started 进总线）再提交第二个：否则两个都可能 Started。
    let started_1 = within(receiver.recv()).await.expect("turn.started 1");
    assert_eq!(started_1.event_type, SessionEventType::TurnStarted);

    let second = runtime
        .submit(SessionInput {
            input_id: Some("in-2".to_string()),
            content: "second prompt".to_string(),
            expected_revision: None,
        })
        .expect("第二次提交");
    assert_eq!(
        second.admission,
        Admission::Queued { position: 1 },
        "turn 运行期间第二个输入入队"
    );
    assert!(
        second.state_revision > first.state_revision,
        "每次接受都推进 stateRevision"
    );

    // 放行 turn 1 → 终态；turn 2 必须在其终态之后才开始。
    gate.notify_one();
    let first_turn_events = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        terminals(&first_turn_events),
        vec![SessionEventType::TurnCompleted],
        "第一个 turn 恰好一条终态"
    );
    let first_terminal_seq = first_turn_events.last().expect("有终态").seq;

    // turn 2 开始（其 provider 也会被 gate 卡住）。
    let started_2 = within(receiver.recv()).await.expect("turn.started 2");
    assert_eq!(started_2.event_type, SessionEventType::TurnStarted);
    assert!(
        started_2.seq > first_terminal_seq,
        "第二个 turn.started ({}) 必须晚于第一个终态 ({})",
        started_2.seq,
        first_terminal_seq
    );
    gate.notify_one();
    let second_turn_events = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        terminals(&second_turn_events),
        vec![SessionEventType::TurnCompleted]
    );

    // 串行落库：两个 user message，两个 assistant message。
    assert_eq!(count_rows(&fixture.db_path, "message"), 4);
    assert_eq!(count_rows(&fixture.db_path, "turn_usage"), 2);

    drop(runtime);
    settle().await;
}

// ---------------------------------------------------------------------------
// 2. 幂等 + revision 冲突
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_input_id_is_idempotent_and_conflict_rejects_without_turn() {
    let fixture = fixture("idempotent");
    let gate = Arc::new(Notify::new());
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(GatedProvider {
            release: Arc::clone(&gate),
        }),
    );
    let (_replay, mut receiver) = runtime.subscribe(None);

    let first = runtime
        .submit(SessionInput {
            input_id: Some("dup-1".to_string()),
            content: "only once".to_string(),
            expected_revision: None,
        })
        .expect("首次提交");
    let duplicate = runtime
        .submit(SessionInput {
            input_id: Some("dup-1".to_string()),
            content: "different content, same id".to_string(),
            expected_revision: None,
        })
        .expect("重复提交返回首次回执");
    assert_eq!(first, duplicate, "同 inputId 的回执必须完全一致");
    assert_eq!(duplicate.admission, Admission::Started);

    // revision 冲突：显式报错，不启动 turn。
    let conflict = runtime
        .submit(SessionInput {
            input_id: Some("conflict-1".to_string()),
            content: "stale revision".to_string(),
            expected_revision: Some(u64::MAX),
        })
        .expect_err("版本冲突必须报错");
    assert!(
        matches!(conflict, RuntimeError::RevisionConflict { .. }),
        "实际：{conflict:?}"
    );

    // 放行唯一的 turn；确认没有第二个 turn。
    gate.notify_one();
    let events = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        event_kinds(&events),
        vec!["turn.started", "model.streaming", "turn.completed"]
    );
    // 幂等重放：同一 id 再提交仍返回同一回执（终态后依旧）。
    let after = runtime
        .submit(SessionInput {
            input_id: Some("dup-1".to_string()),
            content: "again".to_string(),
            expected_revision: None,
        })
        .expect("终态后重复提交仍幂等");
    assert_eq!(after, first);

    // 库里只有一个 turn 的行。
    assert_eq!(count_rows(&fixture.db_path, "message"), 2);
    assert_eq!(count_rows(&fixture.db_path, "turn_usage"), 1);

    drop(runtime);
    settle().await;
}

// ---------------------------------------------------------------------------
// 3. 事件顺序 + 持久化行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn text_and_tool_turns_persist_messages_parts_and_usage() {
    let fixture = fixture("persist");
    let file_path = fixture.cwd.join("hello.txt");
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptStep::Text("first turn answer".to_string()),
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Read".to_string(),
            input: json!({ "file_path": file_path.to_string_lossy() }),
        }]),
        ScriptStep::Text("second turn answer".to_string()),
    ]));
    let runtime = spawn_runtime(&fixture, provider);
    let (replay, mut receiver) = runtime.subscribe(None);
    assert!(replay.is_empty());

    runtime
        .submit(SessionInput::text("first prompt"))
        .expect("提交 1");
    let first_turn = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        event_kinds(&first_turn),
        vec!["turn.started", "model.streaming", "turn.completed"],
        "文本 turn 的事件顺序"
    );

    runtime
        .submit(SessionInput::text("Read hello.txt and summarize"))
        .expect("提交 2");
    let second_turn = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        event_kinds(&second_turn),
        vec![
            "turn.started",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "model.streaming",
            "turn.completed",
        ],
        "工具 turn 的事件顺序（scheduled→started→result→batch）"
    );
    let tool_kinds: Vec<String> = second_turn
        .iter()
        .filter(|event| event.event_type == SessionEventType::ToolUpdated)
        .map(|event| {
            event
                .payload_as::<serde_json::Value>()
                .expect("能解码")
                .expect("有载荷")["kind"]
                .as_str()
                .expect("kind 是字符串")
                .to_string()
        })
        .collect();
    assert_eq!(tool_kinds, vec!["scheduled", "started", "result", "batch"]);

    let kinds = streaming_kinds(&first_turn);
    assert_eq!(kinds.len(), 1);
    assert_eq!(kinds[0].delta.as_deref(), Some("first turn answer"));

    // 两个 turn 各一条终态，全部是 completed（没有 failed）。
    let all_events: Vec<SessionEvent> = first_turn
        .into_iter()
        .chain(second_turn.into_iter())
        .collect();
    assert_eq!(
        terminals(&all_events),
        vec![
            SessionEventType::TurnCompleted,
            SessionEventType::TurnCompleted
        ],
        "终态恰好每 turn 一条"
    );

    // 持久化行：2 个 user + 3 个 assistant；parts：2×(text) + 3×(step-start/text|tool/step-finish)。
    assert_eq!(count_rows(&fixture.db_path, "message"), 5);
    assert_eq!(count_rows(&fixture.db_path, "part"), 11);
    assert_eq!(count_rows(&fixture.db_path, "turn_usage"), 2);
    assert_eq!(count_rows(&fixture.db_path, "tool_usage"), 1);
    assert_eq!(
        count_rows(&fixture.db_path, "session_input"),
        0,
        "裁决 5：immediate 路径不写 session_input"
    );

    // message sequence 连续；part 归属与顺序正确。
    let connection = raw_connection(&fixture.db_path);
    let sequences: Vec<i64> = {
        let mut statement = connection
            .prepare(
                "SELECT sequence FROM message WHERE session_id = 'sess_runtime' ORDER BY sequence",
            )
            .expect("prepare");
        statement
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集")
    };
    assert_eq!(sequences, vec![0, 1, 2, 3, 4]);
    drop(connection);

    // user 行形状：role=user、text part 是原文。
    let user_json: String = raw_connection(&fixture.db_path)
        .query_row("SELECT data FROM message WHERE sequence = 0", [], |row| {
            row.get(0)
        })
        .expect("读 user 行");
    let user_data: Value = serde_json::from_str(&user_json).expect("user 行是 JSON");
    assert_eq!(user_data["role"], json!("user"));
    assert_eq!(user_data["anchor"]["turnId"], json!("turn_0"));
    let user_part_json: String = raw_connection(&fixture.db_path)
        .query_row(
            "SELECT part.data FROM part JOIN message ON part.message_id = message.id \
             WHERE message.sequence = 0",
            [],
            |row| row.get(0),
        )
        .expect("读 user part");
    let user_part: Value = serde_json::from_str(&user_part_json).expect("part JSON");
    assert_eq!(user_part["type"], json!("text"));
    assert_eq!(user_part["text"], json!("first prompt"));

    // turn_usage：文本 turn 10/12/22，工具 turn 20/32/52（ScriptedProvider 的剧本值）。
    let connection = raw_connection(&fixture.db_path);
    let usage: Vec<(String, i64, i64, i64)> = {
        let mut statement = connection
            .prepare(
                "SELECT status, input_tokens, output_tokens, computed_total_tokens \
                      FROM turn_usage ORDER BY started_at",
            )
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集")
    };
    assert_eq!(
        usage,
        vec![
            ("completed".to_string(), 10, 12, 22),
            ("completed".to_string(), 20, 32, 52),
        ]
    );
    let tool_status: String = connection
        .query_row("SELECT status FROM tool_usage", [], |row| row.get(0))
        .expect("tool_usage");
    assert_eq!(tool_status, "completed");

    drop(runtime);
    settle().await;
}

// ---------------------------------------------------------------------------
// 4. provider 失败：turn.failed 恰好一次；无半成品行；已提交的行保留
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_failure_mid_turn_keeps_committed_rows_only() {
    let fixture = fixture("failure");
    let file_path = fixture.cwd.join("hello.txt");
    let provider = Arc::new(ToolThenErrorProvider {
        file_path: file_path.to_string_lossy().to_string(),
    });
    let runtime = spawn_runtime(&fixture, provider);
    let (replay, mut receiver) = runtime.subscribe(None);
    assert!(replay.is_empty());

    runtime
        .submit(SessionInput::text("trigger failure"))
        .expect("提交");
    let events = drain_until_terminal(&mut receiver).await;

    // 事件：工具批次完整产出（batch 在 failed 之前），终态恰好一条 turn.failed。
    assert_eq!(
        event_kinds(&events),
        vec![
            "turn.started",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "model.streaming",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "tool.updated",
            "turn.failed",
        ]
    );
    assert_eq!(terminals(&events), vec![SessionEventType::TurnFailed]);
    let failed_payload = events
        .last()
        .expect("终态")
        .payload_as::<TurnFailedPayload>()
        .expect("能解码")
        .expect("类型匹配");
    assert_eq!(failed_payload.error.message, "第二次模型请求失败");
    assert_eq!(failed_payload.turn_phase, "processing_input");

    // 行：user（turn 开始已提交）+ 工具 assistant（batch 边界已提交）。
    // 失败的那一步没有可提交的消息 → 没有半成品行。
    assert_eq!(count_rows(&fixture.db_path, "message"), 2);
    assert_eq!(count_rows(&fixture.db_path, "part"), 4);
    // 每个 part 都归属已存在的 message（无悬挂行）。
    let dangling: i64 = raw_connection(&fixture.db_path)
        .query_row(
            "SELECT COUNT(*) FROM part WHERE message_id NOT IN (SELECT id FROM message)",
            [],
            |row| row.get(0),
        )
        .expect("悬挂检查");
    assert_eq!(dangling, 0);

    // turn_usage 一条，status=failed；tool_usage 记录了已完成的 Read。
    let connection = raw_connection(&fixture.db_path);
    let (status, tool_error_count): (String, i64) = connection
        .query_row(
            "SELECT status, tool_error_count FROM turn_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("turn_usage");
    assert_eq!(status, "error");
    assert_eq!(tool_error_count, 0, "Read 成功，错误来自 provider");
    let tool_status: String = connection
        .query_row("SELECT status FROM tool_usage", [], |row| row.get(0))
        .expect("tool_usage");
    assert_eq!(tool_status, "completed", "批次已提交的工具有 usage 行");

    drop(runtime);
    settle().await;
}

// ---------------------------------------------------------------------------
// 5. 取消：唯一终态；排队中的下一个输入照常执行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_mid_turn_emits_one_terminal_and_next_queued_input_proceeds() {
    let fixture = fixture("cancel");
    let gate = Arc::new(Notify::new());
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(GatedProvider {
            release: Arc::clone(&gate),
        }),
    );
    let (_replay, mut receiver) = runtime.subscribe(None);

    runtime
        .submit(SessionInput {
            input_id: Some("in-1".to_string()),
            content: "will be cancelled".to_string(),
            expected_revision: None,
        })
        .expect("提交 1");
    let started_1 = within(receiver.recv()).await.expect("turn.started 1");
    assert_eq!(started_1.event_type, SessionEventType::TurnStarted);

    // 排队第二个输入（turn 1 还在运行），然后取消当前 turn。
    let queued = runtime
        .submit(SessionInput {
            input_id: Some("in-2".to_string()),
            content: "queued behind".to_string(),
            expected_revision: None,
        })
        .expect("提交 2");
    assert_eq!(queued.admission, Admission::Queued { position: 1 });

    runtime.cancel_turn().expect("取消");
    let events = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        terminals(&events),
        vec![SessionEventType::TurnCompleted],
        "取消的 turn 恰好一条终态"
    );
    let payload = events
        .last()
        .expect("终态")
        .payload_as::<TurnCompletedPayload>()
        .expect("能解码")
        .expect("类型匹配");
    assert_eq!(
        payload.result_type,
        agent_protocol::session_event::TurnResultType::Cancelled,
        "取消走 turn.completed + resultType:cancelled（协议注释口径）"
    );
    assert_eq!(
        count_rows(&fixture.db_path, "turn_usage"),
        1,
        "取消的 turn 也有 usage 行"
    );
    let cancel_status: String = read_column(
        &fixture.db_path,
        "turn_usage",
        "status",
        "cancelled_by_user = 1",
    );
    assert_eq!(cancel_status, "cancelled");

    // 排队中的输入照常执行（放行它的 provider 门）。
    let started_2 = within(receiver.recv()).await.expect("turn.started 2");
    assert_eq!(started_2.event_type, SessionEventType::TurnStarted);
    gate.notify_one();
    let second = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        terminals(&second),
        vec![SessionEventType::TurnCompleted],
        "下一个输入正常完成"
    );
    // 行数：turn 1 只有 user（取消的步没有完整 assistant 行）；turn 2 是 user+assistant。
    assert_eq!(count_rows(&fixture.db_path, "message"), 3);
    assert_eq!(count_rows(&fixture.db_path, "part"), 5);
    assert_eq!(count_rows(&fixture.db_path, "turn_usage"), 2);

    // 空闲时取消是幂等 no-op。
    runtime.cancel_turn().expect("空闲取消 no-op");

    drop(runtime);
    settle().await;
}

// ---------------------------------------------------------------------------
// 6. 订阅：replay + live 无重复无缺口；晚订阅者拿得到终态
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscribe_replay_then_live_without_gap_or_duplicate() {
    let fixture = fixture("subscribe");
    let gate = Arc::new(Notify::new());
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(GatedProvider {
            release: Arc::clone(&gate),
        }),
    );

    // 先建早期订阅，捕获完整事件流。
    let (_, mut early) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("probe subscribe"))
        .expect("提交");
    let started = within(early.recv()).await.expect("turn.started");
    assert_eq!(started.event_type, SessionEventType::TurnStarted);

    // provider 还没放行：此刻日志里只有 turn.started → after_seq 订阅 replay 为空。
    let (replay, mut live) = runtime.subscribe(Some(started.seq));
    assert!(
        replay.is_empty(),
        "turn.started 之后还没有新事件，replay 应为空"
    );

    gate.notify_one();
    let live_events = drain_until_terminal(&mut live).await;
    assert_eq!(
        event_kinds(&live_events),
        vec!["model.streaming", "turn.completed"],
        "live 段从 replay 游标之后无缝衔接"
    );

    // 早期订阅者收到同一批事件：两段拼接 = 完整序列，无重复。
    let mut rest = vec![within(early.recv()).await.expect("model.streaming")];
    rest.push(within(early.recv()).await.expect("turn.completed"));
    assert_eq!(
        event_kinds(&rest),
        vec!["model.streaming", "turn.completed"]
    );
    assert_eq!(rest[0].seq, live_events[0].seq);
    assert_eq!(rest[1].seq, live_events[1].seq);

    // 晚订阅者：终态已在日志缓冲里，replay 直接给全量。
    let (late_replay, mut late_live) = runtime.subscribe(None);
    let late_kinds = event_kinds(&late_replay);
    assert_eq!(
        late_kinds,
        vec!["turn.started", "model.streaming", "turn.completed"]
    );
    // replay 之后 live 不应再有同一条事件（turn 已结束）。
    assert!(
        late_live.try_recv().is_err(),
        "replay 与 live 之间不得有重复"
    );

    // after_seq 过滤：只要终态之后的事件 → 空；只过滤掉 turn.started → 两条。
    let (tail, _) = runtime.subscribe(Some(late_replay.last().expect("有事件").seq));
    assert!(tail.is_empty());
    let (middle, _) = runtime.subscribe(Some(started.seq));
    assert_eq!(
        event_kinds(&middle),
        vec!["model.streaming", "turn.completed"]
    );

    drop(runtime);
    settle().await;
}

// ---------------------------------------------------------------------------
// 7. Send 断言（worker future 进 tokio::spawn）+ 快照形状
// ---------------------------------------------------------------------------

/// `SessionRuntime`（含内部 JoinHandle）可以被 move 进 `tokio::spawn`：
/// worker future（`Session::run_turn` + `TurnRecorder` + store 句柄）必须 `Send`。
/// 裁决 1 的 trait 上界（`EventSink`/`PermissionHandler`/`RuleStore`: Send）缺任何
/// 一个，本用例编译失败。
#[tokio::test]
async fn runtime_moves_into_tokio_spawn() {
    let fixture = fixture("send");
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "send probe".to_string(),
        )])),
    );
    let handle = tokio::spawn(async move {
        let accepted: Accepted = runtime
            .submit(SessionInput::text("probe send"))
            .expect("submit 在 spawn 的任务里也能用");
        accepted
    });
    let accepted = handle.await.expect("spawn 任务不应 panic");
    assert_eq!(accepted.admission, Admission::Started);
    // runtime 已随 spawn 任务结束而 Drop（队列发送端关闭，worker 随之退出）。
    settle().await;
}

#[tokio::test]
async fn snapshot_reflects_live_state_and_serializes() {
    let fixture = fixture("snapshot");
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "snapshot answer".to_string(),
        )])),
    );

    // 初始：idle、零计数、mode/model 来自 config。
    let before: SessionStateSnapshot = runtime.snapshot();
    assert_eq!(before.session.session_id, "sess_runtime");
    assert_eq!(
        before.session.mode,
        agent_protocol::session_read::SessionMode::Build
    );
    assert_eq!(
        before.session.status,
        agent_protocol::session_read::SessionStatus::Idle
    );
    let model = before.session.model.as_ref().expect("有 model");
    assert_eq!(model.provider_id, "fixture-provider");
    assert_eq!(model.model_id, "fixture-model");
    assert_eq!(before.projection.turn_count, 0);
    assert_eq!(before.projection.total_token_count, 0);
    assert_eq!(before.projection.context_window, 128_000);
    assert!(before.messages.is_empty(), "活跃快照不带历史消息");
    assert_eq!(before.runtime.event_seq, 0);
    // serde round-trip：快照必须是协议合法形状。
    serde_json::to_value(&before).expect("快照可序列化");

    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("count my tokens"))
        .expect("提交");
    drain_until_terminal(&mut receiver).await;
    wait_settled(&runtime).await;

    let after: SessionStateSnapshot = runtime.snapshot();
    assert_eq!(
        after.session.status,
        agent_protocol::session_read::SessionStatus::Idle
    );
    assert_eq!(after.projection.turn_count, 1);
    assert_eq!(
        after.projection.total_token_count, 22,
        "10 input + 12 output"
    );
    assert_eq!(
        after.runtime.event_seq, 3,
        "turn.started + model.streaming + turn.completed"
    );
    assert!(after.runtime.state_revision > 0);

    // 失败终态投影 last_error（用空剧本 provider 的报错路径太重，这里直接构造冲突输入验证错误面）。
    let conflict = runtime
        .submit(SessionInput {
            input_id: None,
            content: "nope".to_string(),
            expected_revision: Some(u64::MAX),
        })
        .expect_err("版本冲突");
    assert_eq!(conflict.code(), "revision_conflict");

    drop(runtime);
    settle().await;
}

// ---------------------------------------------------------------------------
// C2 T1：runtime.contextUsage（观察形状见 agent-c2-contract.md §4）
// ---------------------------------------------------------------------------

async fn run_prompt(
    runtime: &SessionRuntime,
    receiver: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
    prompt: &str,
) {
    runtime
        .submit(SessionInput::text(prompt))
        .expect("提交 prompt");
    drain_until_terminal(receiver).await;
    wait_settled(runtime).await;
}

fn context_usage_json(snapshot: &SessionStateSnapshot) -> Value {
    serde_json::to_value(
        snapshot
            .runtime
            .context_usage
            .as_ref()
            .expect("快照应包含 runtime.contextUsage"),
    )
    .expect("contextUsage 可序列化")
}

fn context_usage_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("contextUsage 是对象")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn context_usage_after_scripted_turn_matches_the_observed_shape() {
    let fixture = fixture("context-usage-shape");
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "answer".to_string(),
        )])),
    );
    let (_, mut receiver) = runtime.subscribe(None);

    assert!(
        runtime.snapshot().runtime.context_usage.is_none(),
        "首个模型请求之前没有 contextUsage（与 create 快照一致）"
    );

    run_prompt(&runtime, &mut receiver, "first prompt").await;

    let snapshot = runtime.snapshot();
    let usage = context_usage_json(&snapshot);
    assert_eq!(
        context_usage_keys(&usage),
        vec!["cache", "cost", "size", "used"],
        "T0 观察键固定为 cache,cost,size,used（没有 percent/breakdown）"
    );
    assert_eq!(usage["used"], 22, "used = 最后一次请求的 total tokens");
    assert_eq!(usage["size"], 128_000, "size 来自 context_window");
    assert!(usage["cost"].is_null(), "T0 观察 cost 恒为 null");
    assert!(usage.get("percent").is_none(), "T0 没有 percent 字段");
    assert!(
        usage.get("breakdown").is_none(),
        "T1 不产出 breakdown（T6 再对齐）"
    );

    let cache = &usage["cache"];
    assert_eq!(cache["inputTokens"], 10);
    assert_eq!(cache["cacheReadTokens"], 0);
    assert_eq!(cache["cacheWriteTokens"], 0);
    assert_eq!(cache["latestHitRate"], 0);
    assert_eq!(cache["hitRate"], 0);
    assert_eq!(cache["hitRateRequestCount"], 1);
    assert_eq!(cache["totalInputTokens"], 10);
    assert_eq!(cache["totalCacheReadTokens"], 0);
    assert_eq!(cache["totalCacheWriteTokens"], 0);

    runtime.shutdown().await;
}

#[tokio::test]
async fn context_usage_uses_the_last_request_instead_of_the_turn_total() {
    let fixture = fixture("context-usage-last-request");
    let file_path = fixture.cwd.join("hello.txt").to_string_lossy().to_string();
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![
            ScriptStep::ToolCalls(vec![ScriptToolCall {
                name: "Read".to_string(),
                input: json!({ "file_path": file_path }),
            }]),
            ScriptStep::Text("finished".to_string()),
        ])),
    );
    let (_, mut receiver) = runtime.subscribe(None);

    run_prompt(&runtime, &mut receiver, "read the file").await;

    let snapshot = runtime.snapshot();
    let usage = context_usage_json(&snapshot);
    // 工具调用请求 10+20，收尾请求 10+12；turn 聚合是 52，但 contextUsage.used
    // 必须只是最后一次请求的 22（T0 明确 used 不是累计值）。
    assert_eq!(snapshot.projection.total_token_count, 52);
    assert_eq!(usage["used"], 22);
    assert_eq!(usage["cache"]["inputTokens"], 10);
    assert_eq!(usage["cache"]["hitRateRequestCount"], 2);
    assert_eq!(usage["cache"]["totalInputTokens"], 20);

    runtime.shutdown().await;
}

#[tokio::test]
async fn context_usage_does_not_fabricate_an_unknown_window() {
    let fixture = fixture("context-usage-unknown-window");
    let runtime = spawn_runtime_with_capabilities(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "answer".to_string(),
        )])),
        agent_core::ModelCapabilities::default(),
    );
    let (_, mut receiver) = runtime.subscribe(None);

    run_prompt(&runtime, &mut receiver, "first prompt").await;

    let snapshot = runtime.snapshot();
    let usage = context_usage_json(&snapshot);
    assert_eq!(
        usage["size"], 0,
        "窗口未知时 size 为 0，不编 200000 之类的默认值"
    );
    assert_eq!(snapshot.projection.context_window, 0);
    assert_eq!(usage["used"], 22, "窗口未知不影响 provider usage");

    runtime.shutdown().await;
}

#[tokio::test]
async fn resumed_materialization_rebuilds_context_usage_without_breakdown() {
    let fixture = fixture("context-usage-resume");
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![
            ScriptStep::Text("first answer".to_string()),
            ScriptStep::Text("second answer".to_string()),
            ScriptStep::Text("third answer".to_string()),
        ])),
    );
    let (_, mut receiver) = runtime.subscribe(None);
    for prompt in ["first prompt", "second prompt", "third prompt"] {
        run_prompt(&runtime, &mut receiver, prompt).await;
    }
    runtime.shutdown().await;

    // 冷 resume：从持久化的 assistant 消息重建用量，而不是从投影计数器。
    let resumed = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "unused".to_string(),
        )])),
    );
    let snapshot = resumed.snapshot();
    let usage = context_usage_json(&snapshot);
    assert_eq!(
        context_usage_keys(&usage),
        vec!["cache", "cost", "size", "used"],
        "resume 后 contextUsage 没有 breakdown（T0 观察）"
    );
    assert_eq!(usage["used"], 22);
    assert_eq!(usage["size"], 128_000);
    assert_eq!(usage["cache"]["inputTokens"], 10);
    assert_eq!(usage["cache"]["hitRateRequestCount"], 3);
    assert_eq!(usage["cache"]["totalInputTokens"], 30);
    assert_eq!(usage["cache"]["totalCacheReadTokens"], 0);
    assert_eq!(usage["cache"]["totalCacheWriteTokens"], 0);
    assert!(usage["cost"].is_null());
    // T0 的 resume：projection 计数归 0，但 contextUsage 保留。
    assert_eq!(snapshot.projection.turn_count, 0);
    assert_eq!(snapshot.projection.total_token_count, 0);
    assert_eq!(snapshot.projection.context_used, 0);

    resumed.shutdown().await;
}

// ---------------------------------------------------------------------------
// 12. 冷物化恢复 / 终态落库顺序 / 身份 / 缓存 / 陈旧游标（最终评审 C2/C3/I3/I5/I6/I10）
// ---------------------------------------------------------------------------

use std::collections::VecDeque;
use std::sync::Mutex;

use agent_core::{ModelMessage, PermissionHandler};
use agent_protocol::permission::{PermissionDecision, PermissionRequest, PermissionResponse};
use agent_protocol::session_event::TurnStartedPayload;
use agent_server::RuntimeWiring;

/// 按队列出剧本并记录每次模型请求的 provider。
///
/// 与 `ScriptedProvider` 的「按 assistant 条数取步」不同：冷恢复后的历史会让条数
/// 偏移，本 provider 用显式队列，测试可以精确决定每一轮输出。
struct RecordingProvider {
    steps: Mutex<VecDeque<ScriptStep>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    next_call: AtomicU64,
}

impl RecordingProvider {
    fn new(steps: Vec<ScriptStep>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into_iter().collect()),
            requests: Arc::new(Mutex::new(Vec::new())),
            next_call: AtomicU64::new(0),
        })
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("requests").clone()
    }
}

impl ModelProvider for RecordingProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        self.requests.lock().expect("requests").push(request);
        let step = self.steps.lock().expect("steps").pop_front();
        let call_index = self.next_call.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let Some(step) = step else {
                return Err(ProviderError::new("recording provider 剧本已空"));
            };
            match step {
                ScriptStep::Text(text) => {
                    deltas
                        .send(ModelDelta::Text { text })
                        .map_err(|_| ProviderError::local("delta sink closed"))?;
                    Ok(ModelResponse {
                        usage: ModelUsage {
                            input_tokens: 10,
                            output_tokens: 12,
                            ..ModelUsage::default()
                        },
                        stop_reason: "stop",
                    })
                }
                ScriptStep::ToolCalls(calls) => {
                    for (position, call) in calls.iter().enumerate() {
                        let tool_call_id = format!("toolu_rec_{call_index}_{position}");
                        deltas
                            .send(ModelDelta::ToolInputStart {
                                tool_call_id: tool_call_id.clone(),
                                tool_name: call.name.clone(),
                            })
                            .map_err(|_| ProviderError::local("delta sink closed"))?;
                        deltas
                            .send(ModelDelta::ToolInputDelta {
                                tool_call_id: tool_call_id.clone(),
                                delta: serde_json::to_string(&call.input).expect("入参可序列化"),
                            })
                            .map_err(|_| ProviderError::local("delta sink closed"))?;
                        deltas
                            .send(ModelDelta::ToolInputEnd {
                                tool_call_id: tool_call_id.clone(),
                            })
                            .map_err(|_| ProviderError::local("delta sink closed"))?;
                        deltas
                            .send(ModelDelta::ToolCall {
                                tool_call_id,
                                tool_name: call.name.clone(),
                                input: call.input.clone(),
                            })
                            .map_err(|_| ProviderError::local("delta sink closed"))?;
                    }
                    Ok(ModelResponse {
                        usage: ModelUsage {
                            input_tokens: 10,
                            output_tokens: 20,
                            ..ModelUsage::default()
                        },
                        stop_reason: "tool-calls",
                    })
                }
            }
        })
    }
}

fn spawn_runtime_with_wiring(
    fixture: &Fixture,
    provider: Arc<dyn ModelProvider>,
    wiring: RuntimeWiring,
) -> SessionRuntime {
    SessionRuntime::new(
        fixture.record.clone(),
        Arc::clone(&fixture.store),
        Arc::new(FixedSource(provider)),
        runtime_config(),
        wiring,
    )
    .expect("构造运行时")
}

/// 某个请求的模型历史形状（消息种类 + 文本/工具 id），用于断言冷恢复内容。
fn history_shape(request: &ModelRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::System(_) => None,
            ModelMessage::User(text) => Some(format!("user:{text}")),
            ModelMessage::Assistant { text, tool_calls } => Some(format!(
                "assistant:{}:{}",
                text.as_deref().unwrap_or(""),
                tool_calls
                    .iter()
                    .map(|call| call.tool_call_id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            ModelMessage::ToolResult {
                tool_call_id,
                content,
            } => Some(format!("tool:{tool_call_id}:{}", !content.is_empty())),
        })
        .collect()
}

fn turn_usage_rows(path: &Path) -> Vec<(String, String, Option<String>)> {
    let connection = raw_connection(path);
    let mut statement = connection
        .prepare("SELECT turn_id, status, user_message_id FROM turn_usage ORDER BY turn_id")
        .expect("prepare turn_usage");
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .expect("query turn_usage")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect turn_usage")
}

fn started_ids(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.event_type == SessionEventType::TurnStarted)
        .filter_map(|event| event.payload_as::<TurnStartedPayload>().expect("能解码"))
        .filter_map(|payload| payload.foreground_execution_id)
        .collect()
}

#[tokio::test]
async fn cold_materialization_restores_history_file_state_and_turn_counter() {
    let fixture = fixture("resume-history");
    let provider = RecordingProvider::new(vec![
        ScriptStep::Text("first turn answer".to_string()),
        ScriptStep::Text("second turn answer".to_string()),
        ScriptStep::Text("third turn answer".to_string()),
    ]);

    let mut foreground = Vec::new();

    // Turn 1（原始 runtime）。
    let runtime = spawn_runtime(&fixture, provider.clone());
    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("first prompt"))
        .expect("提交 turn 1");
    let events = drain_until_terminal(&mut receiver).await;
    foreground.extend(started_ids(&events));
    wait_settled(&runtime).await;
    runtime.shutdown().await;

    // Turn 2（close 后冷恢复）。
    let runtime = spawn_runtime(&fixture, provider.clone());
    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("second prompt"))
        .expect("提交 turn 2");
    let events = drain_until_terminal(&mut receiver).await;
    foreground.extend(started_ids(&events));
    wait_settled(&runtime).await;
    runtime.shutdown().await;

    // Turn 3（进程重启后再次冷恢复）。
    let runtime = spawn_runtime(&fixture, provider.clone());
    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("third prompt"))
        .expect("提交 turn 3");
    let events = drain_until_terminal(&mut receiver).await;
    foreground.extend(started_ids(&events));
    wait_settled(&runtime).await;
    runtime.shutdown().await;

    let requests = provider.requests();
    assert_eq!(requests.len(), 3, "三个 turn 各一次模型请求");
    assert_eq!(
        history_shape(&requests[1]),
        vec![
            "user:first prompt",
            "assistant:first turn answer:",
            "user:second prompt",
        ],
        "resume 后的请求必须带前一轮 user/assistant 历史"
    );
    assert_eq!(
        history_shape(&requests[2]),
        vec![
            "user:first prompt",
            "assistant:first turn answer:",
            "user:second prompt",
            "assistant:second turn answer:",
            "user:third prompt",
        ],
        "重启后必须从 DB 恢复全部历史"
    );

    // turn 计数器：turn id 不复用、turn_usage 无主键冲突、user_message_id 可对账。
    let rows = turn_usage_rows(&fixture.db_path);
    assert_eq!(
        rows.iter()
            .map(|(id, _, _)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn_0", "turn_1", "turn_2"]
    );
    assert!(rows.iter().all(|(_, status, _)| status == "completed"));
    assert!(
        rows.iter()
            .all(|(_, _, user_message_id)| user_message_id.is_some()),
        "每个 turn_usage 行都要有 user_message_id"
    );
    let distinct_user_messages: std::collections::BTreeSet<_> = rows
        .iter()
        .filter_map(|(_, _, user_message_id)| user_message_id.clone())
        .collect();
    assert_eq!(
        distinct_user_messages.len(),
        3,
        "user_message_id 必须各不相同"
    );
    assert_eq!(
        foreground,
        vec![
            "runtime_command_1",
            "runtime_command_2",
            "runtime_command_3"
        ],
        "turn.started 的 foregroundExecutionId 跨 resume/重启递增"
    );
}

#[tokio::test]
async fn resumed_session_restores_file_state_for_write_precondition() {
    let fixture = fixture("resume-file-state");
    let hello = fixture.cwd.join("hello.txt");
    let provider = RecordingProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Read".to_string(),
            input: json!({ "file_path": hello.to_string_lossy() }),
        }]),
        ScriptStep::Text("read done".to_string()),
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Write".to_string(),
            input: json!({
                "file_path": hello.to_string_lossy(),
                "content": "rewritten\n"
            }),
        }]),
        ScriptStep::Text("write done".to_string()),
    ]);

    let runtime = spawn_runtime(&fixture, provider.clone());
    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("read hello"))
        .expect("提交读");
    let events = drain_until_terminal(&mut receiver).await;
    assert!(
        events
            .iter()
            .filter(|event| event.event_type == SessionEventType::ToolUpdated)
            .any(|event| matches!(
                event
                    .payload_as::<agent_core::ToolUpdatedPayload>()
                    .expect("能解码"),
                Some(agent_core::ToolUpdatedPayload::Result(_))
            )),
        "第一次 Read 成功"
    );
    wait_settled(&runtime).await;
    runtime.shutdown().await;

    // 冷恢复后 Write 覆盖已存在文件：'先读再写' 事实必须从 DB 恢复，否则会报
    // `write_file_not_read`（错误事件而不是 result）。
    let runtime = spawn_runtime(&fixture, provider.clone());
    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("write hello"))
        .expect("提交写");
    let events = drain_until_terminal(&mut receiver).await;
    let kinds: Vec<&'static str> = events
        .iter()
        .filter(|event| event.event_type == SessionEventType::ToolUpdated)
        .filter_map(|event| {
            event
                .payload_as::<agent_core::ToolUpdatedPayload>()
                .expect("能解码")
        })
        .map(|payload| match payload {
            agent_core::ToolUpdatedPayload::Result(_) => "result",
            agent_core::ToolUpdatedPayload::Error(_) => "error",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["other", "other", "result", "other"],
        "Write 必须成功（scheduled → started → result → batch）"
    );
    assert_eq!(
        std::fs::read_to_string(&hello).expect("读回文件"),
        "rewritten\n"
    );
    wait_settled(&runtime).await;
    runtime.shutdown().await;
}

struct DenyAll;

impl PermissionHandler for DenyAll {
    fn request(&mut self, _request: &PermissionRequest) -> PermissionResponse {
        PermissionResponse {
            decision: PermissionDecision::Deny,
            reason: Some(agent_core::permission::DENY_REASON.to_string()),
            modified_input: None,
            permission_updates: None,
            extra: agent_protocol::ExtraFields::new(),
        }
    }
}

#[tokio::test]
async fn denied_tool_call_has_matching_tool_result_in_next_model_request() {
    let fixture = fixture("denied-tool-result");
    let target = fixture.cwd.join("denied.txt");
    let provider = RecordingProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Write".to_string(),
            input: json!({
                "file_path": target.to_string_lossy(),
                "content": "should-not-exist\n"
            }),
        }]),
        ScriptStep::Text("after denial".to_string()),
    ]);
    let wiring = RuntimeWiring {
        permission_handler: Some(Box::new(DenyAll)),
        rule_db_path: None,
        reverse: None,
        mcp: None,
    };
    let runtime = spawn_runtime_with_wiring(&fixture, provider.clone(), wiring);
    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("write it"))
        .expect("提交");
    let events = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == SessionEventType::TurnCompleted)
            .count(),
        1,
        "deny 后 turn 仍正常完成"
    );
    assert!(!target.exists(), "deny 后工具不能执行");

    let requests = provider.requests();
    assert_eq!(requests.len(), 2, "deny 后仍会发起下一轮模型请求");
    let tool_calls: Vec<&str> = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Assistant { tool_calls, .. } => {
                Some(tool_calls.iter().map(|call| call.tool_call_id.as_str()))
            }
            _ => None,
        })
        .flatten()
        .collect();
    let tool_results: Vec<&str> = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1, "历史里有一个 assistant tool_call");
    assert_eq!(
        tool_calls, tool_results,
        "每个 assistant tool_call 必须有配对的 tool result（deny 也要补）"
    );
    let denied_content = requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            ModelMessage::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .expect("有 tool result");
    assert!(
        denied_content.contains("doesn't want to proceed"),
        "拒绝原因来自宿主应答：{denied_content}"
    );
    wait_settled(&runtime).await;
    runtime.shutdown().await;
}

struct GatedFailProvider {
    release: Arc<Notify>,
}

impl ModelProvider for GatedFailProvider {
    fn stream(
        &self,
        _request: ModelRequest,
        _deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            release.notified().await;
            Err(ProviderError::new("provider 失败"))
        })
    }
}

#[tokio::test]
async fn failure_usage_row_is_committed_before_turn_failed_is_published() {
    let fixture = fixture("failure-order");
    let gate = Arc::new(Notify::new());
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(GatedFailProvider {
            release: Arc::clone(&gate),
        }),
    );
    let (_, mut receiver) = runtime.subscribe(None);
    runtime
        .submit(SessionInput::text("will fail"))
        .expect("提交");

    // 等 turn.started 落库后再拿住 SQLite 写锁：失败路径的 usage 插入会阻塞在
    // busy_timeout 上；若先发布 turn.failed，这里就能提前收到终态（修复前的行为）。
    loop {
        let event = within(receiver.recv()).await.expect("事件流");
        if event.event_type == SessionEventType::TurnStarted {
            break;
        }
    }
    let lock = rusqlite::Connection::open(&fixture.db_path).expect("锁连接");
    lock.busy_timeout(Duration::from_secs(5))
        .expect("busy_timeout");
    lock.execute_batch("BEGIN IMMEDIATE").expect("拿写锁");

    gate.notify_one();
    let premature = tokio::time::timeout(Duration::from_millis(400), receiver.recv()).await;
    assert!(
        premature.is_err(),
        "turn_usage 未提交前不得发布终态（实际提前收到 {premature:?}）"
    );
    lock.execute_batch("ROLLBACK").expect("释放写锁");

    let events = drain_until_terminal(&mut receiver).await;
    assert_eq!(
        terminals(&events),
        vec![SessionEventType::TurnFailed],
        "失败终态恰好一次"
    );
    let rows = turn_usage_rows(&fixture.db_path);
    assert_eq!(rows.len(), 1, "收到终态时 usage 行已可读");
    assert_eq!(rows[0].1, "error");
    wait_settled(&runtime).await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn hot_snapshot_preserves_workspace_identity() {
    let mut fixture = fixture("identity-snapshot");
    fixture.record.workspace_id = Some("ws-identity-123".to_string());
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "ok".to_string(),
        )])),
    );
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.session.workspace.workspace_identity.as_deref(),
        Some("ws-identity-123"),
        "热快照必须保留 workspaceIdentity"
    );
    assert_eq!(
        snapshot.session.workspace.workspace_key, "ws-identity-123",
        "workspaceKey = identity?.trim() || path"
    );
    assert_eq!(
        snapshot.session.workspace.workspace_path,
        fixture.cwd.to_string_lossy()
    );
    runtime.shutdown().await;
}

struct CountingSource {
    generation: Mutex<String>,
    resolves: AtomicU64,
    provider: Arc<dyn ModelProvider>,
}

impl agent_server::ProviderSource for CountingSource {
    fn resolve(
        &self,
        _provider_id: &str,
        _model_id: &str,
        _context: Option<&agent_server::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::clone(&self.provider))
    }

    fn cache_key(&self) -> String {
        self.generation.lock().expect("generation").clone()
    }
}

#[tokio::test]
async fn provider_is_re_resolved_when_config_generation_changes() {
    let fixture = fixture("provider-cache");
    let source = Arc::new(CountingSource {
        generation: Mutex::new("generation-a".to_string()),
        resolves: AtomicU64::new(0),
        provider: Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "answer".to_string(),
        )])),
    });
    let runtime = SessionRuntime::new(
        fixture.record.clone(),
        Arc::clone(&fixture.store),
        source.clone(),
        runtime_config(),
        RuntimeWiring::default(),
    )
    .expect("构造运行时");
    let (_, mut receiver) = runtime.subscribe(None);

    for prompt in ["one", "two"] {
        runtime.submit(SessionInput::text(prompt)).expect("提交");
        drain_until_terminal(&mut receiver).await;
        wait_settled(&runtime).await;
    }
    assert_eq!(
        source.resolves.load(Ordering::SeqCst),
        1,
        "配置代际不变的连续 turn 复用 provider"
    );

    *source.generation.lock().expect("generation") = "generation-b".to_string();
    runtime.submit(SessionInput::text("three")).expect("提交");
    drain_until_terminal(&mut receiver).await;
    wait_settled(&runtime).await;
    assert_eq!(
        source.resolves.load(Ordering::SeqCst),
        2,
        "personal 配置轮换后下一个 turn 必须重新解析 provider"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn stale_after_seq_is_treated_as_reset() {
    let fixture = fixture("stale-cursor");
    let runtime = spawn_runtime(
        &fixture,
        Arc::new(ScriptedProvider::new(vec![ScriptStep::Text(
            "answer".to_string(),
        )])),
    );
    let (replay, mut receiver, effective) = runtime.subscribe_with_cursor(Some(1_000));
    assert!(replay.is_empty(), "旧游标不能从中读出未来事件");
    assert_eq!(
        effective, None,
        "afterSeq > eventSeq 按 reset（不过滤）处理"
    );

    runtime
        .submit(SessionInput::text("after reset"))
        .expect("提交");
    let events = drain_until_terminal(&mut receiver).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == SessionEventType::TurnStarted),
        "reset 后 live 事件不能被旧游标过滤掉"
    );
    wait_settled(&runtime).await;
    runtime.shutdown().await;
}
