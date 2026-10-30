//! M2 Task 1：共享协议夹具的 Rust 侧断言。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md` §1.2（stdio 的实测线上形状）。
//! 镜像面（ledger Ruling 13）：旧运行时在 stdio 上实际说的话，**不是** `mode-protocol-v4/**`。
//! 纪律：`docs/specs/clean-room-policy.md`——本文件只用**允许输入**（协议类型声明/帧结构、
//! 导出签名）与**黑盒观察**（oracle 的输入输出与事件序列），不读旧实现体。
//!
//! 夹具：`packages/shared/test/fixtures/agent-protocol/*.json`——**TS 侧读的是同一个目录**
//! （`packages/desktop/tests/agent-runtime-oracle/protocol-fixtures.test.mts`）。
//! 夹具由 `packages/desktop/tests/agent-runtime-oracle/record-protocol-fixtures.mts`
//! 从 oracle 的 stdout/stdin 上**整帧原样**录下，不是照着镜像类型写出来的：
//! 两端各自用自己的类型去解同一份字节，哪一端单方面改了类型而另一端没跟上，那一端就红。
//! 例外：`request-session-list` / `response-session-list` 是按协议声明 schema 构造的形状夹具
//! （note 已注明「非录制帧」），等会话只读链路接上运行时后补录制版本。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use agent_protocol::frame::{Frame, RequestId};
use agent_protocol::session::{
    RuntimePreferencesRequestParams, RuntimePreferencesResult, RuntimePreferencesScope,
    SessionCreateParams, SessionSendParams, SessionSendResult,
};
use agent_protocol::session_debug::{NetworkStatusType, SessionDebugParams, SessionDebugSnapshot};
use agent_protocol::session_event::{
    ModelStreamingKind, ModelStreamingPayload, SessionEvent, SessionEventType,
    SessionTitleUpdatedPayload, TitleSource, ToolUpdatedPayload, TurnCompletedPayload,
    TurnStartedPayload,
};
use agent_protocol::message::{MessageInfo, MessagePart, ToolState};
use agent_protocol::session_read::{
    EndedSubagentStatus, RunningSubagentStatus, SessionEventsParams, SessionEventsResult,
    SessionKind, SessionListParams, SessionListResult, SessionMessagesParams,
    SessionMessagesResult, SessionReadParams, SessionSubagentsParams, SessionSubagentsResult,
};
use agent_protocol::session_write::{
    SessionCloseParams, SessionCloseResult, SessionPersistence, SessionSetModeParams,
    SessionSetModelParams, SessionSetThoughtLevelParams,
};
use agent_protocol::snapshot::SessionStateSnapshot;
use serde::Deserialize;
use serde_json::{json, Value};

/// 夹具外壳（我们的记录格式，不属于线上形状）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureFile {
    case: String,
    direction: String,
    #[serde(default)]
    responds_to: Option<String>,
    note: String,
    frame: Value,
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/shared/test/fixtures/agent-protocol")
}

fn load_fixtures() -> BTreeMap<String, FixtureFile> {
    let dir = fixture_dir();
    let mut fixtures = BTreeMap::new();
    for entry in std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("夹具目录读不到（{}）：{error}", dir.display()))
    {
        let path = entry.expect("读夹具目录项失败").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let fixture: FixtureFile = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{} 不是合法夹具：{error}", path.display()));
        assert!(!fixture.note.is_empty(), "{} 缺 note", fixture.case);
        fixtures.insert(fixture.case.clone(), fixture);
    }
    fixtures
}

fn parse_frame(case: &str, fixture: &FixtureFile) -> Result<Frame, String> {
    serde_json::from_value(fixture.frame.clone())
        .map_err(|error| format!("{case} 的帧解不进镜像 Frame：{error}"))
}

/// 载荷的严格解码：解不开就报出是哪个夹具、哪个类型（夹具一多，不然找不到是谁坏了）。
fn decode<T: serde::de::DeserializeOwned>(case: &str, value: &Value) -> T {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("{case} 的载荷解不进镜像类型：{error}"))
}

