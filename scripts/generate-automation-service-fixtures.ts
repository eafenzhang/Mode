/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明用例矩阵，拆分会降低可核对性。 */
/**
 * 生成 `automation-service` 的 Rust 对拍数据（create / update 调度语义 + 管理面读写）。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/session/automationService.ts`
 *
 * 时钟由 `Date.now` 固定在每条用例的 `now`，数据库为每条用例独立临时文件；
 * 两个固定时区（UTC / Asia/Shanghai）各生成一份，Rust 侧用相同 `offsetSecs` 重放。
 * 自动化 ID、runId 含 UUID，归一化为 `<automation-id>` / `<run-id>`。
 *
 * ```bash
 * pnpm automation-service:fixtures
 * cargo test -p automation-service
 * ```
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import type { AutomationService } from "../packages/services/src/session/automationService.js";
import type { AutomationRepo } from "../packages/services/src/session/automationRepo.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT_DIR = path.join(ROOT_DIR, "crates", "automation-service", "fixtures");
const WORKSPACE_PATH = "/ws/demo";

const ZONES = [
  { name: "utc", iana: "UTC", offsetSecs: 0 },
  { name: "asia-shanghai", iana: "Asia/Shanghai", offsetSecs: 8 * 3600 },
] as const;

function local(y: number, month: number, day: number, hour = 0, minute = 0): number {
  return new Date(y, month - 1, day, hour, minute, 0, 0).getTime();
}

type OpKind =
  | "create"
  | "update"
  | "list"
  | "listAll"
  | "setEnabled"
  | "restart"
  | "runNow"
  | "listRuns"
  | "deleteRun"
  | "delete";

interface Op {
  kind: OpKind;
  fromOp?: number;
  runFromOp?: number;
  automationId?: string;
  enabled?: boolean;
  params?: Record<string, unknown>;
}

interface FixtureCase {
  name: string;
  now: number;
  workspacePath: string;
  ops: Op[];
  expected: unknown[];
}

function scheduleRule(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    unit: "daily",
    interval: 1,
    hour: 9,
    minute: 0,
    anchorAt: 1_000,
    ...over,
  };
}

function createParams(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    title: "Task",
    cronExpr: "0 9 * * *",
    prompt: "say hi",
    recurring: true,
    ...over,
  };
}

function normalizeRule(rule: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {
    unit: rule.unit,
    interval: rule.interval,
    hour: rule.hour,
    minute: rule.minute,
    anchorAt: rule.anchorAt,
  };
  for (const key of ["weekdays", "monthDays", "months", "monthlyMode"] as const) {
    if (rule[key] !== undefined) out[key] = rule[key];
  }
  return out;
}

/** 与 `automation-service` 的 `ModeAutomation` 投影保持同一字段集。 */
function projectAutomation(value: unknown): Record<string, unknown> {
  const automation = value as Record<string, unknown>;
  const out: Record<string, unknown> = {
    automationId: automation.automationId,
    title: automation.title,
    cronExpr: automation.cronExpr,
    prompt: automation.prompt,
  };
  if (automation.modelSelection !== undefined) out.modelSelection = automation.modelSelection;
  if (automation.mode !== undefined) out.mode = automation.mode;
  if (automation.targetTaskId !== undefined) out.targetTaskId = automation.targetTaskId;
  out.enabled = automation.enabled;
  out.lifecycleStatus = automation.lifecycleStatus;
  if (automation.nextRunAt !== undefined) out.nextRunAt = automation.nextRunAt;
  if (automation.lastRunAt !== undefined) out.lastRunAt = automation.lastRunAt;
  out.runCount = automation.runCount;
  out.recurring = automation.recurring;
  if (automation.maxRuns !== undefined) out.maxRuns = automation.maxRuns;
  if (automation.scheduleRule !== undefined) {
    out.scheduleRule = normalizeRule(automation.scheduleRule as Record<string, unknown>);
  }
  return out;
}

