/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 seed 与逐步输入/输出，拆分会降低可核对性。 */
/**
 * 生成 `task-index-repo` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/session/taskIndexRepo.ts`
 * - `packages/services/src/session/modeTaskListTypes.ts`
 *
 * ```bash
 * pnpm task-index:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p task-index-repo"
 * ```
 *
 * 时间戳策略：
 * - cron/off-peak 系统分组成员行的时间列在快照里统一归一为 0；
 * - `archiveStaleTasks` 的 cutoff 依赖 Date.now，但 seed 行的 `updated_at` 都远早于
 *   “生成时间 - 1 天”，归档集合稳定；fixture 固定 `clockMs`，Rust 侧用同一时钟重放。
 * - `tasks.meta_json` 在快照里按“解析后的 JSON”记录（属性顺序不是语义）；
 *   无法解析的原始文本保留在 `{ unparsable: string }`。
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

import { TaskIndexRepo } from "../packages/services/src/session/taskIndexRepo.js";
import { runTasksDatabaseMigrations } from "../packages/services/src/session/tasksDatabase/migrations.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "task-index-repo", "fixtures", "task-index.json");

interface MetaSeed {
  taskId: string;
  traceId?: string;
  title: string;
  titleOverridden?: boolean;
  workspacePath: string;
  workspaceIdentity?: string;
  workspacePurpose?: "project" | "conversation";
  createdAt: number;
  updatedAt: number;
  mode: "yolo" | "plan" | "edit" | "auto" | "autoEdit" | "build";
  model?: string;
  provider?: "glm";
  migrationSource?: "claudeCode";
  forkedFromTaskId?: string;
  cronAutomationId?: string;
  offPeakTaskId?: string;
  unreadAt?: number;
  status?: "running" | "completed" | "error";
  lastError?: Record<string, unknown>;
  target?: Record<string, unknown> | null;
  changeSummary?: Record<string, unknown>;
}

function buildMeta(seed: MetaSeed): Record<string, unknown> {
  const meta: Record<string, unknown> = {
    taskId: seed.taskId,
    traceId: seed.traceId ?? `mode-${seed.taskId}`,
    title: seed.title,
  };
  if (seed.titleOverridden !== undefined) meta.titleOverridden = seed.titleOverridden;
  meta.workspacePath = seed.workspacePath;
  if (seed.workspaceIdentity !== undefined) meta.workspaceIdentity = seed.workspaceIdentity;
  if (seed.workspacePurpose !== undefined) meta.workspacePurpose = seed.workspacePurpose;
  meta.createdAt = seed.createdAt;
  meta.updatedAt = seed.updatedAt;
  meta.mode = seed.mode;
  if (seed.model !== undefined) meta.model = seed.model;
  if (seed.provider !== undefined) meta.provider = seed.provider;
  if (seed.migrationSource !== undefined) meta.migrationSource = seed.migrationSource;
  if (seed.forkedFromTaskId !== undefined) meta.forkedFromTaskId = seed.forkedFromTaskId;
  if (seed.cronAutomationId !== undefined) meta.cronAutomationId = seed.cronAutomationId;
  if (seed.offPeakTaskId !== undefined) meta.offPeakTaskId = seed.offPeakTaskId;
  if (seed.unreadAt !== undefined) meta.unreadAt = seed.unreadAt;
  if (seed.status !== undefined) meta.status = seed.status;
  if (seed.lastError !== undefined) meta.lastError = seed.lastError;
  if (seed.target !== undefined) meta.target = seed.target;
  if (seed.changeSummary !== undefined) meta.changeSummary = seed.changeSummary;
  return meta;
}

const WS_A = "D:/ws-alpha";
const WS_B = "remote:ssh:box:22:me:/srv/ws";
const WS_C = "D:/ws-beta";

interface TaskSeedRow {
  workspaceKey: string;
  workspacePath: string;
  workspaceIdentity: string | null;
  taskId: string;
  title: string;
  taskStatus: string | null;
  provider: string | null;
  mode: string;
  model: string | null;
  migrationSource: string | null;
  forkedFromTaskId: string | null;
  cronAutomationId: string | null;
  offPeakTaskId: string | null;
  createdAt: number;
  updatedAt: number;
  unreadAt: number | null;
  lastUnreadAt: number;
  pinned: number;
  archived: number;
  deleted: number;
  titleOverridden: number;
  searchableText: string;
  metaJson: string;
}

function seedRow(params: {
  meta: MetaSeed;
  workspaceKey: string;
  workspacePath: string;
  workspaceIdentity?: string;
  title?: string;
  status?: string | null;
  provider?: string | null;
  mode?: string;
  model?: string | null;
  migrationSource?: string | null;
  forkedFromTaskId?: string | null;
  cronAutomationId?: string | null;
  offPeakTaskId?: string | null;
  unreadAt?: number | null;
  lastUnreadAt?: number;
  pinned?: boolean;
  archived?: boolean;
  deleted?: boolean;
  titleOverridden?: boolean;
  searchableText?: string;
  metaJsonOverride?: string;
  extraMeta?: Record<string, unknown>;
}): TaskSeedRow {
  const meta = params.meta;
  const metaObject = buildMeta(meta);
  if (params.extraMeta) Object.assign(metaObject, params.extraMeta);
  return {
    workspaceKey: params.workspaceKey,
    workspacePath: params.workspacePath,
    workspaceIdentity: params.workspaceIdentity ?? null,
    taskId: meta.taskId,
    title: params.title ?? meta.title,
    taskStatus: params.status === undefined ? (meta.status ?? null) : params.status,
    provider: params.provider === undefined ? (meta.provider ?? null) : params.provider,
    mode: params.mode ?? meta.mode,
    model: params.model === undefined ? (meta.model ?? null) : params.model,
    migrationSource:
      params.migrationSource === undefined ? (meta.migrationSource ?? null) : params.migrationSource,
    forkedFromTaskId:
      params.forkedFromTaskId === undefined
        ? (meta.forkedFromTaskId ?? null)
        : params.forkedFromTaskId,
    cronAutomationId:
      params.cronAutomationId === undefined
        ? (meta.cronAutomationId ?? null)
        : params.cronAutomationId,
    offPeakTaskId:
      params.offPeakTaskId === undefined ? (meta.offPeakTaskId ?? null) : params.offPeakTaskId,
    createdAt: meta.createdAt,
    updatedAt: meta.updatedAt,
    unreadAt: params.unreadAt === undefined ? (meta.unreadAt ?? null) : params.unreadAt,
    lastUnreadAt: params.lastUnreadAt ?? params.unreadAt ?? meta.unreadAt ?? 0,
    pinned: params.pinned ? 1 : 0,
    archived: params.archived ? 1 : 0,
    deleted: params.deleted ? 1 : 0,
    titleOverridden: (params.titleOverridden ?? Boolean(meta.titleOverridden)) ? 1 : 0,
    searchableText: params.searchableText ?? "",
    metaJson: params.metaJsonOverride ?? JSON.stringify(metaObject),
  };
}

const taskSeeds: TaskSeedRow[] = [
  seedRow({
    meta: {
      taskId: "task-active",
      title: "alpha release notes",
      workspacePath: WS_A,
      createdAt: 900,
      updatedAt: 1000,
      mode: "build",
      model: "glm-4.6",
      provider: "glm",
      status: "running",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    searchableText: "alpha release notes and build pipeline",
    titleOverridden: false,
  }),
  seedRow({
    meta: {
      taskId: "task-pinned",
      title: "pinned deploy checklist",
      titleOverridden: true,
      workspacePath: WS_A,
      createdAt: 950,
      updatedAt: 1100,
      mode: "plan",
      provider: "glm",
      unreadAt: 5000,
      status: "completed",
      cronAutomationId: "auto-1",
      lastError: { message: "pinned boom" },
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    pinned: true,
    unreadAt: 5000,
    searchableText: "pinned deploy checklist",
    titleOverridden: true,
  }),
  seedRow({
    meta: {
      taskId: "task-archived",
      title: "legacy claude import",
      workspacePath: WS_A,
      createdAt: 1000,
      updatedAt: 1200,
      mode: "edit",
      model: "claude-sonnet",
      provider: "glm",
      migrationSource: "claudeCode",
      forkedFromTaskId: "task-active",
      status: "completed",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    provider: "claude",
    archived: true,
    searchableText: "legacy claude import",
  }),
  seedRow({
    meta: {
      taskId: "task-deleted",
      title: "deleted task",
      workspacePath: WS_A,
      createdAt: 700,
      updatedAt: 1300,
      mode: "build",
      provider: "glm",
      status: "completed",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    archived: true,
    deleted: true,
    searchableText: "deleted task",
  }),
  seedRow({
    meta: {
      taskId: "task-unread",
      title: "unread deploy",
      workspacePath: WS_A,
      createdAt: 800,
      updatedAt: 1400,
      mode: "auto",
      provider: "glm",
      unreadAt: 4000,
      status: "running",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    unreadAt: 4000,
    searchableText: "deploy failed; retry deploy",
  }),
  seedRow({
    meta: {
      taskId: "task-offpeak",
      title: "off peak task",
      workspacePath: WS_A,
      createdAt: 850,
      updatedAt: 1500,
      mode: "yolo",
      provider: "glm",
      status: "running",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    searchableText: "off peak body",
  }),
  seedRow({
    meta: {
      taskId: "task-invalid-meta",
      title: "invalid meta task",
      workspacePath: WS_A,
      createdAt: 760,
      updatedAt: 1600,
      mode: "build",
      provider: "glm",
      status: "error",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    titleOverridden: true,
    metaJsonOverride: "{}",
    searchableText: "invalid meta body",
  }),
  seedRow({
    meta: {
      taskId: "task-invalid-json",
      title: "invalid json task",
      workspacePath: WS_A,
      createdAt: 765,
      updatedAt: 1650,
      mode: "build",
      provider: "glm",
      status: "running",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    metaJsonOverride: "{oops",
    searchableText: "invalid json body",
  }),
  seedRow({
    meta: {
      taskId: "task-extra-meta",
      traceId: "  mode-extra  ",
      title: "extra meta",
      workspacePath: WS_A,
      createdAt: 750,
      updatedAt: 1700,
      mode: "autoEdit",
      provider: "glm",
      status: "completed",
      target: {
        sessionID: "s-extra",
        targetID: "g-extra",
        objective: "ship it",
        status: "active",
        tokenBudget: 1000,
        tokensUsed: 5,
        timeUsedSeconds: 3,
        time: { created: 1, updated: 2 },
      },
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    extraMeta: { legacyField: "dropped-by-schema" },
    searchableText: "extra meta body",
  }),
  seedRow({
    meta: {
      taskId: "task-identity-mismatch",
      title: "identity mismatch",
      workspacePath: WS_A,
      workspaceIdentity: "remote:ssh:other:22:u:/elsewhere",
      createdAt: 755,
      updatedAt: 1800,
      mode: "build",
      provider: "glm",
      status: "running",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    workspaceIdentity: "remote:ssh:other:22:u:/elsewhere",
    searchableText: "identity mismatch body",
  }),
  seedRow({
    meta: {
      taskId: "task-remote",
      title: "remote task",
      workspacePath: "/srv/ws",
      workspaceIdentity: WS_B,
      createdAt: 770,
      updatedAt: 1900,
      mode: "build",
      provider: "glm",
      status: "completed",
    },
    workspaceKey: WS_B,
    workspacePath: "/srv/ws",
    workspaceIdentity: WS_B,
    searchableText: "remote workspace task",
  }),
  seedRow({
    meta: {
      taskId: "task-beta",
      title: "beta channel",
      workspacePath: WS_C,
      createdAt: 780,
      updatedAt: 2000,
      mode: "build",
      provider: "glm",
      status: "running",
    },
    workspaceKey: WS_C,
    workspacePath: WS_C,
    searchableText: "beta channel",
  }),
  seedRow({
    meta: {
      taskId: "task-cron-extra",
      title: "cron extra",
      workspacePath: WS_A,
      createdAt: 800,
      updatedAt: 2100,
      mode: "build",
      provider: "glm",
      status: "completed",
      cronAutomationId: "auto-1",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    searchableText: "cron extra body",
  }),
  seedRow({
    meta: {
      taskId: "task-stale",
      title: "stale task",
      workspacePath: WS_A,
      createdAt: 50,
      updatedAt: 100,
      mode: "build",
      provider: "glm",
      status: "completed",
    },
    workspaceKey: WS_A,
    workspacePath: WS_A,
    searchableText: "stale body",
  }),
  seedRow({
    meta: {
      taskId: "task-archived-2",
      title: "beta archived",
      workspacePath: WS_C,
      createdAt: 600,
      updatedAt: 1250,
      mode: "build",
      provider: "glm",
      status: "completed",
    },
    workspaceKey: WS_C,
    workspacePath: WS_C,
    archived: true,
    searchableText: "beta archived body",
  }),
];

const TASK_COLUMNS = [
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
  "meta_json",
] as const;

const offPeakSeeds = [
  {
    off_peak_task_id: "op-1",
    title: "off peak",
    session_id: "task-offpeak",
    prompt: "prompt",
    permission_mode: "default",
    workspace_key: WS_A,
    workspace_path: WS_A,
    status: "queued",
    queued_at: 1,
    created_at: 1,
    updated_at: 1,
  },
];

const taskGroupSeeds = [
  { group_id: "g-legacy", title: "legacy", color: "gray", created_at: 1, updated_at: 1 },
];

const taskGroupMemberSeeds = [
  {
    group_id: "g-legacy",
    workspace_key: WS_A,
    workspace_path: WS_A,
    workspace_identity: null,
    task_id: "task-deleted",
    sort_order: 1000,
    added_at: 1,
    created_at: 1,
    updated_at: 1,
  },
];

const taskGroupOrderSeeds = [
  {
    node_type: "task",
    node_key: JSON.stringify([WS_A, "task-deleted"]),
    sort_order: 500,
    created_at: 1,
    updated_at: 1,
  },
  { node_type: "task", node_key: WS_A, sort_order: 600, created_at: 1, updated_at: 1 },
];

function insertSeedDatabase(db: DatabaseSync): void {
  const insertTask = db.prepare(
    `INSERT INTO tasks (${TASK_COLUMNS.join(", ")})
     VALUES (${TASK_COLUMNS.map(() => "?").join(", ")})`,
  );
  for (const row of taskSeeds) {
    insertTask.run(
      row.workspaceKey,
      row.workspacePath,
      row.workspaceIdentity,
      row.taskId,
      row.title,
      row.taskStatus,
      row.provider,
      row.mode,
      row.model,
      row.migrationSource,
      row.forkedFromTaskId,
      row.cronAutomationId,
      row.offPeakTaskId,
      row.createdAt,
      row.updatedAt,
      row.unreadAt,
      row.lastUnreadAt,
      row.pinned,
      row.archived,
      row.deleted,
      row.titleOverridden,
      row.searchableText,
      row.metaJson,
    );
  }

  const insertOffPeak = db.prepare(
    `INSERT INTO off_peak_tasks (
       off_peak_task_id, title, session_id, prompt, permission_mode,
       workspace_key, workspace_path, status, queued_at, created_at, updated_at
     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  );
  for (const row of offPeakSeeds) {
    insertOffPeak.run(
      row.off_peak_task_id,
      row.title,
      row.session_id,
      row.prompt,
      row.permission_mode,
      row.workspace_key,
      row.workspace_path,
      row.status,
      row.queued_at,
      row.created_at,
      row.updated_at,
    );
  }

  // 已删除任务的历史分组脏引用：ensure_ready 的 cleanup 应清掉它们。
  const insertGroup = db.prepare(
    `INSERT INTO task_groups (group_id, title, color, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?)`,
  );
  for (const row of taskGroupSeeds) {
    insertGroup.run(row.group_id, row.title, row.color, row.created_at, row.updated_at);
  }
  const insertMember = db.prepare(
    `INSERT INTO task_group_members (
       group_id, workspace_key, workspace_path, workspace_identity, task_id,
       sort_order, added_at, created_at, updated_at
     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  );
  for (const row of taskGroupMemberSeeds) {
    insertMember.run(
      row.group_id,
      row.workspace_key,
      row.workspace_path,
      row.workspace_identity,
      row.task_id,
      row.sort_order,
      row.added_at,
      row.created_at,
      row.updated_at,
    );
  }
  const insertOrder = db.prepare(
    `INSERT INTO task_group_view_node_orders (node_type, node_key, sort_order, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?)`,
  );
  for (const row of taskGroupOrderSeeds) {
    insertOrder.run(row.node_type, row.node_key, row.sort_order, row.created_at, row.updated_at);
  }
}

interface Snapshot {
  tasks: Array<Record<string, unknown>>;
  taskGroups: Array<Record<string, unknown>>;
  taskGroupMembers: Array<Record<string, unknown>>;
  taskGroupViewNodeOrders: Array<Record<string, unknown>>;
}

function readMetaJson(raw: string): unknown {
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return { unparsable: raw };
  }
}

// `createTaskGroup` 使用 `randomUUID()`，分组时间戳来自 `Date.now()`：
// 两者都不是跨进程可比的事实，fixture 统一归一化为占位 id 与 0 时间。
const normalizeGroupId = (value: unknown): unknown =>
  typeof value === "string" && value.startsWith("task-group-") ? "task-group-normalized" : value;

function normalizeGroupValue(group: Record<string, unknown>): Record<string, unknown> {
  return { ...group, id: normalizeGroupId(group.id), createdAt: 0, updatedAt: 0 };
}

function normalizeViewOutput(value: unknown): unknown {
  const view = value as { nodes: Array<Record<string, unknown>> };
  return {
    nodes: view.nodes.map((node) =>
      node.type === "group"
        ? { ...node, group: normalizeGroupValue(node.group as Record<string, unknown>) }
        : node,
    ),
  };
}

function normalizeStructureOutput(value: unknown): unknown {
  const structure = value as {
    groups: Array<Record<string, unknown>>;
    members: Array<Record<string, unknown>>;
    topLevelOrders: unknown[];
  };
  return {
    ...structure,
    groups: structure.groups.map(normalizeGroupValue),
    // addedAt 来自 `Date.now()`（系统分组回填），跨进程不可比，归一为 0。
    members: structure.members.map((member) => ({ ...member, addedAt: 0 })),
  };
}

function snapshot(dbPath: string): Snapshot {
  const db = new DatabaseSync(dbPath);
  try {
    const rows = db
      .prepare(`SELECT ${TASK_COLUMNS.join(", ")} FROM tasks ORDER BY workspace_key, task_id`)
      .all() as unknown as Array<Record<string, unknown>>;
    const tasks = rows.map((row) => ({
      ...row,
      meta_json: readMetaJson(String(row.meta_json)),
    }));
    const taskGroups = (
      db
        .prepare("SELECT * FROM task_groups ORDER BY group_id")
        .all() as unknown as Array<Record<string, unknown>>
    )
      .map((row) => ({ ...row, group_id: normalizeGroupId(row.group_id), created_at: 0, updated_at: 0 }))
      // 归一生效后才出现重复 id（多个 createTaskGroup），按 (id,title) 补稳定序。
      .sort(
        (left, right) =>
          String(left.group_id).localeCompare(String(right.group_id)) ||
          String(left.title).toLowerCase().localeCompare(String(right.title).toLowerCase()),
      );
    const taskGroupMembers = (
      db
        .prepare("SELECT * FROM task_group_members ORDER BY workspace_key, task_id")
        .all() as unknown as Array<Record<string, unknown>>
    ).map((row) => ({
      ...row,
      group_id: normalizeGroupId(row.group_id),
      added_at: 0,
      created_at: 0,
      updated_at: 0,
    }));
    const taskGroupViewNodeOrders = (
      db
        .prepare("SELECT * FROM task_group_view_node_orders ORDER BY node_type, node_key")
        .all() as unknown as Array<Record<string, unknown>>
    )
      .map((row) => ({
        ...row,
        node_key: row.node_type === "group" ? normalizeGroupId(row.node_key) : row.node_key,
        created_at: 0,
        updated_at: 0,
      }))
      .sort(
        (left, right) =>
          String(left.node_type).localeCompare(String(right.node_type)) ||
          String(left.node_key).localeCompare(String(right.node_key)) ||
          Number(left.sort_order) - Number(right.sort_order),
      );
    return { tasks, taskGroups, taskGroupMembers, taskGroupViewNodeOrders };
  } finally {
    db.close();
  }
}

interface StepRecord {
  id: string;
  op: string;
  input: unknown;
  output?: unknown;
  error?: string;
  snapshot: Snapshot;
}

const steps: StepRecord[] = [];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function record(
  dbPath: string,
  id: string,
  op: string,
  input: unknown,
  run: () => Promise<unknown> | unknown,
  normalizeOutput?: (output: unknown) => unknown,
): Promise<void> {
  try {
    const raw = await run();
    const output = raw === undefined ? null : raw;
    steps.push({
      id,
      op,
      input,
      output: normalizeOutput ? normalizeOutput(output) : output,
      snapshot: snapshot(dbPath),
    });
  } catch (error) {
    steps.push({
      id,
      op,
      input,
      error: errorMessage(error),
      snapshot: snapshot(dbPath),
    });
  }
}

function taskDetail(meta: MetaSeed): Record<string, unknown> {
  return buildMeta(meta);
}

async function main(): Promise<void> {
  const workDir = mkdtempSync(path.join(tmpdir(), "task-index-fixtures-"));
  const dbPath = path.join(workDir, "tasks-index.sqlite");
  try {
    const seeding = new DatabaseSync(dbPath);
    try {
      runTasksDatabaseMigrations(seeding);
      insertSeedDatabase(seeding);
    } finally {
      seeding.close();
    }

    // 固定时钟并从 fixture 回放，保证重复生成字节稳定；归档 cutoff 仍能覆盖所有 seed 行
    // （seed 的 updated_at 都在 3000 以内）。
    const clockMs = 2_000_000_000_000;
    const repo = new TaskIndexRepo(dbPath);

    await record(dbPath, "list-all", "listTaskMetas", {}, () => repo.listTaskMetas({}));
    await record(dbPath, "get-active", "getTaskMeta", { workspacePath: WS_A, taskId: "task-active" }, () =>
      repo.getTaskMeta({ workspacePath: WS_A, taskId: "task-active" }),
    );
    await record(dbPath, "get-deleted", "getTaskMeta", { workspacePath: WS_A, taskId: "task-deleted" }, () =>
      repo.getTaskMeta({ workspacePath: WS_A, taskId: "task-deleted" }),
    );
    await record(dbPath, "get-missing", "getTaskMeta", { workspacePath: WS_A, taskId: "nope" }, () =>
      repo.getTaskMeta({ workspacePath: WS_A, taskId: "nope" }),
    );
    await record(dbPath, "list-workspace-a", "listTaskMetas", { workspacePath: WS_A }, () =>
      repo.listTaskMetas({ workspacePath: WS_A }),
    );
    await record(
      dbPath,
      "list-workspace-a-provider-glm-deleted",
      "listTaskMetas",
      { workspacePath: WS_A, provider: "glm", includeDeleted: true },
      () => repo.listTaskMetas({ workspacePath: WS_A, provider: "glm", includeDeleted: true }),
    );
    await record(dbPath, "list-pinned", "listTaskMetas", { workspacePath: WS_A, pinned: true }, () =>
      repo.listTaskMetas({ workspacePath: WS_A, pinned: true }),
    );
    await record(dbPath, "list-archived", "listTaskMetas", { workspacePath: WS_A, archived: true }, () =>
      repo.listTaskMetas({ workspacePath: WS_A, archived: true }),
    );
    await record(dbPath, "deleted-ids-a", "listDeletedTaskIds", { workspacePath: WS_A }, () =>
      repo.listDeletedTaskIds({ workspacePath: WS_A }),
    );
    await record(
      dbPath,
      "deleted-ids-a-claude",
      "listDeletedTaskIds",
      { workspacePath: WS_A, provider: "claude" },
      () => repo.listDeletedTaskIds({ workspacePath: WS_A, provider: "claude" }),
    );
    await record(dbPath, "sessions-by-automation", "listSessionsByAutomation", { automationId: "auto-1" }, () =>
      repo.listSessionsByAutomation("auto-1"),
    );

    const listQuery = (overrides: Record<string, unknown>) => ({
      kind: "active",
      workspaceScopes: [{ workspacePath: WS_A }],
      sortBy: "updated",
      ...overrides,
    });
    await record(dbPath, "query-active-a", "queryTaskList", listQuery({}), (): Promise<unknown> =>
      repo.queryTaskList(listQuery({})),
    );
    await record(
      dbPath,
      "query-pinned-a",
      "queryTaskList",
      listQuery({ kind: "pinned" }),
      () => repo.queryTaskList(listQuery({ kind: "pinned" })),
    );
    await record(
      dbPath,
      "query-archived-ac",
      "queryTaskList",
      listQuery({
        kind: "archived",
        workspaceScopes: [{ workspacePath: WS_A }, { workspacePath: WS_C }],
      }),
      () =>
        repo.queryTaskList(
          listQuery({
            kind: "archived",
            workspaceScopes: [{ workspacePath: WS_A }, { workspacePath: WS_C }],
          }),
        ),
    );
    await record(
      dbPath,
      "query-timeline-ab-created",
      "queryTaskList",
      listQuery({
        kind: "timeline",
        sortBy: "created",
        workspaceScopes: [
          { workspacePath: WS_A },
          { workspacePath: "/srv/ws", workspaceIdentity: WS_B },
        ],
      }),
      () =>
        repo.queryTaskList(
          listQuery({
            kind: "timeline",
            sortBy: "created",
            workspaceScopes: [
              { workspacePath: WS_A },
              { workspacePath: "/srv/ws", workspaceIdentity: WS_B },
            ],
          }),
        ),
    );
    await record(dbPath, "query-search-deploy", "queryTaskList", listQuery({ search: "deploy" }), () =>
      repo.queryTaskList(listQuery({ search: "deploy" })),
    );
    await record(dbPath, "query-search-alpha", "queryTaskList", listQuery({ search: "Alpha" }), () =>
      repo.queryTaskList(listQuery({ search: "Alpha" })),
    );
    await record(dbPath, "query-search-miss", "queryTaskList", listQuery({ search: "zzz" }), () =>
      repo.queryTaskList(listQuery({ search: "zzz" })),
    );
    await record(dbPath, "query-limit-2", "queryTaskList", listQuery({ limit: 2 }), () =>
      repo.queryTaskList(listQuery({ limit: 2 })),
    );
    await record(
      dbPath,
      "query-empty-scopes",
      "queryTaskList",
      listQuery({ workspaceScopes: [] }),
      () => repo.queryTaskList(listQuery({ workspaceScopes: [] })),
    );
    await record(
      dbPath,
      "query-remote-b",
      "queryTaskList",
      listQuery({
        kind: "timeline",
        workspaceScopes: [{ workspacePath: "/srv/ws", workspaceIdentity: WS_B }],
      }),
      () =>
        repo.queryTaskList(
          listQuery({
            kind: "timeline",
            workspaceScopes: [{ workspacePath: "/srv/ws", workspaceIdentity: WS_B }],
          }),
        ),
    );
    await record(
      dbPath,
      "query-purpose",
      "queryTaskList",
      listQuery({ workspaceScopes: [{ workspacePath: WS_A, workspacePurpose: "conversation" }] }),
      () =>
        repo.queryTaskList(
          listQuery({ workspaceScopes: [{ workspacePath: WS_A, workspacePurpose: "conversation" }] }),
        ),
    );

    await record(
      dbPath,
      "query-provider-glm",
      "queryTaskList",
      listQuery({ provider: "glm" }),
      () => repo.queryTaskList(listQuery({ provider: "glm" })),
    );
    await record(
      dbPath,
      "query-provider-claude",
      "queryTaskList",
      listQuery({ provider: "claude" }),
      () => repo.queryTaskList(listQuery({ provider: "claude" })),
    );

    // ── 分组视图与分组管理 ──
    const aScope = { workspacePath: WS_A };
    const groupedQuery = (overrides: Record<string, unknown>) => ({
      workspaceScopes: [aScope],
      ...overrides,
    });

    await record(dbPath, "grouped-has-run-before", "hasGroupedWorkspaceBootstrapRun", {}, () =>
      repo.hasGroupedWorkspaceBootstrapRun(),
    );
    await record(
      dbPath,
      "grouped-structure-before-bootstrap",
      "queryGroupedTaskViewStructure",
      groupedQuery({}),
      () => repo.queryGroupedTaskViewStructure(groupedQuery({}) as never),
      normalizeStructureOutput,
    );
    await record(
      dbPath,
      "grouped-view-a-bootstrap",
      "queryGroupedTaskView",
      groupedQuery({}),
      () => repo.queryGroupedTaskView(groupedQuery({}) as never),
      normalizeViewOutput,
    );
    await record(dbPath, "grouped-has-run-after", "hasGroupedWorkspaceBootstrapRun", {}, () =>
      repo.hasGroupedWorkspaceBootstrapRun(),
    );
    await record(
      dbPath,
      "grouped-structure-after-bootstrap",
      "queryGroupedTaskViewStructure",
      groupedQuery({}),
      () => repo.queryGroupedTaskViewStructure(groupedQuery({}) as never),
      normalizeStructureOutput,
    );
    await record(
      dbPath,
      "grouped-view-ac",
      "queryGroupedTaskView",
      { workspaceScopes: [aScope, { workspacePath: WS_C }] },
      () =>
        repo.queryGroupedTaskView({ workspaceScopes: [aScope, { workspacePath: WS_C }] }),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-view-all",
      "queryGroupedTaskView",
      { workspaceScopes: [], includeAllWorkspaces: true },
      () => repo.queryGroupedTaskView({ workspaceScopes: [], includeAllWorkspaces: true }),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-view-provider-glm",
      "queryGroupedTaskView",
      groupedQuery({ provider: "glm" }),
      () => repo.queryGroupedTaskView(groupedQuery({ provider: "glm" }) as never),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-structure-remote",
      "queryGroupedTaskViewStructure",
      { workspaceScopes: [{ workspacePath: "/srv/ws", workspaceIdentity: WS_B }] },
      () =>
        repo.queryGroupedTaskViewStructure({
          workspaceScopes: [{ workspacePath: "/srv/ws", workspaceIdentity: WS_B }],
        }),
      normalizeStructureOutput,
    );

    const betaRef = { workspacePath: WS_C, taskId: "task-beta" };
    await record(dbPath, "grouped-init-top-beta", "initializeGroupedTaskAtTop", betaRef, () =>
      repo.initializeGroupedTaskAtTop(betaRef),
    );
    await record(dbPath, "grouped-init-top-beta-again", "initializeGroupedTaskAtTop", betaRef, () =>
      repo.initializeGroupedTaskAtTop(betaRef),
    );
    await record(
      dbPath,
      "grouped-init-top-pinned",
      "initializeGroupedTaskAtTop",
      { workspacePath: WS_A, taskId: "task-pinned" },
      () => repo.initializeGroupedTaskAtTop({ workspacePath: WS_A, taskId: "task-pinned" }),
    );
    await record(
      dbPath,
      "grouped-init-top-member",
      "initializeGroupedTaskAtTop",
      { workspacePath: WS_A, taskId: "task-active" },
      () => repo.initializeGroupedTaskAtTop({ workspacePath: WS_A, taskId: "task-active" }),
    );
    await record(
      dbPath,
      "grouped-init-top-missing",
      "initializeGroupedTaskAtTop",
      { workspacePath: WS_A, taskId: "nope" },
      () => repo.initializeGroupedTaskAtTop({ workspacePath: WS_A, taskId: "nope" }),
    );

    const orderInput = {
      workspaceScopes: [aScope],
      topLevelNodes: [
        { type: "task", task: { workspacePath: WS_A, taskId: "task-offpeak" } },
        { type: "group", groupId: "g-legacy" },
      ],
      groups: [
        {
          groupId: "g-legacy",
          taskRefs: [
            { workspacePath: WS_A, taskId: "task-unread" },
            { workspacePath: WS_A, taskId: "task-extra-meta" },
          ],
        },
      ],
    };
    await record(
      dbPath,
      "grouped-apply-order-a",
      "applyGroupedTaskViewOrder",
      orderInput,
      () => repo.applyGroupedTaskViewOrder(orderInput as never),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-view-after-apply",
      "queryGroupedTaskView",
      groupedQuery({}),
      () => repo.queryGroupedTaskView(groupedQuery({}) as never),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-apply-order-unknown-group",
      "applyGroupedTaskViewOrder",
      {
        workspaceScopes: [aScope],
        topLevelNodes: [{ type: "group", groupId: "nope" }],
        groups: [],
      },
      () =>
        repo.applyGroupedTaskViewOrder({
          workspaceScopes: [aScope],
          topLevelNodes: [{ type: "group", groupId: "nope" }],
          groups: [],
        }),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-apply-order-out-of-scope",
      "applyGroupedTaskViewOrder",
      {
        workspaceScopes: [{ workspacePath: WS_C }],
        topLevelNodes: [
          { type: "task", task: { workspacePath: WS_A, taskId: "task-active" } },
        ],
        groups: [],
      },
      () =>
        repo.applyGroupedTaskViewOrder({
          workspaceScopes: [{ workspacePath: WS_C }],
          topLevelNodes: [{ type: "task", task: { workspacePath: WS_A, taskId: "task-active" } }],
          groups: [],
        }),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-apply-order-invisible",
      "applyGroupedTaskViewOrder",
      {
        workspaceScopes: [aScope],
        topLevelNodes: [
          { type: "task", task: { workspacePath: WS_A, taskId: "task-deleted" } },
        ],
        groups: [],
      },
      () =>
        repo.applyGroupedTaskViewOrder({
          workspaceScopes: [aScope],
          topLevelNodes: [{ type: "task", task: { workspacePath: WS_A, taskId: "task-deleted" } }],
          groups: [],
        }),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-apply-order-duplicate",
      "applyGroupedTaskViewOrder",
      {
        workspaceScopes: [aScope],
        topLevelNodes: [],
        groups: [
          { groupId: "g-legacy", taskRefs: [{ workspacePath: WS_A, taskId: "task-active" }] },
          { groupId: "g-legacy", taskRefs: [{ workspacePath: WS_A, taskId: "task-active" }] },
        ],
      },
      () =>
        repo.applyGroupedTaskViewOrder({
          workspaceScopes: [aScope],
          topLevelNodes: [],
          groups: [
            { groupId: "g-legacy", taskRefs: [{ workspacePath: WS_A, taskId: "task-active" }] },
            { groupId: "g-legacy", taskRefs: [{ workspacePath: WS_A, taskId: "task-active" }] },
          ],
        }),
      normalizeViewOutput,
    );
    await record(
      dbPath,
      "grouped-apply-order-provider-skip",
      "applyGroupedTaskViewOrder",
      {
        workspaceScopes: [aScope],
        provider: "claude",
        topLevelNodes: [
          { type: "task", task: { workspacePath: WS_A, taskId: "task-offpeak" } },
        ],
        groups: [{ groupId: "g-legacy", taskRefs: [{ workspacePath: WS_A, taskId: "task-unread" }] }],
      },
      () =>
        repo.applyGroupedTaskViewOrder({
          workspaceScopes: [aScope],
          provider: "claude",
          topLevelNodes: [
            { type: "task", task: { workspacePath: WS_A, taskId: "task-offpeak" } },
          ],
          groups: [{ groupId: "g-legacy", taskRefs: [{ workspacePath: WS_A, taskId: "task-unread" }] }],
        }),
      normalizeViewOutput,
    );

    await record(
      dbPath,
      "group-rename",
      "renameTaskGroup",
      { groupId: "g-legacy", title: "  renamed legacy  " },
      () => repo.renameTaskGroup({ groupId: "g-legacy", title: "  renamed legacy  " }),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );
    await record(
      dbPath,
      "group-rename-blank",
      "renameTaskGroup",
      { groupId: "g-legacy", title: "   " },
      () => repo.renameTaskGroup({ groupId: "g-legacy", title: "   " }),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );
    await record(
      dbPath,
      "group-rename-missing",
      "renameTaskGroup",
      { groupId: "nope", title: "x" },
      () => repo.renameTaskGroup({ groupId: "nope", title: "x" }),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );
    await record(
      dbPath,
      "group-color",
      "updateTaskGroupColor",
      { groupId: "g-legacy", color: "purple" },
      () => repo.updateTaskGroupColor({ groupId: "g-legacy", color: "purple" }),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );
    await record(
      dbPath,
      "group-color-invalid",
      "updateTaskGroupColor",
      { groupId: "g-legacy", color: "chartreuse" },
      () => repo.updateTaskGroupColor({ groupId: "g-legacy", color: "chartreuse" as never }),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );
    await record(
      dbPath,
      "group-color-missing",
      "updateTaskGroupColor",
      { groupId: "nope", color: "blue" },
      () => repo.updateTaskGroupColor({ groupId: "nope", color: "blue" }),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );

    const rootDraftMeta = taskDetail({
      taskId: "task-root-draft",
      title: "root draft",
      workspacePath: WS_A,
      createdAt: 3100,
      updatedAt: 3100,
      mode: "build",
      provider: "glm",
      status: "running",
    });
    await record(
      dbPath,
      "sync-at-top-new",
      "syncTaskMetaAtGroupedTop",
      { meta: rootDraftMeta, searchableText: "root draft body" },
      () =>
        repo.syncTaskMetaAtGroupedTop({
          meta: rootDraftMeta as never,
          searchableText: "root draft body",
        }),
    );
    await record(
      dbPath,
      "sync-at-top-again",
      "syncTaskMetaAtGroupedTop",
      { meta: rootDraftMeta },
      () => repo.syncTaskMetaAtGroupedTop({ meta: rootDraftMeta as never }),
    );

    await record(
      dbPath,
      "group-create",
      "createTaskGroup",
      { title: "  custom group  ", color: "green" },
      () => repo.createTaskGroup({ title: "  custom group  ", color: "green" }),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );
    await record(
      dbPath,
      "group-create-default",
      "createTaskGroup",
      {},
      () => repo.createTaskGroup(),
      (value) => normalizeGroupValue(value as Record<string, unknown>),
    );
    await record(
      dbPath,
      "group-delete",
      "deleteTaskGroup",
      { groupId: "g-legacy" },
      () => repo.deleteTaskGroup({ groupId: "g-legacy" }),
    );
    await record(
      dbPath,
      "group-delete-again",
      "deleteTaskGroup",
      { groupId: "g-legacy" },
      () => repo.deleteTaskGroup({ groupId: "g-legacy" }),
    );

    const seededMeta = taskDetail({
      taskId: "task-seeded",
      title: "seeded task",
      workspacePath: WS_A,
      createdAt: 3000,
      updatedAt: 3000,
      mode: "build",
      provider: "glm",
      status: "running",
    });
    await record(dbPath, "seed-new", "seedTaskMetaIfMissing", { meta: seededMeta }, () =>
      repo.seedTaskMetaIfMissing(seededMeta as never),
    );
    await record(
      dbPath,
      "seed-existing",
      "seedTaskMetaIfMissing",
      { meta: { ...seededMeta, title: "should be ignored" } },
      () => repo.seedTaskMetaIfMissing({ ...seededMeta, title: "should be ignored" } as never),
    );

    const syncedMeta = taskDetail({
      taskId: "task-synced",
      title: "synced task",
      workspacePath: WS_A,
      createdAt: 3001,
      updatedAt: 3001,
      mode: "edit",
      provider: "glm",
      status: "running",
    });
    await record(
      dbPath,
      "sync-new",
      "syncTaskMeta",
      { meta: syncedMeta, searchableText: "synced body" },
      () => repo.syncTaskMeta({ meta: syncedMeta as never, searchableText: "synced body" }),
    );
    await record(
      dbPath,
      "sync-monotonic-updated-at",
      "syncTaskMeta",
      {
        meta: taskDetail({
          taskId: "task-active",
          title: "agent lower",
          workspacePath: WS_A,
          createdAt: 900,
          updatedAt: 400,
          mode: "build",
          provider: "glm",
          status: "running",
        }),
      },
      () =>
        repo.syncTaskMeta({
          meta: taskDetail({
            taskId: "task-active",
            title: "agent lower",
            workspacePath: WS_A,
            createdAt: 900,
            updatedAt: 400,
            mode: "build",
            provider: "glm",
            status: "running",
          }) as never,
        }),
    );
    await record(
      dbPath,
      "sync-preserve-terminal",
      "syncTaskMeta",
      {
        meta: taskDetail({
          taskId: "task-pinned",
          title: "terminal snapshot",
          workspacePath: WS_A,
          createdAt: 950,
          updatedAt: 1050,
          mode: "plan",
          provider: "glm",
          status: "running",
        }),
      },
      () =>
        repo.syncTaskMeta({
          meta: taskDetail({
            taskId: "task-pinned",
            title: "terminal snapshot",
            workspacePath: WS_A,
            createdAt: 950,
            updatedAt: 1050,
            mode: "plan",
            provider: "glm",
            status: "running",
          }) as never,
        }),
    );
    await record(
      dbPath,
      "sync-preserve-user-title",
      "syncTaskMeta",
      {
        meta: taskDetail({
          taskId: "task-pinned",
          title: "overwritten by agent",
          workspacePath: WS_A,
          createdAt: 950,
          updatedAt: 1150,
          mode: "plan",
          provider: "glm",
          status: "completed",
        }),
      },
      () =>
        repo.syncTaskMeta({
          meta: taskDetail({
            taskId: "task-pinned",
            title: "overwritten by agent",
            workspacePath: WS_A,
            createdAt: 950,
            updatedAt: 1150,
            mode: "plan",
            provider: "glm",
            status: "completed",
          }) as never,
        }),
    );
    await record(
      dbPath,
      "sync-cron-marker",
      "syncTaskMeta",
      {
        meta: taskDetail({
          taskId: "task-cron-synced",
          title: "cron synced",
          workspacePath: WS_A,
          createdAt: 3002,
          updatedAt: 3002,
          mode: "build",
          provider: "glm",
          status: "running",
          cronAutomationId: "auto-2",
        }),
      },
      () =>
        repo.syncTaskMeta({
          meta: taskDetail({
            taskId: "task-cron-synced",
            title: "cron synced",
            workspacePath: WS_A,
            createdAt: 3002,
            updatedAt: 3002,
            mode: "build",
            provider: "glm",
            status: "running",
            cronAutomationId: "auto-2",
          }) as never,
        }),
    );
    await record(
      dbPath,
      "sync-offpeak-marker",
      "syncTaskMeta",
      {
        meta: taskDetail({
          taskId: "task-offpeak-synced",
          title: "off peak synced",
          workspacePath: WS_A,
          createdAt: 3003,
          updatedAt: 3003,
          mode: "build",
          provider: "glm",
          status: "running",
          offPeakTaskId: "op-2",
        }),
      },
      () =>
        repo.syncTaskMeta({
          meta: taskDetail({
            taskId: "task-offpeak-synced",
            title: "off peak synced",
            workspacePath: WS_A,
            createdAt: 3003,
            updatedAt: 3003,
            mode: "build",
            provider: "glm",
            status: "running",
            offPeakTaskId: "op-2",
          }) as never,
        }),
    );
    await record(
      dbPath,
      "sync-offpeak-remote",
      "syncTaskMeta",
      {
        meta: taskDetail({
          taskId: "task-offpeak-remote",
          title: "off peak remote",
          workspacePath: "/srv/ws",
          workspaceIdentity: WS_B,
          createdAt: 3004,
          updatedAt: 3004,
          mode: "build",
          provider: "glm",
          status: "running",
          offPeakTaskId: "op-3",
        }),
      },
      () =>
        repo.syncTaskMeta({
          meta: taskDetail({
            taskId: "task-offpeak-remote",
            title: "off peak remote",
            workspacePath: "/srv/ws",
            workspaceIdentity: WS_B,
            createdAt: 3004,
            updatedAt: 3004,
            mode: "build",
            provider: "glm",
            status: "running",
            offPeakTaskId: "op-3",
          }) as never,
        }),
    );
    await record(
      dbPath,
      "sync-target-null",
      "syncTaskMeta",
      {
        meta: {
          ...taskDetail({
            taskId: "task-extra-meta",
            title: "extra meta",
            workspacePath: WS_A,
            createdAt: 750,
            updatedAt: 1750,
            mode: "autoEdit",
            provider: "glm",
            status: "completed",
          }),
          target: null,
        },
      },
      () =>
        repo.syncTaskMeta({
          meta: {
            ...taskDetail({
              taskId: "task-extra-meta",
              title: "extra meta",
              workspacePath: WS_A,
              createdAt: 750,
              updatedAt: 1750,
              mode: "autoEdit",
              provider: "glm",
              status: "completed",
            }),
            target: null,
          } as never,
        }),
    );
    await record(
      dbPath,
      "sync-target-value",
      "syncTaskMeta",
      {
        meta: {
          ...taskDetail({
            taskId: "task-extra-meta",
            title: "extra meta",
            workspacePath: WS_A,
            createdAt: 750,
            updatedAt: 1760,
            mode: "autoEdit",
            provider: "glm",
            status: "completed",
          }),
          target: {
            sessionID: "s-extra",
            targetID: "g-extra",
            objective: "ship it",
            summaryTitle: "Ship",
            status: "active",
            tokenBudget: null,
            tokensUsed: 9,
            timeUsedSeconds: 4,
            time: { created: 1, updated: 3 },
          },
        },
      },
      () =>
        repo.syncTaskMeta({
          meta: {
            ...taskDetail({
              taskId: "task-extra-meta",
              title: "extra meta",
              workspacePath: WS_A,
              createdAt: 750,
              updatedAt: 1760,
              mode: "autoEdit",
              provider: "glm",
              status: "completed",
            }),
            target: {
              sessionID: "s-extra",
              targetID: "g-extra",
              objective: "ship it",
              summaryTitle: "Ship",
              status: "active",
              tokenBudget: null,
              tokensUsed: 9,
              timeUsedSeconds: 4,
              time: { created: 1, updated: 3 },
            },
          } as never,
        }),
    );
    await record(
      dbPath,
      "sync-searchable-preserved",
      "syncTaskMeta",
      {
        meta: taskDetail({
          taskId: "task-active",
          title: "alpha release notes",
          workspacePath: WS_A,
          createdAt: 900,
          updatedAt: 1005,
          mode: "build",
          provider: "glm",
          status: "running",
        }),
      },
      () =>
        repo.syncTaskMeta({
          meta: taskDetail({
            taskId: "task-active",
            title: "alpha release notes",
            workspacePath: WS_A,
            createdAt: 900,
            updatedAt: 1005,
            mode: "build",
            provider: "glm",
            status: "running",
          }) as never,
        }),
    );

    const unreadRef = { workspacePath: WS_A, taskId: "task-unread" };
    await record(
      dbPath,
      "update-unread-title",
      "updateTaskState",
      {
        ...unreadRef,
        patch: { title: "renamed unread", model: "glm-5", updatedAt: 1450, pinned: true },
      },
      () =>
        repo.updateTaskState({
          ...unreadRef,
          patch: { title: "renamed unread", model: "glm-5", updatedAt: 1450, pinned: true },
        }),
    );
    await record(
      dbPath,
      "update-unread-allocate",
      "updateTaskState",
      { ...unreadRef, patch: { unreadAt: 100 } },
      () => repo.updateTaskState({ ...unreadRef, patch: { unreadAt: 100 } }),
    );
    await record(
      dbPath,
      "update-unread-clear",
      "updateTaskState",
      { ...unreadRef, patch: { unreadAt: null } },
      () => repo.updateTaskState({ ...unreadRef, patch: { unreadAt: undefined } }),
    );
    await record(
      dbPath,
      "update-status-target",
      "updateTaskState",
      {
        workspacePath: WS_A,
        taskId: "task-identity-mismatch",
        patch: {
          status: "completed",
          lastError: { message: "boom", code: "E_BOOM" },
          target: {
            sessionID: "s1",
            targetID: "g1",
            objective: "do it",
            summaryTitle: null,
            status: "paused",
            tokenBudget: 10,
            tokensUsed: 1,
            timeUsedSeconds: 1,
            time: { created: 1, updated: 1 },
          },
          updatedAt: 1810,
        },
      },
      () =>
        repo.updateTaskState({
          workspacePath: WS_A,
          taskId: "task-identity-mismatch",
          patch: {
            status: "completed",
            lastError: { message: "boom", code: "E_BOOM" },
            target: {
              sessionID: "s1",
              targetID: "g1",
              objective: "do it",
              summaryTitle: null,
              status: "paused",
              tokenBudget: 10,
              tokensUsed: 1,
              timeUsedSeconds: 1,
              time: { created: 1, updated: 1 },
            },
            updatedAt: 1810,
          },
        }),
    );
    await record(
      dbPath,
      "update-delete",
      "updateTaskState",
      { workspacePath: WS_A, taskId: "task-archived", patch: { deleted: true } },
      () => repo.updateTaskState({ workspacePath: WS_A, taskId: "task-archived", patch: { deleted: true } }),
    );
    await record(
      dbPath,
      "update-missing",
      "updateTaskState",
      { workspacePath: WS_A, taskId: "nope", patch: { pinned: true } },
      () => repo.updateTaskState({ workspacePath: WS_A, taskId: "nope", patch: { pinned: true } }),
    );
    await record(
      dbPath,
      "update-deleted-row",
      "updateTaskState",
      { workspacePath: WS_A, taskId: "task-deleted", patch: { pinned: true } },
      () => repo.updateTaskState({ workspacePath: WS_A, taskId: "task-deleted", patch: { pinned: true } }),
    );

    const activeRef = { workspacePath: WS_A, taskId: "task-active" };
    await record(
      dbPath,
      "agent-title-accepted",
      "applyAgentPatch",
      { ...activeRef, patch: { title: "agent title", status: "completed", updatedAt: 2100 } },
      () =>
        repo.applyAgentPatch({
          ...activeRef,
          patch: { title: "agent title", status: "completed", updatedAt: 2100 },
        }),
    );
    await record(
      dbPath,
      "agent-title-rejected",
      "applyAgentPatch",
      { workspacePath: WS_A, taskId: "task-pinned", patch: { title: "should not win" } },
      () =>
        repo.applyAgentPatch({
          workspacePath: WS_A,
          taskId: "task-pinned",
          patch: { title: "should not win" },
        }),
    );
    await record(
      dbPath,
      "agent-empty-title",
      "applyAgentPatch",
      { ...activeRef, patch: { title: "" } },
      () => repo.applyAgentPatch({ ...activeRef, patch: { title: "" } }),
    );
    await record(
      dbPath,
      "agent-clear-and-target-null",
      "applyAgentPatch",
      { ...activeRef, patch: { lastError: null, target: null } },
      () => repo.applyAgentPatch({ ...activeRef, patch: { lastError: undefined, target: null } }),
    );
    await record(
      dbPath,
      "agent-missing",
      "applyAgentPatch",
      { workspacePath: WS_A, taskId: "nope", patch: { title: "x" } },
      () => repo.applyAgentPatch({ workspacePath: WS_A, taskId: "nope", patch: { title: "x" } }),
    );
    await record(
      dbPath,
      "agent-deleted",
      "applyAgentPatch",
      { workspacePath: WS_A, taskId: "task-deleted", patch: { title: "x" } },
      () => repo.applyAgentPatch({ workspacePath: WS_A, taskId: "task-deleted", patch: { title: "x" } }),
    );

    const pinnedRef = { workspacePath: WS_A, taskId: "task-pinned" };
    await record(
      dbPath,
      "clear-unread-match",
      "clearTaskUnreadIfMatches",
      { ...pinnedRef, expectedUnreadAt: 5000 },
      () => repo.clearTaskUnreadIfMatches({ ...pinnedRef, expectedUnreadAt: 5000 }),
    );
    await record(
      dbPath,
      "clear-unread-mismatch",
      "clearTaskUnreadIfMatches",
      { ...pinnedRef, expectedUnreadAt: 5000 },
      () => repo.clearTaskUnreadIfMatches({ ...pinnedRef, expectedUnreadAt: 5000 }),
    );
    await record(
      dbPath,
      "clear-unread-missing",
      "clearTaskUnreadIfMatches",
      { workspacePath: WS_A, taskId: "nope", expectedUnreadAt: 1 },
      () => repo.clearTaskUnreadIfMatches({ workspacePath: WS_A, taskId: "nope", expectedUnreadAt: 1 }),
    );
    await record(
      dbPath,
      "clear-unread-deleted",
      "clearTaskUnreadIfMatches",
      { workspacePath: WS_A, taskId: "task-deleted", expectedUnreadAt: 1 },
      () =>
        repo.clearTaskUnreadIfMatches({ workspacePath: WS_A, taskId: "task-deleted", expectedUnreadAt: 1 }),
    );

    const archived2 = { workspacePath: WS_C, taskId: "task-archived-2" };
    await record(dbPath, "delete-archived", "deleteArchivedTask", archived2, () =>
      repo.deleteArchivedTask(archived2),
    );
    await record(dbPath, "delete-archived-again", "deleteArchivedTask", archived2, () =>
      repo.deleteArchivedTask(archived2),
    );
    await record(
      dbPath,
      "delete-not-archived",
      "deleteArchivedTask",
      { workspacePath: WS_C, taskId: "task-beta" },
      () => repo.deleteArchivedTask({ workspacePath: WS_C, taskId: "task-beta" }),
    );
    await record(
      dbPath,
      "delete-missing",
      "deleteArchivedTask",
      { workspacePath: WS_C, taskId: "nope" },
      () => repo.deleteArchivedTask({ workspacePath: WS_C, taskId: "nope" }),
    );

    await record(
      dbPath,
      "archive-stale-a",
      "archiveStaleTasks",
      { workspacePath: WS_A, olderThanDays: 1, provider: "glm" },
      () => repo.archiveStaleTasks({ workspacePath: WS_A, olderThanDays: 1, provider: "glm" }),
    );
    await record(
      dbPath,
      "archive-stale-fraction",
      "archiveStaleTasks",
      { workspacePath: WS_A, olderThanDays: 0.4 },
      () => repo.archiveStaleTasks({ workspacePath: WS_A, olderThanDays: 0.4 }),
    );
    await record(
      dbPath,
      "archive-stale-provider",
      "archiveStaleTasks",
      { workspacePath: "/srv/ws", workspaceIdentity: WS_B, olderThanDays: 1, provider: "claude" },
      () =>
        repo.archiveStaleTasks({
          workspacePath: "/srv/ws",
          workspaceIdentity: WS_B,
          olderThanDays: 1,
          provider: "claude",
        }),
    );

    await record(dbPath, "close", "close", {}, () => {
      repo.close();
      return null;
    });
    await record(dbPath, "reopen-list", "listTaskMetas", { workspacePath: WS_A }, () =>
      repo.listTaskMetas({ workspacePath: WS_A }),
    );
    repo.close();

    const fixture = {
      generatedBy: "scripts/generate-task-index-fixtures.ts",
      clockMs,
      seeds: {
        tasks: taskSeeds,
        offPeakTasks: offPeakSeeds,
        taskGroups: taskGroupSeeds,
        taskGroupMembers: taskGroupMemberSeeds,
        taskGroupViewNodeOrders: taskGroupOrderSeeds,
      },
      steps,
    };
    mkdirSync(path.dirname(OUTPUT), { recursive: true });
    writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
    console.log(
      `[task-index] steps=${steps.length} tasks=${taskSeeds.length} → ${path.relative(ROOT_DIR, OUTPUT)}`,
    );
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

await main();
