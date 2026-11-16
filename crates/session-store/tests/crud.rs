//! 核心 CRUD：session create/get/list、message/part 顺序与 limit、entry 与 usage 写入。

mod common;

use common::{
    count_rows, create_baseline_database, raw_connection, read_raw_text, sample_entry,
    sample_message, sample_part, sample_session, FixtureOptions, TempRoot,
};
use serde_json::{json, Value};
use session_store::{
    MessageWithParts, ModelUsageRecord, SessionRecord, SessionStore, StoreError, ToolUsageRecord,
    TurnUsageRecord,
};

fn ids(sessions: Vec<SessionRecord>) -> Vec<String> {
    sessions.into_iter().map(|session| session.id).collect()
}

fn part_ids(messages: &[MessageWithParts]) -> Vec<Vec<String>> {
    messages
        .iter()
        .map(|message| message.parts.iter().map(|part| part.id.clone()).collect())
        .collect()
}

fn open_fixture(label: &str) -> (TempRoot, SessionStore) {
    let root = TempRoot::new(label);
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());
    let store = SessionStore::open(&db_path).expect("打开测试库");
    (root, store)
}

#[test]
fn reads_session_rows_written_with_node_shapes() {
    let root = TempRoot::new("crud-node-rows");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());

    // 行形状取自 `docs/specs/agent-server-c1-contract.md` §3（oracle 观察值）。
    let permission = r#"{"mode":"build"}"#;
    let message_data = r#"{"role":"user","time":{"created":1000},"modelSelection":{"providerId":"oracle-mock","modelId":"mock-1","options":{"reasoningLevel":"disabled"}},"semantics":{"origin":"real_user","kind":"user_prompt","uiVisibility":"visible","providerVisibility":"visible","transcriptVisibility":"visible"},"anchor":{"turnId":"turn-1","origin":"realUser"},"tools":{"Read":true},"model":{"providerID":"oracle-mock","modelID":"mock-1","variant":"default"}}"#;
    let part_data = r#"{"type":"text","text":"first prompt","time":{"start":1000,"end":1001}}"#;
    {
        let connection = rusqlite::Connection::open(&db_path).expect("直接写 Node 形状行");
        connection
            .execute(
                "INSERT INTO session (\
                 id, project_id, workspace_id, parent_id, slug, directory, path, title, version, \
                 share_url, summary_additions, summary_deletions, summary_files, summary_diffs, \
                 revert, permission, time_created, time_updated, time_compacting, time_archived, \
                 task_type, title_source, title_message_id, time_title_updated, trace_id) \
                 VALUES (?1, ?2, NULL, NULL, ?1, ?3, ?3, ?4, '0.0.0', NULL, NULL, NULL, NULL, \
                 NULL, NULL, ?5, 1000, 1000, NULL, NULL, 'interactive', 'first_input', NULL, \
                 NULL, ?6)",
                rusqlite::params![
                    "sess_node",
                    "proj_c-users-ws",
                    r"C:\ws\node",
                    "first prompt",
                    permission,
                    "trace-node",
                ],
            )
            .expect("写入 session");
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence) \
                 VALUES (?1, 'sess_node', 1000, 1001, ?2, 0)",
                rusqlite::params!["msg_node", message_data],
            )
            .expect("写入 message");
        connection
            .execute(
                "INSERT INTO part (\
                 id, message_id, session_id, time_created, time_updated, data, sequence) \
                 VALUES (?1, 'msg_node', 'sess_node', 1000, 1001, ?2, 0)",
                rusqlite::params!["part_node", part_data],
            )
            .expect("写入 part");
    }

    let store = SessionStore::open(&db_path).expect("读取 Node 形状库");
    let session = store
        .get_session("sess_node")
        .expect("读取 session")
        .expect("session 必须存在");
    assert_eq!(session.slug, "sess_node");
    assert_eq!(session.title, "first prompt");
    assert_eq!(session.permission.as_deref(), Some(permission));
    assert_eq!(session.directory, r"C:\ws\node");
    assert_eq!(session.task_type, "interactive");
    assert_eq!(session.title_source, "first_input");
    assert_eq!(session.trace_id.as_deref(), Some("trace-node"));

    let messages = store
        .messages("sess_node", None, None)
        .expect("读取 messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message.id, "msg_node");
    assert_eq!(messages[0].message.sequence, Some(0));
    assert_eq!(
        messages[0].message.data,
        serde_json::from_str::<Value>(message_data).expect("解析 message data")
    );
    assert_eq!(messages[0].parts.len(), 1);
    assert_eq!(
        messages[0].parts[0].data,
        serde_json::from_str::<Value>(part_data).expect("解析 part data")
    );

    assert_eq!(
        ids(store
            .list_sessions(r"C:\ws\node", false, None)
            .expect("按 directory 列出 Node 行")),
        vec!["sess_node".to_owned()]
    );
}

