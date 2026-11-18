//! `session/subagents` 子代理只读投影（LT-1 C3 T4）。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §4（T0 黑盒夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c3/subagents-*.json`）与
//! `docs/specs/agent-c3-recovery-goal.md` §5。
//!
//! 覆盖：`-32004 Session not found` 边界（缺失会话 / 首 turn 前）、空数据形状
//! （`endedLimit` 缺省 20 = 协议 schema 值、`0`/负数 `-32602`）、running/ended 项
//! 形状（前台完成 / 后台运行 / 后台通知消费）、`endedAt` 倒序分页 + 不透明
//! base64 游标、子会话行与 tool part 缺失/损坏的 fail-soft、严格参数与
//! `SessionSubagentsResult` 的 round-trip。
//!
//! 纪律：**纯读投影**——只用 `session-store` 公开助手种库（会话行 / 消息 / part，
//! 形状与 `agent-server/src/runtime.rs` 的落库一致），不 spawn 会话、不实现 child
//! runtime（实际子代理执行归 C4，见契约 §4.8）。provider 装配源恒失败：本读面
//! 不发起模型请求。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_protocol::session_read::{
    EndedSubagent, EndedSubagentStatus, RunningSubagent, RunningSubagentStatus,
    SessionSubagentsEnded, SessionSubagentsParams, SessionSubagentsResult,
};
use agent_server::{
    run_stdio, AgentServer, ProviderConfigHub, ProviderRequestContext, ProviderSource, ServerConfig,
};
use base64::Engine as _;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use session_store::{MessageRecord, PartRecord, SessionRecord, SessionStore};
use std::time::Duration;

// ---------------------------------------------------------------------------
// 临时数据根 + 基线测试库（与 tests/methods.rs 同一口径）
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
            "agent-server-subagents-{label}-{}-{}",
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

// ---------------------------------------------------------------------------
// server 夹具：存储门先跑（空 stdio 输入 → 门完成即 EOF），再直接 dispatch
// ---------------------------------------------------------------------------

/// 本读面不发起模型请求：装配源显式失败即可。
struct UnusedSource;

impl ProviderSource for UnusedSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&ProviderRequestContext>,
    ) -> Result<Arc<dyn agent_core::ModelProvider>, String> {
        Err(format!("Provider 不存在: {provider_id}"))
    }

    fn cache_key(&self) -> String {
        "unused".to_string()
    }
}

struct Fixture {
    server: Arc<AgentServer>,
    db_path: PathBuf,
    ws: PathBuf,
    _root: TempRoot,
}

async fn fixture(label: &str) -> Fixture {
    let root = TempRoot::new(label);
    let db_dir = root.path().join(".mode").join("cli").join("db");
    std::fs::create_dir_all(&db_dir).expect("建 cli/db 目录");
    let db_path = db_dir.join("db.sqlite");
    create_baseline_database(&db_path);
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).expect("建工作目录");
    std::fs::write(ws.join("hello.txt"), "hello\n").expect("写种子文件");

    // 空 provider 规则文件：hub 可构造，模型面与本读面无关。
    let builtin = root.path().join("mode-builtin.json");
    std::fs::write(
        &builtin,
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
    let personal = root.path().join("provider-config.json");
    std::fs::write(
        &personal,
        r#"{
          "schemaVersion": 1,
          "config": {
            "providerConfigRules": {
              "providerRules": []
            },
            "modelConfigRules": {
              "providerModelRules": [],
              "manualProviderModelRules": []
            }
          }
        }"#,
    )
    .unwrap();

    let config = ServerConfig::from_data_base_dir(root.path());
    let server = Arc::new(AgentServer::with_parts(
        config,
        ProviderConfigHub::with_paths(builtin.to_string_lossy().into_owned(), personal),
        Arc::new(UnusedSource),
    ));
    // 空输入的 run_stdio：先跑存储门，随后 EOF 干净返回——此后 store() 就绪。
    run_stdio(&b""[..], &mut Vec::new(), Arc::clone(&server))
        .await
        .expect("存储门必须完成");
    Fixture {
        server,
        db_path,
        ws,
        _root: root,
    }
}

fn opened_store(db_path: &Path) -> SessionStore {
    SessionStore::open(db_path).expect("打开测试库")
}

// ---------------------------------------------------------------------------
// 种子助手：session-store 公开 API，行形状与 runtime.rs 落库一致
// ---------------------------------------------------------------------------

const T0: i64 = 1_794_965_760_000; // 夹具同量级的固定毫秒（断言可读）

fn session_row(
    id: &str,
    parent_id: Option<&str>,
    task_type: &str,
    title: &str,
    directory: &Path,
    time_created: i64,
    time_updated: i64,
) -> SessionRecord {
    SessionRecord {
        id: id.to_string(),
        project_id: "proj_seed".to_string(),
        workspace_id: None,
        parent_id: parent_id.map(str::to_owned),
        slug: id.to_string(),
        directory: directory.to_string_lossy().into_owned(),
        path: Some(directory.to_string_lossy().into_owned()),
        title: title.to_string(),
        version: "0.0.0".to_string(),
        share_url: None,
        summary_additions: None,
        summary_deletions: None,
        summary_files: None,
        summary_diffs: None,
        revert: None,
        permission: Some("{\"mode\":\"build\"}".to_string()),
        time_created,
        time_updated,
        time_compacting: None,
        time_archived: None,
        task_type: task_type.to_string(),
        title_source: "first_input".to_string(),
        title_message_id: None,
        time_title_updated: None,
        trace_id: Some("trace-seed".to_string()),
    }
}

