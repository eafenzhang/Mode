/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 seed 与逐步输入/输出，拆分会降低可核对性。 */
/**
 * 生成 `off-peak-repo` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/session/offPeakTaskRepo.ts`
 * - `packages/shared/src/off-peak-types.ts`
 * - `packages/shared/src/model-selection.ts`
 *
 * ```bash
 * pnpm off-peak:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p off-peak-repo"
 * ```
 *
 * 时间戳策略：所有写入口都显式传 `now`，Rust 侧用同一值重放；
 * `create` 一律显式给 `offPeakTaskId`，避免 `randomUUID` 造成不稳定。
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

import { OffPeakTaskRepo } from "../packages/services/src/session/offPeakTaskRepo.js";
import { runTasksDatabaseMigrations } from "../packages/services/src/session/tasksDatabase/migrations.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "off-peak-repo", "fixtures", "off-peak.json");

const WS_A = "D:/ws-alpha";
const WS_B = "remote:ssh:box:22:me:/srv/ws";

const SELECTION = {
  providerId: "zai-api",
  modelId: "glm-4.6",
  options: { reasoningLevel: "high" },
};
const SELECTION_PLAIN = { providerId: "zai-api", modelId: "glm-4.6" };

interface Snapshot {
  offPeakTasks: Array<Record<string, unknown>>;
  tasks: Array<Record<string, unknown>>;
}

function snapshot(dbPath: string): Snapshot {
  const db = new DatabaseSync(dbPath);
  try {
    const offPeakTasks = db
      .prepare("SELECT * FROM off_peak_tasks ORDER BY off_peak_task_id")
      .all() as unknown as Array<Record<string, unknown>>;
    const tasks = db
      .prepare("SELECT workspace_key, workspace_path, workspace_identity, task_id, title FROM tasks ORDER BY workspace_key, task_id")
      .all() as unknown as Array<Record<string, unknown>>;
    return { offPeakTasks, tasks };
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
): Promise<void> {
  try {
    const output = await run();
    steps.push({
      id,
      op,
      input,
      output: output === undefined ? null : output,
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

function seedLegacyRow(
  db: DatabaseSync,
  row: {
    id: string;
    modelSelection: string | null;
    status?: string;
    sessionId?: string | null;
    startedAt?: number | null;
  },
): void {
  db.prepare(
    `INSERT INTO off_peak_tasks (
       off_peak_task_id, server_ticket_id, title, conversation_id, session_id,
       prompt, permission_mode, model, thought_level, model_selection,
       workspace_key, workspace_path, workspace_identity,
       status, queued_at, started_at, claim_running, attempt_count, created_at, updated_at
     ) VALUES (?, NULL, ?, NULL, ?, ?, 'build', 'legacy-model', 'low', ?,
       ?, ?, NULL, ?, 10, ?, 0, 0, 10, 10)`,
  ).run(
    row.id,
    `legacy ${row.id}`,
    row.sessionId ?? null,
    "legacy prompt",
    row.modelSelection,
    WS_A,
    WS_A,
    row.status ?? "queued",
    row.startedAt ?? null,
  );
}

async function main(): Promise<void> {
  const workDir = mkdtempSync(path.join(tmpdir(), "off-peak-fixtures-"));
  const dbPath = path.join(workDir, "tasks-index.sqlite");
  try {
    const seeding = new DatabaseSync(dbPath);
    try {
      runTasksDatabaseMigrations(seeding);
      const insertTask = seeding.prepare(
        `INSERT INTO tasks (
           workspace_key, workspace_path, workspace_identity, task_id, title, task_status,
           provider, mode, created_at, updated_at, meta_json
         ) VALUES (?, ?, NULL, ?, ?, 'running', 'glm', 'build', 1, 1, '{}')`,
      );
      insertTask.run(WS_A, WS_A, "session-bound", "bound session title");
      insertTask.run(WS_A, WS_A, "session-empty-title", "");
      seedLegacyRow(seeding, {
        id: "legacy-null-selection",
        modelSelection: null,
        sessionId: "session-bound",
      });
      seedLegacyRow(seeding, { id: "legacy-bad-json", modelSelection: "{oops" });
      seedLegacyRow(seeding, {
        id: "legacy-running-started",
        modelSelection: JSON.stringify(SELECTION_PLAIN),
        status: "running",
        startedAt: 20,
      });
    } finally {
      seeding.close();
    }

    const repo = new OffPeakTaskRepo(dbPath);

    const createParams = (overrides: Record<string, unknown> = {}) => ({
      title: "off peak task",
      prompt: "do the thing",
      permissionMode: "build",
      modelSelection: SELECTION,
      workspacePath: WS_A,
      ...overrides,
    });

    await record(
      dbPath,
      "create-a",
      "create",
      { params: createParams(), options: { now: 1000, offPeakTaskId: "op-a" } },
      () => repo.create(createParams() as never, { now: 1000, offPeakTaskId: "op-a" }),
    );
    await record(
      dbPath,
      "create-b",
      "create",
      {
        params: createParams({ workspacePath: "/srv/ws", workspaceIdentity: WS_B, title: "remote" }),
        options: {
          now: 1100,
          offPeakTaskId: "op-b",
          serverTicketId: "ticket-1",
          queuePosition: 4,
          registeredAt: 1050,
          schedulable: false,
        },
      },
      () =>
        repo.create(
          createParams({
            workspacePath: "/srv/ws",
            workspaceIdentity: WS_B,
            title: "remote",
          }) as never,
          {
            now: 1100,
            offPeakTaskId: "op-b",
            serverTicketId: "ticket-1",
            queuePosition: 4,
            registeredAt: 1050,
            schedulable: false,
          },
        ),
    );
    await record(
      dbPath,
      "create-bound",
      "create",
      {
        params: createParams({ boundSessionId: "session-bound" }),
        options: { now: 1200, offPeakTaskId: "op-c", schedulable: true },
      },
      () =>
        repo.create(createParams({ boundSessionId: "session-bound" }) as never, {
          now: 1200,
          offPeakTaskId: "op-c",
          schedulable: true,
        }),
    );

    await record(dbPath, "list-all", "list", {}, () => repo.list());
    await record(dbPath, "list-a", "list", { workspacePath: WS_A }, () =>
      repo.list({ workspacePath: WS_A }),
    );
    await record(
      dbPath,
      "list-remote",
      "list",
      { workspacePath: "/srv/ws", workspaceIdentity: WS_B },
      () => repo.list({ workspacePath: "/srv/ws", workspaceIdentity: WS_B }),
    );
    await record(dbPath, "get-a", "get", { offPeakTaskId: "op-a" }, () => repo.get("op-a"));
    await record(dbPath, "get-missing", "get", { offPeakTaskId: "nope" }, () =>
      repo.get("nope"),
    );
    await record(dbPath, "count-non-terminal", "countNonTerminal", {}, () =>
      repo.countNonTerminal(),
    );
    await record(dbPath, "count-active", "countActive", {}, () => repo.countActive());
    await record(
      dbPath,
      "has-active-bound-true",
      "hasActiveBoundTask",
      { workspaceKey: WS_A, sessionId: "session-bound" },
      () => repo.hasActiveBoundTask(WS_A, "session-bound"),
    );
    await record(
      dbPath,
      "has-active-bound-false",
      "hasActiveBoundTask",
      { workspaceKey: WS_A, sessionId: "session-empty-title" },
      () => repo.hasActiveBoundTask(WS_A, "session-empty-title"),
    );

    await record(
      dbPath,
      "update-editable-a",
      "updateEditableFields",
      {
        offPeakTaskId: "op-a",
        fields: {
          title: "renamed",
          prompt: "new prompt",
          permissionMode: "edit",
          modelSelection: SELECTION_PLAIN,
        },
        options: { now: 1300 },
      },
      () =>
        repo.updateEditableFields(
          "op-a",
          {
            title: "renamed",
            prompt: "new prompt",
            permissionMode: "edit",
            modelSelection: SELECTION_PLAIN as never,
          },
          { now: 1300 },
        ),
    );
    await record(
      dbPath,
      "update-editable-missing",
      "updateEditableFields",
      { offPeakTaskId: "nope", fields: { title: "x" }, options: { now: 1300 } },
      () => repo.updateEditableFields("nope", { title: "x" }, { now: 1300 }),
    );
    await record(
      dbPath,
      "update-editable-clear",
      "updateEditableFields",
      { offPeakTaskId: "op-a", fields: { modelSelection: null }, options: { now: 1300 } },
      () => repo.updateEditableFields("op-a", { modelSelection: null }, { now: 1300 }),
    );
    await record(
      dbPath,
      "update-editable-legacy-missing-selection",
      "updateEditableFields",
      { offPeakTaskId: "legacy-null-selection", fields: { title: "x" }, options: { now: 1300 } },
      () => repo.updateEditableFields("legacy-null-selection", { title: "x" }, { now: 1300 }),
    );

    await record(
      dbPath,
      "update-snapshot-a",
      "updateSchedulingSnapshot",
      {
        offPeakTaskId: "op-a",
        patch: {
          schedulable: true,
          queuePosition: 3,
          nextPollAt: 1500,
          serverTicketId: "ticket-a",
          registeredAt: 1250,
          now: 1400,
        },
      },
      () =>
        repo.updateSchedulingSnapshot("op-a", {
          schedulable: true,
          queuePosition: 3,
          nextPollAt: 1500,
          serverTicketId: "ticket-a",
          registeredAt: 1250,
          now: 1400,
        }),
    );
    await record(
      dbPath,
      "update-snapshot-missing",
      "updateSchedulingSnapshot",
      { offPeakTaskId: "nope", patch: { schedulable: true } },
      () => repo.updateSchedulingSnapshot("nope", { schedulable: true }),
    );

    await record(dbPath, "claim-due", "claimDue", { now: 2000 }, () => repo.claimDue(2000));
    await record(dbPath, "claim-due-again", "claimDue", { now: 2001 }, () =>
      repo.claimDue(2001),
    );

    await record(
      dbPath,
      "mark-running-a",
      "markRunning",
      {
        offPeakTaskId: "op-a",
        options: {
          startedAt: 2100,
          conversationId: "conv-a",
          sessionId: "session-a",
          serverTicketId: "ticket-a2",
        },
      },
      () =>
        repo.markRunning("op-a", {
          startedAt: 2100,
          conversationId: "conv-a",
          sessionId: "session-a",
          serverTicketId: "ticket-a2",
        }),
    );
    await record(
      dbPath,
      "mark-running-a-again",
      "markRunning",
      { offPeakTaskId: "op-a", options: { startedAt: 2200 } },
      () => repo.markRunning("op-a", { startedAt: 2200 }),
    );
    await record(dbPath, "count-active-running", "countActive", {}, () =>
      repo.countActive(),
    );
    await record(
      dbPath,
      "mark-terminal-a",
      "markTerminal",
      {
        offPeakTaskId: "op-a",
        options: { status: "completed", endedAt: 2300, filesChanged: 3 },
      },
      () =>
        repo.markTerminal("op-a", { status: "completed", endedAt: 2300, filesChanged: 3 }),
    );
    await record(
      dbPath,
      "mark-terminal-a-again",
      "markTerminal",
      { offPeakTaskId: "op-a", options: { status: "failed", endedAt: 2400 } },
      () => repo.markTerminal("op-a", { status: "failed", endedAt: 2400 }),
    );
    await record(
      dbPath,
      "mark-terminal-c-dispatch-error",
      "markTerminal",
      {
        offPeakTaskId: "op-c",
        options: {
          status: "failed",
          endedAt: 2500,
          failureReason: "dispatch boom",
          dispatchError: "boom",
        },
      },
      () =>
        repo.markTerminal("op-c", {
          status: "failed",
          endedAt: 2500,
          failureReason: "dispatch boom",
          dispatchError: "boom",
        }),
    );
    await record(
      dbPath,
      "mark-terminal-legacy-running",
      "markTerminal",
      { offPeakTaskId: "legacy-running-started", options: { status: "cancelled", endedAt: 2600 } },
      () => repo.markTerminal("legacy-running-started", { status: "cancelled", endedAt: 2600 }),
    );

    await record(
      dbPath,
      "release-claim-legacy",
      "releaseClaim",
      { offPeakTaskId: "legacy-running-started", options: { error: "x", now: 2700 } },
      () => repo.releaseClaim("legacy-running-started", { error: "x", now: 2700 }),
    );
    await record(
      dbPath,
      "release-claim-no-claim",
      "releaseClaim",
      { offPeakTaskId: "op-b", options: { now: 2700 } },
      () => repo.releaseClaim("op-b", { now: 2700 }),
    );

    await record(dbPath, "recover-interrupted", "recoverInterrupted", { now: 3000 }, () =>
      repo.recoverInterrupted(3000),
    );
    await record(dbPath, "list-non-terminal", "listNonTerminal", {}, () =>
      repo.listNonTerminal(),
    );

    await record(
      dbPath,
      "requeue-legacy",
      "requeueForContinuation",
      { offPeakTaskId: "legacy-running-started", options: { now: 3100 } },
      () => repo.requeueForContinuation("legacy-running-started", { now: 3100 }),
    );
    await record(
      dbPath,
      "requeue-legacy-again",
      "requeueForContinuation",
      { offPeakTaskId: "legacy-running-started", options: { now: 3100 } },
      () => repo.requeueForContinuation("legacy-running-started", { now: 3100 }),
    );
    await record(
      dbPath,
      "pause-b",
      "setPaused",
      { offPeakTaskId: "op-b", paused: true, options: { now: 3200 } },
      () => repo.setPaused("op-b", true, { now: 3200 }),
    );
    await record(
      dbPath,
      "pause-b-again",
      "setPaused",
      { offPeakTaskId: "op-b", paused: true, options: { now: 3200 } },
      () => repo.setPaused("op-b", true, { now: 3200 }),
    );
    await record(
      dbPath,
      "resume-b",
      "setPaused",
      { offPeakTaskId: "op-b", paused: false, options: { now: 3300 } },
      () => repo.setPaused("op-b", false, { now: 3300 }),
    );

    await record(
      dbPath,
      "invalidate-same",
      "invalidateModelSelection",
      { offPeakTaskId: "op-b", selection: SELECTION, options: { now: 3400 } },
      () => repo.invalidateModelSelection("op-b", SELECTION as never, { now: 3400 }),
    );
    await record(
      dbPath,
      "invalidate-different",
      "invalidateModelSelection",
      { offPeakTaskId: "op-b", selection: SELECTION_PLAIN, options: { now: 3500 } },
      () => repo.invalidateModelSelection("op-b", SELECTION_PLAIN as never, { now: 3500 }),
    );
    await record(
      dbPath,
      "invalidate-missing",
      "invalidateModelSelection",
      { offPeakTaskId: "nope", selection: SELECTION, options: { now: 3500 } },
      () => repo.invalidateModelSelection("nope", SELECTION as never, { now: 3500 }),
    );

    await record(
      dbPath,
      "history-deleted-unstarted",
      "markHistoryDeleted",
      { offPeakTaskId: "op-b", options: { now: 3600 } },
      () => repo.markHistoryDeleted("op-b", { now: 3600 }),
    );
    await record(
      dbPath,
      "history-deleted-started",
      "markHistoryDeleted",
      { offPeakTaskId: "op-a", options: { now: 3600 } },
      () => repo.markHistoryDeleted("op-a", { now: 3600 }),
    );
    await record(
      dbPath,
      "history-deleted-again",
      "markHistoryDeleted",
      { offPeakTaskId: "op-a", options: { now: 3700 } },
      () => repo.markHistoryDeleted("op-a", { now: 3700 }),
    );
    await record(
      dbPath,
      "history-deleted-missing",
      "markHistoryDeleted",
      { offPeakTaskId: "nope", options: { now: 3700 } },
      () => repo.markHistoryDeleted("nope", { now: 3700 }),
    );

    await record(dbPath, "settle-a", "markSettled", { offPeakTaskId: "op-a", settledAt: 4000 }, () =>
      repo.markSettled("op-a", 4000),
    );
    await record(dbPath, "settle-queued", "markSettled", { offPeakTaskId: "op-b", settledAt: 4000 }, () =>
      repo.markSettled("op-b", 4000),
    );
    await record(dbPath, "list-unsettled", "listUnsettledTerminal", {}, () =>
      repo.listUnsettledTerminal(),
    );

    await record(dbPath, "delete-b", "delete", { offPeakTaskId: "op-b" }, () =>
      repo.delete("op-b"),
    );
    await record(dbPath, "get-b-after-delete", "get", { offPeakTaskId: "op-b" }, () =>
      repo.get("op-b"),
    );
    await record(dbPath, "close", "close", {}, () => {
      repo.close();
      return null;
    });
    await record(dbPath, "reopen-list", "list", {}, () => repo.list());
    repo.close();

    const fixture = {
      generatedBy: "scripts/generate-off-peak-fixtures.ts",
      clockMs: 2_000_000_000_000,
      steps,
    };
    mkdirSync(path.dirname(OUTPUT), { recursive: true });
    writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
    console.log(
      `[off-peak] steps=${steps.length} → ${path.relative(ROOT_DIR, OUTPUT)}`,
    );
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

await main();
