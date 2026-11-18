//! `session/subagents`：子代理只读投影（LT-1 C3 T4）。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §4（T0 黑盒夹具
//! `subagents-empty` / `subagents-foreground` / `subagents-background` /
//! `subagents-pagination`）；Rust 投影规则与差异登记在 §4.8。
//!
//! 边界与形状（夹具观察值）：
//! - 会话行缺失、或会话还没有任何 turn（父会话无 role=user 的 message 行）→
//!   `-32004 Session not found: <sessionId>`；
//! - `endedLimit` 缺省 20（协议 schema `.int().positive().max(100).default(20)`
//!   镜像），`0`/负数与 `>100` 都是 `-32602`（`too_small` 文案逐字对齐夹具；
//!   `too_big`/上界是 schema 派生值，运行时未观察，登记 §4.8）；
//! - `childSessionIds` 来自 `task_type='subagent_child'` 的子会话行（创建顺序
//!   别名 `child-<n>`）；running/ended 项来自父会话的 Agent tool part；
//! - ended 按 `(endedAt, 创建序)` 倒序，`endedLimit` 截页 + 不透明 base64 游标
//!   （JSON `{childSessionId, endedAt}`），keyset 按同序二元组取剩余项（同刻
//!   平局不丢项），无更多页时 `nextCursor` 键省略；
//! - **纯读**：不写任何表、不 spawn 会话——实际子代理执行归 C4。
//!
//! fail-soft 纪律（§4.8）：子会话行/tool part 缺失或损坏 → 列表为空、键省略或
//! 中性空串，不 panic；只有库级读失败显式 `-32603`。

use agent_protocol::session_read::{
    EndedSubagent, EndedSubagentStatus, RunningSubagent, RunningSubagentStatus,
    SessionSubagentsEnded, SessionSubagentsParams, SessionSubagentsResult,
    SESSION_SUBAGENTS_METHOD,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use session_store::MessageWithParts;

use crate::server::ServerError;

use super::{epoch_u64, parse_params, store_error};

/// `endedLimit` 上界（协议 schema `.max(100)`；上界拒绝是 schema 派生值，
/// oracle 运行时行为未观察，登记 §4.8）。
const ENDED_LIMIT_MAX: u64 = 100;

/// 夹具逐字错误文案（`endedLimit` 不满足 `>0` 时的 `-32602`）。
const ENDED_LIMIT_TOO_SMALL_MESSAGE: &str =
    "Invalid params — endedLimit: Too small: expected number to be >0";

/// 夹具逐字 `data`（TS zod 的 too_small 详情；线格式事实源，不重排）。
const ENDED_LIMIT_TOO_SMALL_DATA: &str = "[\n  {\n    \"origin\": \"number\",\n    \"code\": \"too_small\",\n    \"minimum\": 0,\n    \"inclusive\": false,\n    \"path\": [\n      \"endedLimit\"\n    ],\n    \"message\": \"Too small: expected number to be >0\"\n  }\n]";

/// `endedLimit > 100` 的 `-32602`：文案与 data 按 schema `.max(100)` 的 zod
/// `too_big` 形状构造（与观察到的 too_small 形状同构；运行时未观察，§4.8 登记）。
const ENDED_LIMIT_TOO_BIG_MESSAGE: &str =
    "Invalid params — endedLimit: Too big: expected number to be <=100";

const ENDED_LIMIT_TOO_BIG_DATA: &str = "[\n  {\n    \"origin\": \"number\",\n    \"code\": \"too_big\",\n    \"maximum\": 100,\n    \"inclusive\": true,\n    \"path\": [\n      \"endedLimit\"\n    ],\n    \"message\": \"Too big: expected number to be <=100\"\n  }\n]";

/// 后台启动输出里 agentId 行的前缀（夹具逐字模板 §4.3 / subagents-background）。
const AGENT_ID_LINE_PREFIX: &str = "agentId:";

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    validate_ended_limit(&params)?;
    let params: SessionSubagentsParams = parse_params(SESSION_SUBAGENTS_METHOD, params)?;
    let store = server.store()?;
    let record = store
        .get_session(&params.session_id)
        .map_err(store_error)?
        .ok_or_else(|| session_not_found(&params.session_id))?;
    let messages = store
        .messages(&record.id, None, None)
        .map_err(store_error)?;

    // §4.1：首 turn 之前（即使 session/read 可见）与缺失会话同样报 not found。
    // turn 输入行都是 role=user（user_prompt / background_notification /
    // system_reminder），timeline 行是 assistant，不混淆。
    if !messages
        .iter()
        .any(|message| message.message.data.get("role").and_then(Value::as_str) == Some("user"))
    {
        return Err(session_not_found(&params.session_id));
    }

    let children = store
        .subagent_child_sessions(&record.id)
        .map_err(store_error)?;
    let notifications = notification_results(&messages);
    let records = agent_tool_records(&messages);

    let child_session_ids: Vec<String> = children
        .iter()
        .enumerate()
        .map(|(index, _)| format!("child-{}", index + 1))
        .collect();

    let mut running: Vec<RunningSubagent> = Vec::new();
    let mut ended: Vec<(usize, EndedSubagent)> = Vec::new();
    for record in &records {
        match classify(record, &notifications) {
            Classification::Running(item) => running.push(item),
            Classification::Ended(item) => ended.push((record.ordinal, item)),
            Classification::Skip => {}
        }
    }

    // §4.4：ended 按 `(endedAt, 创建序)` 倒序（Important 3：keyset 与排序同键，
    // 同刻平局不丢项）；running 保持创建顺序（并行时顺序未观察，取稳定的消息序，
    // §4.7 登记）。
    ended.sort_by(|(left_ordinal, left), (right_ordinal, right)| {
        right
            .ended_at
            .unwrap_or(0)
            .cmp(&left.ended_at.unwrap_or(0))
            .then(right_ordinal.cmp(left_ordinal))
    });

    let revision = projection_revision(&messages);
    let result = paginate(
        revision,
        child_session_ids,
        running,
        ended,
        params.ended_limit,
        params.ended_cursor.as_deref(),
    )?;
    serde_json::to_value(result).map_err(|error| ServerError::internal(error.to_string()))
}