#[test]
fn create_get_and_list_sessions() {
    let (_root, store) = open_fixture("crud-sessions");
    let workspace = r"C:\ws\a";

    let s1 = sample_session("s1", workspace, 100, false);
    let s2 = sample_session("s2", workspace, 300, true);
    let s3 = sample_session("s3", workspace, 200, false);
    let s4 = sample_session("s4", r"C:\ws\b", 400, false);
    for record in [s1.clone(), s2.clone(), s3.clone(), s4.clone()] {
        store.create_session(record).expect("写入 session");
    }

    assert_eq!(
        store.get_session("s1").expect("读取 session"),
        Some(s1.clone())
    );
    assert_eq!(
        store.get_session("absent").expect("读取不存在的 session"),
        None
    );

    assert_eq!(
        ids(store.list_sessions(workspace, false, None).expect("list")),
        vec!["s3".to_owned(), "s1".to_owned()],
        "默认排除归档，按 time_updated DESC 排序"
    );
    assert_eq!(
        ids(store.list_sessions(workspace, true, None).expect("list")),
        vec!["s2".to_owned(), "s3".to_owned(), "s1".to_owned()]
    );
    assert_eq!(
        ids(store
            .list_sessions(workspace, true, Some(2))
            .expect("list limit")),
        vec!["s2".to_owned(), "s3".to_owned()]
    );
    assert_eq!(
        ids(store
            .list_sessions(r"C:\ws\b", false, None)
            .expect("list other workspace")),
        vec!["s4".to_owned()]
    );
}

#[test]
fn list_sessions_accepts_workspace_path_or_project_id() {
    let (_root, store) = open_fixture("crud-list-project-id");

    let by_directory = sample_session("by-directory", r"C:\ws\a", 100, false);
    let mut by_project = sample_session("by-project", r"C:\ws\other", 200, false);
    by_project.project_id = "proj_c-ws-a".to_owned();
    store
        .create_session(by_directory)
        .expect("写入 directory 会话");
    store
        .create_session(by_project)
        .expect("写入 project_id 会话");

    assert_eq!(
        ids(store
            .list_sessions(r"C:\ws\a", false, None)
            .expect("按 directory 查询")),
        vec!["by-directory".to_owned()]
    );
    assert_eq!(
        ids(store
            .list_sessions("proj_c-ws-a", false, None)
            .expect("按 project_id 查询")),
        vec!["by-project".to_owned()]
    );
}

