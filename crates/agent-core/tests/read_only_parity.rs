//! 只读切片的**三层对拍**（第一层：工具调用序列；第二层：文件快照）。
//!
//! 期望值来自行为夹具 `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json` 的 `expect`
//! ——由 oracle 录制，**不是手写的**。本文件按 Ruling 4 的做法：把 Rust 会话循环发出的事件
//! 投影成夹具里的同一形状（spec §2.1 的投影 + §3 的规范化），然后**精确相等**比较。
//! 投影助手在 `agent_tools::test_support`（两端共用一份，避免出现第二份 §3 实现）。
//!
//! **第三层（事件流）不在这里比**：Rust 侧目前只发工具面事件（见 `agent_core::session` 的
//! 模块注释），模型流 / 状态推送 / 终态事件要 Task 5 才落地。硬比第三层会让这个任务变成
//! 「把 Task 5 提前做一半」，而不是「把只读工具做对」。
//!
//! 单独跑：`node scripts/rust-cargo.ts test -p agent-core`

use std::fs;

use agent_core::session::{Session, ToolCallRequest};
use agent_core::{SessionEvent, ToolUpdatedPayload};
use agent_tools::test_support::{load_fixture, project, project_input, Fixture, Workspace};
use agent_tools::UNIMPLEMENTED_ERROR_TYPE;

/// 夹具里的 `${cwd}` 是占位符，录到的入参已是规范化形状——因此脚本入参要做同一份投影。
fn script_calls(fixture: &Fixture, workspace: &Workspace) -> Vec<ToolCallRequest> {
    fixture
        .calls
        .iter()
        .enumerate()
        .map(|(index, call)| ToolCallRequest {
            // 工具调用 id 在 §3 里归一成 `<id>`；用序号构造稳定的 id。
            tool_call_id: format!("call_{index}"),
            name: call.name.clone(),
            input: workspace.substitute(&call.input),
        })
        .collect()
}

/// 按 spec §2.1 的投影，从事件流里折出第一层形状。
///
/// | 来源 | 取什么 |
/// | --- | --- |
/// | `tool.updated` / `scheduled` | `name = toolName`（入参在调度事件里**故意缺失**，由调用方比） |
/// | `tool.updated` / `result` | `ok = result.success === true`，`output = result.content` |
/// | `tool.updated` / `error` | `ok = false`，`output = error.message` |
fn project_tool_calls(events: &[SessionEvent], workspace: &str) -> Vec<(String, bool, String)> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::BTreeMap<String, (String, bool, String)> =
        std::collections::BTreeMap::new();
    for event in events {
        if event.event_type != agent_core::SessionEventType::ToolUpdated {
            continue;
        }
        let Some(payload) = event
            .payload_as::<ToolUpdatedPayload>()
            .expect("载荷能解码")
        else {
            continue;
        };
        match payload {
            ToolUpdatedPayload::Scheduled(scheduled) => {
                let id = scheduled.call.tool_call_id.clone();
                order.push(id.clone());
                by_id.insert(id, (scheduled.tool_name, false, String::new()));
            }
            ToolUpdatedPayload::Started(_) | ToolUpdatedPayload::Batch(_) => {}
            ToolUpdatedPayload::Result(result) => {
                let Some(slot) = by_id.get_mut(&result.call.tool_call_id) else {
                    continue;
                };
                slot.1 = result.result.success == Some(true);
                slot.2 = project(
                    result
                        .result
                        .content
                        .as_ref()
                        .and_then(|value| value.as_str())
                        .unwrap_or(""),
                    workspace,
                );
            }
            ToolUpdatedPayload::Error(error) => {
                let Some(slot) = by_id.get_mut(&error.call.tool_call_id) else {
                    continue;
                };
                slot.1 = false;
                slot.2 = project(&error.error.message, workspace);
            }
        }
    }
    order
        .into_iter()
        .map(|id| by_id.remove(&id).expect("scheduled 先出现"))
        .collect()
}

/// 第一层 + 第二层，逐条夹具。
fn assert_parity(case: &str) {
    let fixture = load_fixture(case);
    let workspace = Workspace::new(case);
    workspace.seed(&fixture.seed);
    let calls = script_calls(&fixture, &workspace);

    let mut session = Session::new("sess_parity", workspace.cwd());
    let _turn = session.begin_turn("对拍：只读工具");
    let events = session.run_tool_batch(&calls);

    // 第一层：工具调用序列逐项按序比较（名字 / 成败 / 输出；入参见下）。
    let actual = project_tool_calls(&events, &workspace.cwd());
    assert_eq!(
        actual.len(),
        fixture.expect_calls.len(),
        "{case}：工具调用条数不一致（实际 {}，期望 {}）",
        actual.len(),
        fixture.expect_calls.len()
    );
    for (index, expected) in fixture.expect_calls.iter().enumerate() {
        let got = &actual[index];
        assert_eq!(got.0, expected.name, "{case}：第 {index} 个调用的工具名");
        assert_eq!(got.1, expected.ok, "{case}：第 {index} 个调用的成败");
        assert_eq!(
            got.2, expected.output,
            "{case}：第 {index} 个调用的输出与录制不一致"
        );
    }

    // 入参层：脚本入参投影后必须逐字等于录制的 `input`。
    for (index, call) in calls.iter().enumerate() {
        assert_eq!(
            project_input(&call.input, &workspace.cwd()),
            fixture.expect_inputs[index],
            "{case}：第 {index} 个调用的入参投影与录制不一致"
        );
    }

    // 「M2 未实现的面」绝不能出现在对拍里（fail closed，见 agent_tools::UNIMPLEMENTED_ERROR_TYPE）。
    for event in &events {
        if let Some(ToolUpdatedPayload::Error(error)) = event
            .payload_as::<ToolUpdatedPayload>()
            .expect("载荷能解码")
        {
            assert_ne!(
                error.error.error_type, UNIMPLEMENTED_ERROR_TYPE,
                "{case}：会话循环落到了 M2 未实现的面：{}",
                error.error.message
            );
        }
    }

    // 第二层：只读工具不改工作目录——键集与录制一致，内容逐字未变。
    let mut keys: Vec<String> = fixture
        .seed
        .iter()
        .map(|(relative, _)| relative.replace('\\', "/"))
        .collect();
    keys.sort();
    let mut expected_keys = fixture.expect_file_keys.clone();
    expected_keys.sort();
    assert_eq!(keys, expected_keys, "{case}：文件快照键集不一致");
    for (relative, bytes) in &fixture.seed {
        let after = fs::read(workspace.path().join(relative)).expect("种子文件仍在");
        assert_eq!(&after, bytes, "{case}：只读工具改了 {relative} 的内容");
    }
}