function projectRun(value: unknown): Record<string, unknown> {
  const run = value as Record<string, unknown>;
  const out: Record<string, unknown> = {
    runId: run.runId,
    automationId: run.automationId,
  };
  if (run.scheduledAt !== undefined) out.scheduledAt = run.scheduledAt;
  out.trigger = run.trigger;
  if (run.modelSelection !== undefined) out.modelSelection = run.modelSelection;
  out.dispatchStatus = run.dispatchStatus;
  if (run.outcome !== undefined) out.outcome = run.outcome;
  if (run.sessionId !== undefined) out.sessionId = run.sessionId;
  if (run.error !== undefined) out.error = run.error;
  out.createdAt = run.createdAt;
  out.updatedAt = run.updatedAt;
  return out;
}

const MANUAL_RUN_ID =
  /automation-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}:manual:[0-9a-f-]{36}/gu;
const SCHEDULED_RUN_ID =
  /automation-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}:\d+/gu;
const AUTOMATION_ID = /automation-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gu;

function normalizeValue(value: unknown): unknown {
  if (typeof value === "string") {
    return value
      .replace(MANUAL_RUN_ID, "<run-id>")
      .replace(SCHEDULED_RUN_ID, "<run-id>")
      .replace(AUTOMATION_ID, "<automation-id>");
  }
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

function errorResult(error: unknown): unknown {
  const typed = error as { name?: string; message?: string };
  // stale one-shot 的文案含本地化时间；调度语义已由 automation-schedule 对拍，这里只保留类型。
  if (typed.name === "StaleOneShotAutomationScheduleError") {
    return { staleOneShot: true };
  }
  return { error: typed.message ?? String(error) };
}

function buildCases(nowMorning: number, nowMinute: number): FixtureCase[] {
  return [
    {
      name: "create-basics",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams() },
        {
          kind: "create",
          params: createParams({
            title: "Weekdays",
            cronExpr: "0 9 * * 1-5",
            modelSelection: {
              providerId: "anthropic",
              modelId: "claude-sonnet",
              options: { reasoningLevel: "high" },
            },
          }),
        },
        {
          kind: "create",
          params: createParams({
            title: "One shot",
            cronExpr: "0 9 10 3 *",
            recurring: false,
          }),
        },
        { kind: "list" },
        { kind: "listAll" },
      ],
      expected: [],
    },
    {
      name: "create-minute-anchor",
      now: nowMinute,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams({ cronExpr: "*/10 * * * *" }) },
        { kind: "create", params: createParams({ cronExpr: "*/1 * * * *" }) },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "create-relative-delay",
      now: nowMinute,
      workspacePath: WORKSPACE_PATH,
      ops: [
        {
          kind: "create",
          params: createParams({ cronExpr: "0 9 * * *", relativeDelayMinutes: 3, recurring: false }),
        },
        {
          kind: "create",
          params: createParams({ cronExpr: "bad cron", relativeDelayMinutes: 90, recurring: false }),
        },
        { kind: "create", params: createParams({ relativeDelayMinutes: 0, recurring: false }) },
        { kind: "create", params: createParams({ relativeDelayMinutes: 1, recurring: true }) },
        {
          kind: "create",
          params: createParams({ relativeDelayMinutes: 1, recurring: false, maxRuns: 2 }),
        },
        {
          kind: "create",
          params: createParams({
            relativeDelayMinutes: 1,
            recurring: false,
            scheduleRule: scheduleRule(),
          }),
        },
        {
          kind: "create",
          params: createParams({ relativeDelayMinutes: 525_601, recurring: false }),
        },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "create-carrier",
      now: nowMinute,
      workspacePath: WORKSPACE_PATH,
      ops: [
        {
          kind: "create",
          params: createParams({ cronExpr: "0 */2 * * *", intervalUnit: "hourly", interval: 2 }),
        },
        {
          kind: "create",
          params: createParams({ cronExpr: "*/15 * * * *", intervalUnit: "minute", interval: 15 }),
        },
        {
          kind: "create",
          params: createParams({ cronExpr: "0 9 * * 1", intervalUnit: "weekly", interval: 2 }),
        },
        {
          kind: "create",
          params: createParams({ cronExpr: "0 9 1 * *", intervalUnit: "monthly", interval: 3 }),
        },
        {
          kind: "create",
          params: createParams({
            intervalUnit: "daily",
            interval: 1,
            scheduleRule: scheduleRule(),
          }),
        },
        {
          kind: "create",
          params: createParams({ intervalUnit: "daily", interval: 1, relativeDelayMinutes: 5 }),
        },
        {
          kind: "create",
          params: createParams({ intervalUnit: "daily", interval: 1, recurring: false }),
        },
        { kind: "create", params: createParams({ intervalUnit: "daily", interval: 0 }) },
        { kind: "create", params: createParams({ intervalUnit: "daily", interval: 1.5 }) },
        { kind: "create", params: createParams({ intervalUnit: "daily" }) },
        { kind: "create", params: createParams({ interval: 5 }) },
        { kind: "create", params: createParams({ intervalUnit: "daily", interval: 201 }) },
        {
          kind: "create",
          params: createParams({ intervalUnit: "daily", interval: 1, maxRuns: 2 }),
        },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "create-explicit-rule",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 * * 1,3,5",
            scheduleRule: scheduleRule({ unit: "weekly", weekdays: [1, 3, 5] }),
          }),
        },
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 * * 1",
            scheduleRule: scheduleRule({
              unit: "monthly",
              monthlyMode: "weekday",
              weekdays: [1],
            }),
          }),
        },
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 15 * *",
            scheduleRule: scheduleRule({ unit: "monthly", monthDays: [15, 28] }),
          }),
        },
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 1 1 *",
            scheduleRule: scheduleRule({ unit: "yearly", months: [1, 6], monthDays: [1] }),
          }),
        },
        { kind: "create", params: createParams({ scheduleRule: scheduleRule({ interval: 0 }) }) },
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 * * 1",
            scheduleRule: scheduleRule({ unit: "weekly" }),
          }),
        },
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 1 * *",
            scheduleRule: scheduleRule({ unit: "monthly", interval: 1_201, monthDays: [1] }),
          }),
        },
        {
          kind: "create",
          params: createParams({ scheduleRule: scheduleRule({ unit: "yearly", months: [13] }) }),
        },
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 1 * *",
            scheduleRule: scheduleRule({ unit: "monthly", monthDays: [32] }),
          }),
        },
        { kind: "create", params: createParams({ scheduleRule: scheduleRule({ hour: 24 }) }) },
        { kind: "create", params: createParams({ scheduleRule: scheduleRule({ minute: 60 }) }) },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "create-end-at",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams({ endAt: nowMorning + 60_000 }) },
        {
          kind: "create",
          params: createParams({ endAt: nowMorning + 365 * 24 * 3600_000 }),
        },
        {
          kind: "create",
          params: createParams({
            relativeDelayMinutes: 60,
            recurring: false,
            endAt: nowMorning + 30 * 60_000,
          }),
        },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "create-invalid-cron",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams({ cronExpr: "0 9 * *" }) },
        { kind: "create", params: createParams({ cronExpr: "61 9 * * *" }) },
      ],
      expected: [],
    },
    {
      name: "update-cron",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams() },
        {
          kind: "create",
          params: createParams({ cronExpr: "0 9 * * *", scheduleRule: scheduleRule() }),
        },
        { kind: "update", fromOp: 0, params: { cronExpr: "30 10 * * *" } },
        { kind: "update", fromOp: 1, params: { scheduleRule: null } },
        { kind: "update", fromOp: 0, params: { cronExpr: "*/5 * * * *" } },
        { kind: "update", fromOp: 0, params: { title: "Renamed" } },
        { kind: "update", fromOp: 0, params: { cronExpr: "not a cron" } },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "update-max-runs",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams({ recurring: false, maxRuns: 3 }) },
        { kind: "update", fromOp: 0, params: { maxRuns: null } },
        { kind: "update", fromOp: 0, params: { maxRuns: null, recurring: true } },
        { kind: "update", fromOp: 0, params: { maxRuns: 5, recurring: true } },
        { kind: "update", fromOp: 0, params: { maxRuns: 5, recurring: false } },
        { kind: "update", fromOp: 0, params: { recurring: true } },
        {
          kind: "create",
          params: createParams({ title: "Finite", recurring: false, maxRuns: 2 }),
        },
        { kind: "update", fromOp: 1, params: { recurring: true } },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "update-end-at",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams() },
        { kind: "update", fromOp: 0, params: { endAt: nowMorning + 60_000 } },
        { kind: "update", fromOp: 0, params: { endAt: null } },
        { kind: "update", fromOp: 0, params: { endAt: nowMorning + 30 * 24 * 3600_000 } },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "update-carrier",
      now: nowMinute,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams() },
        { kind: "update", fromOp: 0, params: { intervalUnit: "minute", interval: 7 } },
        {
          kind: "update",
          fromOp: 0,
          params: { cronExpr: "0 */2 * * *", intervalUnit: "hourly", interval: 2 },
        },
        {
          kind: "update",
          fromOp: 0,
          params: { intervalUnit: "daily", interval: 1, scheduleRule: null },
        },
        {
          kind: "update",
          fromOp: 0,
          params: { intervalUnit: "daily", interval: 1, recurring: false },
        },
        { kind: "update", fromOp: 0, params: { intervalUnit: "daily", interval: 1, maxRuns: 2 } },
        { kind: "update", fromOp: 0, params: { intervalUnit: "daily" } },
        { kind: "update", fromOp: 0, params: { intervalUnit: "daily", interval: 1.5 } },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "update-rule-anchor",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        {
          kind: "create",
          params: createParams({
            cronExpr: "0 9 * * 1,3,5",
            scheduleRule: scheduleRule({ unit: "weekly", weekdays: [1, 3, 5], anchorAt: 1_000 }),
          }),
        },
        {
          kind: "update",
          fromOp: 0,
          params: {
            title: "Renamed",
            scheduleRule: scheduleRule({ unit: "weekly", weekdays: [1, 3, 5] }),
          },
        },
        {
          kind: "update",
          fromOp: 0,
          params: { scheduleRule: scheduleRule({ unit: "weekly", weekdays: [2, 4] }) },
        },
        {
          kind: "update",
          fromOp: 0,
          params: { scheduleRule: scheduleRule({ unit: "weekly" }) },
        },
        {
          kind: "update",
          fromOp: 0,
          params: { scheduleRule: scheduleRule({ unit: "monthly", monthDays: [] }) },
        },
        { kind: "list" },
      ],
      expected: [],
    },
    {
      name: "update-missing",
      now: nowMorning,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "update", automationId: "automation-missing", params: { title: "Nope" } },
        { kind: "update", automationId: "automation-missing", params: { cronExpr: "*/5 * * * *" } },
      ],
      expected: [],
    },
    {
      name: "lifecycle-ops",
      now: nowMinute,
      workspacePath: WORKSPACE_PATH,
      ops: [
        { kind: "create", params: createParams() },
        { kind: "runNow", fromOp: 0 },
        { kind: "listRuns", fromOp: 0 },
        { kind: "setEnabled", fromOp: 0, enabled: false },
        { kind: "list" },
        { kind: "restart", fromOp: 0 },
        { kind: "deleteRun", runFromOp: 1 },
        { kind: "listRuns", fromOp: 0 },
        { kind: "delete", fromOp: 0 },
        { kind: "list" },
        { kind: "listAll" },
      ],
      expected: [],
    },
  ];
}

