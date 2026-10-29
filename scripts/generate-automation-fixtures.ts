/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 seed 与逐步输入/输出，拆分会降低可核对性。 */
/**
 * 生成 `automation-repo` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/session/automationRepo.ts`
 * - `packages/shared/src/automation-types.ts`
 *
 * ```bash
 * pnpm automation:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p automation-repo"
 * ```
 *
 * 时间戳策略：仓库内部大量使用 `Date.now()`；生成期把 `Date.now` 固定为 `clockMs`，
 * Rust 侧用同一时钟重放。`create` / `runNow` 的主键含 `randomUUID()`，输出与快照里
 * 统一归一为 `automation-normalized` / `:manual:normalized`。
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

import { AutomationRepo } from "../packages/services/src/session/automationRepo.js";
import { runTasksDatabaseMigrations } from "../packages/services/src/session/tasksDatabase/migrations.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "automation-repo", "fixtures", "automation.json");

const WS_A = "D:/ws-alpha";
const WS_B = "remote:ssh:box:22:me:/srv/ws";
const CLOCK = 2_000_000_000_000;

const SELECTION = {
  providerId: "zai-api",
  modelId: "glm-4.6",
  options: { reasoningLevel: "high" },
};

const UUID_RE = /^automation-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function normalizeString(value: string): string {
  if (UUID_RE.test(value)) return "automation-normalized";
  const manual = /^(.*):manual:([0-9a-f-]{36})$/i.exec(value);
  if (manual) return `${manual[1]}:manual:normalized`;
  return value;
}

function normalizeValue(value: unknown): unknown {
  if (typeof value === "string") return normalizeString(value);
  if (Array.isArray(value)) return value.map(normalizeValue);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      out[key] = normalizeValue(child);
    }
    return out;
  }
  return value;
}

interface Snapshot {
  automations: Array<Record<string, unknown>>;
  automationRuns: Array<Record<string, unknown>>;
}

function snapshot(dbPath: string): Snapshot {
  const db = new DatabaseSync(dbPath);
  try {
    const parseJson = (raw: unknown) => {
      if (typeof raw !== "string") return raw;
      try {
        return JSON.parse(raw) as unknown;
      } catch {
        return { unparsable: raw };
      }
    };
    const automations = (
      db.prepare("SELECT * FROM automations ORDER BY automation_id").all() as unknown as Array<
        Record<string, unknown>
      >
    ).map((row) => ({
      ...row,
      automation_id: normalizeString(String(row.automation_id)),
      schedule_rule: parseJson(row.schedule_rule),
      bot_delivery_target: parseJson(row.bot_delivery_target),
    }));
    const automationRuns = (
      db.prepare("SELECT * FROM automation_runs ORDER BY run_id").all() as unknown as Array<
        Record<string, unknown>
      >
    ).map((row) => ({
      ...row,
      run_id: normalizeString(String(row.run_id)),
    }));
    return { automations, automationRuns };
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
      output: normalizeValue(output === undefined ? null : output),
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

function seedAutomation(
  db: DatabaseSync,
  row: {
    id: string;
    title?: string;
    modelSelection?: string | null;
    mode?: string | null;
    botDeliveryTarget?: string | null;
    workspaceKey?: string;
    workspacePath?: string;
    workspaceIdentity?: string | null;
    targetTaskId?: string | null;
    recurring?: number;
    maxRuns?: number | null;
    endAt?: number | null;
    scheduleRule?: string | null;
    scheduleEditedByUser?: number;
    runCount?: number;
    scheduledRunCount?: number;
    enabled?: number;
    lifecycleStatus?: string;
    nextRunAt?: number | null;
    lastRunAt?: number | null;
    running?: number;
    claimedAt?: number | null;
    dispatchStatus?: string;
    dispatchAttempts?: number;
    retryAt?: number | null;
    createdAt: number;
  },
): void {
  const workspaceKey = row.workspaceKey ?? WS_A;
  const workspacePath = row.workspacePath ?? WS_A;
  db.prepare(
    `INSERT INTO automations (
       automation_id, title, cron_expr, prompt, model, provider, model_selection,
       workspace_key, workspace_path, workspace_identity, target_task_id, bot_delivery_target,
       location_kind, recurring, max_runs, end_at, schedule_rule, schedule_edited_by_user,
       run_count, scheduled_run_count, enabled, lifecycle_status, next_run_at, last_run_at,
       running, claimed_at, dispatch_status, dispatch_attempts, retry_at, last_error,
       mode, thought_level, created_at, updated_at
     ) VALUES (?, ?, '*/5 * * * *', ?, NULL, NULL, ?, ?, ?, ?, ?, ?, 'local', ?, ?, ?, ?, ?,
       ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, 'low', ?, ?)`,
  ).run(
    row.id,
    row.title ?? `title ${row.id}`,
    `prompt ${row.id}`,
    row.modelSelection === undefined ? JSON.stringify(SELECTION) : row.modelSelection,
    workspaceKey,
    workspacePath,
    row.workspaceIdentity ?? null,
    row.targetTaskId ?? null,
    row.botDeliveryTarget ?? null,
    row.recurring ?? 1,
    row.maxRuns ?? null,
    row.endAt ?? null,
    row.scheduleRule ?? null,
    row.scheduleEditedByUser ?? 0,
    row.runCount ?? 0,
    row.scheduledRunCount ?? 0,
    row.enabled ?? 1,
    row.lifecycleStatus ?? "active",
    row.nextRunAt ?? 5_000_000,
    row.lastRunAt ?? null,
    row.running ?? 0,
    row.claimedAt ?? null,
    row.dispatchStatus ?? "idle",
    row.dispatchAttempts ?? 0,
    row.retryAt ?? null,
    row.mode === undefined ? "build" : row.mode,
    row.createdAt,
    row.createdAt,
  );
}

function seedRun(
  db: DatabaseSync,
  row: {
    runId: string;
    automationId: string;
    workspaceKey?: string;
    scheduledAt?: number | null;
    trigger?: string;
    dispatchStatus?: string;
    outcome?: string | null;
    sessionId?: string | null;
    error?: string | null;
    attempts?: number;
    createdAt: number;
  },
): void {
  db.prepare(
    `INSERT INTO automation_runs (
       run_id, automation_id, workspace_key, scheduled_at, trigger, model_selection,
       dispatch_status, outcome, session_id, error, attempts, created_at, updated_at
     ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    row.runId,
    row.automationId,
    row.workspaceKey ?? WS_A,
    row.scheduledAt ?? null,
    row.trigger ?? "schedule",
    row.dispatchStatus ?? "claimed",
    row.outcome ?? null,
    row.sessionId ?? null,
    row.error ?? null,
    row.attempts ?? 0,
    row.createdAt,
    row.createdAt,
  );
}

