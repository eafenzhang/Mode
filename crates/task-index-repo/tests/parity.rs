//! 与 TS 的黄金数据对拍：任务索引核心行 + 列表/搜索。
//!
//! fixture 由 `pnpm task-index:fixtures` 用真实 TS `TaskIndexRepo` 生成；
//! Rust 侧在临时目录重建同一 DB 并逐步重放。比较口径：
//! - 方法返回 JSON 深度相等；
//! - `tasks` 关键列原值；`meta_json` 比较解析后的 JSON（属性顺序不是语义）；
//! - 分组表快照的时间列由 fixture 归一为 0。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use task_index_repo::{
    parse_last_error_value, parse_mode_task_meta, parse_task_goal_value, AgentPatch,
    ArchiveStaleTasksParams, GroupedTaskGroupRefs, GroupedTaskRef, GroupedTaskViewOrderInput,
    GroupedTaskViewQuery, GroupedTaskViewTopLevelNodeRef, ListDeletedTaskIdsParams,
    ListTaskMetasParams, ModeTaskMeta, SyncTaskMetaParams, TaskIndexRepo, TaskIndexStatePatch,
    TaskListKind, TaskListQuery, TaskListSortBy, TaskListWorkspaceScope, TaskRef,
};
use tasks_db::RunOptions;

const FIXTURE: &str = include_str!("../fixtures/task-index.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture 必须是合法 JSON")
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} 必须是字符串"))
        .to_owned()
}

fn optional_string_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn int_at(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("{key} 必须是整数"))
}

fn optional_int_at(value: &Value, key: &str) -> Option<i64> {
    match value.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => value.as_i64(),
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> TempDir {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "task-index-parity-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建临时目录");
    TempDir(path)
}