interface ServiceHandle {
  service: AutomationService;
  close: () => void;
}

async function runCase(
  createService: (dbPath: string) => ServiceHandle,
  testCase: FixtureCase,
  sandbox: string,
  zoneName: string,
  caseIndex: number,
): Promise<void> {
  const { service, close } = createService(
    path.join(sandbox, `case-${zoneName}-${caseIndex}.sqlite`),
  );
  const scope = { workspacePath: testCase.workspacePath };
  const automationIds: string[] = [];
  const runIds: string[] = [];
  const expected: unknown[] = [];

  for (const op of testCase.ops) {
    try {
      switch (op.kind) {
        case "create": {
          const created = await service.create({
            ...(op.params as Record<string, unknown>),
            workspacePath: testCase.workspacePath,
          } as never);
          automationIds.push(created.automationId);
          runIds.push("");
          expected.push(projectAutomation(created));
          break;
        }
        case "update": {
          const id =
            op.fromOp === undefined
              ? (op.automationId as string)
              : (automationIds[op.fromOp] as string);
          const updated = await service.update(id, op.params as never, scope as never);
          runIds.push("");
          expected.push(updated ? projectAutomation(updated) : null);
          break;
        }
        case "list":
          expected.push(
            (await service.list(scope as never)).map((item: unknown) => projectAutomation(item)),
          );
          runIds.push("");
          break;
        case "listAll":
          expected.push((await service.list()).map((item: unknown) => projectAutomation(item)));
          runIds.push("");
          break;
        case "setEnabled":
          await service.setEnabled(
            automationIds[op.fromOp as number] as string,
            op.enabled as boolean,
            scope as never,
          );
          runIds.push("");
          expected.push(null);
          break;
        case "restart":
          await service.restart(automationIds[op.fromOp as number] as string, scope as never);
          runIds.push("");
          expected.push(null);
          break;
        case "runNow": {
          const claimed = await service.runNow(
            automationIds[op.fromOp as number] as string,
            scope as never,
          );
          runIds.push(claimed ? claimed.run.runId : "");
          expected.push(
            claimed
              ? {
                  automation: projectAutomation(claimed.automation),
                  run: projectRun(claimed.run),
                }
              : null,
          );
          break;
        }
        case "listRuns":
          expected.push(
            (
              await service.listRuns(automationIds[op.fromOp as number] as string, scope as never)
            ).map((run: unknown) => projectRun(run)),
          );
          runIds.push("");
          break;
        case "deleteRun":
          await service.deleteRun(runIds[op.runFromOp as number] as string, scope as never);
          runIds.push("");
          expected.push(null);
          break;
        case "delete":
          expected.push(
            await service.delete(automationIds[op.fromOp as number] as string, scope as never),
          );
          runIds.push("");
          break;
        default:
          throw new Error(`unknown op ${String((op as { kind?: string }).kind)}`);
      }
    } catch (error) {
      runIds.push("");
      expected.push(errorResult(error));
    }
  }

  close();
  testCase.expected = expected;
}

