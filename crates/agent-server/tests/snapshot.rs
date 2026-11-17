//! C2 T6：快照/读面对齐（settings / projection / runtime / contextUsage / slashCommands）。
//!
//! 行为依据：`docs/specs/agent-c2-contract.md` §4（4.1 create / 4.2 read / 4.3
//! resume / 4.4 settings.model.available）与机器录制的夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/context-usage-settings.json`
//! （只读；本文件把录制值换算到测试 provider `mock-chat`/`Mock Chat`——oracle 夹具
//! 是 `oracle-mock`/`Oracle Mock`，字段形状一一对应）。与观察的差异（create
//! `stateRevision`、resume `eventSeq`、read `deliveryKind`、`contextUsage.breakdown`）
//! 是登记缺省（契约 §4.5），断言的是 Rust 现状而不是硬凑观察值。
//!
//! 测试库 DDL 从 `scripts/sqlite-schema-baseline.json` 生成（与 methods/runtime 测试
//! 同一口径）；provider 配置走临时 builtin/personal 文件；模型端点 `127.0.0.1:9`
//! （保留端口，**绝不**发起真实请求——provider 由脚本化 fake 装配）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::provider::{ModelProvider, ScriptStep, ScriptedProvider};
use agent_protocol::snapshot::SessionStateSnapshot;
use agent_server::{
    run_stdio, AgentServer, Frame, ProviderConfigHub, ProviderSource, RequestId, ServerConfig,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

// ---------------------------------------------------------------------------
// 基线测试库夹具（与 tests/methods.rs 同一口径）
// ---------------------------------------------------------------------------

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS schema_migration (
    id text primary key,
    checksum text not null,
    app_version text not null,
    time_applied integer not null
)";

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "agent-server-snapshot-{label}-{}-{}",
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
        let mut definition = format!("\"{}\" {}", column_name.replace('"', "\"\""), column_type);
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
            .map(|(position, name)| {
                let _ = position;
                format!("\"{}\"", name.replace('"', "\"\""))
            })
            .collect::<Vec<_>>()
            .join(", ");
        definitions.push(format!("PRIMARY KEY ({names})"));
    }
    format!(
        "CREATE TABLE \"{name}\" (\n  {}\n)",
        definitions.join(",\n  ")
    )
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

// ---------------------------------------------------------------------------
// provider 夹具：临时 builtin/personal 文件 + 脚本化 provider 装配源
// ---------------------------------------------------------------------------

/// 写最小 Personal provider 文件：`mock-chat` 的 `mock-1` / `mock-2` 两个模型。
///
/// 模型规则与 oracle mock（`mock-provider.mts` 的 `writeOracleProviderConfig`）同形状：
/// `contextWindow:200000`、`maxOutputTokens:8192`、`reasoningLevel:["disabled","enabled"]`、
/// `inputFormat:{supportsText:true}`（其余 supports* 缺省）、`outputFormat:{supportsText:true}`。
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

/// 脚本化装配源：`mock-chat` 的模型解析成同一份剧本的 provider。
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
    ws: PathBuf,
    _root: TempRoot,
}