fn message_row(session_id: &str, id: &str, sequence: i64, data: Value, time: i64) -> MessageRecord {
    MessageRecord {
        id: id.to_string(),
        session_id: session_id.to_string(),
        time_created: time,
        time_updated: time,
        data,
        sequence: Some(sequence),
    }
}

fn part_row(
    session_id: &str,
    message_id: &str,
    id: &str,
    sequence: i64,
    data: Value,
    time: i64,
) -> PartRecord {
    PartRecord {
        id: id.to_string(),
        message_id: message_id.to_string(),
        session_id: session_id.to_string(),
        time_created: time,
        time_updated: time,
        data,
        sequence: Some(sequence),
    }
}

/// turn 输入行（`user_prompt` 语义；形状对齐 `runtime.rs::persist_user_message`）。
fn user_prompt(
    session_id: &str,
    sequence: i64,
    text: &str,
    time: i64,
) -> (MessageRecord, Vec<PartRecord>) {
    let message_id = format!("msg-u-{session_id}-{sequence}");
    let message = message_row(
        session_id,
        &message_id,
        sequence,
        json!({
            "role": "user",
            "messageId": message_id,
            "sequence": sequence,
            "time": { "created": time },
            "model": { "providerID": "mock-chat", "modelID": "mock-1" },
            "semantics": {
                "origin": "real_user",
                "kind": "user_prompt",
                "uiVisibility": "visible",
                "providerVisibility": "visible",
                "transcriptVisibility": "visible",
            },
        }),
        time,
    );
    let part = part_row(
        session_id,
        &message_id,
        &format!("part-u-{session_id}-{sequence}"),
        0,
        json!({ "type": "text", "text": text, "time": { "start": time, "end": time } }),
        time,
    );
    (message, vec![part])
}

/// `background_task` 通知 turn 的输入行（`runtime.rs` 的 BackgroundNotification 语义）。
fn background_notification_row(
    session_id: &str,
    sequence: i64,
    text: &str,
    time: i64,
) -> (MessageRecord, Vec<PartRecord>) {
    let message_id = format!("msg-n-{session_id}-{sequence}");
    let message = message_row(
        session_id,
        &message_id,
        sequence,
        json!({
            "role": "user",
            "messageId": message_id,
            "sequence": sequence,
            "time": { "created": time },
            "model": { "providerID": "mock-chat", "modelID": "mock-1" },
            "semantics": {
                "origin": "agent_runtime",
                "kind": "background_notification",
                "source": "background_task",
                "uiVisibility": "hidden",
                "providerVisibility": "visible",
                "transcriptVisibility": "hidden",
            },
        }),
        time,
    );
    let part = part_row(
        session_id,
        &message_id,
        &format!("part-n-{session_id}-{sequence}"),
        0,
        json!({ "type": "text", "text": text, "time": { "start": time, "end": time } }),
        time,
    );
    (message, vec![part])
}

/// assistant 步（`persist_assistant_step` 形状）：finish + 可选工具 part。
/// 工具 part 的时间列用 `time_completed`（与落库一致，state 无 time 时的回退源）。
fn assistant_step(
    session_id: &str,
    sequence: i64,
    finish: &str,
    time_created: i64,
    time_completed: i64,
    tools: Vec<(String, Value)>,
) -> (MessageRecord, Vec<PartRecord>) {
    let message_id = format!("msg-a-{session_id}-{sequence}");
    let message = message_row(
        session_id,
        &message_id,
        sequence,
        json!({
            "role": "assistant",
            "messageId": message_id,
            "sequence": sequence,
            "parentID": "msg-u",
            "finish": finish,
            "providerId": "mock-chat",
            "modelId": "mock-1",
            "mode": "build",
            "time": { "created": time_created, "completed": time_completed },
            "semantics": {
                "origin": "agent_runtime",
                "kind": "assistant_response",
                "uiVisibility": "visible",
                "providerVisibility": "visible",
                "transcriptVisibility": "visible",
            },
        }),
        time_created,
    );
    let mut parts = vec![part_row(
        session_id,
        &message_id,
        &format!("part-s-{session_id}-{sequence}"),
        0,
        json!({ "type": "step-start" }),
        time_created,
    )];
    for (index, (call_id, state)) in tools.into_iter().enumerate() {
        parts.push(part_row(
            session_id,
            &message_id,
            &format!("part-t-{session_id}-{sequence}-{index}"),
            (index + 1) as i64,
            json!({ "type": "tool", "callID": call_id, "tool": "Agent", "state": state }),
            time_completed,
        ));
    }
    parts.push(part_row(
        session_id,
        &message_id,
        &format!("part-f-{session_id}-{sequence}"),
        (parts.len()) as i64,
        json!({ "type": "step-finish", "reason": finish, "cost": 0 }),
        time_completed,
    ));
    (message, parts)
}

/// Agent tool 的 state（与 `runtime.rs::tool_state_value` 同形状）。
fn agent_state_pending(input: Value) -> Value {
    json!({ "status": "pending", "input": input })
}

fn agent_state_completed(input: Value, output: &str, start: i64, end: i64) -> Value {
    json!({
        "status": "completed",
        "input": input,
        "output": output,
        "title": "Agent",
        "metadata": { "schemaVersion": 1 },
        "time": { "start": start, "end": end },
    })
}

fn agent_state_error(input: Value, error: &str, start: i64, end: i64) -> Value {
    json!({
        "status": "error",
        "input": input,
        "error": error,
        "title": "Agent",
        "metadata": {},
        "time": { "start": start, "end": end },
    })
}

