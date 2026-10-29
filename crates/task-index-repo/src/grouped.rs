//! 任务索引仓库：分组视图与分组管理。
//!
//! 行为真源：`packages/services/src/session/taskIndexRepo.ts` 中
//! `hasGroupedWorkspaceBootstrapRun` / `createTaskGroup` / `renameTaskGroup` /
//! `updateTaskGroupColor` / `deleteTaskGroup` / `initializeGroupedTaskAtTop` /
//! `queryGroupedTaskView` / `queryGroupedTaskViewStructure` / `applyGroupedTaskViewOrder` /
//! `syncTaskMetaAtGroupedTop`。规范见 `docs/specs/task-index-repo.md`。
//!
//! 与 TS 的排序口径一致：所有 `localeCompare` 在 Rust 侧退化为码点序
//!（涉及值均为 ASCII，实际结果相同）。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{Result, TaskIndexRepoError};
use crate::js::{js_trim, resolve_workspace_key};
use crate::meta::CRON_DEFAULT_GROUP_ID;
use crate::repo::{
    normalize_workspace_keys, row_to_task_list_item, TaskIndexRow, TaskIndexRepo, TaskListItem,
    TaskListWorkspaceScope, GROUPED_TASK_ORDER_STEP,
};

const GROUPED_WORKSPACE_BOOTSTRAP_ONCE_KEY: &str = "__mode_internal_grouped_workspace_bootstrap_once__";
const DEFAULT_TASK_GROUP_COLOR: &str = "gray";
const WORKSPACE_BOOTSTRAP_TASK_GROUP_COLORS: [&str; 6] =
    ["red", "orange", "yellow", "green", "blue", "purple"];

// ── 公开类型 ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroup {
    pub id: String,
    pub title: String,
    pub color: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct GroupedTaskRef {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub task_id: String,
}

#[derive(Clone, Debug)]
pub enum GroupedTaskViewTopLevelNodeRef {
    Group { group_id: String },
    Task { task: GroupedTaskRef },
}

#[derive(Clone, Debug)]
pub struct GroupedTaskGroupRefs {
    pub group_id: String,
    pub task_refs: Vec<GroupedTaskRef>,
}