fn fixture(label: &str, steps: Vec<ScriptStep>) -> Fixture {
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
        Arc::new(ScriptedSource { steps }),
    ));
    Fixture {
        server,
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

    /// 物化即发的 `session/requestRuntimePreferences` 反向请求由宿主按 spec §1.3
    /// 的默认应答回掉（memoryEnabled:false，与 context-usage 观察口径一致）。
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
        match self.error_or_result(id).await {
            Ok(value) => value,
            Err((code, message)) => panic!("请求 {id} 失败: {message} ({code})"),
        }
    }

    /// 等指定请求的应答：`Ok(result)` 或 `Err((code, message))`。
    async fn error_or_result(&mut self, id: u64) -> Result<Value, (i64, String)> {
        let expected = RequestId::Number(id as i64);
        loop {
            match self.next_frame().await {
                Frame::Response(response) if response.id == expected => return Ok(response.result),
                Frame::ErrorResponse(response) if response.id == expected => {
                    return Err((response.error.code, response.error.message))
                }
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

    /// 等下一帧 `session/event` 通知（跳过其他帧）。
    async fn next_event(&mut self) -> Value {
        loop {
            if let Frame::Notification(notification) = self.next_frame().await {
                assert_eq!(notification.method, "session/event");
                return notification.params.expect("事件通知必须带 params");
            }
        }
    }

    /// 收集事件直到 `turn.completed` / `turn.failed`。
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

/// create 参数（观察口径：`mock-chat/mock-1`、`reasoningLevel:"disabled"`、
/// `thoughtLevel:"disabled"`、`titleGenerationEnabled:false`）。
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

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("期望对象，实际：{value}"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

async fn create_session(fixture: &Fixture, conn: &mut TestConn) -> String {
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let created = conn.result(id).await;
    created["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string()
}

async fn run_prompt(conn: &mut TestConn, session_id: &str, prompt: &str) {
    let id = conn
        .send(
            "session/send",
            json!({ "sessionId": session_id, "content": prompt }),
        )
        .await;
    let receipt = conn.result(id).await;
    assert_eq!(receipt["accepted"], json!(true));
    conn.drain_until_terminal().await;
}

async fn read_snapshot(conn: &mut TestConn, session_id: &str) -> Value {
    let id = conn
        .send("session/read", json!({ "sessionId": session_id }))
        .await;
    conn.result(id).await
}

// ---------------------------------------------------------------------------
// 1. create 快照：9 顶层键 / runtime 5 键 / settings / slashCommands（契约 §4.1、§4.4）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_snapshot_matches_the_recorded_read_face() {
    let fixture = fixture("create-face", vec![ScriptStep::Text("unused".to_string())]);
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, true))
        .await;
    let snapshot = conn.result(id).await;

    // 顶层 9 键（夹具 `createSnapshot.resultKeys`）。
    assert_eq!(
        sorted_keys(&snapshot),
        vec![
            "messages",
            "projection",
            "protocol",
            "runtime",
            "session",
            "settings",
            "slashCommands",
            "todoGroups",
            "todos",
        ],
        "create 快照顶层键必须精确等于观察的 9 键"
    );
    // 严格协议形状。
    serde_json::from_value::<SessionStateSnapshot>(snapshot.clone()).expect("快照是严格协议形状");

    // runtime 5 键（夹具 `createSnapshot.runtimeKeys`）：无 contextUsage、无 deliveryKind。
    let runtime = &snapshot["runtime"];
    assert_eq!(
        sorted_keys(runtime),
        vec![
            "eventSeq",
            "goalVerificationTimeline",
            "goalVerifications",
            "pendingRequestIds",
            "stateRevision",
        ]
    );
    assert_eq!(runtime["eventSeq"], json!(0));
    // 登记差异（契约 §4.5 #1）：oracle 的 create stateRevision 是 2；Rust 的 revision
    // 语义是「每次接受/终态 +1」，create 没有接受动作，保持 0 不硬凑。
    assert_eq!(runtime["stateRevision"], json!(0));
    assert_eq!(runtime["goalVerifications"], json!([]));
    assert_eq!(runtime["goalVerificationTimeline"], json!([]));
    assert_eq!(runtime["pendingRequestIds"], json!([]));

    // settings 4 键（契约 §4.4）。
    let settings = &snapshot["settings"];
    assert_eq!(
        sorted_keys(settings),
        vec!["mode", "model", "permission", "thoughtLevel"]
    );
    assert_eq!(settings["mode"], json!({ "current": "build" }));
    assert_eq!(settings["permission"], json!({ "mode": "build" }));

    // settings.model 3 键 + available 目录投影（夹具 `settingsModelKeys` 与
    // `model.available[0]`；provider/model 换成测试配置的 mock-chat/Mock Chat）。
    let model = &settings["model"];
    assert_eq!(sorted_keys(model), vec!["available", "current", "lastUsed"]);
    let available = model["available"].as_array().expect("available 数组");
    let model_ids: Vec<&str> = available
        .iter()
        .map(|entry| entry["ref"]["modelId"].as_str().expect("modelId"))
        .collect();
    assert_eq!(model_ids, vec!["mock-1", "mock-2"], "目录按 resolver 保序");
    let entry = &available[0];
    assert_eq!(
        sorted_keys(entry),
        vec![
            "contextWindow",
            "label",
            "maxOutputTokens",
            "properties",
            "providerLabel",
            "reasoning",
            "ref"
        ],
        "目录条目键面与观察一致（无 description/disabledReason 等未观察键）"
    );
    assert_eq!(
        entry["ref"],
        json!({ "providerId": "mock-chat", "modelId": "mock-1" }),
        "ref 不带 options"
    );
    assert_eq!(
        entry["label"],
        json!("mock-1"),
        "label = modelId（目录无独立 label 事实）"
    );
    assert_eq!(entry["providerLabel"], json!("Mock Chat"));
    assert_eq!(entry["contextWindow"], json!(200000));
    assert_eq!(entry["maxOutputTokens"], json!(8192));
    assert_eq!(
        entry["reasoning"],
        json!({
            "levels": [
                { "value": "disabled", "label": "disabled" },
                { "value": "enabled", "label": "enabled" }
            ],
            "defaultLevel": "enabled"
        }),
        "levels 取 optionSpecs；defaultLevel 取最后一个档位（观察 [disabled,enabled]→enabled，登记 §4.5）"
    );
    assert_eq!(
        entry["properties"],
        json!({
            "inputFormat": {
                "supportsText": true,
                "supportsImage": false,
                "supportsVideo": false,
                "supportsAudio": false,
                "supportsPdf": false
            },
            "outputFormat": { "supportsText": true }
        }),
        "协议必填形状；未配置的 supports* 按中性 false 落"
    );

    // current / lastUsed（夹具 `model.current` / `model.lastUsed`）。
    assert_eq!(
        model["current"],
        json!({
            "providerId": "mock-chat",
            "modelId": "mock-1",
            "options": { "reasoningLevel": "disabled" }
        })
    );
    assert_eq!(
        model["lastUsed"],
        json!({ "providerId": "mock-chat", "modelId": "mock-1" }),
        "lastUsed 只有 providerId/modelId 两个键"
    );

    // thoughtLevel（夹具 `settings.thoughtLevel`）。
    assert_eq!(
        settings["thoughtLevel"],
        json!({
            "available": [
                { "label": "disabled", "value": "disabled" },
                { "label": "enabled", "value": "enabled" }
            ],
            "current": "disabled",
            "enabled": true
        }),
        "enabled = 模型有档位；available 与 reasoning.levels 同源同序"
    );

    // slashCommands：4 条 builtin，compact 逐字（契约 §4.4 数据契约）。
    let commands = snapshot["slashCommands"].as_array().expect("slashCommands");
    let names: Vec<&str> = commands
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["goal", "compact", "init", "plan"]);
    let compact = commands
        .iter()
        .find(|command| command["name"] == "compact")
        .expect("compact 条目");
    assert_eq!(
        compact,
        &json!({
            "name": "compact",
            "description": "Compact the current conversation with optional instructions.",
            "inputHint": "/compact [instructions]",
            "source": "builtin"
        })
    );
    for command in commands {
        assert_eq!(command["source"], json!("builtin"), "全部是 builtin 来源");
    }

    // projection：create 时计数 0、无 currentTurnId、contextWindow 来自能力。
    let projection = &snapshot["projection"];
    assert_eq!(projection["contextUsed"], json!(0));
    assert_eq!(projection["totalTokenCount"], json!(0));
    assert_eq!(projection["turnCount"], json!(0));
    assert_eq!(projection["contextWindow"], json!(200000));
    assert!(
        projection.get("currentTurnId").is_none(),
        "create 无 currentTurnId"
    );
    assert_eq!(
        projection["target"],
        Value::Null,
        "target 键恒在，无 goal 事实为 null"
    );
    // session.target 同面（C1 契约 §2.1 观察键）。
    assert_eq!(snapshot["session"]["target"], Value::Null);

    // messages 空、todos/todoGroups 空数组。
    assert_eq!(snapshot["messages"], json!([]));
    assert_eq!(snapshot["todos"], json!([]));
    assert_eq!(snapshot["todoGroups"], json!([]));
}

/// 无模型 create：settings.model 退最小面（不造空 provider——控制器裁决 7）。
#[tokio::test]
async fn create_without_model_keeps_the_settings_face_minimal() {
    let fixture = fixture(
        "create-no-model",
        vec![ScriptStep::Text("unused".to_string())],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let id = conn
        .send("session/create", create_params(&fixture.ws, false))
        .await;
    let snapshot = conn.result(id).await;

    let model = &snapshot["settings"]["model"];
    assert_eq!(
        sorted_keys(model),
        vec!["available"],
        "无模型时无 current/lastUsed"
    );
    assert_eq!(model["available"], json!([]), "无模型 → 无目录投影");
    let thought = &snapshot["settings"]["thoughtLevel"];
    assert_eq!(thought["enabled"], json!(false));
    assert_eq!(thought["available"], json!([]));
    // thoughtLevel 事实独立于模型：create 参数带 thoughtLevel 时 current 照常投影。
    assert_eq!(thought["current"], json!("disabled"));
    assert_eq!(
        snapshot["settings"]["permission"],
        json!({ "mode": "build" })
    );
    assert_eq!(
        sorted_keys(&snapshot["runtime"]),
        vec![
            "eventSeq",
            "goalVerificationTimeline",
            "goalVerifications",
            "pendingRequestIds",
            "stateRevision",
        ],
        "无模型 create 同样没有 contextUsage"
    );
}

// ---------------------------------------------------------------------------
// 2. turn 后 read：contextUsage（T1 口径）+ projection 双计数（契约 §4.2）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_after_turns_follows_last_request_and_projection_facts() {
    let fixture = fixture(
        "read-face",
        vec![
            ScriptStep::Text("first answer".to_string()),
            ScriptStep::Text("second answer".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&fixture, &mut conn).await;
    conn.subscribe_default(&session_id).await;

    run_prompt(&mut conn, &session_id, "first prompt").await;
    let snapshot = read_snapshot(&mut conn, &session_id).await;

    // 顶层仍是 9 键；runtime 增加 contextUsage 与（oracle 的）deliveryKind 差异登记。
    assert_eq!(sorted_keys(&snapshot).len(), 9);
    let runtime = &snapshot["runtime"];
    // 登记差异（契约 §4.5 #3）：oracle 的 hot read 带 deliveryKind:"desktop-continuous"，
    // Rust 不产出（subscribe 快照的链路回显除外）。
    assert!(runtime.get("deliveryKind").is_none());
    let usage = &runtime["contextUsage"];
    assert_eq!(
        sorted_keys(usage),
        vec!["cache", "cost", "size", "used"],
        "T0 观察键固定 cache,cost,size,used（无 percent/breakdown——breakdown 是登记缺省）"
    );
    assert_eq!(
        usage["used"],
        json!(22),
        "used = 最后一次请求 total（10+12）"
    );
    assert_eq!(usage["size"], json!(200000));
    assert!(usage["cost"].is_null());
    let cache = &usage["cache"];
    assert_eq!(cache["inputTokens"], json!(10));
    assert_eq!(cache["hitRateRequestCount"], json!(1));
    assert_eq!(cache["totalInputTokens"], json!(10));
    assert_eq!(cache["totalCacheReadTokens"], json!(0));
    assert_eq!(cache["totalCacheWriteTokens"], json!(0));
    assert_eq!(cache["latestHitRate"], json!(0));
    assert_eq!(cache["hitRate"], json!(0));

    // projection 双计数（契约 §4.2）：contextUsed=22（最近一次请求），totalTokenCount
    // 才是累计；currentTurnId 在 turn 后出现并保留到终态后。
    let projection = &snapshot["projection"];
    assert_eq!(projection["contextUsed"], json!(22));
    assert_eq!(projection["totalTokenCount"], json!(22));
    assert_eq!(projection["turnCount"], json!(1));
    let turn_id = projection["currentTurnId"]
        .as_str()
        .expect("turn 后 currentTurnId 存在")
        .to_string();
    assert!(!turn_id.is_empty());
    assert_eq!(projection["status"], json!("idle"));

    // 第二轮：used 仍是 22（不是 44/66），cache 计数累计；projection 双计数分离。
    run_prompt(&mut conn, &session_id, "second prompt").await;
    let snapshot = read_snapshot(&mut conn, &session_id).await;
    let usage = &snapshot["runtime"]["contextUsage"];
    assert_eq!(usage["used"], json!(22), "used 跟随最后一次请求，不累计");
    assert_eq!(usage["cache"]["hitRateRequestCount"], json!(2));
    assert_eq!(usage["cache"]["totalInputTokens"], json!(20));
    let projection = &snapshot["projection"];
    assert_eq!(
        projection["contextUsed"],
        json!(22),
        "contextUsed 也跟随最近一次请求"
    );
    assert_eq!(
        projection["totalTokenCount"],
        json!(44),
        "totalTokenCount 是累计口径"
    );
    assert_eq!(projection["turnCount"], json!(2));
    assert_ne!(
        projection["currentTurnId"].as_str().expect("currentTurnId"),
        turn_id,
        "currentTurnId 跟随最近一次已开始的 turn"
    );

    // settings 与 create 同面（观察里三轮 settings 不变）。
    assert_eq!(
        snapshot["settings"]["model"]["available"]
            .as_array()
            .expect("available")
            .len(),
        2
    );
    assert_eq!(
        snapshot["settings"]["model"]["current"]["options"]["reasoningLevel"],
        json!("disabled")
    );
    assert_eq!(
        snapshot["settings"]["thoughtLevel"]["current"],
        json!("disabled")
    );
}

// ---------------------------------------------------------------------------
// 3. close → resume → read：冷形状（契约 §4.3）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_read_matches_the_recorded_cold_shape() {
    let fixture = fixture(
        "resume-face",
        vec![
            ScriptStep::Text("first answer".to_string()),
            ScriptStep::Text("second answer".to_string()),
            ScriptStep::Text("third answer".to_string()),
        ],
    );
    let mut conn = connect(Arc::clone(&fixture.server)).await;
    let session_id = create_session(&fixture, &mut conn).await;
    conn.subscribe_default(&session_id).await;
    for prompt in ["first prompt", "second prompt", "third prompt"] {
        run_prompt(&mut conn, &session_id, prompt).await;
    }

    // close（先验证冷读面，再 resume——互不影响）。
    let id = conn
        .send("session/close", json!({ "sessionId": session_id }))
        .await;
    assert_eq!(conn.result(id).await, json!({ "closed": true }));

    // 冷读（无 runtime）：9 顶层键、slashCommands 常驻、计数 0、无 contextUsage。
    let cold = read_snapshot(&mut conn, &session_id).await;
    assert_eq!(sorted_keys(&cold).len(), 9, "冷读顶层与 create 同面");
    let names: Vec<&str> = cold["slashCommands"]
        .as_array()
        .expect("slashCommands")
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["goal", "compact", "init", "plan"]);
    assert_eq!(cold["projection"]["contextUsed"], json!(0));
    assert_eq!(cold["projection"]["totalTokenCount"], json!(0));
    assert_eq!(cold["projection"]["turnCount"], json!(0));
    assert!(
        cold["runtime"].get("contextUsage").is_none(),
        "冷读无 contextUsage 事实"
    );
    // 冷读不重算 settings.model（store 无模型目录面；契约 §4.5 #5）。
    assert_eq!(cold["settings"]["model"]["available"], json!([]));
    assert!(cold["settings"]["model"].get("current").is_none());

    // resume（drain 窗口内可能显式 -32603，轮询直到可用）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let resumed = loop {
        let id = conn
            .send("session/resume", json!({ "sessionId": session_id }))
            .await;
        match tokio::time::timeout(Duration::from_secs(5), conn.error_or_result(id)).await {
            Ok(Ok(value)) => break value,
            Ok(Err((_, message))) if message.contains("正在关闭") => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "drain 窗口未在期限内关闭"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(Err((code, message))) => panic!("resume 失败 ({code}): {message}"),
            Err(_) => panic!("resume 轮询超时"),
        }
    };
    serde_json::from_value::<SessionStateSnapshot>(resumed.clone()).expect("resume 快照可解析");

    // read（热）：contextUsage 保留但无 breakdown；projection 计数归 0。
    let snapshot = read_snapshot(&mut conn, &session_id).await;
    let runtime = &snapshot["runtime"];
    assert_eq!(runtime["stateRevision"], json!(0));
    // 登记差异（契约 §4.5 #2）：oracle 的 resume 后 eventSeq 是 2（物化产生事件）；
    // Rust 物化不发事件，保持 0 不硬凑。
    assert_eq!(runtime["eventSeq"], json!(0));
    assert!(runtime.get("deliveryKind").is_none());
    let usage = &runtime["contextUsage"];
    assert_eq!(
        sorted_keys(usage),
        vec!["cache", "cost", "size", "used"],
        "resume 后 contextUsage 无 breakdown（契约 §4.3）"
    );
    assert_eq!(
        usage["used"],
        json!(22),
        "used 从持久化 tokens 恢复（3 轮后仍是最后一次的 22）"
    );
    assert_eq!(usage["size"], json!(200000));
    assert_eq!(usage["cache"]["inputTokens"], json!(10));
    assert_eq!(usage["cache"]["hitRateRequestCount"], json!(3));
    assert_eq!(usage["cache"]["totalInputTokens"], json!(30));
    assert_eq!(usage["cache"]["totalCacheReadTokens"], json!(0));
    assert_eq!(usage["cache"]["totalCacheWriteTokens"], json!(0));

    let projection = &snapshot["projection"];
    assert_eq!(projection["contextUsed"], json!(0), "resume 后投影计数归 0");
    assert_eq!(projection["totalTokenCount"], json!(0));
    assert_eq!(projection["turnCount"], json!(0));
    assert!(
        projection.get("currentTurnId").is_none(),
        "resume 后无 currentTurnId（本 runtime 生命周期内还没有 turn）"
    );

    // settings 面在 resume 后保持：目录、current、thoughtLevel（恢复自 entry 的
    // reasoningLevel——契约 §4.5 登记的恢复事实源）。
    assert_eq!(
        snapshot["settings"]["model"]["available"]
            .as_array()
            .expect("available")
            .len(),
        2
    );
    assert_eq!(
        snapshot["settings"]["model"]["current"],
        json!({
            "providerId": "mock-chat",
            "modelId": "mock-1",
            "options": { "reasoningLevel": "disabled" }
        })
    );
    assert_eq!(
        snapshot["settings"]["model"]["lastUsed"],
        json!({ "providerId": "mock-chat", "modelId": "mock-1" })
    );
    assert_eq!(
        snapshot["settings"]["thoughtLevel"]["current"],
        json!("disabled")
    );
    assert_eq!(snapshot["settings"]["thoughtLevel"]["enabled"], json!(true));
    // 顶层 9 键保持。
    assert_eq!(sorted_keys(&snapshot).len(), 9);
}