/// 种父会话行 + 首 turn 的输入面：timeline(0) + user_prompt(1)。
/// （timeline 行角色 assistant、kind timeline_event，不计 turn 边界；turn 边界
/// 以 role=user 行与 assistant_response 终态行推导。）
fn seed_parent_base(store: &SessionStore, ws: &Path, session_id: &str) {
    store
        .create_session(session_row(
            session_id,
            None,
            "interactive",
            "plain prompt",
            ws,
            T0,
            T0 + 4_000,
        ))
        .expect("种父会话行");
    let timeline_id = format!("msg-tl-{session_id}");
    store
        .append_message(message_row(
            session_id,
            &timeline_id,
            0,
            json!({
                "role": "assistant",
                "messageId": timeline_id,
                "sequence": 0,
                "parentID": session_id,
                "finish": "completed",
                "time": { "created": T0, "completed": T0 },
                "semantics": {
                    "origin": "system",
                    "kind": "timeline_event",
                    "uiVisibility": "visible",
                    "providerVisibility": "hidden",
                    "transcriptVisibility": "visible",
                },
            }),
            T0,
        ))
        .expect("种 timeline 行");
    let (user, user_parts) = user_prompt(session_id, 1, "plain prompt", T0 + 100);
    store
        .append_message_with_parts(user, &user_parts)
        .expect("种 user_prompt 行");
}

/// 在 base 之上补一个无工具的 turn 终态步（空数据夹具的完整 turn）。
fn seed_turn_final_step(store: &SessionStore, session_id: &str, sequence: i64, time: i64) {
    let (stop, stop_parts) = assistant_step(session_id, sequence, "stop", time, time, vec![]);
    store
        .append_message_with_parts(stop, &stop_parts)
        .expect("种 turn 终态步");
}

fn seed_child(
    store: &SessionStore,
    ws: &Path,
    parent_id: &str,
    alias_ordinal: i64,
    title: &str,
    time_created: i64,
) {
    let id = format!("child-row-{alias_ordinal}-{parent_id}");
    store
        .create_session(session_row(
            &id,
            Some(parent_id),
            "subagent_child",
            title,
            ws,
            time_created,
            time_created + 2_000,
        ))
        .expect("种子会话行");
}

fn dispatch_subagents(
    server: &AgentServer,
    params: Value,
) -> Result<Value, agent_server::ServerError> {
    server.dispatch("session/subagents", params)
}

fn b64_encode(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

fn b64_decode(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .expect("游标必须是合法 base64")
}

// ---------------------------------------------------------------------------
// 边界：-32004 Session not found
// ---------------------------------------------------------------------------

/// 契约 §4.1：不存在的 session → `-32004 Session not found: sess_missing`。
#[tokio::test]
async fn missing_session_returns_session_not_found() {
    let fixture = fixture("missing").await;
    let error = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_missing" }))
        .expect_err("缺失会话必须显式报错");
    assert_eq!(error.code(), -32004);
    assert_eq!(error.message(), "Session not found: sess_missing");
    assert_eq!(error.data(), None);
}

/// 契约 §4.1：会话在 session/read 可见但还没有 turn → 同样 `-32004`。
#[tokio::test]
async fn before_first_turn_returns_session_not_found() {
    let fixture = fixture("before-turn").await;
    {
        let store = opened_store(&fixture.db_path);
        store
            .create_session(session_row(
                "sess_cold",
                None,
                "interactive",
                "plain prompt",
                &fixture.ws,
                T0,
                T0,
            ))
            .expect("种无 turn 的会话行");
    }

    // 对照：同一会话 session/read 可见（冷读）。
    let read = fixture
        .server
        .dispatch("session/read", json!({ "sessionId": "sess_cold" }))
        .expect("session/read 冷读可见");
    assert_eq!(read["session"]["sessionId"], json!("sess_cold"));

    let error = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_cold" }))
        .expect_err("首 turn 前必须报 not found");
    assert_eq!(error.code(), -32004);
    assert_eq!(error.message(), "Session not found: sess_cold");
}

// ---------------------------------------------------------------------------
// 空数据：形状 + endedLimit 缺省 20（协议 schema 值）+ 0/负数 -32602
// ---------------------------------------------------------------------------

/// 契约 §4.1：完成 turn 后空数据 —— 省略 endedLimit 与传 20 相同，
/// `nextCursor` 键省略（不是 null）。
#[tokio::test]
async fn empty_after_turn_matches_fixture() {
    let fixture = fixture("empty").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_empty");
        seed_turn_final_step(&store, "sess_empty", 2, T0 + 2_000);
    }

    let expected = json!({
        "revision": 4,
        "childSessionIds": [],
        "running": [],
        "ended": { "total": 0, "items": [] }
    });
    for params in [
        json!({ "sessionId": "sess_empty" }),
        json!({ "sessionId": "sess_empty", "endedLimit": 20 }),
    ] {
        let result = dispatch_subagents(&fixture.server, params).expect("空数据必须成功");
        assert_eq!(result, expected, "空数据形状必须与夹具一致");
        assert!(
            result["ended"].get("nextCursor").is_none(),
            "无更多页时 nextCursor 键必须省略"
        );
    }
}

