//! 镜像模型、有界化与两个发布入口的 origin 校验。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use conversation_mirror::{
    bound_record, bound_snapshot, dedupe_records, validate_snapshot,
    ConversationMirrorService, Message, Record, Snapshot,
};

fn unique_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "mode-mirror-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建临时根");
    root
}

fn record(session_id: &str, updated_at: u64, messages: Vec<Message>) -> Record {
    Record {
        session_id: session_id.into(),
        title: format!("t-{session_id}"),
        created_at: 1,
        updated_at,
        message_count: messages.len() as u64,
        truncated: false,
        messages,
    }
}

fn text_message(text: &str) -> Message {
    Message {
        role: "user".into(),
        text: text.into(),
        timestamp: Some(1),
    }
}

fn snapshot(origin: &str, records: Vec<Record>) -> Snapshot {
    Snapshot {
        version: 1,
        origin_device_id: origin.into(),
        origin_label: "host".into(),
        generated_at: 10,
        workspace_path: "D:/w".into(),
        records,
    }
}

fn as_value(snapshot: &Snapshot) -> serde_json::Value {
    serde_json::to_value(snapshot).expect("序列化")
}

#[test]
fn validation_trims_identity_fields_and_rejects_bad_shapes() {
    let mut value = as_value(&snapshot(" dev-1 ", vec![]));
    value["originLabel"] = serde_json::json!("  host  ");
    let parsed = validate_snapshot(&value).expect("合法快照");
    assert_eq!(parsed.origin_device_id, "dev-1");
    assert_eq!(parsed.origin_label, "host");

    let mut unknown = as_value(&snapshot("dev-1", vec![]));
    unknown["extra"] = serde_json::json!(1);
    assert!(validate_snapshot(&unknown).is_err(), "strict：未知字段必须拒绝");

    let mut bad_role = as_value(&snapshot(
        "dev-1",
        vec![record(
            "s1",
            1,
            vec![Message { role: "system".into(), text: "x".into(), timestamp: None }],
        )],
    ));
    bad_role["version"] = serde_json::json!(1);
    assert!(validate_snapshot(&bad_role).is_err());

    let mut bad_version = as_value(&snapshot("dev-1", vec![]));
    bad_version["version"] = serde_json::json!(2);
    assert!(validate_snapshot(&bad_version).is_err());
}

#[test]
fn dedupe_keeps_the_newer_record_for_each_session() {
    let records = vec![
        record("s1", 10, vec![text_message("old")]),
        record("s2", 5, vec![text_message("other")]),
        record("s1", 20, vec![text_message("new")]),
    ];
    let deduped = dedupe_records(&records);
    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].updated_at, 20);
    assert_eq!(deduped[1].session_id, "s2");
}

#[test]
fn record_bounding_truncates_text_with_ellipsis() {
    let long_text = "x".repeat(conversation_mirror::model::MAX_TEXT_CHARS_PER_MESSAGE + 10);
    let bounded = bound_record(&record("s1", 1, vec![text_message(&long_text)]));
    assert!(bounded.truncated);
    let text = &bounded.messages[0].text;
    assert!(text.ends_with('…'), "超长文本要补省略号");
    let chars: Vec<char> = text.chars().collect();
    assert_eq!(chars.len(), conversation_mirror::model::MAX_TEXT_CHARS_PER_MESSAGE + 1);
}

#[test]
fn record_bounding_keeps_only_the_tail_messages() {
    let messages: Vec<Message> = (0..(conversation_mirror::model::MAX_MESSAGES_PER_SESSION + 5))
        .map(|index| text_message(&format!("m{index}")))
        .collect();
    let bounded = bound_record(&record("s1", 1, messages));
    assert_eq!(
        bounded.messages.len(),
        conversation_mirror::model::MAX_MESSAGES_PER_SESSION
    );
    assert!(bounded.truncated);
    assert_eq!(bounded.messages[0].text, "m5");
}

#[test]
fn snapshot_bounding_sorts_caps_and_drops_oldest_under_budget() {
    let mut records = Vec::new();
    for index in 0..25u64 {
        records.push(record(&format!("s{index}"), index, vec![text_message("x")]));
    }
    let bounded = bound_snapshot(&snapshot("dev-1", records));
    assert_eq!(bounded.records.len(), conversation_mirror::model::MAX_RECORDS);
    assert_eq!(bounded.records[0].updated_at, 24, "按 updatedAt 降序");
    assert_eq!(bounded.records[19].updated_at, 5);
}

#[test]
fn service_enforces_origin_rules_and_round_trips_snapshots() {
    let root = unique_root("service");
    let service = ConversationMirrorService::new(&root);
    let identity = service.get_mirror_identity().expect("身份可用");

    let own = as_value(&snapshot(&identity.device_id, vec![]));
    assert!(
        service.write_local_snapshot(&own).stored,
        "本机自快照通道接受自己的快照"
    );
    assert_eq!(service.publish_snapshot(&own).reason, Some("self-origin"));

    let foreign = as_value(&snapshot("other-device", vec![record("s1", 1, vec![text_message("hi")])]));
    assert_eq!(service.publish_snapshot(&foreign).reason, Some("stored"));
    assert_eq!(
        service.write_local_snapshot(&foreign).reason,
        Some("foreign-origin")
    );

    let snapshots = service.read_workspace_snapshots("D:/w");
    assert_eq!(snapshots.len(), 2, "自己的和外来的各一份");
    let foreign_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.origin_device_id == "other-device")
        .expect("对端快照");
    assert_eq!(foreign_snapshot.records.len(), 1);

    let invalid = serde_json::json!({ "version": 1 });
    assert_eq!(
        service.publish_snapshot(&invalid).reason,
        Some("invalid-record")
    );
}

#[test]
fn store_skips_invalid_files() {
    let root = unique_root("skip");
    let service = ConversationMirrorService::new(&root);
    let identity = service.get_mirror_identity().expect("身份可用");
    service.write_local_snapshot(&as_value(&snapshot(&identity.device_id, vec![])));

    let dir = root.join("v2/conversation-mirror");
    let workspace_dir = fs::read_dir(&dir)
        .expect("镜像目录")
        .flatten()
        .find(|entry| entry.path().is_dir())
        .expect("工作区目录")
        .path();
    fs::write(workspace_dir.join("broken.json"), ":wq").expect("写坏文件");

    let snapshots = service.read_workspace_snapshots("D:/w");
    assert_eq!(snapshots.len(), 1, "坏文件被跳过，不影响其它快照");
}