async function main(): Promise<void> {
  const sandbox = mkdtempSync(path.join(tmpdir(), "mode-automation-service-fixtures-"));
  mkdirSync(OUTPUT_DIR, { recursive: true });
  const { AutomationService } = await import(
    "../packages/services/src/session/automationService.js"
  );
  const { AutomationRepo } = await import("../packages/services/src/session/automationRepo.js");

  for (const zone of ZONES) {
    process.env.TZ = zone.iana;
    const offsetMinutes = new Date(0).getTimezoneOffset();
    if (-offsetMinutes * 60 !== zone.offsetSecs) {
      throw new Error(
        `时区切换失败：${zone.iana} 期望偏移 ${zone.offsetSecs}s，实得 ${-offsetMinutes * 60}s`,
      );
    }
    const nowMorning = local(2026, 3, 10, 9, 30);
    const nowMinute = local(2026, 3, 10, 11, 46);
    const cases = buildCases(nowMorning, nowMinute);

    for (const [index, testCase] of cases.entries()) {
      const fixedNow = testCase.now;
      const realNow = Date.now;
      Date.now = () => fixedNow;
      try {
        await runCase(
          (dbPath) => {
            const repo = new AutomationRepo(dbPath) as AutomationRepo;
            return {
              service: new AutomationService(repo),
              close: () => repo.close(),
            };
          },
          testCase,
          sandbox,
          zone.name,
          index,
        );
      } finally {
        Date.now = realNow;
      }
    }

    const fixture = {
      zone: zone.name,
      iana: zone.iana,
      offsetSecs: zone.offsetSecs,
      cases: cases.map((testCase) => ({
        ...testCase,
        expected: normalizeValue(testCase.expected),
      })),
    };
    const output = path.join(OUTPUT_DIR, `service-${zone.name}.json`);
    writeFileSync(output, `${JSON.stringify(fixture, null, 2)}\n`);
    process.stdout.write(`wrote ${path.relative(ROOT_DIR, output)} (${cases.length} cases)\n`);
  }
  rmSync(sandbox, { recursive: true, force: true });
}

await main();