/// 契约 §4.1：`endedLimit:0` / `-1` 都是 `-32602`，message 与 data 逐字对齐夹具。
#[tokio::test]
async fn ended_limit_zero_and_negative_are_invalid_params() {
    let fixture = fixture("bad-limit").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_limit");
        seed_turn_final_step(&store, "sess_limit", 2, T0 + 2_000);
    }

    let expected_data = json!({
        "name": "ZodError",
        "message": "[\n  {\n    \"origin\": \"number\",\n    \"code\": \"too_small\",\n    \"minimum\": 0,\n    \"inclusive\": false,\n    \"path\": [\n      \"endedLimit\"\n    ],\n    \"message\": \"Too small: expected number to be >0\"\n  }\n]",
    });
    for limit in [0, -1] {
        let error = dispatch_subagents(
            &fixture.server,
            json!({ "sessionId": "sess_limit", "endedLimit": limit }),
        )
        .expect_err("endedLimit 必须 >0");
        assert_eq!(error.code(), -32602, "limit={limit}");
        assert_eq!(
            error.message(),
            "Invalid params — endedLimit: Too small: expected number to be >0"
        );
        assert_eq!(error.data(), Some(expected_data.clone()));
    }
}

/// 协议 schema 的缺省值是 20（`modeSessionSubagentsParamsSchema` 的 `.default(20)`；
/// 标注为 schema 值——夹具空数据下「省略」与「传 20」不可区分，>20 截断未观察）。
#[tokio::test]
async fn ended_limit_schema_default_is_20() {
    let fixture = fixture("default-limit").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_many");
        // 一个 assistant 步里 22 个并行的已完成前台 Agent 调用 + 子会话行，endedAt 递增。
        let tools = (0..22_i64)
            .map(|index| {
                let call_id = format!("toolu_many_{index}");
                let start = T0 + 1_000 + index * 1_000;
                let end = start + 2_000;
                (
                    call_id,
                    agent_state_completed(
                        json!({
                            "description": format!("child {index}"),
                            "prompt": format!("child {index} prompt"),
                            "subagent_type": "general-purpose",
                        }),
                        &format!(
                            "child {index} result\nagentId: agent_many_{index} (use SendMessage with to: 'agent_many_{index}' to continue this agent)\n<usage>subagent_tokens: 22\ntool_uses: 0\nduration_ms: 2000</usage>"
                        ),
                        start,
                        end,
                    ),
                )
            })
            .collect();
        let (assistant, parts) =
            assistant_step("sess_many", 2, "tool-calls", T0 + 1_000, T0 + 24_000, tools);
        store
            .append_message_with_parts(assistant, &parts)
            .expect("种 22 个 Agent 调用");
        seed_turn_final_step(&store, "sess_many", 3, T0 + 24_100);
        for index in 0..22_i64 {
            seed_child(
                &store,
                &fixture.ws,
                "sess_many",
                index + 1,
                &format!("child {index} prompt"),
                T0 + 1_100 + index * 1_000,
            );
        }
    }

    // 缺省（= schema 20）：20 项 + nextCursor，total 是全集。
    let result =
        dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_many" })).expect("缺省分页");
    assert_eq!(result["revision"], json!(4));
    assert_eq!(result["childSessionIds"].as_array().unwrap().len(), 22);
    assert_eq!(result["ended"]["total"], json!(22));
    assert_eq!(result["ended"]["items"].as_array().unwrap().len(), 20);
    assert!(
        result["ended"].get("nextCursor").is_some(),
        "缺省 20 截页后必须给 nextCursor"
    );

    // 显式 22：全量返回、无 nextCursor。
    let result = dispatch_subagents(
        &fixture.server,
        json!({ "sessionId": "sess_many", "endedLimit": 22 }),
    )
    .expect("全量分页");
    assert_eq!(result["ended"]["items"].as_array().unwrap().len(), 22);
    assert!(result["ended"].get("nextCursor").is_none());
}

// ---------------------------------------------------------------------------
// running / ended 形状
// ---------------------------------------------------------------------------

/// 契约 §4.2 前台 running：tool part 还在执行（pending）→ running[] 一项。
///
/// 已登记差异（§4.8）：DB 投影下前台 running 项的 `agentId` 无事实源
/// （agentId 只在 tool 输出的 `agentId:` 行出现），键省略。
#[tokio::test]
async fn foreground_running_shape() {
    let fixture = fixture("fg-running").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_fg_run");
        let (assistant, parts) = assistant_step(
            "sess_fg_run",
            2,
            "tool-calls",
            T0 + 200,
            T0 + 200,
            vec![(
                "toolu_mock_0_0".to_string(),
                agent_state_pending(json!({
                    "description": "child task",
                    "prompt": "child task prompt: inspect the workspace",
                    "subagent_type": "general-purpose",
                })),
            )],
        );
        store
            .append_message_with_parts(assistant, &parts)
            .expect("种 Agent tool 调用");
        seed_child(
            &store,
            &fixture.ws,
            "sess_fg_run",
            1,
            "child task prompt: inspect the workspace",
            T0 + 250,
        );
    }

    let result = dispatch_subagents(
        &fixture.server,
        json!({ "sessionId": "sess_fg_run", "endedLimit": 20 }),
    )
    .expect("running 投影");
    assert_eq!(
        result,
        json!({
            "revision": 3,
            "childSessionIds": ["child-1"],
            "running": [
                {
                    "childSessionId": "child-1",
                    "toolCallId": "toolu_mock_0_0",
                    "subagentType": "general-purpose",
                    "title": "child task",
                    "startedAt": T0 + 200,
                    "status": "running",
                }
            ],
            "ended": { "total": 0, "items": [] }
        })
    );
}