/// `-32004 Session not found: <sessionId>`（夹具的边界文案；注意与 C1 读面的
/// `-32602` 会话不存在不同——subagents 面的观察值是 -32004）。
fn session_not_found(session_id: &str) -> ServerError {
    ServerError::rpc(-32004, format!("Session not found: {session_id}"), None)
}

/// `endedLimit` 的 schema 边界（`.int().positive().max(100)`）先行校验：serde 的
/// u64 解析对 `0` 会成功、对 `-1` 报「invalid type」，都给不出夹具的 `too_small`
/// 文案，所以在这里按数值显式判定。非整数值仍走 serde 的 `-32602`（未观察边界，
/// 文案差异登记 §4.8）。
fn validate_ended_limit(params: &Value) -> Result<(), ServerError> {
    let Some(limit) = params.get("endedLimit") else {
        return Ok(());
    };
    match (limit.as_u64(), limit.as_i64()) {
        // 非负整数：0 违反 `.positive()`，>100 违反 `.max(100)`。
        (Some(value), _) => {
            if value == 0 {
                return Err(too_small_error());
            }
            if value > ENDED_LIMIT_MAX {
                return Err(too_big_error());
            }
        }
        // 负整数：as_u64 解析失败，只有 `.positive()` 违反一种可能。
        (None, Some(value)) if value <= 0 => return Err(too_small_error()),
        // 浮点/非数值：交给 serde 的严格 `-32602`。
        _ => {}
    }
    Ok(())
}

fn too_small_error() -> ServerError {
    // `-32602` + 夹具的 ZodError data（线格式事实源，逐字不重排）。
    ServerError::rpc(
        -32602,
        ENDED_LIMIT_TOO_SMALL_MESSAGE,
        Some(json!({
            "name": "ZodError",
            "message": ENDED_LIMIT_TOO_SMALL_DATA,
        })),
    )
}