/// 事件载荷的严格解码，走 [`SessionEvent::payload_as`]（M2 消费方要用的那条通路）。
fn decode_payload<T: serde::de::DeserializeOwned>(case: &str, event: &SessionEvent) -> T {
    event
        .payload_as::<T>()
        .unwrap_or_else(|error| panic!("{case} 的载荷解不进镜像类型：{error}"))
        .unwrap_or_else(|| panic!("{case}：{} 应当带载荷", event.event_type.as_str()))
}

/// 已建模载荷里的 `extra`（未建模字段的落脚处）应该是空的：非空说明线上多出了字段，
/// 要么补镜像，要么在报告里写明它为什么可以忽略。
fn assert_no_unmodeled_payload_fields(case: &str, extra: &BTreeMap<String, Value>) {
    assert!(
        extra.is_empty(),
        "{case} 的载荷里出现了镜像未建模的字段（{:?}）：补镜像，或说明为什么可以忽略",
        extra.keys().collect::<Vec<_>>()
    );
}

#[test]
fn every_shared_fixture_parses_into_the_mirror_types() {
    let fixtures = load_fixtures();
    assert!(!fixtures.is_empty(), "夹具目录是空的，这条用例等于没跑");
    for (case, fixture) in &fixtures {
        let frame = parse_frame(case, fixture).unwrap_or_else(|error| panic!("{error}"));
        match (&frame, fixture.direction.as_str()) {
            (Frame::Notification(_), "runtime->host") => {}
            (Frame::Request(request), "runtime->host") => {
                // 实测事实：运行时发出的反向请求用的是字符串 id（"server-1"）。
                assert!(
                    matches!(request.id, RequestId::String(_)),
                    "{case}：运行时方向的反向请求应当是字符串 id"
                );
            }
            (Frame::Response(_), "runtime->host") | (Frame::ErrorResponse(_), "runtime->host") => {}
            (Frame::Request(request), "host->runtime") => {
                // host 侧自己发请求时用数字 id。
                assert!(
                    matches!(request.id, RequestId::Number(_)),
                    "{case}：host 方向的请求应当是数字 id"
                );
            }
            (Frame::Response(_), "host->runtime") => {}
            (other, direction) => panic!("{case}：{direction} 方向出现了不匹配的帧形状 {other:?}"),
        }
        if fixture.frame.get("method").is_none() {
            assert!(
                fixture.responds_to.is_some(),
                "{case}：应答帧必须带 respondsTo（靠它才知道该用哪张 result schema）"
            );
        }
    }
}