/// 契约 §4.2 前台 ended：完成项带 `summary`（`agentId:` 行之前的前缀）、
/// `agentId`（该行的首个 token）、`endedAt`。
#[tokio::test]
async fn foreground_ended_shape() {
    let fixture = fixture("fg-ended").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_fg_end");
        let (assistant, parts) = assistant_step(
            "sess_fg_end",
            2,
            "tool-calls",
            T0 + 200,
            T0 + 300,
            vec![(
                "toolu_mock_0_0".to_string(),
                agent_state_completed(
                    json!({
                        "description": "child task",
                        "prompt": "child task prompt: inspect the workspace",
                        "subagent_type": "general-purpose",
                    }),
                    "child result text\nagentId: agent_abc123 (use SendMessage with to: 'agent_abc123' to continue this agent)\n<usage>subagent_tokens: 22\ntool_uses: 0\nduration_ms: 3105</usage>",
                    T0 + 200,
                    T0 + 4_843,
                ),
            )],
        );
        store
            .append_message_with_parts(assistant, &parts)
            .expect("种完成的 Agent tool 调用");
        seed_turn_final_step(&store, "sess_fg_end", 3, T0 + 5_000);
        seed_child(
            &store,
            &fixture.ws,
            "sess_fg_end",
            1,
            "child task prompt: inspect the workspace",
            T0 + 250,
        );
    }

    let result = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_fg_end" }))
        .expect("ended 投影");
    assert_eq!(
        result,
        json!({
            "revision": 4,
            "childSessionIds": ["child-1"],
            "running": [],
            "ended": {
                "total": 1,
                "items": [
                    {
                        "childSessionId": "child-1",
                        "agentId": "agent_abc123",
                        "toolCallId": "toolu_mock_0_0",
                        "subagentType": "general-purpose",
                        "title": "child task",
                        "startedAt": T0 + 200,
                        "status": "success",
                        "summary": "child result text",
                        "endedAt": T0 + 4_843,
                    }
                ]
            }
        })
    );
}

/// 夹具 subagents-background：`run_in_background:true` 的 tool 调用在启动后即
/// completed，但子代理仍在跑 → running[]；通知（`<task-id>` 匹配 agentId）到达后
/// 才落入 ended，summary 取通知 `<result>`，endedAt 取通知消息时间。
#[tokio::test]
async fn background_running_then_settled_by_notification() {
    let fixture = fixture("bg-settle").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_bg");
        let launch_output = "Async agent launched successfully.\nagentId: agent_bg1 (internal ID - do not mention to user. Use SendMessage with to: 'agent_bg1' to continue this agent.)\nThe agent is working in the background. You will be notified automatically when it completes.\noutput_file: output.txt";
        let (assistant, parts) = assistant_step(
            "sess_bg",
            2,
            "tool-calls",
            T0 + 200,
            T0 + 300,
            vec![(
                "toolu_mock_0_0".to_string(),
                agent_state_completed(
                    json!({
                        "description": "background child",
                        "prompt": "background child prompt: inspect in background",
                        "subagent_type": "general-purpose",
                        "run_in_background": true,
                    }),
                    launch_output,
                    T0 + 200,
                    T0 + 300,
                ),
            )],
        );
        store
            .append_message_with_parts(assistant, &parts)
            .expect("种后台启动 tool 调用");
        seed_child(
            &store,
            &fixture.ws,
            "sess_bg",
            1,
            "background child prompt: inspect in background",
            T0 + 250,
        );
    }

    // mid：通知未到（父 turn 也未完成——夹具 mid 的 revision 3）。
    let mid = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_bg" }))
        .expect("后台 running 投影");
    assert_eq!(
        mid,
        json!({
            "revision": 3,
            "childSessionIds": ["child-1"],
            "running": [
                {
                    "childSessionId": "child-1",
                    "agentId": "agent_bg1",
                    "toolCallId": "toolu_mock_0_0",
                    "subagentType": "general-purpose",
                    "title": "background child",
                    "startedAt": T0 + 200,
                    "status": "running",
                }
            ],
            "ended": { "total": 0, "items": [] }
        })
    );

    // after：父 turn 0 完成（assistant stop）+ 完成通知 turn（输入 + 终态步）到达
    // → started×2 + completed×2 → revision 6；ended 一项。
    {
        let store = opened_store(&fixture.db_path);
        seed_turn_final_step(&store, "sess_bg", 3, T0 + 4_000);
        let notification_time = T0 + 6_000;
        let notification_text = "<task-notification>\n<task-id>agent_bg1</task-id>\n<tool-use-id>toolu_mock_0_0</tool-use-id>\n<output-file>output.txt</output-file>\n<status>completed</status>\n<summary>Agent general-purpose task &quot;background child&quot; completed.</summary>\n<result>background child result</result>\n<usage><subagent_tokens>22</subagent_tokens><tool_uses>0</tool_uses><duration_ms>3105</duration_ms></usage>\n</task-notification>";
        let (notification, notification_parts) =
            background_notification_row("sess_bg", 4, notification_text, notification_time);
        store
            .append_message_with_parts(notification, &notification_parts)
            .expect("种完成通知");
        seed_turn_final_step(&store, "sess_bg", 5, T0 + 6_100);
    }
    let after = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_bg" }))
        .expect("后台 ended 投影");
    assert_eq!(
        after,
        json!({
            "revision": 6,
            "childSessionIds": ["child-1"],
            "running": [],
            "ended": {
                "total": 1,
                "items": [
                    {
                        "childSessionId": "child-1",
                        "agentId": "agent_bg1",
                        "toolCallId": "toolu_mock_0_0",
                        "subagentType": "general-purpose",
                        "title": "background child",
                        "startedAt": T0 + 200,
                        "status": "success",
                        "summary": "background child result",
                        "endedAt": T0 + 6_000,
                    }
                ]
            }
        })
    );
}

