//! 工具切片（`Read` + `Write`）的**三层对拍**（第一层：工具调用序列；第二层：文件快照）。
//!
//! 期望值来自行为夹具 `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json` 的 `expect`
//! ——由 oracle 录制，**不是手写的**。本文件按 Ruling 4 的做法：把 Rust 会话循环发出的事件
//! 投影成夹具里的同一形状（spec §2.1 的投影 + §3 的规范化），然后**精确相等**比较。
//! 投影助手在 `agent_tools::test_support`（两端共用一份，避免出现第二份 §3 实现）。
//!
//! 命名（Task 3 改）：原名 `read_only_parity`。Task 3 起 `Write` 也走同一条会话循环对拍
//! ——它有**会话级**前置条件（「先读再写」），只能在这一层验。
//!
//! **第三层（事件流）不在这里比**：Rust 侧目前只发工具面事件（见 `agent_core::session` 的
//! 模块注释），模型流 / 状态推送 / 终态事件要 Task 5 才落地。硬比第三层会让这个任务变成
//! 「把 Task 5 提前做一半」，而不是「把工具做对」。
//!
//! 单独跑：`node scripts/rust-cargo.ts test -p agent-core`

use agent_core::session::{Session, ToolCallRequest};
use agent_core::{SessionEvent, ToolUpdatedPayload};
use agent_tools::test_support::{
    load_fixture, project, project_input, snapshot_directory, Fixture, ResultObject, Workspace,
};
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
/// | `tool.updated` / `result` | `ok = result.success === true`，`output = result.content`，外加结果对象 |
/// | `tool.updated` / `error` | `ok = false`，`output = error.message` |
fn project_tool_calls(
    events: &[SessionEvent],
    workspace: &str,
) -> Vec<(String, bool, String, Option<ResultObject>)> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::BTreeMap<
        String,
        (String, bool, String, Option<ResultObject>),
    > = std::collections::BTreeMap::new();
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
                by_id.insert(id, (scheduled.tool_name, false, String::new(), None));
            }
            ToolUpdatedPayload::Started(_)
            | ToolUpdatedPayload::Progress(_)
            | ToolUpdatedPayload::Batch(_) => {}
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
                let extras = &result.result.extra;
                slot.3 = Some(ResultObject {
                    truncated: extras
                        .get("truncated")
                        .and_then(serde_json::Value::as_bool)
                        .expect("result.truncated"),
                    original_bytes: extras
                        .get("originalBytes")
                        .and_then(serde_json::Value::as_u64)
                        .expect("result.originalBytes"),
                    returned_bytes: extras
                        .get("returnedBytes")
                        .and_then(serde_json::Value::as_u64)
                        .expect("result.returnedBytes"),
                    budget_strategy: extras
                        .get("budgetStrategy")
                        .and_then(serde_json::Value::as_str)
                        .expect("result.budgetStrategy")
                        .to_string(),
                    display: extras
                        .get("display")
                        .map(|value| project_input(value, workspace)),
                });
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
    // 按夹具的 turn 切分驱动（`read-dedup` 的「跨 turn 仍然命中」只能这样验）。
    let mut events = Vec::new();
    for (start, end) in fixture.turn_ranges(calls.len()) {
        events.push(session.begin_turn("对拍：只读工具"));
        events.extend(session.run_tool_batch(&calls[start..end]));
    }

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
        // 结果对象层：`truncated` / `budgetStrategy` / `display`（Write/Edit 的 `file_diff`）
        // 逐字段比较。以前只解析不比较，diff 呈现缺失因此长期未被发现。
        //
        // 字节计数**不比**：`originalBytes` / `returnedBytes` 依赖工作目录长度（正文里含绝对路径）
        // 与 shell 前缀的规范化差异，同一行为在不同机器上数值不同（证据：write-new 的 10 字节
        // 差值 = cwd 与 `<cwd>` 的长度差；bash-command-not-found 的前缀第 1 行长度随 shell 变）。
        // 这条口径登记在 spec §6.3「已知误差」，不是产品差异。
        match (&got.3, &fixture.expect_result_objects[index]) {
            (Some(actual_object), Some(expected_object)) => {
                assert_eq!(
                    actual_object.truncated, expected_object.truncated,
                    "{case}：第 {index} 个调用的 truncated 与录制不一致"
                );
                assert_eq!(
                    actual_object.budget_strategy, expected_object.budget_strategy,
                    "{case}：第 {index} 个调用的 budgetStrategy 与录制不一致"
                );
                assert_eq!(
                    actual_object.display, expected_object.display,
                    "{case}：第 {index} 个调用的 display 与录制不一致"
                );
            }
            (None, None) => {}
            (actual_object, expected_object) => panic!(
                "{case}：第 {index} 个调用的结果对象有无不一致（实际 {:?}，期望 {:?}）",
                actual_object.is_some(),
                expected_object.is_some()
            ),
        }
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
        if event.event_type != agent_core::SessionEventType::ToolUpdated {
            continue;
        }
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

    // 第二层：跑完之后的工作目录快照必须与录制**逐字**一致（相对路径 → 内容 sha256）。
    // 写成通用比较而不是「只读工具不改目录」：写工具会**新增**文件，只比键集会漏掉内容错。
    assert_eq!(
        snapshot_directory(workspace.path()),
        fixture.expect_files,
        "{case}：文件快照与录制不一致"
    );
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
            ToolUpdatedPayload::Progress(_) => "progress",
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