#[test]
fn read_single_parity() {
    assert_parity("read-single");
}

#[test]
fn read_missing_parity() {
    assert_parity("read-missing");
}

#[test]
fn read_twice_one_turn_parity() {
    assert_parity("read-twice-one-turn");
}

#[test]
fn read_offset_edges_parity() {
    assert_parity("read-offset-edges");
}

#[test]
fn read_error_paths_parity() {
    assert_parity("read-error-paths");
}

#[test]
fn read_absent_search_tools_parity() {
    assert_parity("read-absent-search-tools");
}

/// `scheduled` 事件**不带入参**（spec §1.6）：带上了就等于把「入参只有一处来源」这条契约改掉了。
#[test]
fn scheduled_event_omits_input_like_the_oracle() {
    let workspace = Workspace::new("scheduled-omits-input");
    workspace.seed(&[("hello.txt".to_string(), b"hello\n".to_vec())]);
    let mut session = Session::new("sess", workspace.cwd());
    let _ = session.begin_turn("读一下");
    let events = session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_0".to_string(),
        name: "Read".to_string(),
        input: serde_json::json!({ "file_path": format!("{}/hello.txt", workspace.cwd()) }),
    }]);

    let scheduled = events
        .iter()
        .filter_map(|event| event.payload_as::<ToolUpdatedPayload>().expect("解码"))
        .find_map(|payload| match payload {
            ToolUpdatedPayload::Scheduled(scheduled) => Some(scheduled),
            _ => None,
        })
        .expect("有 scheduled 事件");
    assert!(scheduled.input.is_none(), "scheduled 不该带 input");
    assert_eq!(scheduled.input_omitted, Some(true));
    assert_eq!(scheduled.input_ref.as_deref(), Some("model_stream"));
    assert_eq!(scheduled.can_run_parallel, Some(true));
}

/// 未注册的工具**没有** `started` 事件（观察：`Glob`/`Grep` 的 `scheduled` 之后直接 `error`）。
#[test]
fn unregistered_tools_have_no_started_event() {
    let workspace = Workspace::new("unregistered-no-started");
    let mut session = Session::new("sess", workspace.cwd());
    let _ = session.begin_turn("试一下 Glob");
    let events = session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_0".to_string(),
        name: "Glob".to_string(),
        input: serde_json::json!({ "pattern": "*.txt" }),
    }]);

    let kinds: Vec<&'static str> = events
        .iter()
        .filter_map(|event| event.payload_as::<ToolUpdatedPayload>().expect("解码"))
        .map(|payload| match payload {
            ToolUpdatedPayload::Scheduled(_) => "scheduled",
            ToolUpdatedPayload::Started(_) => "started",
            ToolUpdatedPayload::Result(_) => "result",
            ToolUpdatedPayload::Error(_) => "error",
            ToolUpdatedPayload::Batch(_) => "batch",
        })
        .collect();
    assert_eq!(kinds, vec!["scheduled", "error", "batch"]);
}

/// 事件信封是封闭契约（Task 1 的镜像）：发出来的每一个都能原样再解一遍。
#[test]
fn emitted_events_round_trip_through_the_mirror() {
    let workspace = Workspace::new("envelope-round-trip");
    workspace.seed(&[("hello.txt".to_string(), b"hello\n".to_vec())]);
    let mut session = Session::new("sess_round_trip", workspace.cwd());
    let mut events = vec![session.begin_turn("读一下")];
    events.extend(session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_0".to_string(),
        name: "Read".to_string(),
        input: serde_json::json!({ "file_path": format!("{}/hello.txt", workspace.cwd()) }),
    }]));

    for event in &events {
        let text = serde_json::to_string(event).expect("可序列化");
        let decoded: SessionEvent = serde_json::from_str(&text).expect("镜像能解自己发的帧");
        assert_eq!(&decoded, event);
    }
    // 信封字段：seq 单调、turn 归属、链路。
    assert!(events.iter().all(|event| event.turn_id.is_some()));
    assert!(events
        .iter()
        .all(|event| event.delivery_kind == Some(agent_core::DeliveryKind::DesktopContinuous)));
    let sequences: Vec<u64> = events.iter().map(|event| event.seq).collect();
    let mut sorted = sequences.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sequences, sorted, "seq 必须严格递增");
}