#[test]
fn append_messages_and_parts_preserve_order_and_limit() {
    let (_root, store) = open_fixture("crud-messages");
    store
        .create_session(sample_session("s1", r"C:\ws\a", 100, false))
        .expect("写入 session");

    let m0 = sample_message("m0", "s1", 0);
    let m1 = sample_message("m1", "s1", 1);
    let m2 = sample_message("m2", "s1", 2);
    for message in [m0.clone(), m1.clone(), m2.clone()] {
        store.append_message(message).expect("追加 message");
    }

    // 故意先写 sequence=1 的 part，验证读取按 sequence 排序而不是写入顺序。
    let p0b = sample_part("p0b", "m0", "s1", 1);
    let p0a = sample_part("p0a", "m0", "s1", 0);
    let p1 = sample_part("p1", "m1", "s1", 0);
    let p2 = sample_part("p2", "m2", "s1", 0);
    for part in [p0b.clone(), p0a.clone(), p1.clone(), p2.clone()] {
        store.append_part(part).expect("追加 part");
    }

    let all = store.messages("s1", None, None).expect("读取 messages");
    assert_eq!(
        all.iter()
            .map(|message| message.message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["m0", "m1", "m2"]
    );
    assert_eq!(
        part_ids(&all),
        vec![
            vec!["p0a".to_owned(), "p0b".to_owned()],
            vec!["p1".to_owned()],
            vec!["p2".to_owned()],
        ]
    );
    assert_eq!(all[0].message.data, m0.data);
    assert_eq!(all[0].parts[0].data, p0a.data);

    let after_m0 = store
        .messages("s1", Some("m0"), Some(1))
        .expect("after + limit");
    assert_eq!(
        after_m0
            .iter()
            .map(|message| message.message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["m1"]
    );
    let after_m1 = store.messages("s1", Some("m1"), None).expect("after");
    assert_eq!(
        after_m1
            .iter()
            .map(|message| message.message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["m2"]
    );
    assert!(store
        .messages("s1", Some("m2"), None)
        .expect("after last")
        .is_empty());
    assert!(store
        .messages("s1", None, Some(0))
        .expect("limit 0")
        .is_empty());
    assert!(store
        .messages("absent", None, None)
        .expect("未知会话")
        .is_empty());

    let error = store
        .messages("s1", Some("absent"), None)
        .expect_err("未知游标必须报错，不能静默从头返回");
    assert!(matches!(error, StoreError::InvalidInput(_)));
    assert_eq!(error.code(), "invalid_input");
}

#[test]
fn message_and_part_data_round_trip_byte_stably() {
    let (root, store) = open_fixture("crud-byte-stable");
    let db_path = root.db_path();
    store
        .create_session(sample_session("s1", r"C:\ws\a", 100, false))
        .expect("写入 session");

    let message_data: Value = serde_json::from_str(
        r#"{"role":"assistant","text":"中文\nline","tokens":{"total":22,"input":10,"output":12},"cost":0}"#,
    )
    .expect("解析 message JSON");
    let mut m1 = sample_message("m1", "s1", 0);
    m1.data = message_data.clone();
    store.append_message(m1).expect("追加 message");

    let raw_message = read_raw_text(&db_path, "message", "data", "m1");
    assert_eq!(
        raw_message,
        serde_json::to_string(&message_data).expect("序列化"),
        "store 写入的 JSON 文本必须与 Value 的序列化逐字节一致"
    );

    // 从库里的原始文本重新解析再写一条：读-写-读必须逐字节稳定。
    let reparsed: Value = serde_json::from_str(&raw_message).expect("解析库内 JSON");
    let mut m2 = sample_message("m2", "s1", 1);
    m2.data = reparsed;
    store.append_message(m2).expect("追加第二条 message");
    assert_eq!(
        read_raw_text(&db_path, "message", "data", "m2"),
        raw_message,
        "message.data 往返必须逐字节稳定"
    );

    let part_data = json!({
        "type": "text",
        "text": "中文 part",
        "time": { "start": 1, "end": 2 },
    });
    let mut p1 = sample_part("p1", "m1", "s1", 0);
    p1.data = part_data.clone();
    store.append_part(p1).expect("追加 part");
    let raw_part = read_raw_text(&db_path, "part", "data", "p1");
    assert_eq!(
        raw_part,
        serde_json::to_string(&part_data).expect("序列化"),
        "part.data 必须保留原始 JSON 形状"
    );

    let messages = store.messages("s1", None, None).expect("读取 messages");
    assert_eq!(messages[0].message.data, message_data);
    assert_eq!(messages[0].parts[0].data, part_data);
    assert_eq!(
        read_raw_text(&db_path, "message", "data", "m1"),
        raw_message
    );
}

#[test]
fn append_entry_persists_type_and_json() {
    let (root, store) = open_fixture("crud-entry");
    let db_path = root.db_path();
    let entry = sample_entry("e1", "s1");
    store.append_entry(entry.clone()).expect("追加 entry");

    let connection = raw_connection(&db_path);
    let (entry_type, data): (String, String) = connection
        .query_row(
            "SELECT type, data FROM session_entry WHERE id = ?1",
            ["e1"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("读取 session_entry");
    assert_eq!(entry_type, entry.entry_type);
    assert_eq!(
        serde_json::from_str::<Value>(&data).expect("解析 entry data"),
        entry.data
    );
    assert_eq!(count_rows(&db_path, "session_entry"), 1);
}

#[test]
fn record_model_turn_and_tool_usage() {
    let (root, store) = open_fixture("crud-usage");
    let db_path = root.db_path();
    store
        .create_session(sample_session("s1", r"C:\ws\a", 100, false))
        .expect("写入 session");

    store
        .record_model_usage(ModelUsageRecord {
            id: "mu1".to_owned(),
            session_id: "s1".to_owned(),
            logical_request_id: "lr1".to_owned(),
            provider_id: "fixture-provider".to_owned(),
            model_id: "fixture-model".to_owned(),
            query_source: "chat".to_owned(),
            started_at: 10,
            status: "completed".to_owned(),
            input_tokens: 10,
            output_tokens: 12,
            computed_total_tokens: 22,
            ..ModelUsageRecord::default()
        })
        .expect("写入 model_usage");
    store
        .record_turn_usage(TurnUsageRecord {
            session_id: "s1".to_owned(),
            turn_id: "turn-1".to_owned(),
            started_at: 10,
            status: "completed".to_owned(),
            model_request_count: 2,
            tool_call_count: 1,
            tool_error_count: 0,
            ..TurnUsageRecord::default()
        })
        .expect("写入 turn_usage");
    store
        .record_tool_usage(ToolUsageRecord {
            id: "tu1".to_owned(),
            session_id: "s1".to_owned(),
            tool_call_id: "call-1".to_owned(),
            tool_name: "Read".to_owned(),
            started_at: 11,
            status: "completed".to_owned(),
            output_bytes: 10,
            truncated: true,
            ..ToolUsageRecord::default()
        })
        .expect("写入 tool_usage");

    assert_eq!(count_rows(&db_path, "model_usage"), 1);
    assert_eq!(count_rows(&db_path, "turn_usage"), 1);
    assert_eq!(count_rows(&db_path, "tool_usage"), 1);

    let connection = raw_connection(&db_path);
    let model: (String, i64, i64, i64) = connection
        .query_row(
            "SELECT status, input_tokens, output_tokens, cancelled_by_user \
             FROM model_usage WHERE id = 'mu1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("读取 model_usage");
    assert_eq!(model, ("completed".to_owned(), 10, 12, 0));

    let turn: (String, i64, i64) = connection
        .query_row(
            "SELECT status, model_request_count, tool_call_count \
             FROM turn_usage WHERE session_id = 's1' AND turn_id = 'turn-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("读取 turn_usage");
    assert_eq!(turn, ("completed".to_owned(), 2, 1));

    let tool: (String, i64, i64) = connection
        .query_row(
            "SELECT tool_name, output_bytes, truncated \
             FROM tool_usage WHERE session_id = 's1' AND tool_call_id = 'call-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("读取 tool_usage");
    assert_eq!(tool, ("Read".to_owned(), 10, 1));
}

#[test]
fn usage_records_keep_message_data_shape() {
    // 这条用例防止后续把 usage 的 JSON 列改成二次投影：raw_usage_json 必须原样保存。
    let (root, store) = open_fixture("crud-usage-json");
    let db_path = root.db_path();
    let raw_usage = r#"{"input_tokens":10,"output_tokens":12,"cache_read_input_tokens":0}"#;
    store
        .record_model_usage(ModelUsageRecord {
            id: "mu1".to_owned(),
            session_id: "s1".to_owned(),
            logical_request_id: "lr1".to_owned(),
            provider_id: "fixture-provider".to_owned(),
            model_id: "fixture-model".to_owned(),
            query_source: "chat".to_owned(),
            started_at: 10,
            status: "completed".to_owned(),
            raw_usage_json: Some(raw_usage.to_owned()),
            provider_metadata_json: Some("{\"requestId\":\"r1\"}".to_owned()),
            ..ModelUsageRecord::default()
        })
        .expect("写入 model_usage");
    let connection = raw_connection(&db_path);
    let (raw, metadata): (String, String) = connection
        .query_row(
            "SELECT raw_usage_json, provider_metadata_json FROM model_usage WHERE id = 'mu1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("读取 model_usage JSON 列");
    assert_eq!(raw, raw_usage);
    assert_eq!(metadata, "{\"requestId\":\"r1\"}");
}