/// 失败路径（未观察状态里唯一可观察的映射源）：tool part `error` → ended
/// `failed`，无 summary（§4.8 登记）。
#[tokio::test]
async fn error_tool_state_maps_to_failed_without_summary() {
    let fixture = fixture("failed").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_fail");
        let (assistant, parts) = assistant_step(
            "sess_fail",
            2,
            "tool-calls",
            T0 + 200,
            T0 + 300,
            vec![(
                "toolu_fail".to_string(),
                agent_state_error(
                    json!({
                        "description": "doomed child",
                        "prompt": "doomed child prompt",
                        "subagent_type": "general-purpose",
                    }),
                    "child crashed",
                    T0 + 200,
                    T0 + 900,
                ),
            )],
        );
        store
            .append_message_with_parts(assistant, &parts)
            .expect("种失败的 Agent tool 调用");
        seed_turn_final_step(&store, "sess_fail", 3, T0 + 1_000);
        seed_child(
            &store,
            &fixture.ws,
            "sess_fail",
            1,
            "doomed child prompt",
            T0 + 250,
        );
    }

    let result = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_fail" }))
        .expect("failed 投影");
    assert_eq!(result["running"], json!([]));
    assert_eq!(result["ended"]["total"], json!(1));
    let item = &result["ended"]["items"][0];
    assert_eq!(item["status"], json!("failed"));
    assert_eq!(item["childSessionId"], json!("child-1"));
    assert_eq!(item["title"], json!("doomed child"));
    assert!(
        item.get("summary").is_none(),
        "失败项没有 summary（未观察形状，键省略）"
    );
    assert!(
        item.get("agentId").is_none(),
        "error 态没有 output，agentId 无事实源"
    );
    assert_eq!(item["endedAt"], json!((T0 + 900) as u64));
}

// ---------------------------------------------------------------------------
// 分页：endedAt 倒序 + 不透明游标（夹具 subagents-pagination）
// ---------------------------------------------------------------------------

/// 夹具 §4.4：串行创建的两个子代理按 endedAt 倒序分页；endedLimit:1 先返回
/// 后完成的 child-2 与 base64 游标；第二页返回 child-1 且无 nextCursor；
/// childSessionIds 两页都保持创建顺序。
#[tokio::test]
async fn pagination_returns_newest_first_with_opaque_cursor() {
    let fixture = fixture("paging").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_page");

        // 第一个 turn：child one（general-purpose，先完成）。
        let one_start = T0 + 200;
        let one_end = T0 + 2_000;
        let (user_two, user_two_parts) = user_prompt("sess_page", 4, "spawn child two", T0 + 2_400);
        let (first, first_parts) = assistant_step(
            "sess_page",
            2,
            "tool-calls",
            one_start,
            one_start + 100,
            vec![(
                "toolu_one".to_string(),
                agent_state_completed(
                    json!({
                        "description": "child one",
                        "prompt": "child one prompt",
                        "subagent_type": "general-purpose",
                    }),
                    "child one result\nagentId: agent_one (use SendMessage with to: 'agent_one' to continue this agent)\n<usage>subagent_tokens: 22\ntool_uses: 0\nduration_ms: 1800</usage>",
                    one_start,
                    one_end,
                ),
            )],
        );
        store
            .append_message_with_parts(first, &first_parts)
            .expect("种 child one 步");
        seed_turn_final_step(&store, "sess_page", 3, T0 + 2_100);
        seed_child(
            &store,
            &fixture.ws,
            "sess_page",
            1,
            "child one prompt",
            one_start + 50,
        );

        // 第二个 turn：child two（Explore，后完成 → ended 第一位）。
        let two_start = T0 + 2_500;
        let two_end = T0 + 4_000;
        let (second, second_parts) = assistant_step(
            "sess_page",
            5,
            "tool-calls",
            two_start,
            two_start + 100,
            vec![(
                "toolu_two".to_string(),
                agent_state_completed(
                    json!({
                        "description": "child two",
                        "prompt": "child two prompt",
                        "subagent_type": "Explore",
                    }),
                    "child two result\nagentId: agent_two (use SendMessage with to: 'agent_two' to continue this agent)\n<usage>subagent_tokens: 22\ntool_uses: 0\nduration_ms: 1500</usage>",
                    two_start,
                    two_end,
                ),
            )],
        );
        store
            .append_message_with_parts(second, &second_parts)
            .expect("种 child two 步");
        store
            .append_message_with_parts(user_two, &user_two_parts)
            .expect("种第二个 turn 输入");
        seed_turn_final_step(&store, "sess_page", 6, T0 + 4_100);
        seed_child(
            &store,
            &fixture.ws,
            "sess_page",
            2,
            "child two prompt",
            two_start + 50,
        );
    }

    // 第一页：endedLimit 1 → child two（最新完成）+ nextCursor。
    let first_page = dispatch_subagents(
        &fixture.server,
        json!({ "sessionId": "sess_page", "endedLimit": 1 }),
    )
    .expect("第一页");
    let two_end = T0 + 4_000;
    assert_eq!(
        first_page,
        json!({
            "revision": 6,
            "childSessionIds": ["child-1", "child-2"],
            "running": [],
            "ended": {
                "total": 2,
                "items": [
                    {
                        "childSessionId": "child-2",
                        "agentId": "agent_two",
                        "toolCallId": "toolu_two",
                        "subagentType": "Explore",
                        "title": "child two",
                        "startedAt": T0 + 2_500,
                        "status": "success",
                        "summary": "child two result",
                        "endedAt": two_end as u64,
                    }
                ],
                "nextCursor": first_page["ended"]["nextCursor"],
            }
        })
    );
    // 游标解码形状（夹具 `cursor.decoded`）：{childSessionId, endedAt}。
    let cursor_text = first_page["ended"]["nextCursor"]
        .as_str()
        .expect("游标是字符串")
        .to_string();
    let decoded: Value =
        serde_json::from_slice(&b64_decode(&cursor_text)).expect("游标解码为 JSON");
    assert_eq!(
        decoded,
        json!({ "childSessionId": "child-2", "endedAt": two_end as u64 })
    );

    // 第二页：endedCursor → 剩余 child one，无 nextCursor。
    let second_page = dispatch_subagents(
        &fixture.server,
        json!({
            "sessionId": "sess_page",
            "endedLimit": 1,
            "endedCursor": cursor_text,
        }),
    )
    .expect("第二页");
    assert_eq!(second_page["ended"]["total"], json!(2));
    let items = second_page["ended"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["childSessionId"], json!("child-1"));
    assert_eq!(items[0]["subagentType"], json!("general-purpose"));
    assert!(
        second_page["ended"].get("nextCursor").is_none(),
        "最后一页没有 nextCursor"
    );
    // childSessionIds 两页都保持创建顺序。
    assert_eq!(
        second_page["childSessionIds"],
        json!(["child-1", "child-2"])
    );
}