async function main(): Promise<void> {
  const workDir = mkdtempSync(path.join(tmpdir(), "automation-fixtures-"));
  const dbPath = path.join(workDir, "tasks-index.sqlite");
  const realNow = Date.now;
  Date.now = () => CLOCK;
  try {
    const seeding = new DatabaseSync(dbPath);
    try {
      runTasksDatabaseMigrations(seeding);
      seedAutomation(seeding, {
        id: "a-1",
        title: "alpha",
        targetTaskId: "session-1",
        botDeliveryTarget: JSON.stringify({
          provider: "feishu",
          botId: "bot-1",
          providerUserId: "u-1",
          chatType: "private",
        }),
        scheduleRule: JSON.stringify({
          unit: "daily",
          interval: 1,
          hour: 9,
          minute: 0,
          anchorAt: 1000,
        }),
        scheduleEditedByUser: 1,
        runCount: 2,
        scheduledRunCount: 2,
        createdAt: 100,
      });
      seedAutomation(seeding, {
        id: "a-2",
        modelSelection: "null",
        mode: "",
        recurring: 0,
        maxRuns: 2,
        createdAt: 200,
      });
      seedAutomation(seeding, { id: "a-3", endAt: 500, createdAt: 300 });
      seedAutomation(seeding, {
        id: "a-4",
        enabled: 0,
        lifecycleStatus: "paused",
        createdAt: 400,
      });
      seedAutomation(seeding, {
        id: "a-5",
        modelSelection: null,
        enabled: 0,
        lifecycleStatus: "paused",
        createdAt: 500,
      });
      seedAutomation(seeding, {
        id: "a-6",
        botDeliveryTarget: "{oops",
        retryAt: 400_000,
        dispatchStatus: "failed_to_dispatch",
        dispatchAttempts: 1,
        createdAt: 600,
      });
      seedAutomation(seeding, { id: "a-7", dispatchAttempts: 4, createdAt: 700 });
      seedAutomation(seeding, {
        id: "a-8",
        running: 1,
        claimedAt: 100,
        createdAt: 800,
      });
      seedAutomation(seeding, { id: "a-9", createdAt: 900 });
      seedAutomation(seeding, {
        id: "a-10",
        workspaceKey: WS_B,
        workspacePath: "/srv/ws",
        workspaceIdentity: WS_B,
        createdAt: 1000,
      });
      seedAutomation(seeding, {
        id: "a-11",
        enabled: 0,
        lifecycleStatus: "paused",
        createdAt: 1100,
      });
      seedRun(seeding, {
        runId: "a-11:manual:seed",
        automationId: "a-11",
        scheduledAt: 1100,
        trigger: "manual",
        createdAt: 1100,
      });
    } finally {
      seeding.close();
    }

    const repo = new AutomationRepo(dbPath);

    await record(dbPath, "list-all", "list", {}, () => repo.list());
    await record(dbPath, "list-a", "list", { workspacePath: WS_A }, () =>
      repo.list({ workspacePath: WS_A }),
    );
    await record(
      dbPath,
      "list-b",
      "list",
      { workspacePath: "/srv/ws", workspaceIdentity: WS_B },
      () => repo.list({ workspacePath: "/srv/ws", workspaceIdentity: WS_B }),
    );
    await record(dbPath, "get-a1", "get", { automationId: "a-1" }, () => repo.get("a-1"));
    await record(
      dbPath,
      "get-a1-wrong-workspace",
      "get",
      { automationId: "a-1", workspaceKey: WS_B },
      () => repo.get("a-1", WS_B),
    );
    await record(dbPath, "get-missing", "get", { automationId: "nope" }, () => repo.get("nope"));

    await record(
      dbPath,
      "dispatch-selection-a1",
      "getModelSelectionForDispatch",
      { automationId: "a-1", workspaceKey: WS_A },
      () => repo.getModelSelectionForDispatch("a-1", WS_A),
    );
    await record(
      dbPath,
      "dispatch-selection-follows",
      "getModelSelectionForDispatch",
      { automationId: "a-2", workspaceKey: WS_A },
      () => repo.getModelSelectionForDispatch("a-2", WS_A),
    );
    await record(
      dbPath,
      "dispatch-selection-broken",
      "getModelSelectionForDispatch",
      { automationId: "a-5", workspaceKey: WS_A },
      () => repo.getModelSelectionForDispatch("a-5", WS_A),
    );
    await record(
      dbPath,
      "dispatch-selection-missing",
      "getModelSelectionForDispatch",
      { automationId: "nope", workspaceKey: WS_A },
      () => repo.getModelSelectionForDispatch("nope", WS_A),
    );

    await record(
      dbPath,
      "bot-target-a1",
      "getBotDeliveryTarget",
      { automationId: "a-1" },
      () => repo.getBotDeliveryTarget("a-1"),
    );
    await record(
      dbPath,
      "bot-target-none",
      "getBotDeliveryTarget",
      { automationId: "a-2" },
      () => repo.getBotDeliveryTarget("a-2"),
    );
    await record(
      dbPath,
      "bot-target-invalid",
      "getBotDeliveryTarget",
      { automationId: "a-6" },
      () => repo.getBotDeliveryTarget("a-6"),
    );

    await record(
      dbPath,
      "has-binding-true",
      "hasTaskBinding",
      { workspacePath: WS_A, targetTaskId: "session-1" },
      () => repo.hasTaskBinding({ workspacePath: WS_A, targetTaskId: "session-1" }),
    );
    await record(
      dbPath,
      "has-binding-false",
      "hasTaskBinding",
      { workspacePath: WS_A, targetTaskId: "session-x" },
      () => repo.hasTaskBinding({ workspacePath: WS_A, targetTaskId: "session-x" }),
    );

    await record(
      dbPath,
      "scheduled-count-a1",
      "getScheduledRunCount",
      { automationId: "a-1" },
      () => repo.getScheduledRunCount("a-1"),
    );
    await record(
      dbPath,
      "scheduled-count-missing",
      "getScheduledRunCount",
      { automationId: "nope" },
      () => repo.getScheduledRunCount("nope"),
    );

    const updateParams = {
      title: "renamed",
      prompt: "new prompt",
      modelSelection: null,
      mode: null,
      recurring: false,
      maxRuns: 5,
      endAt: 999,
      scheduleRule: null,
      scheduleEditedByUser: true,
    };
    await record(
      dbPath,
      "update-a1",
      "update",
      {
        automationId: "a-1",
        params: updateParams,
        options: { nextRunAt: 7777, lifecycleStatus: "paused", resetRetry: true },
      },
      () =>
        repo.update("a-1", updateParams as never, {
          nextRunAt: 7777,
          lifecycleStatus: "paused",
          resetRetry: true,
        }),
    );
    await record(
      dbPath,
      "update-invalid-mode",
      "update",
      { automationId: "a-1", params: { mode: "bogus" } },
      () => repo.update("a-1", { mode: "bogus" as never }),
    );
    await record(
      dbPath,
      "update-missing",
      "update",
      { automationId: "nope", params: { title: "x" } },
      () => repo.update("nope", { title: "x" }),
    );

    await record(dbPath, "delete-a4", "delete", { automationId: "a-4" }, () =>
      repo.delete("a-4"),
    );
    await record(dbPath, "delete-a4-again", "delete", { automationId: "a-4" }, () =>
      repo.delete("a-4"),
    );

    await record(
      dbPath,
      "disable-a2",
      "setEnabled",
      { automationId: "a-2", enabled: false },
      () => repo.setEnabled("a-2", false),
    );
    await record(
      dbPath,
      "enable-a2",
      "setEnabled",
      { automationId: "a-2", enabled: true },
      () => repo.setEnabled("a-2", true),
    );
    await record(
      dbPath,
      "restart-a3",
      "restart",
      { automationId: "a-3", options: { nextRunAt: 8888 } },
      () => repo.restart("a-3", { nextRunAt: 8888 }),
    );

    await record(dbPath, "claim-due", "claimDue", { now: 1_000_000 }, () =>
      repo.claimDue(1_000_000),
    );
    await record(dbPath, "claim-due-again", "claimDue", { now: 1_000_001 }, () =>
      repo.claimDue(1_000_001),
    );
    await record(dbPath, "claim-manual", "claimManualRuns", { now: 1_000_000 }, () =>
      repo.claimManualRuns(1_000_000),
    );
    await record(dbPath, "claim-manual-again", "claimManualRuns", { now: 1_000_001 }, () =>
      repo.claimManualRuns(1_000_001),
    );

    await record(
      dbPath,
      "manual-dispatched",
      "markManualRunDispatched",
      { runId: "a-11:manual:seed", sessionId: "s-1", dispatchedAt: 1_100_000 },
      () =>
        repo.markManualRunDispatched({
          runId: "a-11:manual:seed",
          sessionId: "s-1",
          dispatchedAt: 1_100_000,
        }),
    );
    await record(
      dbPath,
      "manual-dispatched-again",
      "markManualRunDispatched",
      { runId: "a-11:manual:seed", sessionId: "s-1", dispatchedAt: 1_100_001 },
      () =>
        repo.markManualRunDispatched({
          runId: "a-11:manual:seed",
          sessionId: "s-1",
          dispatchedAt: 1_100_001,
        }),
    );

    await record(
      dbPath,
      "mark-dispatched-a6",
      "markDispatched",
      { automationId: "a-6", options: { dispatchedAt: 1_200_000, nextRunAt: 5_000_000 } },
      () => repo.markDispatched("a-6", { dispatchedAt: 1_200_000, nextRunAt: 5_000_000 }),
    );
    await record(
      dbPath,
      "mark-dispatched-a2-first",
      "markDispatched",
      { automationId: "a-2", options: { dispatchedAt: 1_200_000, nextRunAt: 5_000_000 } },
      () => repo.markDispatched("a-2", { dispatchedAt: 1_200_000, nextRunAt: 5_000_000 }),
    );
    await record(
      dbPath,
      "mark-dispatched-a2-second",
      "markDispatched",
      { automationId: "a-2", options: { dispatchedAt: 1_200_001, nextRunAt: 5_000_001 } },
      () => repo.markDispatched("a-2", { dispatchedAt: 1_200_001, nextRunAt: 5_000_001 }),
    );
    await record(
      dbPath,
      "mark-dispatched-missing",
      "markDispatched",
      { automationId: "nope", options: { dispatchedAt: 1, nextRunAt: null } },
      () => repo.markDispatched("nope", { dispatchedAt: 1, nextRunAt: null }),
    );

    await record(
      dbPath,
      "failed-permanent-a1",
      "markDispatchFailed",
      { automationId: "a-1", options: { failedAt: 1_300_000, error: "boom", kind: "permanent" } },
      () =>
        repo.markDispatchFailed("a-1", {
          failedAt: 1_300_000,
          error: "boom",
          kind: "permanent",
        }),
    );
    await record(
      dbPath,
      "failed-transient-a7-cap",
      "markDispatchFailed",
      {
        automationId: "a-7",
        options: { failedAt: 1_300_000, error: "retry", kind: "transient", nextRunAt: 6_000_000 },
      },
      () =>
        repo.markDispatchFailed("a-7", {
          failedAt: 1_300_000,
          error: "retry",
          kind: "transient",
          nextRunAt: 6_000_000,
        }),
    );
    await record(
      dbPath,
      "failed-transient-a10",
      "markDispatchFailed",
      { automationId: "a-10", options: { failedAt: 1_300_000, error: "retry", kind: "transient" } },
      () =>
        repo.markDispatchFailed("a-10", {
          failedAt: 1_300_000,
          error: "retry",
          kind: "transient",
        }),
    );
    await record(
      dbPath,
      "failed-missing",
      "markDispatchFailed",
      { automationId: "nope", options: { failedAt: 1, error: "x", kind: "permanent" } },
      () => repo.markDispatchFailed("nope", { failedAt: 1, error: "x", kind: "permanent" }),
    );

    await record(
      dbPath,
      "run-now-a9",
      "runNow",
      { automationId: "a-9", options: { now: 1_400_000 } },
      () => repo.runNow("a-9", { now: 1_400_000 }),
    );
    await record(
      dbPath,
      "run-now-a9-again",
      "runNow",
      { automationId: "a-9", options: { now: 1_400_001 } },
      () => repo.runNow("a-9", { now: 1_400_001 }),
    );
    await record(
      dbPath,
      "run-now-missing",
      "runNow",
      { automationId: "nope", options: { now: 1_400_001 } },
      () => repo.runNow("nope", { now: 1_400_001 }),
    );
    await record(
      dbPath,
      "release-claim-a9",
      "releaseClaim",
      { automationId: "a-9" },
      () => repo.releaseClaim("a-9"),
    );
    await record(
      dbPath,
      "release-manual-a9",
      "releaseManualClaim",
      { automationId: "a-9", workspaceKey: WS_A },
      () => repo.releaseManualClaim("a-9", WS_A),
    );
    await record(
      dbPath,
      "touch-manual-a9",
      "touchManualClaim",
      { automationId: "a-9", workspaceKey: WS_A },
      () => repo.touchManualClaim("a-9", WS_A),
    );

    await record(
      dbPath,
      "skip-a6",
      "skipAndReschedule",
      {
        automationId: "a-6",
        runId: "a-6:schedule:400000",
        workspaceKey: WS_A,
        scheduledAt: 400_000,
        reason: "missed",
        nextRunAt: 7_000_000,
      },
      () =>
        repo.skipAndReschedule({
          automationId: "a-6",
          runId: "a-6:schedule:400000",
          workspaceKey: WS_A,
          scheduledAt: 400_000,
          reason: "missed",
          nextRunAt: 7_000_000,
        }),
    );
    await record(
      dbPath,
      "skip-a2-finalize",
      "skipAndReschedule",
      {
        automationId: "a-2",
        runId: "a-2:schedule:5000",
        workspaceKey: WS_A,
        scheduledAt: 5000,
        reason: "expired",
        nextRunAt: null,
        finalize: true,
      },
      () =>
        repo.skipAndReschedule({
          automationId: "a-2",
          runId: "a-2:schedule:5000",
          workspaceKey: WS_A,
          scheduledAt: 5000,
          reason: "expired",
          nextRunAt: null,
          finalize: true,
        }),
    );

    await record(
      dbPath,
      "ensure-run",
      "ensureRunClaimed",
      {
        runId: "a-10:schedule:900000",
        automationId: "a-10",
        workspaceKey: WS_B,
        scheduledAt: 900_000,
        trigger: "schedule",
      },
      () =>
        repo.ensureRunClaimed({
          runId: "a-10:schedule:900000",
          automationId: "a-10",
          workspaceKey: WS_B,
          scheduledAt: 900_000,
          trigger: "schedule",
        }),
    );
    await record(
      dbPath,
      "ensure-run-again",
      "ensureRunClaimed",
      {
        runId: "a-10:schedule:900000",
        automationId: "a-10",
        workspaceKey: WS_B,
        scheduledAt: 900_000,
        trigger: "schedule",
      },
      () =>
        repo.ensureRunClaimed({
          runId: "a-10:schedule:900000",
          automationId: "a-10",
          workspaceKey: WS_B,
          scheduledAt: 900_000,
          trigger: "schedule",
        }),
    );
    await record(
      dbPath,
      "upsert-run",
      "upsertRunClaimed",
      {
        runId: "a-10:schedule:900001",
        automationId: "a-10",
        workspaceKey: WS_B,
        scheduledAt: 900_001,
        trigger: "schedule",
        modelSelection: SELECTION,
      },
      () =>
        repo.upsertRunClaimed({
          runId: "a-10:schedule:900001",
          automationId: "a-10",
          workspaceKey: WS_B,
          scheduledAt: 900_001,
          trigger: "schedule",
          modelSelection: SELECTION as never,
        }),
    );
    await record(
      dbPath,
      "upsert-run-again",
      "upsertRunClaimed",
      {
        runId: "a-10:schedule:900001",
        automationId: "a-10",
        workspaceKey: WS_B,
        scheduledAt: 900_001,
        trigger: "schedule",
      },
      () =>
        repo.upsertRunClaimed({
          runId: "a-10:schedule:900001",
          automationId: "a-10",
          workspaceKey: WS_B,
          scheduledAt: 900_001,
          trigger: "schedule",
        }),
    );

    await record(
      dbPath,
      "fix-selection-first",
      "fixRunModelSelection",
      { runId: "a-10:schedule:900001", selection: SELECTION },
      () => repo.fixRunModelSelection("a-10:schedule:900001", SELECTION as never),
    );
    await record(
      dbPath,
      "fix-selection-second",
      "fixRunModelSelection",
      {
        runId: "a-10:schedule:900001",
        selection: { providerId: "zai-api", modelId: "other" },
      },
      () =>
        repo.fixRunModelSelection("a-10:schedule:900001", {
          providerId: "zai-api",
          modelId: "other",
        } as never),
    );
    await record(
      dbPath,
      "fix-selection-missing",
      "fixRunModelSelection",
      { runId: "nope", selection: SELECTION },
      () => repo.fixRunModelSelection("nope", SELECTION as never),
    );

    await record(
      dbPath,
      "run-dispatch",
      "markRunDispatch",
      { runId: "a-10:schedule:900001", dispatchStatus: "dispatched", sessionId: "s-9" },
      () =>
        repo.markRunDispatch({
          runId: "a-10:schedule:900001",
          dispatchStatus: "dispatched",
          sessionId: "s-9",
        }),
    );
    await record(
      dbPath,
      "run-dispatch-failed",
      "markRunDispatch",
      { runId: "a-10:schedule:900001", dispatchStatus: "failed_to_dispatch", error: "nope" },
      () =>
        repo.markRunDispatch({
          runId: "a-10:schedule:900001",
          dispatchStatus: "failed_to_dispatch",
          error: "nope",
        }),
    );

    await record(
      dbPath,
      "run-outcome-running",
      "markRunOutcome",
      { runId: "a-10:schedule:900001", outcome: "running" },
      () => repo.markRunOutcome("a-10:schedule:900001", "running"),
    );
    await record(
      dbPath,
      "run-outcome-succeeded",
      "markRunOutcome",
      { runId: "a-10:schedule:900001", outcome: "succeeded" },
      () => repo.markRunOutcome("a-10:schedule:900001", "succeeded"),
    );
    await record(
      dbPath,
      "run-outcome-late-running",
      "markRunOutcome",
      { runId: "a-10:schedule:900001", outcome: "running" },
      () => repo.markRunOutcome("a-10:schedule:900001", "running"),
    );
    await record(
      dbPath,
      "run-outcome-failed",
      "markRunOutcome",
      { runId: "a-10:schedule:900001", outcome: "failed", error: "boom" },
      () => repo.markRunOutcome("a-10:schedule:900001", "failed", "boom"),
    );

    await record(
      dbPath,
      "record-skipped",
      "recordSkippedRun",
      {
        runId: "a-10:schedule:900002",
        automationId: "a-10",
        workspaceKey: WS_B,
        scheduledAt: 900_002,
        trigger: "schedule",
        reason: "window-missed",
      },
      () =>
        repo.recordSkippedRun({
          runId: "a-10:schedule:900002",
          automationId: "a-10",
          workspaceKey: WS_B,
          scheduledAt: 900_002,
          trigger: "schedule",
          reason: "window-missed",
        }),
    );

    await record(dbPath, "list-runs", "listRuns", { automationId: "a-10" }, () =>
      repo.listRuns("a-10"),
    );
    await record(
      dbPath,
      "get-run",
      "getRun",
      { runId: "a-10:schedule:900001" },
      () => repo.getRun("a-10:schedule:900001"),
    );
    await record(dbPath, "delete-run", "deleteRun", { runId: "a-10:schedule:900002" }, () =>
      repo.deleteRun("a-10:schedule:900002"),
    );
    await record(dbPath, "prune-runs", "pruneRuns", { maxAgeMs: 1000 }, () =>
      repo.pruneRuns(1000),
    );

    await record(
      dbPath,
      "create",
      "create",
      {
        params: {
          title: "created",
          cronExpr: "0 9 * * *",
          prompt: "created prompt",
          modelSelection: SELECTION,
          mode: "build",
          workspacePath: WS_A,
          targetTaskId: "session-2",
          botDeliveryTarget: {
            provider: "wecom",
            botId: "bot-2",
            providerUserId: "u-2",
            chatType: "group",
          },
          recurring: true,
          scheduleRule: {
            unit: "weekly",
            interval: 1,
            hour: 10,
            minute: 30,
            anchorAt: 2000,
            weekdays: [1, 3, 5],
          },
        },
        options: { nextRunAt: 9_000_000 },
      },
      () =>
        repo.create(
          {
            title: "created",
            cronExpr: "0 9 * * *",
            prompt: "created prompt",
            modelSelection: SELECTION as never,
            mode: "build",
            workspacePath: WS_A,
            targetTaskId: "session-2",
            botDeliveryTarget: {
              provider: "wecom",
              botId: "bot-2",
              providerUserId: "u-2",
              chatType: "group",
            },
            recurring: true,
            scheduleRule: {
              unit: "weekly",
              interval: 1,
              hour: 10,
              minute: 30,
              anchorAt: 2000,
              weekdays: [1, 3, 5],
            },
          } as never,
          { nextRunAt: 9_000_000 },
        ),
    );
    await record(dbPath, "create-invalid-mode", "create", { params: { mode: "bogus" } }, () =>
      repo.create(
        {
          title: "x",
          cronExpr: "0 9 * * *",
          prompt: "x",
          workspacePath: WS_A,
          mode: "bogus",
          recurring: true,
        } as never,
        { nextRunAt: null },
      ),
    );

    await record(dbPath, "list-after-create", "list", {}, () => repo.list());
    await record(dbPath, "close", "close", {}, () => {
      repo.close();
      return null;
    });
    await record(dbPath, "reopen-list", "list", {}, () => repo.list());
    repo.close();

    const fixture = {
      generatedBy: "scripts/generate-automation-fixtures.ts",
      clockMs: CLOCK,
      steps,
    };
    mkdirSync(path.dirname(OUTPUT), { recursive: true });
    writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
    console.log(`[automation] steps=${steps.length} → ${path.relative(ROOT_DIR, OUTPUT)}`);
  } finally {
    Date.now = realNow;
    rmSync(workDir, { recursive: true, force: true });
  }
}

await main();