/// 会话事件的载荷断言；返回这条夹具覆盖到的「事件类型[:kind]」键。
fn exercise_event_payload(case: &str, event: &SessionEvent) -> String {
    match event.event_type {
        SessionEventType::SessionTitleUpdated => {
            let payload: SessionTitleUpdatedPayload = decode_payload(case, event);
            assert!(!payload.title.is_empty(), "{case}：标题事件应当带标题");
            assert_no_unmodeled_payload_fields(case, &payload.extra);
            // `messageID` 只在**生成**路径上出现（首句标题那条 `source:"first_input"` 不带），
            // 所以两个形状都要有夹具，否则这个字段永远没人验证（Task 1 复评 Important 1）。
            if payload.source == TitleSource::Generated {
                assert!(
                    payload.message_id.is_some(),
                    "{case}：生成路径的标题事件必须带 messageID"
                );
            } else {
                assert!(
                    payload.message_id.is_none(),
                    "{case}：非生成路径的标题事件不该带 messageID"
                );
            }
            // 覆盖键用**线上取值**（不是 Debug 名）：source 的取值本身就是被测面的一部分。
            let source = serde_json::to_value(payload.source).unwrap();
            format!(
                "session.titleUpdated:{}",
                source.as_str().unwrap_or("<non-string>")
            )
        }
        SessionEventType::TurnStarted => {
            let payload: TurnStartedPayload = decode_payload(case, event);
            assert!(!payload.input.is_empty(), "{case}：turn.started 应当带输入原文");
            assert_no_unmodeled_payload_fields(case, &payload.extra);
            "turn.started".to_string()
        }
        SessionEventType::TurnCompleted => {
            let payload: TurnCompletedPayload = decode_payload(case, event);
            assert!(payload.duration >= 0.0, "{case}：duration 不该是负数");
            assert_no_unmodeled_payload_fields(case, &payload.extra);
            "turn.completed".to_string()
        }
        SessionEventType::TurnFailed => {
            // 尚无夹具（要触发它需要一条失败的建会话路径，M2 的只读切片跑不到）；
            // 类型与载荷都已按声明镜像，等有录到的失败流程再补断言。
            panic!("{case}：出现了还没有夹具的 turn.failed，请先补一条录制的失败流程");
        }
        SessionEventType::ModelStreaming => {
            let payload: ModelStreamingPayload = decode_payload(case, event);
            assert_no_unmodeled_payload_fields(case, &payload.extra);
            match payload.kind {
                ModelStreamingKind::ToolCall => {
                    // 第一层的工具入参只能从这里取（调度事件故意不带 input）。
                    assert!(
                        payload.input.is_some(),
                        "{case}：kind=tool_call 必须带 input，否则工具入参这一层失去来源"
                    );
                    "model.streaming:tool_call".to_string()
                }
                other => panic!("{case}：model.streaming 的 {other:?} 还没有夹具"),
            }
        }
        SessionEventType::ToolUpdated => {
            let payload: ToolUpdatedPayload = decode_payload(case, event);
            match payload {
                ToolUpdatedPayload::Scheduled(fields) => {
                    assert_no_unmodeled_payload_fields(case, &fields.extra);
                    assert!(!fields.tool_name.is_empty(), "{case}：调度事件应当带工具名");
                    // 调度事件故意不带入参：入参只出现在 model.streaming/tool_call。
                    assert!(fields.input.is_none(), "{case}：调度事件不该带 input");
                    "tool.updated:scheduled".to_string()
                }
                ToolUpdatedPayload::Started(fields) => {
                    assert_no_unmodeled_payload_fields(case, &fields.extra);
                    "tool.updated:started".to_string()
                }
                ToolUpdatedPayload::Progress(fields) => {
                    assert_no_unmodeled_payload_fields(case, &fields.extra);
                    assert!(
                        !fields.tool_name.is_empty(),
                        "{case}：进度事件应当带工具名"
                    );
                    "tool.updated:progress".to_string()
                }
                ToolUpdatedPayload::Result(fields) => {
                    assert_no_unmodeled_payload_fields(case, &fields.extra);
                    // `result` 本身是**开放对象**（工具自带的输出面），未建模字段是它的正常
                    // 形态，所以只断言镜像真的取到了 `ok` 的判据。
                    assert!(
                        fields.result.success.is_some(),
                        "{case}：工具输出应当带 success（`ok` 的判据）"
                    );
                    "tool.updated:result".to_string()
                }
                ToolUpdatedPayload::Error(fields) => {
                    assert_no_unmodeled_payload_fields(case, &fields.extra);
                    assert!(!fields.error.message.is_empty(), "{case}：错误事件应当带 message");
                    "tool.updated:error".to_string()
                }
                ToolUpdatedPayload::Batch(fields) => {
                    assert_no_unmodeled_payload_fields(case, &fields.extra);
                    assert!(!fields.tool_call_ids.is_empty(), "{case}：批次事件应当带 toolCallIds");
                    "tool.updated:batch".to_string()
                }
            }
        }
        other => panic!("{case}：会话事件 {other:?} 还没有镜像载荷，也没有夹具；不要静默放过"),
    }
}