fn seed_database(connection: &Connection, seeds: &Value) {
    let mut insert_task = connection
        .prepare(
            "INSERT INTO tasks (
               workspace_key, workspace_path, workspace_identity, task_id, title, task_status,
               provider, mode, model, migration_source, forked_from_task_id, cron_automation_id,
               off_peak_task_id, created_at, updated_at, unread_at, last_unread_at, pinned,
               archived, deleted, title_overridden, searchable_text, meta_json
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
               ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
             )",
        )
        .expect("prepare tasks insert");
    for row in seeds["tasks"].as_array().expect("tasks seeds") {
        insert_task
            .execute(params![
                string_at(row, "workspaceKey"),
                string_at(row, "workspacePath"),
                optional_string_at(row, "workspaceIdentity"),
                string_at(row, "taskId"),
                string_at(row, "title"),
                optional_string_at(row, "taskStatus"),
                optional_string_at(row, "provider"),
                string_at(row, "mode"),
                optional_string_at(row, "model"),
                optional_string_at(row, "migrationSource"),
                optional_string_at(row, "forkedFromTaskId"),
                optional_string_at(row, "cronAutomationId"),
                optional_string_at(row, "offPeakTaskId"),
                int_at(row, "createdAt"),
                int_at(row, "updatedAt"),
                optional_int_at(row, "unreadAt"),
                int_at(row, "lastUnreadAt"),
                int_at(row, "pinned"),
                int_at(row, "archived"),
                int_at(row, "deleted"),
                int_at(row, "titleOverridden"),
                string_at(row, "searchableText"),
                string_at(row, "metaJson"),
            ])
            .expect("insert task seed");
    }

    let mut insert_off_peak = connection
        .prepare(
            "INSERT INTO off_peak_tasks (
               off_peak_task_id, title, session_id, prompt, permission_mode,
               workspace_key, workspace_path, status, queued_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .expect("prepare off_peak insert");
    for row in seeds["offPeakTasks"].as_array().expect("off_peak seeds") {
        insert_off_peak
            .execute(params![
                string_at(row, "off_peak_task_id"),
                string_at(row, "title"),
                optional_string_at(row, "session_id"),
                string_at(row, "prompt"),
                string_at(row, "permission_mode"),
                string_at(row, "workspace_key"),
                string_at(row, "workspace_path"),
                string_at(row, "status"),
                int_at(row, "queued_at"),
                int_at(row, "created_at"),
                int_at(row, "updated_at"),
            ])
            .expect("insert off_peak seed");
    }

    let mut insert_group = connection
        .prepare(
            "INSERT INTO task_groups (group_id, title, color, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .expect("prepare group insert");
    for row in seeds["taskGroups"].as_array().expect("group seeds") {
        insert_group
            .execute(params![
                string_at(row, "group_id"),
                string_at(row, "title"),
                string_at(row, "color"),
                int_at(row, "created_at"),
                int_at(row, "updated_at"),
            ])
            .expect("insert group seed");
    }

    let mut insert_member = connection
        .prepare(
            "INSERT INTO task_group_members (
               group_id, workspace_key, workspace_path, workspace_identity, task_id,
               sort_order, added_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .expect("prepare member insert");
    for row in seeds["taskGroupMembers"].as_array().expect("member seeds") {
        insert_member
            .execute(params![
                string_at(row, "group_id"),
                string_at(row, "workspace_key"),
                string_at(row, "workspace_path"),
                optional_string_at(row, "workspace_identity"),
                string_at(row, "task_id"),
                optional_int_at(row, "sort_order"),
                int_at(row, "added_at"),
                int_at(row, "created_at"),
                int_at(row, "updated_at"),
            ])
            .expect("insert member seed");
    }

    let mut insert_order = connection
        .prepare(
            "INSERT INTO task_group_view_node_orders
               (node_type, node_key, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .expect("prepare order insert");
    for row in seeds["taskGroupViewNodeOrders"]
        .as_array()
        .expect("order seeds")
    {
        insert_order
            .execute(params![
                string_at(row, "node_type"),
                string_at(row, "node_key"),
                int_at(row, "sort_order"),
                int_at(row, "created_at"),
                int_at(row, "updated_at"),
            ])
            .expect("insert order seed");
    }
}

fn meta_json_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({ "unparsable": raw }))
}

fn snapshot(db_path: &Path) -> Value {
    let connection = Connection::open(db_path).expect("打开快照连接");
    let tasks = {
        let mut statement = connection
            .prepare(
                "SELECT workspace_key, workspace_path, workspace_identity, task_id, title,
                        task_status, provider, mode, model, migration_source, forked_from_task_id,
                        cron_automation_id, off_peak_task_id, created_at, updated_at, unread_at,
                        last_unread_at, pinned, archived, deleted, title_overridden,
                        searchable_text, meta_json
                 FROM tasks ORDER BY workspace_key, task_id",
            )
            .expect("prepare tasks snapshot");
        statement
            .query_map([], |row| {
                let raw_meta: String = row.get(22)?;
                let mut object = Map::new();
                let keys = [
                    "workspace_key",
                    "workspace_path",
                    "workspace_identity",
                    "task_id",
                    "title",
                    "task_status",
                    "provider",
                    "mode",
                    "model",
                    "migration_source",
                    "forked_from_task_id",
                    "cron_automation_id",
                    "off_peak_task_id",
                    "created_at",
                    "updated_at",
                    "unread_at",
                    "last_unread_at",
                    "pinned",
                    "archived",
                    "deleted",
                    "title_overridden",
                    "searchable_text",
                ];
                for (index, key) in keys.iter().enumerate() {
                    let value: rusqlite::types::Value = row.get(index)?;
                    object.insert((*key).to_owned(), sql_to_json(value));
                }
                object.insert("meta_json".to_owned(), meta_json_value(&raw_meta));
                Ok(Value::Object(object))
            })
            .expect("query tasks snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect tasks snapshot")
    };

    let task_groups = {
        let mut statement = connection
            .prepare("SELECT group_id, title, color, created_at, updated_at FROM task_groups ORDER BY group_id")
            .expect("prepare group snapshot");
        let mut rows = statement
            .query_map([], |row| {
                let group_id: String = row.get(0)?;
                Ok(json!({
                    "group_id": normalize_group_id_json(&group_id),
                    "title": row.get::<_, String>(1)?,
                    "color": row.get::<_, String>(2)?,
                    "created_at": 0,
                    "updated_at": 0,
                }))
            })
            .expect("query group snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect group snapshot");
        // 归一生效后才出现重复 id（多个 createTaskGroup），按 (id,title) 补稳定序。
        rows.sort_by(|left, right| {
            (
                left["group_id"].as_str(),
                left["title"]
                    .as_str()
                    .map(str::to_lowercase)
                    .unwrap_or_default(),
            )
                .cmp(&(
                    right["group_id"].as_str(),
                    right["title"]
                        .as_str()
                        .map(str::to_lowercase)
                        .unwrap_or_default(),
                ))
        });
        rows
    };

    let task_group_members = {
        let mut statement = connection
            .prepare(
                "SELECT group_id, workspace_key, workspace_path, workspace_identity, task_id,
                        sort_order
                 FROM task_group_members ORDER BY workspace_key, task_id",
            )
            .expect("prepare member snapshot");
        statement
            .query_map([], |row| {
                let group_id: String = row.get(0)?;
                Ok(json!({
                    "group_id": normalize_group_id_json(&group_id),
                    "workspace_key": row.get::<_, String>(1)?,
                    "workspace_path": row.get::<_, String>(2)?,
                    "workspace_identity": row.get::<_, Option<String>>(3)?,
                    "task_id": row.get::<_, String>(4)?,
                    "sort_order": row.get::<_, Option<i64>>(5)?,
                    "added_at": 0,
                    "created_at": 0,
                    "updated_at": 0,
                }))
            })
            .expect("query member snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect member snapshot")
    };

    let task_group_view_node_orders = {
        let mut statement = connection
            .prepare(
                "SELECT node_type, node_key, sort_order
                 FROM task_group_view_node_orders ORDER BY node_type, node_key",
            )
            .expect("prepare order snapshot");
        let mut rows = statement
            .query_map([], |row| {
                let node_type: String = row.get(0)?;
                let node_key: String = row.get(1)?;
                let node_key = if node_type == "group" {
                    normalize_group_id_json(&node_key)
                } else {
                    json!(node_key)
                };
                Ok(json!({
                    "node_type": node_type,
                    "node_key": node_key,
                    "sort_order": row.get::<_, i64>(2)?,
                    "created_at": 0,
                    "updated_at": 0,
                }))
            })
            .expect("query order snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect order snapshot");
        rows.sort_by(|left, right| {
            (
                left["node_type"].as_str(),
                left["node_key"].as_str(),
                left["sort_order"].as_i64(),
            )
                .cmp(&(
                    right["node_type"].as_str(),
                    right["node_key"].as_str(),
                    right["sort_order"].as_i64(),
                ))
        });
        rows
    };

    let task_group_workspace_bootstraps = {
        let mut statement = connection
            .prepare(
                "SELECT workspace_key, group_id, created_at, updated_at
                 FROM task_group_workspace_bootstraps
                 ORDER BY workspace_key, group_id",
            )
            .expect("prepare bootstrap snapshot");
        statement
            .query_map([], |row| {
                Ok(json!({
                    "workspace_key": row.get::<_, String>(0)?,
                    "group_id": row.get::<_, Option<String>>(1)?,
                    "created_at": 0,
                    "updated_at": 0,
                }))
            })
            .expect("query bootstrap snapshot")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect bootstrap snapshot")
    };

    json!({
        "tasks": tasks,
        "taskGroups": task_groups,
        "taskGroupMembers": task_group_members,
        "taskGroupViewNodeOrders": task_group_view_node_orders,
        "taskGroupWorkspaceBootstraps": task_group_workspace_bootstraps,
    })
}

fn sql_to_json(value: rusqlite::types::Value) -> Value {
    match value {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(value) => json!(value),
        rusqlite::types::Value::Real(value) => json!(value),
        rusqlite::types::Value::Text(value) => json!(value),
        rusqlite::types::Value::Blob(value) => json!(value),
    }
}

fn normalize_group_id_json(value: &str) -> Value {
    if value.starts_with("task-group-") {
        json!("task-group-normalized")
    } else {
        json!(value)
    }
}

fn normalize_group_value(value: &Value) -> Value {
    let mut object = value.as_object().cloned().unwrap_or_default();
    if let Some(id) = object.get("id").and_then(Value::as_str) {
        object.insert("id".to_owned(), normalize_group_id_json(id));
    }
    object.insert("createdAt".to_owned(), json!(0));
    object.insert("updatedAt".to_owned(), json!(0));
    Value::Object(object)
}

fn normalize_view_output(value: Value) -> Value {
    let mut object = match value {
        Value::Object(object) => object,
        other => return other,
    };
    let Some(Value::Array(nodes)) = object.get("nodes").cloned() else {
        return Value::Object(object);
    };
    let nodes = nodes
        .into_iter()
        .map(|node| {
            let mut node = match node {
                Value::Object(node) => node,
                other => return other,
            };
            if node.get("type").and_then(Value::as_str) == Some("group") {
                if let Some(group) = node.get("group").cloned() {
                    node.insert("group".to_owned(), normalize_group_value(&group));
                }
            }
            Value::Object(node)
        })
        .collect();
    object.insert("nodes".to_owned(), Value::Array(nodes));
    Value::Object(object)
}

fn normalize_structure_output(value: Value) -> Value {
    let mut object = match value {
        Value::Object(object) => object,
        other => return other,
    };
    if let Some(Value::Array(groups)) = object.get("groups").cloned() {
        object.insert(
            "groups".to_owned(),
            Value::Array(groups.iter().map(normalize_group_value).collect()),
        );
    }
    if let Some(Value::Array(members)) = object.get("members").cloned() {
        let members = members
            .into_iter()
            .map(|member| {
                let mut member = match member {
                    Value::Object(member) => member,
                    other => return other,
                };
                member.insert("addedAt".to_owned(), json!(0));
                Value::Object(member)
            })
            .collect();
        object.insert("members".to_owned(), Value::Array(members));
    }
    // 顶层排序里的用户分组 id 是随机 UUID；与 generator 一样归一，才能跨进程比较。
    if let Some(Value::Array(orders)) = object.get("topLevelOrders").cloned() {
        let orders = orders
            .into_iter()
            .map(|order| {
                let mut order = match order {
                    Value::Object(order) => order,
                    other => return other,
                };
                if order.get("type").and_then(Value::as_str) == Some("group") {
                    if let Some(group_id) = order.get("groupId").and_then(Value::as_str) {
                        order.insert("groupId".to_owned(), normalize_group_id_json(group_id));
                    }
                }
                Value::Object(order)
            })
            .collect();
        object.insert("topLevelOrders".to_owned(), Value::Array(orders));
    }
    Value::Object(object)
}

fn normalize_dispatch_output(op: &str, value: Value) -> Value {
    match op {
        "queryGroupedTaskView" | "applyGroupedTaskViewOrder" => normalize_view_output(value),
        "queryGroupedTaskViewStructure" => normalize_structure_output(value),
        "createTaskGroup" | "renameTaskGroup" | "updateTaskGroupColor" => {
            normalize_group_value(&value)
        }
        _ => value,
    }
}

fn workspace_scopes(input: &Value) -> Vec<TaskListWorkspaceScope> {
    input
        .as_array()
        .map(|scopes| {
            scopes
                .iter()
                .map(|scope| TaskListWorkspaceScope {
                    workspace_path: string_at(scope, "workspacePath"),
                    workspace_identity: optional_string_at(scope, "workspaceIdentity"),
                    workspace_purpose: optional_string_at(scope, "workspacePurpose"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn grouped_ref(input: &Value) -> GroupedTaskRef {
    GroupedTaskRef {
        workspace_path: string_at(input, "workspacePath"),
        workspace_identity: optional_string_at(input, "workspaceIdentity"),
        task_id: string_at(input, "taskId"),
    }
}

fn grouped_view_query(input: &Value) -> GroupedTaskViewQuery {
    GroupedTaskViewQuery {
        workspace_scopes: workspace_scopes(&input["workspaceScopes"]),
        include_all_workspaces: input
            .get("includeAllWorkspaces")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        provider: optional_string_at(input, "provider"),
    }
}

fn grouped_order_input(input: &Value) -> GroupedTaskViewOrderInput {
    let top_level_nodes = input["topLevelNodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| match node.get("type").and_then(Value::as_str) {
                    Some("group") => GroupedTaskViewTopLevelNodeRef::Group {
                        group_id: string_at(node, "groupId"),
                    },
                    _ => GroupedTaskViewTopLevelNodeRef::Task {
                        task: grouped_ref(&node["task"]),
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    let groups = input["groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| GroupedTaskGroupRefs {
                    group_id: string_at(group, "groupId"),
                    task_refs: group["taskRefs"]
                        .as_array()
                        .map(|refs| refs.iter().map(grouped_ref).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    GroupedTaskViewOrderInput {
        workspace_scopes: workspace_scopes(&input["workspaceScopes"]),
        top_level_nodes,
        groups,
        provider: optional_string_at(input, "provider"),
    }
}

fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    match (expected, actual) {
        (Value::Object(left), Value::Object(right)) => {
            for key in left.keys().chain(right.keys()) {
                let next = format!("{path}/{key}");
                match (left.get(key), right.get(key)) {
                    (Some(l), Some(r)) => {
                        if let Some(difference) = first_difference(l, r, &next) {
                            return Some(difference);
                        }
                    }
                    (l, r) => {
                        return Some(format!(
                            "{next}: ts={l:?} rust={r:?}"
                        ))
                    }
                }
            }
            None
        }
        (Value::Array(left), Value::Array(right)) => {
            for index in 0..left.len().min(right.len()) {
                let next = format!("{path}[{index}]");
                if let Some(difference) = first_difference(&left[index], &right[index], &next) {
                    return Some(difference);
                }
            }
            if left.len() != right.len() {
                Some(format!(
                    "{path}: length ts={} rust={}",
                    left.len(),
                    right.len()
                ))
            } else {
                None
            }
        }
        (left, right) if left == right => None,
        (left, right) => Some(format!("{path}: ts={left:?} rust={right:?}")),
    }
}

fn task_ref(input: &Value) -> TaskRef {
    TaskRef {
        workspace_path: string_at(input, "workspacePath"),
        workspace_identity: optional_string_at(input, "workspaceIdentity"),
        task_id: string_at(input, "taskId"),
    }
}

fn parse_meta(input: &Value, key: &str) -> ModeTaskMeta {
    parse_mode_task_meta(&input[key]).expect("fixture meta 必须能被 schema 解析")
}

fn list_metas_params(input: &Value) -> ListTaskMetasParams {
    ListTaskMetasParams {
        workspace_path: optional_string_at(input, "workspacePath"),
        workspace_identity: optional_string_at(input, "workspaceIdentity"),
        provider: optional_string_at(input, "provider"),
        pinned: input.get("pinned").and_then(Value::as_bool),
        archived: input.get("archived").and_then(Value::as_bool),
        include_deleted: input
            .get("includeDeleted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn list_query(input: &Value) -> TaskListQuery {
    let kind = match input.get("kind").and_then(Value::as_str) {
        Some("pinned") => TaskListKind::Pinned,
        Some("archived") => TaskListKind::Archived,
        Some("timeline") => TaskListKind::Timeline,
        Some("active") => TaskListKind::Active,
        other => panic!("未知 kind: {other:?}"),
    };
    let sort_by = match input.get("sortBy").and_then(Value::as_str) {
        Some("created") => TaskListSortBy::Created,
        _ => TaskListSortBy::Updated,
    };
    let workspace_scopes = input["workspaceScopes"]
        .as_array()
        .expect("workspaceScopes")
        .iter()
        .map(|scope| TaskListWorkspaceScope {
            workspace_path: string_at(scope, "workspacePath"),
            workspace_identity: optional_string_at(scope, "workspaceIdentity"),
            workspace_purpose: optional_string_at(scope, "workspacePurpose"),
        })
        .collect();
    TaskListQuery {
        kind,
        workspace_scopes,
        sort_by,
        search: optional_string_at(input, "search"),
        provider: optional_string_at(input, "provider"),
        limit: input.get("limit").and_then(Value::as_f64),
    }
}

fn patch_unread(input: &Value) -> Option<Option<i64>> {
    match input.get("unreadAt") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(value.as_i64().expect("unreadAt 必须是数字"))),
    }
}

fn patch_last_error(input: &Value) -> Option<Option<task_index_repo::ModeTaskLastError>> {
    match input.get("lastError") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(
            parse_last_error_value(value).expect("lastError 必须合法"),
        )),
    }
}

fn patch_target(input: &Value) -> Option<Option<task_index_repo::ModeTaskGoal>> {
    match input.get("target") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(parse_task_goal_value(value).expect("target 必须合法"))),
    }
}

fn state_patch(input: &Value) -> TaskIndexStatePatch {
    TaskIndexStatePatch {
        pinned: input.get("pinned").and_then(Value::as_bool),
        archived: input.get("archived").and_then(Value::as_bool),
        deleted: input.get("deleted").and_then(Value::as_bool),
        title: optional_string_at(input, "title"),
        title_overridden: input.get("titleOverridden").and_then(Value::as_bool),
        unread_at: patch_unread(input),
        model: optional_string_at(input, "model"),
        status: optional_string_at(input, "status"),
        last_error: patch_last_error(input),
        target: patch_target(input),
        updated_at: optional_int_at(input, "updatedAt"),
    }
}

fn agent_patch(input: &Value) -> AgentPatch {
    AgentPatch {
        title: optional_string_at(input, "title"),
        status: optional_string_at(input, "status"),
        last_error: patch_last_error(input),
        target: patch_target(input),
        updated_at: optional_int_at(input, "updatedAt"),
    }
}

fn dispatch(
    repo: &TaskIndexRepo,
    op: &str,
    input: &Value,
) -> task_index_repo::Result<Value> {
    Ok(match op {
        "listTaskMetas" => serde_json::to_value(repo.list_task_metas(&list_metas_params(input))?)?,
        "getTaskMeta" => serde_json::to_value(repo.get_task_meta(&task_ref(input))?)?,
        "listDeletedTaskIds" => serde_json::to_value(repo.list_deleted_task_ids(
            &ListDeletedTaskIdsParams {
                workspace_path: string_at(input, "workspacePath"),
                workspace_identity: optional_string_at(input, "workspaceIdentity"),
                provider: optional_string_at(input, "provider"),
            },
        )?)?,
        "listSessionsByAutomation" => {
            serde_json::to_value(repo.list_sessions_by_automation(&string_at(input, "automationId"))?)?
        }
        "queryTaskList" => serde_json::to_value(repo.query_task_list(&list_query(input))?)?,
        "seedTaskMetaIfMissing" => {
            serde_json::to_value(repo.seed_task_meta_if_missing(&parse_meta(input, "meta"))?)?
        }
        "syncTaskMeta" => serde_json::to_value(repo.sync_task_meta(SyncTaskMetaParams {
            meta: parse_meta(input, "meta"),
            pinned: input.get("pinned").and_then(Value::as_bool),
            archived: input.get("archived").and_then(Value::as_bool),
            deleted: input.get("deleted").and_then(Value::as_bool),
            title_overridden: input.get("titleOverridden").and_then(Value::as_bool),
            searchable_text: optional_string_at(input, "searchableText"),
        })?)?,
        "clearTaskUnreadIfMatches" => serde_json::to_value(
            repo.clear_task_unread_if_matches(&task_ref(input), int_at(input, "expectedUnreadAt"))?,
        )?,
        "deleteArchivedTask" => serde_json::to_value(repo.delete_archived_task(&task_ref(input))?)?,
        "updateTaskState" => serde_json::to_value(
            repo.update_task_state(&task_ref(input), &state_patch(&input["patch"]))?,
        )?,
        "applyAgentPatch" => serde_json::to_value(
            repo.apply_agent_patch(&task_ref(input), &agent_patch(&input["patch"]))?,
        )?,
        "archiveStaleTasks" => serde_json::to_value(repo.archive_stale_tasks(
            &ArchiveStaleTasksParams {
                workspace_path: string_at(input, "workspacePath"),
                workspace_identity: optional_string_at(input, "workspaceIdentity"),
                older_than_days: input
                    .get("olderThanDays")
                    .and_then(Value::as_f64)
                    .expect("olderThanDays"),
                provider: optional_string_at(input, "provider"),
            },
        )?)?,
        "hasGroupedWorkspaceBootstrapRun" => {
            serde_json::to_value(repo.has_grouped_workspace_bootstrap_run()?)?
        }
        "queryGroupedTaskViewStructure" => serde_json::to_value(
            repo.query_grouped_task_view_structure(&workspace_scopes(&input["workspaceScopes"]))?,
        )?,
        "queryGroupedTaskView" => {
            serde_json::to_value(repo.query_grouped_task_view(&grouped_view_query(input))?)?
        }
        "createTaskGroup" => serde_json::to_value(repo.create_task_group(
            input.get("title").and_then(Value::as_str),
            input.get("color").and_then(Value::as_str),
        )?)?,
        "renameTaskGroup" => serde_json::to_value(repo.rename_task_group(
            &string_at(input, "groupId"),
            &string_at(input, "title"),
        )?)?,
        "updateTaskGroupColor" => serde_json::to_value(repo.update_task_group_color(
            &string_at(input, "groupId"),
            &string_at(input, "color"),
        )?)?,
        "deleteTaskGroup" => {
            repo.delete_task_group(&string_at(input, "groupId"))?;
            Value::Null
        }
        "initializeGroupedTaskAtTop" => {
            Value::Bool(repo.initialize_grouped_task_at_top(&grouped_ref(input))?)
        }
        "applyGroupedTaskViewOrder" => serde_json::to_value(
            repo.apply_grouped_task_view_order(&grouped_order_input(input))?,
        )?,
        "syncTaskMetaAtGroupedTop" => serde_json::to_value(repo.sync_task_meta_at_grouped_top(
            SyncTaskMetaParams {
                meta: parse_meta(input, "meta"),
                pinned: input.get("pinned").and_then(Value::as_bool),
                archived: input.get("archived").and_then(Value::as_bool),
                deleted: input.get("deleted").and_then(Value::as_bool),
                title_overridden: input.get("titleOverridden").and_then(Value::as_bool),
                searchable_text: optional_string_at(input, "searchableText"),
            },
        )?)?,
        "close" => {
            repo.close();
            Value::Null
        }
        other => panic!("未知 op: {other}"),
    })
}

fn run_steps(repo: &TaskIndexRepo, db_path: &Path, steps: &[Value]) {
    assert!(!steps.is_empty(), "fixture 不能为空");
    for step in steps {
        let id = step["id"].as_str().expect("step id");
        let op = step["op"].as_str().expect("step op");
        let result = dispatch(repo, op, &step["input"]);
        match &step["error"] {
            Value::String(expected) => {
                let error = match result {
                    Ok(_) => panic!("{id}: 期望错误但成功了"),
                    Err(error) => error,
                };
                assert_eq!(
                    error.to_string(),
                    *expected,
                    "{id}: 错误文案必须与 TS 一致"
                );
            }
            _ => {
                let output = result.unwrap_or_else(|error| panic!("{id}: 意外错误 {error}"));
                let output = normalize_dispatch_output(op, output);
                assert_eq!(
                    output, step["output"],
                    "{id}: 方法返回值必须与 TS 深度相等"
                );
            }
        }
        let actual = snapshot(db_path);
        if actual != step["snapshot"] {
            let detail = first_difference(&step["snapshot"], &actual, "")
                .unwrap_or_else(|| "（无差异定位）".to_owned());
            panic!("{id}: DB 快照必须与 TS 一致\n{detail}");
        }
    }
}

#[test]
fn ts_fixture_steps_match() {
    let fixture = fixture();
    let temp = temp_dir();
    let db_path = temp.0.join("tasks-index.sqlite");

    {
        let connection = Connection::open(&db_path).expect("打开 seed 库");
        tasks_db::run_migrations(&connection, RunOptions::default()).expect("迁移");
        seed_database(&connection, &fixture["seeds"]);
    }

    let clock = fixture["clockMs"].as_i64().expect("clockMs");
    let repo = TaskIndexRepo::with_clock(&db_path, move || clock);

    run_steps(
        &repo,
        &db_path,
        fixture["steps"].as_array().expect("steps"),
    );
    repo.close();
}

#[test]
fn empty_library_fixture_steps_match() {
    let fixture = fixture();
    let section = &fixture["emptyLibrary"];
    let temp = temp_dir();
    let db_path = temp.0.join("tasks-index-empty.sqlite");

    {
        let connection = Connection::open(&db_path).expect("打开空库");
        tasks_db::run_migrations(&connection, RunOptions::default()).expect("迁移");
    }

    let clock = section["clockMs"].as_i64().expect("clockMs");
    let repo = TaskIndexRepo::with_clock(&db_path, move || clock);

    run_steps(&repo, &db_path, section["steps"].as_array().expect("steps"));
    repo.close();
}