/// 未观察边界（§4.8 登记）：非法/过期游标显式 `-32602`，不静默回第一页。
#[tokio::test]
async fn malformed_or_expired_cursor_is_invalid_params() {
    let fixture = fixture("cursor").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_cursor");
        let (assistant, parts) = assistant_step(
            "sess_cursor",
            2,
            "tool-calls",
            T0 + 200,
            T0 + 300,
            vec![(
                "toolu_c".to_string(),
                agent_state_completed(
                    json!({
                        "description": "child task",
                        "prompt": "child task prompt",
                        "subagent_type": "general-purpose",
                    }),
                    "child result\nagentId: agent_c (use SendMessage with to: 'agent_c' to continue this agent)",
                    T0 + 200,
                    T0 + 1_000,
                ),
            )],
        );
        store
            .append_message_with_parts(assistant, &parts)
            .expect("种完成的 Agent tool 调用");
        seed_turn_final_step(&store, "sess_cursor", 3, T0 + 1_100);
        seed_child(
            &store,
            &fixture.ws,
            "sess_cursor",
            1,
            "child task prompt",
            T0 + 250,
        );
    }

    // 非法 base64。
    let error = dispatch_subagents(
        &fixture.server,
        json!({ "sessionId": "sess_cursor", "endedCursor": "not-base64!!" }),
    )
    .expect_err("非法游标必须显式报错");
    assert_eq!(error.code(), -32602);

    // 合法 base64 但 childSessionId 不在 ended 全集（过期）。
    let expired = b64_encode(&json!({ "childSessionId": "child-9", "endedAt": 1 }).to_string());
    let error = dispatch_subagents(
        &fixture.server,
        json!({ "sessionId": "sess_cursor", "endedCursor": expired }),
    )
    .expect_err("过期游标必须显式报错");
    assert_eq!(error.code(), -32602);
}

// ---------------------------------------------------------------------------
// fail-soft：缺失/损坏数据（契约 §4.8）
// ---------------------------------------------------------------------------

/// 子会话行存在但没有对应 Agent tool part：只有 childSessionIds，列表为空。
#[tokio::test]
async fn child_rows_without_tool_parts_fail_soft() {
    let fixture = fixture("rows-only").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_rows");
        seed_turn_final_step(&store, "sess_rows", 2, T0 + 2_000);
        seed_child(
            &store,
            &fixture.ws,
            "sess_rows",
            1,
            "orphan prompt",
            T0 + 250,
        );
    }

    let result = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_rows" }))
        .expect("孤儿子会话行必须 fail-soft");
    assert_eq!(result["childSessionIds"], json!(["child-1"]));
    assert_eq!(result["running"], json!([]));
    assert_eq!(result["ended"], json!({ "total": 0, "items": [] }));
}

/// tool part 缺 `state` → 跳过；缺 `state.input.description`/`subagent_type` →
/// 必填字段用中性空串（与 projection.rs 的取值纪律一致），不 panic。
#[tokio::test]
async fn corrupt_tool_parts_fail_soft() {
    let fixture = fixture("corrupt").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_corrupt");
        // 无 state 的 tool part：无法分类，跳过；input 为空对象的完成态：
        // title/subagentType 中性空串。
        let (assistant, parts) = assistant_step(
            "sess_corrupt",
            2,
            "tool-calls",
            T0 + 200,
            T0 + 300,
            vec![
                ("toolu_broken".to_string(), json!({})),
                (
                    "toolu_empty".to_string(),
                    agent_state_completed(json!({}), "", T0 + 400, T0 + 1_000),
                ),
            ],
        );
        store
            .append_message_with_parts(assistant, &parts)
            .expect("种损坏/空 input 的 tool part");
        seed_turn_final_step(&store, "sess_corrupt", 3, T0 + 1_100);
        seed_child(
            &store,
            &fixture.ws,
            "sess_corrupt",
            1,
            "some prompt",
            T0 + 250,
        );
    }

    let result = dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_corrupt" }))
        .expect("损坏数据必须 fail-soft 不 panic");
    assert_eq!(result["childSessionIds"], json!(["child-1"]));
    assert_eq!(result["running"], json!([]), "无 state 的 part 跳过");
    let items = result["ended"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "只有可分类的 part 成为条目");
    assert_eq!(items[0]["toolCallId"], json!("toolu_empty"));
    assert_eq!(items[0]["title"], json!(""), "缺 description 用中性空串");
    assert_eq!(
        items[0]["subagentType"],
        json!(""),
        "缺 subagent_type 用中性空串"
    );
    assert!(items[0].get("agentId").is_none());
    assert!(
        items[0].get("summary").is_none(),
        "空 output 的 summary 键省略"
    );
}