fn too_big_error() -> ServerError {
    // schema `.max(100)` 的 zod `too_big` 形状（schema 派生，未观察运行时值）。
    ServerError::rpc(
        -32602,
        ENDED_LIMIT_TOO_BIG_MESSAGE,
        Some(json!({
            "name": "ZodError",
            "message": ENDED_LIMIT_TOO_BIG_DATA,
        })),
    )
}

/// 夹具 message 行序的 Agent tool 记录（创建顺序 = 消息序 + part 序）。
struct AgentToolRecord {
    ordinal: usize,
    call_id: Option<String>,
    subagent_type: String,
    title: String,
    background: bool,
    state_status: String,
    started_at: Option<u64>,
    ended_at: Option<u64>,
    output: Option<String>,
    agent_id: Option<String>,
}

/// 从父会话消息提取 Agent tool part（`tool:"Agent"`）。缺 `state` / `state.status`
/// 的 part 无法分类，跳过（fail-soft）；缺 `state.input` 时必填 title/subagentType
/// 用中性空串（与 `projection.rs` 的取值纪律一致，不编造语义）。
fn agent_tool_records(messages: &[MessageWithParts]) -> Vec<AgentToolRecord> {
    let mut records = Vec::new();
    for message in messages {
        if message.message.data.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        for part in &message.parts {
            let data = &part.data;
            if data.get("type").and_then(Value::as_str) != Some("tool")
                || data.get("tool").and_then(Value::as_str) != Some("Agent")
            {
                continue;
            }
            let Some(state) = data.get("state") else {
                continue;
            };
            let Some(status) = state.get("status").and_then(Value::as_str) else {
                continue;
            };
            let input = state.get("input").cloned().unwrap_or(Value::Null);
            let started_at = state
                .get("time")
                .and_then(|time| time.get("start"))
                .and_then(Value::as_i64)
                .map(epoch_u64)
                .or_else(|| Some(epoch_u64(part.time_created)));
            let ended_at = state
                .get("time")
                .and_then(|time| time.get("end"))
                .and_then(Value::as_i64)
                .map(epoch_u64)
                .or_else(|| Some(epoch_u64(part.time_updated)));
            let output = state
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|text| !text.is_empty());
            let agent_id = output.as_deref().and_then(parse_agent_id);
            records.push(AgentToolRecord {
                ordinal: records.len(),
                call_id: data
                    .get("callID")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                subagent_type: string_input(&input, "subagent_type"),
                title: string_input(&input, "description"),
                background: input.get("run_in_background").and_then(Value::as_bool) == Some(true),
                state_status: status.to_string(),
                started_at,
                ended_at,
                output,
                agent_id,
            });
        }
    }
    records
}

/// tool input 的字符串字段；缺失/非串用中性空串（必填字段不落空值语义）。
fn string_input(input: &Value, key: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 后台完成通知：`background_notification` 消息文本里的 `<task-notification>`
/// 块 → 按 `<task-id>` 取「最后一条」的（status, result, 消息时间）。
///
/// 关联键是夹具记录的两个事实：通知文本 `<task-id>` 与 tool 输出 `agentId:` 行
/// （`agent-c3-contract.md` §4.8）；`session_task_link` 按观察为空表，不读。
type NotificationMap = std::collections::HashMap<String, (String, String, i64)>;

fn notification_results(messages: &[MessageWithParts]) -> NotificationMap {
    let mut map = NotificationMap::new();
    for message in messages {
        let data = &message.message.data;
        let kind = data
            .get("semantics")
            .and_then(|semantics| semantics.get("kind"))
            .and_then(Value::as_str);
        if kind != Some("background_notification") {
            continue;
        }
        let mut text = String::new();
        for part in &message.parts {
            if part.data.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(value) = part.data.get("text").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
        }
        for block in notification_blocks(&text) {
            if let Some(task_id) = tag_value(block, "task-id") {
                let status = tag_value(block, "status").unwrap_or_default();
                let result = tag_value(block, "result").unwrap_or_default();
                map.insert(task_id, (status, result, message.message.time_created));
            }
        }
    }
    map
}

/// 按 `<task-notification>` 切块（多条通知空行分隔在同一个消息文本里）。
fn notification_blocks(text: &str) -> Vec<&str> {
    text.split("<task-notification>").skip(1).collect()
}

/// 取 `<tag>...</tag>` 的内文（两侧空白裁掉）；没有闭合标签返回 `None`。
fn tag_value(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(block[start..end].trim().to_owned())
}

/// 工具输出里的 `agentId: <token>` 行：取该行 `agentId:` 之后、首个空白之前的
/// token（夹具模板 `agentId: agent_<uuid> (use SendMessage …)`）。
fn parse_agent_id(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(AGENT_ID_LINE_PREFIX) {
            let token = rest.split_whitespace().next().unwrap_or_default();
            if !token.is_empty() {
                return Some(token.to_owned());
            }
        }
    }
    None
}

