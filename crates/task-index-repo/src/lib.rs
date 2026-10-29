//! 任务索引仓库（`TaskIndexRepo`）的核心任务行 + 列表/搜索迁移。
//!
//! 行为真源（clean-room：只读本仓库 TS）：
//!
//! - `packages/services/src/session/taskIndexRepo.ts`（本轮纳入的方法）
//! - `packages/services/src/session/modeTaskListTypes.ts`
//! - `packages/shared/src/validation.ts`（`modeTaskMetaSchema`）
//! - `packages/shared/src/task-realtime-core.ts`（`resolveWorkspaceKey`）
//! - `packages/shared/src/remote-workspace-identity.ts`
//!
//! 数据库 schema 与冻结迁移由 `tasks-db` 提供。规范见
//! `docs/specs/task-index-repo.md`。
//!
//! 明确未纳入：任务索引频道与会话链路（`host-channels` 接线、`sessions-index` 联动）。

mod error;
mod grouped;
mod js;
mod meta;
mod remote;
mod repo;

pub use error::{Result, TaskIndexRepoError};
pub use grouped::{
    GroupedTaskGroupRefs, GroupedTaskRef, GroupedTaskView, GroupedTaskViewNode,
    GroupedTaskViewOrderInput, GroupedTaskViewQuery, GroupedTaskViewStructure,
    GroupedTaskViewStructureMember, GroupedTaskViewStructureTopOrder,
    GroupedTaskViewTopLevelNodeRef, TaskGroup,
};
pub use meta::{
    parse_last_error_value, parse_mode_task_meta, parse_task_goal_value, ErrorAttribution, ModeTaskChangeSummary, ModeTaskChangedFile,
    ModeTaskGoal, ModeTaskGoalTime, ModeTaskLastError, ModeTaskMeta, CRON_DEFAULT_GROUP_ID,
    MODE_AGENT_PROVIDER, OFF_PEAK_DEFAULT_GROUP_ID,
};
pub use repo::{
    mark_tasks_storage_migrated, mark_tasks_storage_prepared, AgentPatch, ArchiveStaleTasksParams,
    ClearUnreadResult, ListDeletedTaskIdsParams, ListTaskMetasParams, SyncTaskMetaAtGroupedTop,
    SyncTaskMetaParams, TaskIndexRepo, TaskIndexStatePatch, TaskListItem, TaskListKind, TaskListQuery,
    TaskListResult, TaskListSortBy, TaskListWorkspaceScope, TaskRef,
};
