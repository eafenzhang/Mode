import type { WorkspacePurpose, ModeTaskMeta } from "@mode/shared";

export type ModeTaskListKind = "pinned" | "archived" | "timeline" | "active";
export type ModeTaskListSortBy = "created" | "updated";

export interface ModeTaskListWorkspaceScope {
  workspacePath: string;
  workspaceIdentity?: string;
  workspacePurpose?: WorkspacePurpose;
}

export interface ModeTaskListQuery {
  kind: ModeTaskListKind;
  workspaceScopes: ModeTaskListWorkspaceScope[];
  sortBy: ModeTaskListSortBy;
  search?: string;
  limit?: number;
}

export type ModeTaskListItem = ModeTaskMeta & {
  searchSnippet?: string;
  searchSnippets?: string[];
};

export interface ModeTaskListResult {
  items: ModeTaskListItem[];
  total: number;
  hasMore: boolean;
}

export type ModeTaskGroupColor =
  | "gray"
  | "red"
  | "orange"
  | "yellow"
  | "green"
  | "blue"
  | "purple";

export interface ModeTaskGroup {
  id: string;
  title: string;
  color: ModeTaskGroupColor;
  createdAt: number;
  updatedAt: number;
}

export interface ModeGroupedTaskRef {
  workspacePath: string;
  workspaceIdentity?: string;
  taskId: string;
}

export type ModeGroupedTaskViewTopLevelNodeRef =
  | { type: "group"; groupId: string }
  | { type: "task"; task: ModeGroupedTaskRef };

export type ModeGroupedTaskViewNode =
  | {
      type: "group";
      group: ModeTaskGroup;
      tasks: ModeTaskListItem[];
      sortOrder?: number;
    }
  | {
      type: "task";
      task: ModeTaskListItem;
      sortOrder?: number;
    };

export interface ModeGroupedTaskView {
  nodes: ModeGroupedTaskViewNode[];
}

export interface ModeGroupedTaskViewQuery {
  workspaceScopes: ModeTaskListWorkspaceScope[];
  includeAllWorkspaces?: boolean;
}

// ── grouped 原始结构（不 join tasks 表）──
// grouped 视图的任务数据源迁到 sessions-index 后，服务端只提供分组结构
// （task_groups / task_group_members / task_group_view_node_orders），
// 由客户端与 sessions-index 会话做 join。

/** 组成员引用（不含任务 meta；task 内容由 sessions-index 提供）。 */
export interface ModeGroupedTaskViewStructureMember {
  groupId: string;
  /** 服务端口径 workspaceKey（resolveWorkspaceKey：identity ?? path），join 匹配键。 */
  workspaceKey: string;
  workspacePath: string;
  workspaceIdentity?: string;
  taskId: string;
  /** null = 尚未落 sort_order（新加入组）；客户端按 addedAt 降序补内存序。 */
  sortOrder: number | null;
  addedAt: number;
}

/** 顶层节点排序（task_group_view_node_orders，node_key 已解析为结构化引用）。 */
export type ModeGroupedTaskViewStructureTopOrder =
  | { type: "group"; groupId: string; sortOrder: number }
  | { type: "task"; workspaceKey: string; taskId: string; sortOrder: number };

export interface ModeGroupedTaskViewStructure {
  /** 已按 workspaceScopes 可见性过滤的 group（bootstrap workspace group 只在其 workspace 可见）。 */
  groups: ModeTaskGroup[];
  /** 全量组成员（含不可见 group 的成员——顶层排除规则需要全量判断）。 */
  members: ModeGroupedTaskViewStructureMember[];
  topLevelOrders: ModeGroupedTaskViewStructureTopOrder[];
}

export interface ModeGroupedTaskViewOrderInput {
  workspaceScopes: ModeTaskListWorkspaceScope[];
  topLevelNodes: ModeGroupedTaskViewTopLevelNodeRef[];
  groups: Array<{
    groupId: string;
    taskRefs: ModeGroupedTaskRef[];
  }>;
}

export interface ModeWorkspaceEventSubscriptionParams {
  workspacePath: string;
  workspaceIdentity?: string;
}