/// 工具输出去掉 `agentId:` 行之后的前缀（夹具 ended 项的 summary = 子代理结果
/// 文本，即该行之前的正文）；没有 `agentId:` 行时整段输出就是正文（corrupt 输出
/// 的 fail-soft 投影）；空正文 → `None`（键省略）。
fn summary_from_output(output: &str) -> Option<String> {
    let mut prefix = String::new();
    let mut saw_agent_line = false;
    for line in output.lines() {
        if line.trim_start().starts_with(AGENT_ID_LINE_PREFIX) {
            saw_agent_line = true;
            break;
        }
        if !prefix.is_empty() {
            prefix.push('\n');
        }
        prefix.push_str(line);
    }
    let text = if saw_agent_line {
        prefix
    } else {
        output.trim_end().to_owned()
    };
    let text = text.trim_end().to_owned();
    (!text.is_empty()).then_some(text)
}

enum Classification {
    Running(RunningSubagent),
    Ended(EndedSubagent),
    /// 状态无法分类（未观察取值）：跳过该 part，不编造列表项（§4.8）。
    Skip,
}

/// 单条 Agent tool 记录 → running/ended 项（§4.8 的分类规则）。
///
/// - pending/running → running（status `running`；waiting/blocked 未观察不产出）；
/// - completed 且 `run_in_background:true`：由 `<task-id>` 匹配的完成通知结算——
///   通知 `<status>completed</status>` → ended `success`（summary 取通知
///   `<result>`，endedAt 取通知消息时间）；无匹配通知 → 仍在 running；
/// - completed（前台）→ ended `success`（summary 取输出 `agentId:` 行前缀）；
/// - error → ended `failed`（无 summary——失败形状未观察，键省略）。
fn classify(record: &AgentToolRecord, notifications: &NotificationMap) -> Classification {
    let agent_id = record.agent_id.clone();
    match record.state_status.as_str() {
        "pending" | "running" => Classification::Running(running_item(record, agent_id)),
        "completed" if record.background => {
            let settled = record
                .agent_id
                .as_deref()
                .and_then(|agent_id| notifications.get(agent_id))
                .filter(|(status, _, _)| status == "completed");
            match settled {
                Some((_, result, notification_time)) => Classification::Ended(ended_item(
                    record,
                    EndedSubagentStatus::Success,
                    (!result.is_empty()).then(|| result.clone()),
                    Some(epoch_u64(*notification_time)),
                    agent_id,
                )),
                None => Classification::Running(running_item(record, agent_id)),
            }
        }
        "completed" => Classification::Ended(ended_item(
            record,
            EndedSubagentStatus::Success,
            record.output.as_deref().and_then(summary_from_output),
            record.ended_at,
            agent_id,
        )),
        // 失败形状未观察（§4.7）：error 是唯一可观察的失败映射源，summary 键省略。
        "error" => Classification::Ended(ended_item(
            record,
            EndedSubagentStatus::Failed,
            None,
            record.ended_at,
            None,
        )),
        // 其余状态（waiting/blocked/cancelled/lost 等未观察取值）无法分类：
        // 跳过（fail-soft，不编造列表项）。
        _ => Classification::Skip,
    }
}