// ---------------------------------------------------------------------------
// 严格参数与协议 round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_params_reject_unknown_keys_and_missing_session_id() {
    let fixture = fixture("strict").await;
    for params in [
        json!({}),
        json!({ "unknown": 1 }),
        json!({ "sessionId": "sess_x", "unexpected": true }),
        json!({ "sessionId": "sess_x", "endedCursor": 5 }),
    ] {
        let error =
            dispatch_subagents(&fixture.server, params.clone()).expect_err("严格参数必须拒绝");
        assert_eq!(error.code(), -32602, "params={params}");
    }
}

/// `SessionSubagentsResult` 的线格式 round-trip：序列化 = 夹具 JSON（camelCase、
/// `nextCursor` 省略语义），反序列化还原相等。
#[test]
fn session_subagents_result_round_trip() {
    let running = RunningSubagent {
        child_session_id: "child-1".to_string(),
        agent_id: Some("agent_bg1".to_string()),
        tool_call_id: Some("toolu_mock_0_0".to_string()),
        subagent_type: "general-purpose".to_string(),
        title: "background child".to_string(),
        summary: None,
        started_at: Some(1_794_965_762_749),
        ended_at: None,
        status: RunningSubagentStatus::Running,
    };
    let ended = EndedSubagent {
        child_session_id: "child-1".to_string(),
        agent_id: Some("agent_abc123".to_string()),
        tool_call_id: Some("toolu_mock_0_0".to_string()),
        subagent_type: "general-purpose".to_string(),
        title: "child task".to_string(),
        summary: Some("child result text".to_string()),
        started_at: Some(1_794_965_762_749),
        ended_at: Some(1_794_965_766_843),
        status: EndedSubagentStatus::Success,
    };
    let result = SessionSubagentsResult {
        revision: 4,
        child_session_ids: vec!["child-1".to_string()],
        running: vec![running.clone()],
        ended: SessionSubagentsEnded {
            total: 1,
            items: vec![ended.clone()],
            next_cursor: None,
        },
    };

    let value = serde_json::to_value(&result).expect("结果可序列化");
    assert_eq!(
        value,
        json!({
            "revision": 4,
            "childSessionIds": ["child-1"],
            "running": [
                {
                    "childSessionId": "child-1",
                    "agentId": "agent_bg1",
                    "toolCallId": "toolu_mock_0_0",
                    "subagentType": "general-purpose",
                    "title": "background child",
                    "startedAt": 1_794_965_762_749i64,
                    "status": "running",
                }
            ],
            "ended": {
                "total": 1,
                "items": [
                    {
                        "childSessionId": "child-1",
                        "agentId": "agent_abc123",
                        "toolCallId": "toolu_mock_0_0",
                        "subagentType": "general-purpose",
                        "title": "child task",
                        "startedAt": 1_794_965_762_749i64,
                        "status": "success",
                        "summary": "child result text",
                        "endedAt": 1_794_965_766_843i64,
                    }
                ]
            }
        })
    );
    let back: SessionSubagentsResult = serde_json::from_value(value).expect("结果可反序列化");
    assert_eq!(back, result);

    // params：`endedLimit` 缺省 20（协议 schema 值），`endedCursor` 可省略。
    let params: SessionSubagentsParams =
        serde_json::from_value(json!({ "sessionId": "sess-1" })).expect("params 缺省解析");
    assert_eq!(params.session_id, "sess-1");
    assert_eq!(params.ended_cursor, None);
    assert_eq!(params.ended_limit, 20);
    let params: SessionSubagentsParams = serde_json::from_value(
        json!({ "sessionId": "sess-1", "endedCursor": "cursor-1", "endedLimit": 5 }),
    )
    .expect("params 全量解析");
    assert_eq!(params.ended_cursor.as_deref(), Some("cursor-1"));
    assert_eq!(params.ended_limit, 5);
}

/// DB 事实不被读面改写：投影后 session/message/part 行保持原样（纯读）。
#[tokio::test]
async fn projection_is_read_only() {
    let fixture = fixture("read-only").await;
    {
        let store = opened_store(&fixture.db_path);
        seed_parent_base(&store, &fixture.ws, "sess_ro");
        seed_turn_final_step(&store, "sess_ro", 2, T0 + 2_000);
        seed_child(&store, &fixture.ws, "sess_ro", 1, "child prompt", T0 + 250);
    }

    let count = |table: &str| -> i64 {
        raw_connection(&fixture.db_path)
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("统计行数")
    };
    let before = (
        count("session"),
        count("message"),
        count("part"),
        count("session_task_link"),
    );
    let _ =
        dispatch_subagents(&fixture.server, json!({ "sessionId": "sess_ro" })).expect("只读投影");
    let after = (
        count("session"),
        count("message"),
        count("part"),
        count("session_task_link"),
    );
    assert_eq!(before, after, "session/subagents 不得写任何表");
}