#[derive(Clone, Debug, Default)]
pub struct GroupedTaskViewQuery {
    pub workspace_scopes: Vec<TaskListWorkspaceScope>,
    pub include_all_workspaces: bool,
    pub provider: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GroupedTaskViewOrderInput {
    pub workspace_scopes: Vec<TaskListWorkspaceScope>,
    pub top_level_nodes: Vec<GroupedTaskViewTopLevelNodeRef>,
    pub groups: Vec<GroupedTaskGroupRefs>,
    pub provider: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GroupedTaskViewNode {
    Group {
        group: TaskGroup,
        tasks: Vec<TaskListItem>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sort_order: Option<i64>,
    },
    Task {
        task: Box<TaskListItem>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sort_order: Option<i64>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroupedTaskView {
    pub nodes: Vec<GroupedTaskViewNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupedTaskViewStructureMember {
    pub group_id: String,
    pub workspace_key: String,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    pub task_id: String,
    pub sort_order: Option<i64>,
    pub added_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GroupedTaskViewStructureTopOrder {
    Group { group_id: String, sort_order: i64 },
    Task {
        workspace_key: String,
        task_id: String,
        sort_order: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupedTaskViewStructure {
    pub groups: Vec<TaskGroup>,
    pub members: Vec<GroupedTaskViewStructureMember>,
    pub top_level_orders: Vec<GroupedTaskViewStructureTopOrder>,
}

// ── 内部行结构 ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TaskGroupRow {
    group_id: String,
    title: String,
    color: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Clone, Debug)]
struct TaskGroupMemberRow {
    group_id: String,
    workspace_key: String,
    workspace_path: String,
    workspace_identity: Option<String>,
    task_id: String,
    sort_order: Option<i64>,
    added_at: i64,
}

#[derive(Clone, Debug)]
struct NodeOrderRow {
    node_type: String,
    node_key: String,
    sort_order: i64,
}

#[derive(Clone, Debug)]
struct WorkspaceBootstrapScope {
    workspace_key: String,
    workspace_path: String,
}

fn read_group_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskGroupRow> {
    Ok(TaskGroupRow {
        group_id: row.get(0)?,
        title: row.get(1)?,
        color: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn read_member_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskGroupMemberRow> {
    Ok(TaskGroupMemberRow {
        group_id: row.get(0)?,
        workspace_key: row.get(1)?,
        workspace_path: row.get(2)?,
        workspace_identity: row.get(3)?,
        task_id: row.get(4)?,
        sort_order: row.get(5)?,
        added_at: row.get(6)?,
    })
}

/// grouped 视图的 activity 查询列。
///
/// 与 `ROW_COLUMNS` 不同：TS `queryGroupedTaskView` 的 SELECT 不读
/// `off_peak_task_id` / `last_unread_at`，因此 `rowToMeta` 只能从 `meta_json`
/// 取 off-peak 身份（列兜底在这里不存在）。为保持逐字节行为一致，Rust 也按这份
/// 列清单构造行，`off_peak_task_id` 恒为 None。
const GROUPED_TASK_COLUMNS: &str = "workspace_key, workspace_path, workspace_identity, task_id, \
title, task_status, provider, mode, model, migration_source, forked_from_task_id, \
cron_automation_id, created_at, updated_at, unread_at, pinned, archived, deleted, \
title_overridden, searchable_text, meta_json";

fn read_grouped_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskIndexRow> {
    Ok(TaskIndexRow {
        workspace_key: row.get(0)?,
        workspace_path: row.get(1)?,
        workspace_identity: row.get(2)?,
        task_id: row.get(3)?,
        title: row.get(4)?,
        task_status: row.get(5)?,
        provider: row.get(6)?,
        mode: row.get(7)?,
        model: row.get(8)?,
        migration_source: row.get(9)?,
        forked_from_task_id: row.get(10)?,
        cron_automation_id: row.get(11)?,
        off_peak_task_id: None,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        unread_at: row.get(14)?,
        last_unread_at: 0,
        pinned: row.get(15)?,
        archived: row.get(16)?,
        deleted: row.get(17)?,
        title_overridden: row.get(18)?,
        searchable_text: row.get(19)?,
        meta_json: row.get(20)?,
    })
}

// ── 纯函数辅助 ────────────────────────────────────────────────────────────

fn is_task_group_color(value: &str) -> bool {
    matches!(
        value,
        "gray" | "red" | "orange" | "yellow" | "green" | "blue" | "purple"
    )
}

fn row_to_task_group(row: &TaskGroupRow) -> TaskGroup {
    TaskGroup {
        id: row.group_id.clone(),
        title: row.title.clone(),
        color: if is_task_group_color(&row.color) {
            row.color.clone()
        } else {
            DEFAULT_TASK_GROUP_COLOR.to_owned()
        },
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn workspace_group_id(workspace_key: &str) -> String {
    let hash = sha256_hex(workspace_key);
    format!("workspace-group-{}", &hash[..24])
}

fn workspace_group_title(workspace_path: &str) -> String {
    let normalized = workspace_path.trim_end_matches(['\\', '/']);
    let leaf = normalized
        .split(['\\', '/'])
        .rfind(|segment| !segment.is_empty());
    if let Some(leaf) = leaf {
        let trimmed = js_trim(leaf);
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    let trimmed = js_trim(normalized);
    if trimmed.is_empty() {
        "Workspace".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn workspace_group_color(workspace_key: &str) -> &'static str {
    let digest = Sha256::digest(workspace_key.as_bytes());
    let index = usize::from(digest[0]) % WORKSPACE_BOOTSTRAP_TASK_GROUP_COLORS.len();
    WORKSPACE_BOOTSTRAP_TASK_GROUP_COLORS[index]
}

fn normalize_workspace_bootstrap_scopes(
    scopes: &[TaskListWorkspaceScope],
) -> Vec<WorkspaceBootstrapScope> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut result = Vec::new();
    for scope in scopes {
        if scope.workspace_purpose.as_deref() == Some("conversation") {
            continue;
        }
        let key = resolve_workspace_key(&scope.workspace_path, scope.workspace_identity.as_deref());
        if js_trim(&key).is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        result.push(WorkspaceBootstrapScope {
            workspace_key: key,
            workspace_path: scope.workspace_path.clone(),
        });
    }
    result
}

fn task_node_key(params: &GroupedTaskRef) -> String {
    format!(
        "{}\u{0000}{}",
        resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref()),
        params.task_id
    )
}

fn task_order_node_key(params: &GroupedTaskRef) -> String {
    // node_key 不能使用 "\u0000" 分隔：SQLite 读 TEXT 时会截断 NUL 后的 taskId。
    serde_json::to_string(&[
        resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref()),
        params.task_id.clone(),
    ])
    .expect("task order node key 必须可序列化")
}

fn item_ref(item: &TaskListItem) -> GroupedTaskRef {
    GroupedTaskRef {
        workspace_path: item.meta.workspace_path.clone(),
        workspace_identity: item.meta.workspace_identity.clone(),
        task_id: item.meta.task_id.clone(),
    }
}

fn node_map_key(node: &GroupedTaskViewNode) -> String {
    match node {
        GroupedTaskViewNode::Group { group, .. } => format!("group:{}", group.id),
        GroupedTaskViewNode::Task { task, .. } => {
            format!("task:{}", task_order_node_key(&item_ref(task)))
        }
    }
}

fn node_sort_order(node: &GroupedTaskViewNode) -> i64 {
    match node {
        GroupedTaskViewNode::Group { sort_order, .. } => sort_order.unwrap_or(0),
        GroupedTaskViewNode::Task { sort_order, .. } => sort_order.unwrap_or(0),
    }
}

fn node_created_at(node: &GroupedTaskViewNode) -> i64 {
    match node {
        GroupedTaskViewNode::Group { group, .. } => group.created_at,
        GroupedTaskViewNode::Task { task, .. } => task.meta.created_at,
    }
}

fn compare_grouped_nodes(left: &GroupedTaskViewNode, right: &GroupedTaskViewNode) -> Ordering {
    node_sort_order(left)
        .cmp(&node_sort_order(right))
        .then_with(|| node_map_key(left).cmp(&node_map_key(right)))
}

fn compare_cron_group_tasks(left: &TaskListItem, right: &TaskListItem) -> Ordering {
    right
        .meta
        .created_at
        .cmp(&left.meta.created_at)
        .then_with(|| task_node_key(&item_ref(right)).cmp(&task_node_key(&item_ref(left))))
}

fn compare_group_tasks(
    left: &TaskListItem,
    right: &TaskListItem,
    members: &HashMap<String, TaskGroupMemberRow>,
) -> Ordering {
    let left_order = members
        .get(&task_node_key(&item_ref(left)))
        .and_then(|member| member.sort_order)
        .unwrap_or(0);
    let right_order = members
        .get(&task_node_key(&item_ref(right)))
        .and_then(|member| member.sort_order)
        .unwrap_or(0);
    left_order.cmp(&right_order).then_with(|| {
        task_node_key(&item_ref(left)).cmp(&task_node_key(&item_ref(right)))
    })
}

fn in_transaction<T>(connection: &Connection, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    connection.execute_batch("BEGIN IMMEDIATE")?;
    match operation() {
        Ok(value) => {
            connection.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn group_message(message: &str) -> TaskIndexRepoError {
    TaskIndexRepoError::Message(message.to_owned())
}

// ── 仓库方法 ──────────────────────────────────────────────────────────────

impl TaskIndexRepo {
    pub fn has_grouped_workspace_bootstrap_run(&self) -> Result<bool> {
        self.ensure_ready()?;
        self.with_conn(|connection| self.has_grouped_workspace_bootstrap_run_conn(connection))
    }

    fn has_grouped_workspace_bootstrap_run_conn(&self, connection: &Connection) -> Result<bool> {
        let found: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM task_group_workspace_bootstraps LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    // ── 分组管理 ──────────────────────────────────────────────────────────

    pub fn create_task_group(&self, title: Option<&str>, color: Option<&str>) -> Result<TaskGroup> {
        self.ensure_ready()?;
        let now = self.now();
        let id = format!("task-group-{}", Uuid::new_v4());
        let title = title
            .map(js_trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("New Group")
            .to_owned();
        let color = color.unwrap_or(DEFAULT_TASK_GROUP_COLOR).to_owned();
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO task_groups (group_id, title, color, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
                params![id, title, color, now, now],
            )?;
            let sort_order = self.next_grouped_top_sort_order(connection)?;
            self.upsert_grouped_top_order(connection, "group", &id, sort_order, now)
        })?;
        Ok(TaskGroup {
            id,
            title,
            color,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn rename_task_group(&self, group_id: &str, title: &str) -> Result<TaskGroup> {
        self.ensure_ready()?;
        let trimmed = js_trim(title);
        let title = if trimmed.is_empty() {
            "New Group"
        } else {
            trimmed
        };
        let now = self.now();
        self.with_conn(|connection| {
            let changes = connection.execute(
                "UPDATE task_groups SET title = ?, updated_at = ? WHERE group_id = ?",
                params![title, now, group_id],
            )?;
            if changes == 0 {
                return Err(group_message("Task group 不存在，无法重命名"));
            }
            let row = connection
                .query_row(
                    "SELECT group_id, title, color, created_at, updated_at
                     FROM task_groups WHERE group_id = ?",
                    params![group_id],
                    read_group_row,
                )
                .optional()?;
            row.map(|row| row_to_task_group(&row))
                .ok_or_else(|| group_message("Task group 重命名后读取失败"))
        })
    }

    pub fn update_task_group_color(&self, group_id: &str, color: &str) -> Result<TaskGroup> {
        self.ensure_ready()?;
        if !is_task_group_color(color) {
            return Err(group_message("Task group 颜色无效"));
        }
        let now = self.now();
        self.with_conn(|connection| {
            let changes = connection.execute(
                "UPDATE task_groups SET color = ?, updated_at = ? WHERE group_id = ?",
                params![color, now, group_id],
            )?;
            if changes == 0 {
                return Err(group_message("Task group 不存在，无法更新颜色"));
            }
            let row = connection
                .query_row(
                    "SELECT group_id, title, color, created_at, updated_at
                     FROM task_groups WHERE group_id = ?",
                    params![group_id],
                    read_group_row,
                )
                .optional()?;
            row.map(|row| row_to_task_group(&row))
                .ok_or_else(|| group_message("Task group 更新颜色后读取失败"))
        })
    }

    pub fn delete_task_group(&self, group_id: &str) -> Result<()> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            in_transaction(connection, || {
                let changes = connection.execute(
                    "DELETE FROM task_groups WHERE group_id = ?",
                    params![group_id],
                )?;
                if changes == 0 {
                    return Err(group_message("Task group 不存在，无法删除"));
                }
                connection.execute(
                    "DELETE FROM task_group_view_node_orders
                     WHERE node_type = 'group' AND node_key = ?",
                    params![group_id],
                )?;
                Ok(())
            })
        })
    }

    // ── 顶层排序初始化 ────────────────────────────────────────────────────

    pub fn initialize_grouped_task_at_top(&self, task: &GroupedTaskRef) -> Result<bool> {
        self.ensure_ready()?;
        self.with_conn(|connection| self.initialize_grouped_task_at_top_ready(connection, task))
    }

    pub(crate) fn initialize_grouped_task_at_top_ready(
        &self,
        connection: &Connection,
        task: &GroupedTaskRef,
    ) -> Result<bool> {
        let workspace_key =
            resolve_workspace_key(&task.workspace_path, task.workspace_identity.as_deref());
        let Some(row) = self.get_task_row(connection, &workspace_key, &task.task_id)? else {
            return Ok(false);
        };
        if row.deleted == 1 || row.archived == 1 || row.pinned == 1 {
            return Ok(false);
        }
        let existing_member: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM task_group_members WHERE workspace_key = ? AND task_id = ? LIMIT 1",
                params![workspace_key, task.task_id],
                |row| row.get(0),
            )
            .optional()?;
        let node_key = task_order_node_key(task);
        let existing_top_order: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM task_group_view_node_orders
                 WHERE node_type = 'task' AND node_key = ? LIMIT 1",
                params![node_key],
                |row| row.get(0),
            )
            .optional()?;
        if existing_member.is_some() || existing_top_order.is_some() {
            return Ok(false);
        }
        let now = self.now();
        let sort_order = self.next_grouped_top_sort_order(connection)?;
        self.upsert_grouped_top_order(connection, "task", &node_key, sort_order, now)?;
        Ok(true)
    }

    fn upsert_grouped_top_order(
        &self,
        connection: &Connection,
        node_type: &str,
        node_key: &str,
        sort_order: i64,
        now: i64,
    ) -> Result<()> {
        connection.execute(
            "INSERT INTO task_group_view_node_orders (
               node_type, node_key, sort_order, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(node_type, node_key) DO UPDATE SET
               sort_order = excluded.sort_order,
               updated_at = excluded.updated_at",
            params![node_type, node_key, sort_order, now, now],
        )?;
        Ok(())
    }

    // ── grouped 视图查询 ──────────────────────────────────────────────────

    pub fn query_grouped_task_view(&self, query: &GroupedTaskViewQuery) -> Result<GroupedTaskView> {
        self.ensure_ready()?;
        let include_all = query.include_all_workspaces;
        let requested_scopes = normalize_workspace_bootstrap_scopes(&query.workspace_scopes);
        let workspace_keys = normalize_workspace_keys(&query.workspace_scopes);
        self.with_conn(|connection| {
            let active_tasks = self.load_active_grouped_tasks(
                connection,
                include_all,
                &workspace_keys,
                query.provider.as_deref(),
            )?;
            let scopes = if include_all {
                let scope_inputs: Vec<TaskListWorkspaceScope> = active_tasks
                    .iter()
                    .map(|row| TaskListWorkspaceScope {
                        workspace_path: row.workspace_path.clone(),
                        workspace_identity: row.workspace_identity.clone(),
                        workspace_purpose: None,
                    })
                    .collect();
                normalize_workspace_bootstrap_scopes(&scope_inputs)
            } else {
                requested_scopes
            };
            self.bootstrap_workspace_groups_for_active_tasks(connection, &scopes, &active_tasks)?;

            let bootstrap_workspace_key_by_group: HashMap<String, String> = {
                let mut statement = connection.prepare(
                    "SELECT workspace_key, group_id FROM task_group_workspace_bootstraps
                     WHERE group_id IS NOT NULL",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.into_iter().map(|(key, group)| (group, key)).collect()
            };
            let visible_workspace_keys: HashSet<String> = if include_all {
                active_tasks
                    .iter()
                    .map(|row| row.workspace_key.clone())
                    .collect()
            } else {
                workspace_keys.iter().cloned().collect()
            };

            let group_rows = {
                let mut statement = connection.prepare(
                    "SELECT group_id, title, color, created_at, updated_at FROM task_groups",
                )?;
                let rows = statement
                    .query_map([], read_group_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let groups: Vec<TaskGroup> = group_rows
                .iter()
                .filter(|row| match bootstrap_workspace_key_by_group.get(&row.group_id) {
                    Some(key) => visible_workspace_keys.contains(key),
                    None => true,
                })
                .map(row_to_task_group)
                .collect();

            let member_rows = {
                let mut statement = connection.prepare(
                    "SELECT group_id, workspace_key, workspace_path, workspace_identity,
                            task_id, sort_order, added_at
                     FROM task_group_members",
                )?;
                let rows = statement
                    .query_map([], read_member_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let order_rows = {
                let mut statement = connection.prepare(
                    "SELECT node_type, node_key, sort_order FROM task_group_view_node_orders",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok(NodeOrderRow {
                            node_type: row.get(0)?,
                            node_key: row.get(1)?,
                            sort_order: row.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };

            let mut order_by_key: HashMap<String, NodeOrderRow> = order_rows
                .into_iter()
                .map(|row| (format!("{}:{}", row.node_type, row.node_key), row))
                .collect();
            let mut member_by_task_key: HashMap<String, TaskGroupMemberRow> = member_rows
                .iter()
                .map(|member| {
                    (
                        format!("{}\u{0000}{}", member.workspace_key, member.task_id),
                        member.clone(),
                    )
                })
                .collect();
            let mut members_by_group: HashMap<String, Vec<TaskGroupMemberRow>> = HashMap::new();
            for member in &member_rows {
                members_by_group
                    .entry(member.group_id.clone())
                    .or_default()
                    .push(member.clone());
            }

            let mut active_items: Vec<TaskListItem> = Vec::with_capacity(active_tasks.len());
            let mut active_by_key: HashMap<String, TaskListItem> = HashMap::new();
            for row in &active_tasks {
                let item = row_to_task_list_item(row, None);
                active_by_key.insert(
                    format!("{}\u{0000}{}", row.workspace_key, row.task_id),
                    item.clone(),
                );
                active_items.push(item);
            }

            let mut grouped_visible_task_keys: HashSet<String> = HashSet::new();
            let mut nodes: Vec<GroupedTaskViewNode> = Vec::with_capacity(groups.len());
            for group in &groups {
                let mut group_tasks: Vec<TaskListItem> = members_by_group
                    .get(&group.id)
                    .map(|members| {
                        members
                            .iter()
                            .filter_map(|member| {
                                active_by_key
                                    .get(&format!(
                                        "{}\u{0000}{}",
                                        member.workspace_key, member.task_id
                                    ))
                                    .cloned()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if group.id == CRON_DEFAULT_GROUP_ID {
                    group_tasks.sort_by(compare_cron_group_tasks);
                } else {
                    self.normalize_group_member_orders(
                        connection,
                        &group.id,
                        &group_tasks,
                        &mut member_by_task_key,
                    )?;
                    group_tasks.sort_by(|left, right| {
                        compare_group_tasks(left, right, &member_by_task_key)
                    });
                }
                for task in &group_tasks {
                    grouped_visible_task_keys.insert(task_node_key(&item_ref(task)));
                }
                let sort_order = order_by_key
                    .get(&format!("group:{}", group.id))
                    .map(|row| row.sort_order);
                nodes.push(GroupedTaskViewNode::Group {
                    group: group.clone(),
                    tasks: group_tasks,
                    sort_order,
                });
            }

            for item in &active_items {
                let key = task_node_key(&item_ref(item));
                if member_by_task_key.contains_key(&key) || grouped_visible_task_keys.contains(&key)
                {
                    continue;
                }
                let node_key = task_order_node_key(&item_ref(item));
                let sort_order = order_by_key
                    .get(&format!("task:{node_key}"))
                    .map(|row| row.sort_order);
                nodes.push(GroupedTaskViewNode::Task {
                    task: Box::new(item.clone()),
                    sort_order,
                });
            }

            self.normalize_grouped_top_node_orders(connection, &mut nodes, &mut order_by_key)?;
            nodes.sort_by(compare_grouped_nodes);
            Ok(GroupedTaskView { nodes })
        })
    }

    fn load_active_grouped_tasks(
        &self,
        connection: &Connection,
        include_all: bool,
        workspace_keys: &[String],
        provider: Option<&str>,
    ) -> Result<Vec<TaskIndexRow>> {
        if !include_all && workspace_keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut where_clause = vec![
            "deleted = 0".to_owned(),
            "archived = 0".to_owned(),
            "pinned = 0".to_owned(),
        ];
        let mut arguments: Vec<SqlValue> = Vec::new();
        if !include_all {
            where_clause.push(format!(
                "workspace_key IN ({})",
                vec!["?"; workspace_keys.len()].join(", ")
            ));
            arguments.extend(workspace_keys.iter().map(|key| SqlValue::Text(key.clone())));
        }
        if let Some(provider) = provider {
            where_clause.push("provider = ?".to_owned());
            arguments.push(SqlValue::Text(provider.to_owned()));
        }
        let mut statement = connection.prepare(&format!(
            "SELECT {GROUPED_TASK_COLUMNS} FROM tasks WHERE {}",
            where_clause.join(" AND ")
        ))?;
        let rows = statement
            .query_map(params_from_iter(arguments.iter()), read_grouped_task_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn normalize_grouped_top_node_orders(
        &self,
        connection: &Connection,
        nodes: &mut [GroupedTaskViewNode],
        order_by_key: &mut HashMap<String, NodeOrderRow>,
    ) -> Result<()> {
        struct MissingNode {
            created_at: i64,
            map_key: String,
            node_type: &'static str,
            node_key: String,
            index: usize,
        }
        let mut missing: Vec<MissingNode> = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            let map_key = node_map_key(node);
            if order_by_key.contains_key(&map_key) {
                continue;
            }
            let (node_type, node_key) = match node {
                GroupedTaskViewNode::Group { group, .. } => ("group", group.id.clone()),
                GroupedTaskViewNode::Task { task, .. } => {
                    ("task", task_order_node_key(&item_ref(task)))
                }
            };
            missing.push(MissingNode {
                created_at: node_created_at(node),
                map_key,
                node_type,
                node_key,
                index,
            });
        }
        if missing.is_empty() {
            return Ok(());
        }
        missing.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.map_key.cmp(&right.map_key))
        });
        let max_sort_order: Option<i64> = connection
            .query_row(
                "SELECT MAX(sort_order) FROM task_group_view_node_orders",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let mut next_sort_order = max_sort_order.unwrap_or(0);
        let now = self.now();
        in_transaction(connection, || {
            for node in &missing {
                next_sort_order += GROUPED_TASK_ORDER_STEP;
                connection.execute(
                    "INSERT INTO task_group_view_node_orders (
                       node_type, node_key, sort_order, created_at, updated_at
                     ) VALUES (?, ?, ?, ?, ?)",
                    params![node.node_type, node.node_key, next_sort_order, now, now],
                )?;
                order_by_key.insert(
                    node.map_key.clone(),
                    NodeOrderRow {
                        node_type: node.node_type.to_owned(),
                        node_key: node.node_key.clone(),
                        sort_order: next_sort_order,
                    },
                );
                match &mut nodes[node.index] {
                    GroupedTaskViewNode::Group { sort_order, .. }
                    | GroupedTaskViewNode::Task { sort_order, .. } => {
                        *sort_order = Some(next_sort_order);
                    }
                }
            }
            Ok(())
        })
    }

    fn normalize_group_member_orders(
        &self,
        connection: &Connection,
        group_id: &str,
        tasks: &[TaskListItem],
        members: &mut HashMap<String, TaskGroupMemberRow>,
    ) -> Result<()> {
        let mut missing: Vec<(i64, String, String, String)> = Vec::new();
        for task in tasks {
            let key = task_node_key(&item_ref(task));
            if let Some(member) = members.get(&key) {
                if member.sort_order.is_none() {
                    missing.push((
                        member.added_at,
                        key,
                        member.workspace_key.clone(),
                        member.task_id.clone(),
                    ));
                }
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        missing.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        let max_sort_order: Option<i64> = connection
            .query_row(
                "SELECT MAX(sort_order) FROM task_group_members WHERE group_id = ?",
                params![group_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let mut next_sort_order = max_sort_order.unwrap_or(0);
        let now = self.now();
        in_transaction(connection, || {
            for (_, key, workspace_key, task_id) in &missing {
                next_sort_order += GROUPED_TASK_ORDER_STEP;
                connection.execute(
                    "UPDATE task_group_members SET sort_order = ?, updated_at = ?
                     WHERE workspace_key = ? AND task_id = ?",
                    params![next_sort_order, now, workspace_key, task_id],
                )?;
                if let Some(member) = members.get_mut(key) {
                    member.sort_order = Some(next_sort_order);
                }
            }
            Ok(())
        })
    }

    fn bootstrap_workspace_groups_for_active_tasks(
        &self,
        connection: &Connection,
        scopes: &[WorkspaceBootstrapScope],
        active_tasks: &[TaskIndexRow],
    ) -> Result<()> {
        if scopes.is_empty() || self.has_grouped_workspace_bootstrap_run_conn(connection)? {
            return Ok(());
        }
        let mut candidate_rows_by_workspace_key: HashMap<String, Vec<TaskIndexRow>> = HashMap::new();
        for row in active_tasks {
            candidate_rows_by_workspace_key
                .entry(row.workspace_key.clone())
                .or_default()
                .push(row.clone());
        }
        let now = self.now();
        if candidate_rows_by_workspace_key.is_empty() {
            self.mark_workspace_bootstrap_run(connection, GROUPED_WORKSPACE_BOOTSTRAP_ONCE_KEY, None, now)?;
            return Ok(());
        }
        let mut existing_group_order_keys: HashSet<String> = {
            let mut statement = connection.prepare(
                "SELECT node_key FROM task_group_view_node_orders WHERE node_type = 'group'",
            )?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().collect()
        };
        let max_sort_order: Option<i64> = connection
            .query_row(
                "SELECT MAX(sort_order) FROM task_group_view_node_orders",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let mut next_group_sort_order = max_sort_order.unwrap_or(0);
        in_transaction(connection, || {
            self.mark_workspace_bootstrap_run(
                connection,
                GROUPED_WORKSPACE_BOOTSTRAP_ONCE_KEY,
                None,
                now,
            )?;
            for scope in scopes {
                let Some(rows) = candidate_rows_by_workspace_key.get(&scope.workspace_key) else {
                    continue;
                };
                let mut grouped_rows: Vec<&TaskIndexRow> = rows.iter().collect();
                grouped_rows.sort_by(|left, right| {
                    right
                        .updated_at
                        .cmp(&left.updated_at)
                        .then_with(|| right.created_at.cmp(&left.created_at))
                        .then_with(|| left.task_id.cmp(&right.task_id))
                });
                if grouped_rows.is_empty() {
                    continue;
                }
                let group_id = workspace_group_id(&scope.workspace_key);
                let title = workspace_group_title(&scope.workspace_path);
                let color = workspace_group_color(&scope.workspace_key);
                connection.execute(
                    "INSERT OR IGNORE INTO task_groups (
                       group_id, title, color, created_at, updated_at
                     ) VALUES (?, ?, ?, ?, ?)",
                    params![group_id, title, color, now, now],
                )?;
                if !existing_group_order_keys.contains(&group_id) {
                    next_group_sort_order += GROUPED_TASK_ORDER_STEP;
                    connection.execute(
                        "INSERT OR IGNORE INTO task_group_view_node_orders (
                           node_type, node_key, sort_order, created_at, updated_at
                         ) VALUES ('group', ?, ?, ?, ?)",
                        params![group_id, next_group_sort_order, now, now],
                    )?;
                    existing_group_order_keys.insert(group_id.clone());
                }
                for (index, row) in grouped_rows.iter().enumerate() {
                    connection.execute(
                        "DELETE FROM task_group_members WHERE workspace_key = ? AND task_id = ?",
                        params![row.workspace_key, row.task_id],
                    )?;
                    connection.execute(
                        "INSERT INTO task_group_members (
                           group_id, workspace_key, workspace_path, workspace_identity, task_id,
                           sort_order, added_at, created_at, updated_at
                         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                         ON CONFLICT(workspace_key, task_id) DO UPDATE SET
                           group_id = excluded.group_id,
                           workspace_path = excluded.workspace_path,
                           workspace_identity = excluded.workspace_identity,
                           sort_order = excluded.sort_order,
                           updated_at = excluded.updated_at",
                        params![
                            group_id,
                            row.workspace_key,
                            row.workspace_path,
                            row.workspace_identity,
                            row.task_id,
                            ((index + 1) as i64) * GROUPED_TASK_ORDER_STEP,
                            now,
                            now,
                            now
                        ],
                    )?;
                    let task_ref = GroupedTaskRef {
                        workspace_path: row.workspace_path.clone(),
                        workspace_identity: row.workspace_identity.clone(),
                        task_id: row.task_id.clone(),
                    };
                    connection.execute(
                        "DELETE FROM task_group_view_node_orders
                         WHERE node_type = 'task' AND node_key = ?",
                        params![task_order_node_key(&task_ref)],
                    )?;
                }
                connection.execute(
                    "INSERT INTO task_group_workspace_bootstraps (
                       workspace_key, group_id, created_at, updated_at
                     ) VALUES (?, ?, ?, ?)
                     ON CONFLICT(workspace_key) DO UPDATE SET
                       group_id = excluded.group_id,
                       updated_at = excluded.updated_at",
                    params![scope.workspace_key, group_id, now, now],
                )?;
            }
            connection.execute(
                "DELETE FROM task_groups
                 WHERE group_id NOT IN (SELECT DISTINCT group_id FROM task_group_members)",
                [],
            )?;
            connection.execute(
                "DELETE FROM task_group_view_node_orders
                 WHERE node_type = 'group'
                   AND node_key NOT IN (SELECT group_id FROM task_groups)",
                [],
            )?;
            Ok(())
        })
    }

    fn mark_workspace_bootstrap_run(
        &self,
        connection: &Connection,
        workspace_key: &str,
        group_id: Option<&str>,
        now: i64,
    ) -> Result<()> {
        connection.execute(
            "INSERT INTO task_group_workspace_bootstraps (
               workspace_key, group_id, created_at, updated_at
             ) VALUES (?, ?, ?, ?)
             ON CONFLICT(workspace_key) DO UPDATE SET
               updated_at = excluded.updated_at",
            params![workspace_key, group_id, now, now],
        )?;
        Ok(())
    }

    // ── grouped 原始结构 ──────────────────────────────────────────────────

    pub fn query_grouped_task_view_structure(
        &self,
        workspace_scopes: &[TaskListWorkspaceScope],
    ) -> Result<GroupedTaskViewStructure> {
        self.ensure_ready()?;
        let visible_workspace_keys: HashSet<String> =
            normalize_workspace_keys(workspace_scopes).into_iter().collect();
        self.with_conn(|connection| {
            let bootstrap_workspace_key_by_group: HashMap<String, String> = {
                let mut statement = connection.prepare(
                    "SELECT workspace_key, group_id FROM task_group_workspace_bootstraps
                     WHERE group_id IS NOT NULL",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.into_iter().map(|(key, group)| (group, key)).collect()
            };
            let group_rows = {
                let mut statement = connection.prepare(
                    "SELECT group_id, title, color, created_at, updated_at FROM task_groups",
                )?;
                let rows = statement
                    .query_map([], read_group_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let groups: Vec<TaskGroup> = group_rows
                .iter()
                .filter(|row| match bootstrap_workspace_key_by_group.get(&row.group_id) {
                    Some(key) => visible_workspace_keys.contains(key),
                    None => true,
                })
                .map(row_to_task_group)
                .collect();
            let member_rows = {
                let mut statement = connection.prepare(
                    "SELECT group_id, workspace_key, workspace_path, workspace_identity,
                            task_id, sort_order, added_at
                     FROM task_group_members",
                )?;
                let rows = statement
                    .query_map([], read_member_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let members: Vec<GroupedTaskViewStructureMember> = member_rows
                .into_iter()
                .map(|row| GroupedTaskViewStructureMember {
                    group_id: row.group_id,
                    workspace_key: row.workspace_key,
                    workspace_path: row.workspace_path,
                    workspace_identity: row.workspace_identity,
                    task_id: row.task_id,
                    sort_order: row.sort_order,
                    added_at: row.added_at,
                })
                .collect();
            let order_rows = {
                let mut statement = connection.prepare(
                    "SELECT node_type, node_key, sort_order FROM task_group_view_node_orders",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok(NodeOrderRow {
                            node_type: row.get(0)?,
                            node_key: row.get(1)?,
                            sort_order: row.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let mut top_level_orders: Vec<GroupedTaskViewStructureTopOrder> = Vec::new();
            for row in order_rows {
                if row.node_type == "group" {
                    top_level_orders.push(GroupedTaskViewStructureTopOrder::Group {
                        group_id: row.node_key,
                        sort_order: row.sort_order,
                    });
                    continue;
                }
                let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&row.node_key) else {
                    continue;
                };
                let Some(items) = parsed.as_array() else {
                    continue;
                };
                if items.len() < 2 {
                    continue;
                }
                let (Some(workspace_key), Some(task_id)) =
                    (items[0].as_str(), items[1].as_str())
                else {
                    continue;
                };
                top_level_orders.push(GroupedTaskViewStructureTopOrder::Task {
                    workspace_key: workspace_key.to_owned(),
                    task_id: task_id.to_owned(),
                    sort_order: row.sort_order,
                });
            }
            Ok(GroupedTaskViewStructure {
                groups,
                members,
                top_level_orders,
            })
        })
    }

    // ── grouped 排序保存 ──────────────────────────────────────────────────

    pub fn apply_grouped_task_view_order(
        &self,
        input: &GroupedTaskViewOrderInput,
    ) -> Result<GroupedTaskView> {
        self.ensure_ready()?;
        let workspace_keys: HashSet<String> =
            normalize_workspace_keys(&input.workspace_scopes).into_iter().collect();
        let now = self.now();
        self.with_conn(|connection| {
            self.write_grouped_task_view_order(connection, input, &workspace_keys, now)
        })?;
        self.query_grouped_task_view(&GroupedTaskViewQuery {
            workspace_scopes: input.workspace_scopes.clone(),
            include_all_workspaces: false,
            provider: input.provider.clone(),
        })
    }

    fn write_grouped_task_view_order(
        &self,
        connection: &Connection,
        input: &GroupedTaskViewOrderInput,
        workspace_keys: &HashSet<String>,
        now: i64,
    ) -> Result<()> {
        let group_ids: HashSet<String> = {
            let mut statement = connection.prepare("SELECT group_id FROM task_groups")?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().collect()
        };

        let validate_task_ref = |task: &GroupedTaskRef| -> Result<Option<String>> {
            let key = resolve_workspace_key(&task.workspace_path, task.workspace_identity.as_deref());
            if !workspace_keys.contains(&key) {
                return Err(group_message("Grouped task order 包含当前 scope 外的 task"));
            }
            let row = self.get_task_row(connection, &key, &task.task_id)?;
            let Some(row) = row else {
                return Err(group_message("Grouped task order 包含不可见 task"));
            };
            if row.deleted == 1 || row.archived == 1 || row.pinned == 1 {
                return Err(group_message("Grouped task order 包含不可见 task"));
            }
            if let Some(provider) = input.provider.as_deref() {
                if row.provider.as_deref() != Some(provider) {
                    return Ok(None);
                }
            }
            Ok(Some(key))
        };

        let mut top_level_task_keys: HashSet<String> = HashSet::new();
        let mut grouped_task_keys: HashSet<String> = HashSet::new();
        let mut visible_top_level_nodes: Vec<(&str, String)> = Vec::new();
        let mut visible_groups: Vec<(&str, Vec<GroupedTaskRef>)> = Vec::new();

        for node in &input.top_level_nodes {
            match node {
                GroupedTaskViewTopLevelNodeRef::Group { group_id } => {
                    if !group_ids.contains(group_id) {
                        return Err(group_message("Grouped task order 包含不存在的 group"));
                    }
                    visible_top_level_nodes.push(("group", group_id.clone()));
                }
                GroupedTaskViewTopLevelNodeRef::Task { task } => {
                    let Some(key) = validate_task_ref(task)? else {
                        continue;
                    };
                    top_level_task_keys.insert(format!("{key}\u{0000}{}", task.task_id));
                    visible_top_level_nodes.push(("task", task_order_node_key(task)));
                }
            }
        }
        for group in &input.groups {
            if !group_ids.contains(&group.group_id) {
                return Err(group_message("Grouped task order 包含不存在的 group"));
            }
            let mut visible_task_refs: Vec<GroupedTaskRef> = Vec::new();
            for task_ref in &group.task_refs {
                let Some(key) = validate_task_ref(task_ref)? else {
                    continue;
                };
                let composite = format!("{key}\u{0000}{}", task_ref.task_id);
                if !grouped_task_keys.insert(composite) {
                    return Err(group_message(
                        "Grouped task order 不能让同一个 task 进入多个 group",
                    ));
                }
                visible_task_refs.push(task_ref.clone());
            }
            visible_groups.push((&group.group_id, visible_task_refs));
        }

        let scoped_task_order_keys: Vec<String> = if workspace_keys.is_empty() {
            Vec::new()
        } else {
            let keys: Vec<&String> = workspace_keys.iter().collect();
            let placeholders = vec!["?"; keys.len()].join(", ");
            let mut statement = connection.prepare(&format!(
                "SELECT workspace_key, task_id FROM tasks WHERE workspace_key IN ({placeholders})"
            ))?;
            let rows = statement
                .query_map(params_from_iter(keys.iter()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(key, task_id)| format!("{key}\u{0000}{task_id}"))
                .collect()
        };

        in_transaction(connection, || {
            self.mark_workspace_bootstrap_run(
                connection,
                GROUPED_WORKSPACE_BOOTSTRAP_ONCE_KEY,
                None,
                now,
            )?;
            for key in &top_level_task_keys {
                let Some((workspace_key, task_id)) = key.split_once('\u{0000}') else {
                    return Err(group_message("Grouped task order 顶层 task key 非法"));
                };
                if workspace_key.is_empty() || task_id.is_empty() {
                    return Err(group_message("Grouped task order 顶层 task key 非法"));
                }
                connection.execute(
                    "DELETE FROM task_group_members WHERE workspace_key = ? AND task_id = ?",
                    params![workspace_key, task_id],
                )?;
            }
            for (group_id, task_refs) in &visible_groups {
                for (index, task_ref) in task_refs.iter().enumerate() {
                    let workspace_key = resolve_workspace_key(
                        &task_ref.workspace_path,
                        task_ref.workspace_identity.as_deref(),
                    );
                    connection.execute(
                        "INSERT INTO task_group_members (
                           group_id, workspace_key, workspace_path, workspace_identity, task_id,
                           sort_order, added_at, created_at, updated_at
                         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                         ON CONFLICT(workspace_key, task_id) DO UPDATE SET
                           group_id = excluded.group_id,
                           workspace_path = excluded.workspace_path,
                           workspace_identity = excluded.workspace_identity,
                           sort_order = excluded.sort_order,
                           updated_at = excluded.updated_at",
                        params![
                            group_id,
                            workspace_key,
                            task_ref.workspace_path,
                            task_ref.workspace_identity,
                            task_ref.task_id,
                            ((index + 1) as i64) * GROUPED_TASK_ORDER_STEP,
                            now,
                            now,
                            now
                        ],
                    )?;
                }
            }

            connection.execute(
                "DELETE FROM task_group_view_node_orders WHERE node_type = 'group'",
                [],
            )?;
            for key in &scoped_task_order_keys {
                let Some((workspace_key, task_id)) = key.split_once('\u{0000}') else {
                    continue;
                };
                if workspace_key.is_empty() || task_id.is_empty() {
                    continue;
                }
                let ref_key = serde_json::to_string(&[workspace_key, task_id])
                    .expect("scoped task order key 必须可序列化");
                connection.execute(
                    "DELETE FROM task_group_view_node_orders
                     WHERE node_type = 'task' AND (node_key = ? OR node_key = ?)",
                    params![ref_key, workspace_key],
                )?;
            }
            for (index, (node_type, node_key)) in visible_top_level_nodes.iter().enumerate() {
                connection.execute(
                    "INSERT INTO task_group_view_node_orders (
                       node_type, node_key, sort_order, created_at, updated_at
                     ) VALUES (?, ?, ?, ?, ?)",
                    params![
                        node_type,
                        node_key,
                        ((index + 1) as i64) * GROUPED_TASK_ORDER_STEP,
                        now,
                        now
                    ],
                )?;
            }
            Ok(())
        })
    }
}