#[test]
fn read_dedup_parity() {
    assert_parity("read-dedup");
}

#[test]
fn read_truncation_errors_parity() {
    assert_parity("read-truncation-errors");
}

#[test]
fn read_partial_view_parity() {
    assert_parity("read-partial-view");
}

#[test]
fn read_extension_tables_parity() {
    assert_parity("read-extension-tables");
}

/// 重复读的去重是**会话级**的：同一个 `Session` 上第二次读同一文件同一范围 → 填充语；
/// 换一个 `Session` → 又给正文（状态不跨会话）。
///
/// 期望值不是手写的：第一条与第二条都来自夹具 `read-dedup` 的录制结果（由上面的
/// `read_dedup_parity` 逐字比过），这里只补「换会话就重置」这半条——它由
/// 「同一个纯函数 + 新会话」推不出别的结果，所以用夹具的第一条期望值当基准。
#[test]
fn read_history_is_scoped_to_the_session() {
    let fixture = load_fixture("read-dedup");
    let workspace = Workspace::new("read-history-scope");
    workspace.seed(&fixture.seed);
    let calls = script_calls(&fixture, &workspace);
    let first_expected = fixture.expect_calls[0].output.clone();
    let stub_expected = fixture.expect_calls[1].output.clone();

    let mut session = Session::new("sess_a", workspace.cwd());
    let _ = session.begin_turn("第一次");
    let events = session.run_tool_batch(&calls[0..2]);
    let projected = project_tool_calls(&events, &workspace.cwd());
    assert_eq!(projected[0].2, first_expected, "首次读应给正文");
    assert_eq!(projected[1].2, stub_expected, "同会话第二次读应给填充语");

    // 换一个会话：**它的第一次读又给正文**——若记忆挂在进程/全局上，这里会命中上一会话留下的
    // 记录（上一个会话对同一文件同一范围的第二次读是 stub），于是这条会红。
    let mut other = Session::new("sess_b", workspace.cwd());
    let _ = other.begin_turn("另一个会话");
    let events = other.run_tool_batch(&calls[0..2]);
    let projected = project_tool_calls(&events, &workspace.cwd());
    assert_eq!(
        projected[0].2, first_expected,
        "新会话的第一次读应当给正文（去重记忆不跨会话）"
    );
    // 而新会话内部照样会去重：同会话第二次读同一范围 → 填充语。
    assert_eq!(projected[1].2, stub_expected, "去重在会话内部仍然生效");
}

// ---- Task 3：`Write` 的对拍（spec §3.3 的观察出处；每条夹具由 `record.mts` 从 oracle 录）。

#[test]
fn write_new_parity() {
    assert_parity("write-new");
}

#[test]
fn write_overwrite_unread_parity() {
    assert_parity("write-overwrite-unread");
}

#[test]
fn write_overwrite_read_parity() {
    assert_parity("write-overwrite-read");
}

#[test]
fn write_empty_args_parity() {
    assert_parity("write-empty-args");
}

#[test]
fn write_missing_content_parity() {
    assert_parity("write-missing-content");
}