fn running_item(record: &AgentToolRecord, agent_id: Option<String>) -> RunningSubagent {
    RunningSubagent {
        child_session_id: child_alias(record.ordinal),
        agent_id,
        tool_call_id: record.call_id.clone(),
        subagent_type: record.subagent_type.clone(),
        title: record.title.clone(),
        // running 项没有 summary/endedAt（§4.2）；startedAt 用 state.time.start。
        summary: None,
        started_at: record.started_at,
        ended_at: None,
        status: RunningSubagentStatus::Running,
    }
}

#[allow(clippy::too_many_arguments)]
fn ended_item(
    record: &AgentToolRecord,
    status: EndedSubagentStatus,
    summary: Option<String>,
    ended_at: Option<u64>,
    agent_id: Option<String>,
) -> EndedSubagent {
    EndedSubagent {
        child_session_id: child_alias(record.ordinal),
        agent_id,
        tool_call_id: record.call_id.clone(),
        subagent_type: record.subagent_type.clone(),
        title: record.title.clone(),
        summary,
        started_at: record.started_at,
        ended_at,
        status,
    }
}

/// 工具调用创建序 → 夹具别名（`child-1`、`child-2`，§4.4）。
fn child_alias(ordinal: usize) -> String {
    format!("child-{}", ordinal + 1)
}

/// `revision` 的回合推进推导（§4.8）：`2 + startedTurns + completedTurns`。
///
/// 夹具六个观察值（空 4 / running 3 / ended 4 / 后台 mid 3、after 4 / 分页 6）。
/// DB 列不直接携带 revision；**后台通知 turn 不计入两个计数**（夹具 `after` 的
/// revision 是 4，而通知 turn 已持久化；`backgroundSubagentResultConsumed:true`
/// 的自然读法是通知 turn 不推进该投影版本）：
/// - started = role=user 且语义**非** `background_notification` 的行；
/// - completed = `assistant_response` 且 `finish:"stop"`，且其 `anchor.turnId`
///   不属于任何 `background_notification` 输入行的 turn（无 anchor 的行不可归属
///   通知 turn，按普通 turn 计入——fail-soft 回退）。
///
/// 基数 2（create/title 类状态推进）来源未观察，按常数标定并登记 §4.8。
fn projection_revision(messages: &[MessageWithParts]) -> u64 {
    fn anchor_turn_id(data: &Value) -> Option<&str> {
        data.get("anchor")
            .and_then(|anchor| anchor.get("turnId"))
            .and_then(Value::as_str)
    }
    let notification_turns: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|message| {
            let data = &message.message.data;
            let is_notification = data.get("role").and_then(Value::as_str) == Some("user")
                && data
                    .get("semantics")
                    .and_then(|semantics| semantics.get("kind"))
                    .and_then(Value::as_str)
                    == Some("background_notification");
            is_notification.then(|| anchor_turn_id(data)).flatten()
        })
        .collect();
    let mut started = 0_u64;
    let mut completed = 0_u64;
    for message in messages {
        let data = &message.message.data;
        match data.get("role").and_then(Value::as_str) {
            Some("user") => {
                let kind = data
                    .get("semantics")
                    .and_then(|semantics| semantics.get("kind"))
                    .and_then(Value::as_str);
                if kind != Some("background_notification") {
                    started += 1;
                }
            }
            Some("assistant") => {
                let kind = data
                    .get("semantics")
                    .and_then(|semantics| semantics.get("kind"))
                    .and_then(Value::as_str);
                if kind == Some("assistant_response")
                    && data.get("finish").and_then(Value::as_str) == Some("stop")
                {
                    // 通知 turn 自己的 stop 不计（夹具 after：2+1+1=4）。
                    let attributed_to_notification = anchor_turn_id(data)
                        .is_some_and(|turn_id| notification_turns.contains(turn_id));
                    if !attributed_to_notification {
                        completed += 1;
                    }
                }
            }
            _ => {}
        }
    }
    2 + started + completed
}