/// 面板上的覆盖清单：夹具**必须**覆盖到这些事件键，否则「解析成功」可能只是因为没读载荷。
fn expected_exercised_event_keys() -> BTreeSet<String> {
    [
        "session.titleUpdated:first_input",
        "session.titleUpdated:generated",
        "turn.started",
        "turn.completed",
        "model.streaming:tool_call",
        "tool.updated:scheduled",
        "tool.updated:started",
        "tool.updated:result",
        "tool.updated:error",
        "tool.updated:batch",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[test]
fn session_event_fixtures_decode_into_typed_payloads() {
    let mut exercised = BTreeSet::new();
    for (case, fixture) in &load_fixtures() {
        let Frame::Notification(notification) = parse_frame(case, fixture).unwrap() else {
            continue;
        };
        if notification.method != "session/event" {
            continue;
        }
        let params = notification
            .params
            .unwrap_or_else(|| panic!("{case}：session/event 通知必须带 params"));
        let event: SessionEvent = decode(case, &params);
        exercised.insert(exercise_event_payload(case, &event));
    }
    assert_eq!(
        exercised,
        expected_exercised_event_keys(),
        "夹具覆盖的事件类型与预期不一致：多/少了类型就要显式改这张清单"
    );
}

#[test]
fn host_command_fixtures_decode_into_typed_params_and_results() {
    let mut exercised = BTreeSet::new();
    for (case, fixture) in &load_fixtures() {
        match parse_frame(case, fixture).unwrap() {
            Frame::Request(request) => {
                let params = request
                    .params
                    .unwrap_or_else(|| panic!("{case}：请求必须带 params"));
                match request.method.as_str() {
                    "session/create" => {
                        let params: SessionCreateParams = decode(case, &params);
                        assert!(
                            !params.workspace.workspace_path.is_empty()
                                && !params.workspace.workspace_key.is_empty(),
                            "{case}：建会话必须带 workspacePath/workspaceKey"
                        );
                        let model = params.model.expect("夹具是显式传 model 的（spec §1.5）");
                        assert!(!model.provider_id.is_empty() && !model.model_id.is_empty());
                        assert_no_unmodeled_payload_fields(case, &params.extra);
                        exercised.insert("session/create".to_string());
                    }
                    "session/send" => {
                        let params: SessionSendParams = decode(case, &params);
                        assert!(!params.session_id.is_empty() && !params.content.is_empty());
                        assert_no_unmodeled_payload_fields(case, &params.extra);
                        exercised.insert("session/send".to_string());
                    }
                    "session/list" => {
                        let params: SessionListParams = decode(case, &params);
                        let workspace = params.workspace.as_ref().expect("夹具带 workspace");
                        assert!(
                            !workspace.workspace_path.is_empty()
                                && !workspace.workspace_key.is_empty(),
                            "{case}：列表请求必须带 workspacePath/workspaceKey"
                        );
                        assert!(!params.include_archived);
                        assert_eq!(params.limit, Some(50));
                        assert!(params.session_ids.is_none(), "{case}：夹具不带 sessionIds");
                        exercised.insert("session/list".to_string());
                    }
                    "session/events" => {
                        let params: SessionEventsParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        assert_eq!(params.after_seq, Some(10));
                        assert_eq!(params.limit, Some(20));
                        exercised.insert("session/events".to_string());
                    }
                    "session/subagents" => {
                        let params: SessionSubagentsParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        assert_eq!(params.ended_cursor.as_deref(), Some("cursor-1"));
                        assert_eq!(params.ended_limit, 20);
                        exercised.insert("session/subagents".to_string());
                    }
                    "session/messages" => {
                        let params: SessionMessagesParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        assert_eq!(params.after_message_id.as_deref(), Some("msg-1"));
                        assert_eq!(params.limit, Some(50));
                        exercised.insert("session/messages".to_string());
                    }
                    "session/read" => {
                        let params: SessionReadParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        assert_eq!(params.message_limit, Some(100));
                        assert_eq!(params.after_seq, Some(5));
                        assert!(params.delivery_kind.is_some());
                        exercised.insert("session/read".to_string());
                    }
                    "session/debug" => {
                        let params: SessionDebugParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        exercised.insert("session/debug".to_string());
                    }
                    "session/close" => {
                        let params: SessionCloseParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        assert_eq!(
                            params.expected_persistence,
                            Some(SessionPersistence::Deferred)
                        );
                        exercised.insert("session/close".to_string());
                    }
                    "session/setModel" => {
                        let params: SessionSetModelParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        assert_eq!(params.model.provider_id, "zai");
                        assert_eq!(params.expected_revision, Some(3));
                        assert_eq!(params.persist_as_workspace_last_used, Some(false));
                        exercised.insert("session/setModel".to_string());
                    }
                    "session/setThoughtLevel" => {
                        let params: SessionSetThoughtLevelParams = decode(case, &params);
                        assert_eq!(params.thought_level.as_deref(), Some("high"));
                        assert_eq!(params.persist_as_workspace_last_used, Some(true));
                        exercised.insert("session/setThoughtLevel".to_string());
                    }
                    "session/setMode" => {
                        let params: SessionSetModeParams = decode(case, &params);
                        assert_eq!(params.session_id, "sess-1");
                        assert_eq!(params.mode, agent_protocol::session_read::SessionMode::Plan);
                        assert_eq!(params.expected_revision, Some(3));
                        exercised.insert("session/setMode".to_string());
                    }
                    "session/requestRuntimePreferences" => {
                        let params: RuntimePreferencesRequestParams = decode(case, &params);
                        assert_eq!(params.scope, RuntimePreferencesScope::RuntimeMaterialization);
                        assert_no_unmodeled_payload_fields(case, &params.extra);
                        exercised.insert("session/requestRuntimePreferences".to_string());
                    }
                    other => panic!("{case}：请求方法 {other} 没有镜像"),
                }
            }
            Frame::Response(response) => {
                let responds_to = fixture
                    .responds_to
                    .as_deref()
                    .unwrap_or_else(|| panic!("{case}：应答帧缺 respondsTo"));
                match responds_to {
                    "session/send" => {
                        let result: SessionSendResult = decode(case, &response.result);
                        assert!(result.accepted, "{case}：受理确认必须是 accepted");
                        // 方向对称性：host 将来要发出同形状的帧，所以序列化也要逐字回到原样。
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/send".to_string());
                    }
                    "session/requestRuntimePreferences" => {
                        let result: RuntimePreferencesResult = decode(case, &response.result);
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/requestRuntimePreferences".to_string());
                    }
                    "session/list" => {
                        let result: SessionListResult = decode(case, &response.result);
                        assert_eq!(result.sessions.len(), 2);
                        assert_eq!(result.sessions[0].session_kind, SessionKind::Interactive);
                        // `summaryTitle` 的两种写法都要原样保留：显式 null 写回 null。
                        let goal = result.sessions[1]
                            .target
                            .as_ref()
                            .expect("第二项带 target")
                            .as_ref()
                            .expect("target 不是 null");
                        assert_eq!(goal.summary_title, Some(None));
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/list".to_string());
                    }
                    "session/events" => {
                        let result: SessionEventsResult = decode(case, &response.result);
                        assert_eq!(result.events.len(), 2);
                        assert_eq!(result.events[0].seq, 11);
                        assert!(result.events[1].turn_id.is_some());
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/events".to_string());
                    }
                    "session/subagents" => {
                        let result: SessionSubagentsResult = decode(case, &response.result);
                        assert_eq!(result.revision, 3);
                        assert_eq!(result.running.len(), 1);
                        assert_eq!(result.running[0].status, RunningSubagentStatus::Running);
                        assert_eq!(result.ended.items.len(), 1);
                        assert_eq!(result.ended.items[0].status, EndedSubagentStatus::Success);
                        assert_eq!(result.ended.next_cursor.as_deref(), Some("cursor-2"));
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/subagents".to_string());
                    }
                    "session/messages" => {
                        let result: SessionMessagesResult = decode(case, &response.result);
                        assert_eq!(result.messages.len(), 2);
                        assert!(matches!(result.messages[0].info, MessageInfo::User(_)));
                        assert!(matches!(result.messages[1].info, MessageInfo::Assistant(_)));
                        assert!(matches!(
                            result.messages[1].parts[0],
                            MessagePart::Text(_)
                        ));
                        let MessagePart::Tool(tool) = &result.messages[1].parts[1] else {
                            panic!("{case}：第二个部件应当是 tool");
                        };
                        assert!(matches!(tool.state, ToolState::Completed(_)));
                        assert!(matches!(
                            result.messages[1].parts[2],
                            MessagePart::StepFinish(_)
                        ));
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/messages".to_string());
                    }
                    "session/read" => {
                        let result: SessionStateSnapshot = decode(case, &response.result);
                        assert_eq!(result.protocol.version, 1);
                        assert!(result.messages.is_empty());
                        assert_eq!(result.settings.model.available.len(), 1);
                        assert!(result.settings.permission.is_some());
                        assert_eq!(result.runtime.api_retry, Some(None));
                        assert!(result.todos.is_some() && result.slash_commands.is_some());
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/read".to_string());
                    }
                    "session/debug" => {
                        let result: SessionDebugSnapshot = decode(case, &response.result);
                        assert_eq!(result.session_id, "sess-1");
                        assert_eq!(result.rounds.len(), 1);
                        assert_eq!(result.rounds[0].hit_rate, None);
                        assert_eq!(result.rounds[0].prefill_duration_ms, Some(None));
                        assert_eq!(result.network_entries.len(), 1);
                        assert_eq!(
                            result.network_entries[0].status_type,
                            NetworkStatusType::Started
                        );
                        assert!(result.cache.is_none());
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/debug".to_string());
                    }
                    "session/close" => {
                        let result: SessionCloseResult = decode(case, &response.result);
                        assert_eq!(result.closed, Some(true));
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert("session/close".to_string());
                    }
                    "session/setModel" | "session/setThoughtLevel" | "session/setMode" => {
                        let result: SessionStateSnapshot = decode(case, &response.result);
                        assert_eq!(result.runtime.state_revision, 1);
                        assert_eq!(
                            serde_json::to_value(&result).unwrap(),
                            response.result,
                            "{case}：镜像的序列化结果与线上帧不一致（字段名/缺省处理有偏差）"
                        );
                        exercised.insert(responds_to.to_string());
                    }
                    other => panic!("{case}：应答 {other} 没有镜像"),
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        exercised,
        [
            "session/close",
            "session/create",
            "session/debug",
            "session/events",
            "session/list",
            "session/messages",
            "session/read",
            "session/requestRuntimePreferences",
            "session/send",
            "session/setMode",
            "session/setModel",
            "session/setThoughtLevel",
            "session/subagents"
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>(),
        "夹具覆盖的命令与预期不一致：多/少一条就要显式改这张清单"
    );
}

/// 协议里 `memoryEnabled` / `askUserQuestionAutoResolutionEnabled` /
/// `modelContextBudgetStrategy` 带 `.default(...)`——旧 host 的合法应答可以**省略**它们
/// （Task 1 复评 Minor 2）。
///
/// 镜像必须**保留省略**（`None`），而不是在解析时替协议补上默认值：否则「读一条合法旧应答、
/// 再写回去」会凭空多出字段，round-trip 相等这条断言就比协议本身更严，会给出假红。
/// 需要协议的默认值时由消费方 `unwrap_or(false)` / `unwrap_or_default()`——枚举的 `Default`
/// 实现就是协议声明的默认值本身。
#[test]
fn runtime_preferences_preserve_omitted_defaults() {
    let mut shapes = BTreeSet::new();
    for (case, fixture) in &load_fixtures() {
        if fixture.responds_to.as_deref() != Some("session/requestRuntimePreferences") {
            continue;
        }
        let Frame::Response(response) = parse_frame(case, fixture).unwrap() else {
            continue;
        };
        let result: RuntimePreferencesResult = decode(case, &response.result);
        for (wire_key, mirrored_omitted) in [
            ("memoryEnabled", result.memory_enabled.is_none()),
            (
                "askUserQuestionAutoResolutionEnabled",
                result.ask_user_question_auto_resolution_enabled.is_none(),
            ),
            (
                "modelContextBudgetStrategy",
                result.model_context_budget_strategy.is_none(),
            ),
        ] {
            assert_eq!(
                mirrored_omitted,
                response.result.get(wire_key).is_none(),
                "{case}：{wire_key} 在线上是省略的，镜像也必须保持省略（不许补默认值）"
            );
        }
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            response.result,
            "{case}：省略必须原样写回"
        );
        shapes.insert(format!("omitsDefaults={}", result.memory_enabled.is_none()));
    }
    assert_eq!(
        shapes,
        ["omitsDefaults=false", "omitsDefaults=true"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        "两种形状都要有夹具：字段都在的完整应答 + 省略带默认值字段的精简应答"
    );
}

#[test]
fn session_event_envelope_is_closed() {
    // 信封是封闭契约：多一个键就整体拒绝（与 TS 侧 zod 的 .strict() 同口径）。
    // 这条用例是「两端读同一份字节」的本地半边：把信封改坏，这一端必须红。
    let fixtures = load_fixtures();
    let (case, fixture) = fixtures
        .iter()
        .find(|(_, fixture)| {
            fixture.frame.get("params").and_then(|params| params.get("type"))
                == Some(&json!("tool.updated"))
        })
        .expect("夹具里至少有一条 tool.updated 会话事件");
    let Frame::Notification(notification) = parse_frame(case, fixture).unwrap() else {
        panic!("{case} 应当是通知帧");
    };
    let mut params = notification.params.unwrap();
    let original: SessionEvent = decode(case, &params);
    assert_eq!(original.event_type, SessionEventType::ToolUpdated);
    params["unmodeledEnvelopeKey"] = json!(1);
    assert!(
        serde_json::from_value::<SessionEvent>(params).is_err(),
        "{case}：信封多了一个键却仍然解析成功——信封必须封闭，否则两端无法互证"
    );
}

/// 线上的 `type` 取值逐条写死：不用 `rename_all` 推导（M1 踩过
/// `OAuthCallback` → `o-auth-callback`，见 `crates/protocol/src/lib.rs` 的注释）。
#[test]
fn session_event_type_matches_the_wire_vocabulary() {
    let declared: [(&str, SessionEventType); 25] = [
        ("session.created", SessionEventType::SessionCreated),
        ("session.resumed", SessionEventType::SessionResumed),
        ("session.updated", SessionEventType::SessionUpdated),
        ("session.titleUpdated", SessionEventType::SessionTitleUpdated),
        ("session.closed", SessionEventType::SessionClosed),
        ("turn.started", SessionEventType::TurnStarted),
        ("turn.steerQueued", SessionEventType::TurnSteerQueued),
        ("turn.steerDrained", SessionEventType::TurnSteerDrained),
        ("turn.completed", SessionEventType::TurnCompleted),
        ("turn.failed", SessionEventType::TurnFailed),
        ("message.upserted", SessionEventType::MessageUpserted),
        ("message.removed", SessionEventType::MessageRemoved),
        ("part.started", SessionEventType::PartStarted),
        ("part.delta", SessionEventType::PartDelta),
        ("part.upserted", SessionEventType::PartUpserted),
        ("part.removed", SessionEventType::PartRemoved),
        ("model.streaming", SessionEventType::ModelStreaming),
        ("tool.updated", SessionEventType::ToolUpdated),
        ("permission.requested", SessionEventType::PermissionRequested),
        ("permission.resolved", SessionEventType::PermissionResolved),
        ("userInput.requested", SessionEventType::UserInputRequested),
        ("userInput.resolved", SessionEventType::UserInputResolved),
        ("checkpoint.created", SessionEventType::CheckpointCreated),
        ("rewind.triggered", SessionEventType::RewindTriggered),
        ("streamRecovery.updated", SessionEventType::StreamRecoveryUpdated),
    ];
    for (wire, expected) in declared {
        let parsed: SessionEventType = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(parsed, expected, "{wire} 解成了 {parsed:?}");
        assert_eq!(serde_json::to_value(expected).unwrap(), json!(wire));
    }
    assert!(
        serde_json::from_value::<SessionEventType>(json!("session.invented")).is_err(),
        "未知事件类型必须被拒绝，否则比较器会把新类型静默当成已知类型"
    );
}