#[test]
fn write_empty_path_parity() {
    assert_parity("write-empty-path");
}

#[test]
fn write_relative_path_parity() {
    assert_parity("write-relative-path");
}

/// 「先读再写」的前置条件也是**会话级**的：同一个 `Session` 上先 `Read` 再 `Write` → 成功；
/// 换一个 `Session` 直接 `Write` 同一个文件 → `write_file_not_read`。
///
/// 期望值全部取自夹具：成功文案来自 `write-overwrite-read` 的录制，失败文案来自
/// `write-overwrite-unread` 的录制——两段都不是手写的。
#[test]
fn write_precondition_is_scoped_to_the_session() {
    let fixture = load_fixture("write-overwrite-read");
    let workspace = Workspace::new("write-precondition-scope");
    workspace.seed(&fixture.seed);
    let calls = script_calls(&fixture, &workspace);
    let read_expected = fixture.expect_calls[0].output.clone();
    let write_expected = fixture.expect_calls[1].output.clone();

    let mut session = Session::new("sess_w1", workspace.cwd());
    let _ = session.begin_turn("先读再写");
    let events = session.run_tool_batch(&calls[0..2]);
    let projected = project_tool_calls(&events, &workspace.cwd());
    assert_eq!(projected[0].2, read_expected, "先读应当成功");
    assert!(projected[1].1, "同一会话里读过的文件可以覆盖");
    assert_eq!(projected[1].2, write_expected, "覆盖成功文案应与录制一致");

    // 换一个会话：没读过它 → 覆盖失败。若红，说明前置条件的记忆被挂到了进程/全局上。
    let mut other = Session::new("sess_w2", workspace.cwd());
    let _ = other.begin_turn("直接覆盖");
    let events = other.run_tool_batch(&calls[1..2]);
    let projected = project_tool_calls(&events, &workspace.cwd());
    assert!(!projected[0].1, "新会话没有读过它，覆盖必须失败");
    assert_eq!(
        projected[0].2,
        load_fixture("write-overwrite-unread").expect_calls[0]
            .output
            .clone(),
        "未读覆盖的失败文案应与录制一致"
    );
}

// ---- Task 3 Task B：`Edit` 的对拍（spec §3.3.1 的观察出处）。

#[test]
fn edit_unique_parity() {
    assert_parity("edit-unique");
}

#[test]
fn edit_replace_all_parity() {
    assert_parity("edit-replace-all");
}

#[test]
fn edit_not_found_parity() {
    assert_parity("edit-not-found");
}

#[test]
fn edit_ambiguous_parity() {
    assert_parity("edit-ambiguous");
}

#[test]
fn edit_same_string_parity() {
    assert_parity("edit-same-string");
}

#[test]
fn edit_no_read_parity() {
    assert_parity("edit-no-read");
}

#[test]
fn edit_missing_file_parity() {
    assert_parity("edit-missing-file");
}

#[test]
fn edit_empty_old_creates_parity() {
    assert_parity("edit-empty-old-creates");
}

#[test]
fn edit_empty_old_existing_parity() {
    assert_parity("edit-empty-old-existing");
}

#[test]
fn edit_missing_new_string_parity() {
    assert_parity("edit-missing-new-string");
}

#[test]
fn bash_stdout_parity() {
    assert_parity("bash-stdout");
}

#[test]
fn bash_exit_7_parity() {
    assert_parity("bash-exit-7");
}

#[test]
fn bash_output_then_exit_7_parity() {
    assert_parity("bash-output-then-exit-7");
}

#[test]
fn bash_stderr_parity() {
    assert_parity("bash-stderr");
}

#[test]
fn bash_mixed_streams_parity() {
    assert_parity("bash-mixed-streams");
}

#[test]
fn bash_empty_output_parity() {
    assert_parity("bash-empty-output");
}

#[test]
fn bash_trailing_whitespace_parity() {
    assert_parity("bash-trailing-whitespace");
}

#[test]
fn bash_command_not_found_parity() {
    assert_parity("bash-command-not-found");
}

#[test]
fn bash_timeout_parity() {
    assert_parity("bash-timeout");
}

#[test]
fn bash_timeout_fractional_parity() {
    assert_parity("bash-timeout-fractional");
}

#[test]
fn bash_empty_args_parity() {
    assert_parity("bash-empty-args");
}