/// 分页：ended 全集按已排序的 `(创建序, 条目)` 列表截页；游标是不透明
/// base64(JSON `{childSessionId, endedAt}`)。
///
/// keyset 按与排序一致的 `(endedAt, 创建序)` **二元组**取严格排在游标之后的剩余
/// 项：游标条目在当前全集里解析出它此刻的 (endedAt, 创建序)，同刻平局按创建序
/// 排在游标之后的项不会被丢（Important 3；夹具游标只解码
/// `{childSessionId, endedAt}`，位置由当前全集解析——二元组自洽，过期 endedAt
/// 不影响）。
///
/// `nextCursor` 引用**本页最后一项**（夹具第一页返回 child-2 + 游标解码即
/// child-2）；还有剩余项时才有该键，最后一页键省略（§4.1）。
fn paginate(
    revision: u64,
    child_session_ids: Vec<String>,
    running: Vec<RunningSubagent>,
    ended: Vec<(usize, EndedSubagent)>,
    ended_limit: u64,
    ended_cursor: Option<&str>,
) -> Result<SessionSubagentsResult, ServerError> {
    let total = ended.len() as u64;
    let remaining: Vec<&(usize, EndedSubagent)> = match ended_cursor {
        None => ended.iter().collect(),
        Some(cursor) => {
            let decoded = decode_cursor(cursor)?;
            let Some((cursor_ordinal, cursor_item)) = ended
                .iter()
                .find(|(_, item)| item.child_session_id == decoded.child_session_id)
            else {
                // 游标指向的条目不在当前全集（过期/被删）：显式 -32602，不静默
                // 回第一页（未观察边界，§4.8 登记）。
                return Err(ServerError::invalid_params(
                    "session/subagents 参数不合法: endedCursor 不匹配当前列表",
                ));
            };
            let cursor_ended_at = cursor_item.ended_at.unwrap_or(0);
            ended
                .iter()
                .filter(|(ordinal, item)| {
                    let item_ended_at = item.ended_at.unwrap_or(0);
                    item_ended_at < cursor_ended_at
                        || (item_ended_at == cursor_ended_at && *ordinal < *cursor_ordinal)
                })
                .collect()
        }
    };
    let limit = usize::try_from(ended_limit).unwrap_or(usize::MAX);
    let page: Vec<EndedSubagent> = remaining
        .iter()
        .take(limit)
        .map(|(_, item)| (*item).clone())
        .collect();
    let next_cursor = if remaining.len() > page.len() {
        let last = page
            .last()
            .ok_or_else(|| ServerError::internal("分页非空却无条目"))?;
        Some(encode_cursor(&Cursor {
            child_session_id: last.child_session_id.clone(),
            ended_at: last.ended_at.unwrap_or(0),
        }))
    } else {
        None
    };
    Ok(SessionSubagentsResult {
        revision,
        child_session_ids,
        running,
        ended: SessionSubagentsEnded {
            total,
            items: page,
            next_cursor,
        },
    })
}

/// 游标的明文形状（夹具 `cursor.decoded`：`{childSessionId, endedAt}`）。
struct Cursor {
    child_session_id: String,
    ended_at: u64,
}

fn encode_cursor(cursor: &Cursor) -> String {
    STANDARD.encode(
        json!({
            "childSessionId": cursor.child_session_id,
            "endedAt": cursor.ended_at,
        })
        .to_string()
        .as_bytes(),
    )
}

fn decode_cursor(text: &str) -> Result<Cursor, ServerError> {
    let bytes = STANDARD.decode(text.as_bytes()).map_err(|_| {
        ServerError::invalid_params("session/subagents 参数不合法: endedCursor 不是合法 base64")
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        ServerError::invalid_params("session/subagents 参数不合法: endedCursor 不是合法 JSON")
    })?;
    let child_session_id = value
        .get("childSessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ServerError::invalid_params(
                "session/subagents 参数不合法: endedCursor 缺 childSessionId",
            )
        })?
        .to_owned();
    let ended_at = value
        .get("endedAt")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            ServerError::invalid_params("session/subagents 参数不合法: endedCursor 缺 endedAt")
        })?;
    Ok(Cursor {
        child_session_id,
        ended_at,
    })
}
