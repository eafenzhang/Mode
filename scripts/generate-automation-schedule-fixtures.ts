/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明输入矩阵，拆分会降低可核对性。 */
/**
 * 生成 `automation-schedule` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/session/automationCron.ts`
 * - `packages/services/src/session/automationCronValidation.ts`
 * - `packages/services/src/session/automationIntervalCarrier.ts`
 *
 * ```bash
 * pnpm automation-schedule:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p automation-schedule"
 * ```
 *
 * 时区策略：TS 的日历计算全部走 `Date` 的本地时区。脚本在同一个进程里切换 `TZ`，
 * 分别按 UTC（偏移 0）与 Asia/Shanghai（偏移 +08:00，无夏令时）各生成一份用例；
 * Rust 侧用同一份固定偏移重放。含夏令时的时区不在本轮口径内（见 spec「已知差异」）。
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { Cron } from "croner";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildIntervalScheduleRule,
  buildRelativeDelaySchedule,
  computeAutomationNextRunAt,
  computeInitialAutomationNextRunAt,
  computeNextRunAt,
  computeScheduleRuleNextRunAt,
  inferMinuteIntervalScheduleRule,
  isOneShotAutomation,
  scheduleRuleDefinition,
} from "../packages/services/src/session/automationCron.js";
import { isValidCronExpr } from "../packages/services/src/session/automationCronValidation.js";
import {
  assertValidAutomationIntervalCarrier,
  forceIntervalCarrierRecurring,
} from "../packages/services/src/session/automationIntervalCarrier.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT_DIR = path.join(ROOT_DIR, "crates", "automation-schedule", "fixtures");

const ZONES = [
  { name: "utc", iana: "UTC", offsetSecs: 0 },
  { name: "asia-shanghai", iana: "Asia/Shanghai", offsetSecs: 8 * 3600 },
] as const;

/** 本地时区下的墙钟时刻 → 时间戳。同一组墙钟时刻在两个时区得到不同时间戳。 */
function local(y: number, month: number, day: number, hour = 0, minute = 0): number {
  return new Date(y, month - 1, day, hour, minute, 0, 0).getTime();
}

const FROM_POINTS = [
  local(2026, 1, 1, 0, 0),
  local(2026, 3, 10, 9, 30),
  local(2026, 6, 15, 23, 59),
  local(2026, 12, 31, 18, 0),
  local(2027, 2, 28, 12, 0),
];

const CRON_EXPRESSIONS = [
  "* * * * *",
  "*/5 * * * *",
  "0 9 * * *",
  "30 4 * * *",
  "0 9 * * 1-5",
  "0 9 * * MON-FRI",
  "0 9 * * mon,wed,fri",
  "0 0 1 * *",
  "0 0 1 * 1",
  "0 0 * * 7",
  "0 0 * * 0",
  "*/15 9-17 * * 1-5",
  "0 */6 * * *",
  "5,10,15 * * * *",
  "0 0 L * *",
  "0 0 LW * *",
  "0 0 15W * *",
  "0 0 1W * *",
  "0 0 * * 5L",
  "0 0 * * 1#1",
  "0 0 * * 1#5",
  "0 0 29 2 *",
  "0 0 31 4 *",
  "0 0 1 1 *",
  "59 23 31 12 *",
  "0 0 1 JAN *",
  "0 0 1 dec *",
  "@daily",
  "@midnight",
  "@hourly",
  "@weekly",
  "@monthly",
  "@yearly",
  "@annually",
  "30 * * * * *",
  "0 0 9 * * *",
  "0 0 0 1 1 * 2028,2030",
  "0 9 * * ?",
  "0 0 ? * 1",
  "*/30 8-9 1,15 * *",
  "0 12 * * 1-5",
  "15 3 * * *",
];

const INVALID_CRON = [
  "",
  "0 9 * *",
  "61 9 * * *",
  "0 24 * * *",
  "0 9 32 * *",
  "0 9 * 13 *",
  "0 9 * * 8",
  "50-10 * * * *",
  "5/15 * * * *",
  "@weekdays",
  "@weekends",
  "not a cron",
  "*/0 * * * *",
  "0 9 * * MONDAY",
];

interface ScheduleRuleCase {
  unit: "minute" | "hourly" | "daily" | "weekly" | "monthly" | "yearly";
  interval: number;
  hour: number;
  minute: number;
  anchorAt: number;
  weekdays?: number[];
  monthDays?: number[];
  months?: number[];
  monthlyMode?: "date" | "weekday";
}

function scheduleRuleCases(): ScheduleRuleCase[] {
  const anchor = local(2026, 3, 10, 9, 30);
  return [
    { unit: "minute", interval: 15, hour: 9, minute: 30, anchorAt: anchor },
    { unit: "minute", interval: 1, hour: 0, minute: 0, anchorAt: anchor },
    { unit: "minute", interval: 90, hour: 9, minute: 30, anchorAt: anchor },
    { unit: "hourly", interval: 2, hour: 0, minute: 15, anchorAt: anchor },
    { unit: "hourly", interval: 50, hour: 0, minute: 0, anchorAt: anchor },
    { unit: "daily", interval: 1, hour: 9, minute: 0, anchorAt: anchor },
    { unit: "daily", interval: 3, hour: 18, minute: 45, anchorAt: anchor },
    { unit: "daily", interval: 40, hour: 7, minute: 5, anchorAt: anchor },
    { unit: "weekly", interval: 1, hour: 9, minute: 0, anchorAt: anchor, weekdays: [1, 3, 5] },
    { unit: "weekly", interval: 2, hour: 20, minute: 0, anchorAt: anchor, weekdays: [0, 6] },
    { unit: "weekly", interval: 1, hour: 8, minute: 30, anchorAt: anchor },
    { unit: "monthly", interval: 1, hour: 9, minute: 0, anchorAt: anchor, monthDays: [1, 15] },
    { unit: "monthly", interval: 1, hour: 9, minute: 0, anchorAt: anchor, monthlyMode: "weekday", weekdays: [1] },
    { unit: "monthly", interval: 13, hour: 9, minute: 0, anchorAt: anchor, monthDays: [31] },
    { unit: "monthly", interval: 1, hour: 9, minute: 0, anchorAt: anchor, monthDays: [30] },
    { unit: "yearly", interval: 1, hour: 9, minute: 0, anchorAt: anchor, months: [2], monthDays: [29] },
    { unit: "yearly", interval: 2, hour: 6, minute: 15, anchorAt: anchor, months: [11], monthDays: [30] },
    { unit: "yearly", interval: 1, hour: 9, minute: 0, anchorAt: anchor, months: [2], monthDays: [30] },
    { unit: "yearly", interval: 1, hour: 9, minute: 0, anchorAt: anchor },
  ];
}

function carrierCases(): Array<{ input: Record<string, unknown>; error: string | null }> {
  const rule = {
    unit: "weekly" as const,
    interval: 1,
    hour: 9,
    minute: 0,
    anchorAt: local(2026, 3, 10, 9, 30),
    weekdays: [1],
  };
  const inputs: Array<Record<string, unknown>> = [
    {},
    { intervalUnit: "minute", interval: 5, recurring: true },
    { intervalUnit: "minute" },
    { interval: 5 },
    { intervalUnit: "minute", interval: 0, recurring: true },
    { intervalUnit: "minute", interval: 201, recurring: true },
    { intervalUnit: "minute", interval: 1.5, recurring: true },
    { intervalUnit: "minute", interval: 5, relativeDelayMinutes: 10, recurring: true },
    { intervalUnit: "minute", interval: 5, scheduleRule: rule, recurring: true },
    { intervalUnit: "minute", interval: 5, scheduleRule: null, recurring: true },
    { intervalUnit: "minute", interval: 5, recurring: false },
    { intervalUnit: "minute", interval: 5, recurring: true, maxRuns: 3 },
    { intervalUnit: "minute", interval: 5, recurring: true, maxRuns: null },
    { scheduleRule: null },
    { relativeDelayMinutes: 30, recurring: false, maxRuns: 1 },
  ];
  return inputs.map((input) => {
    try {
      assertValidAutomationIntervalCarrier(input as never);
      return { input, error: null };
    } catch (error) {
      return { input, error: (error as Error).message };
    }
  });
}

function oneShotCases(): Array<{ recurring: boolean; maxRuns?: number | null; expected: boolean }> {
  const inputs: Array<{ recurring: boolean; maxRuns?: number | null }> = [
    { recurring: true },
    { recurring: true, maxRuns: 1 },
    { recurring: false },
    { recurring: false, maxRuns: 1 },
    { recurring: false, maxRuns: 3 },
    { recurring: false, maxRuns: 0 },
    { recurring: true, maxRuns: null },
  ];
  return inputs.map((automation) => ({
    ...automation,
    expected: isOneShotAutomation(automation as never),
  }));
}

function initialNextCases() {
  const justPast = local(2026, 3, 10, 9, 30);
  const cases: Array<{ cronExpr: string; recurring: boolean; scheduleRule?: ScheduleRuleCase; from: number }> = [
    // 一次性固定月日 cron：目标刚过去 20 秒 → 立即补执行（返回 from）。
    { cronExpr: "30 9 10 3 *", recurring: false, from: justPast + 20_000 },
    // 目标过去 2 分钟 → 拒绝（StaleOneShotAutomationScheduleError）。
    { cronExpr: "30 9 10 3 *", recurring: false, from: justPast + 120_000 },
    // 目标过去 31 分钟 → 超出窗口，保留「下一年」语义。
    { cronExpr: "30 9 10 3 *", recurring: false, from: justPast + 31 * 60_000 },
    // 目标还在未来 → 正常下一次。
    { cronExpr: "45 10 10 3 *", recurring: false, from: justPast },
    // 带 scheduleRule 的一次性任务不走陈旧判定。
    {
      cronExpr: "30 9 10 3 *",
      recurring: false,
      scheduleRule: { unit: "daily", interval: 1, hour: 9, minute: 30, anchorAt: justPast },
      from: justPast + 120_000,
    },
    // 循环任务的固定月日 cron 同样不走陈旧判定。
    { cronExpr: "27 9 10 3 *", recurring: true, from: justPast + 120_000 },
    // 非固定日历的 cron（带步长）不参与陈旧判定。
    { cronExpr: "*/5 * * * *", recurring: false, from: justPast + 120_000 },
  ];
  return cases.map((input) => {
    try {
      return { input, next: computeInitialAutomationNextRunAt(input as never, input.from), error: null };
    } catch (error) {
      const stale = error as { name?: string };
      if (stale.name !== "StaleOneShotAutomationScheduleError") throw error;
      // 消息里含 toLocaleString() 的本地化日期，逐字节不可比；只对拍错误类型与目标时刻。
      const targetAt = previousRuns(input.cronExpr, input.from, 1)?.[0] ?? null;
      return { input, next: null, error: { name: stale.name, targetAt } };
    }
  });
}

/**
 * croner 10 的 `previousRuns` 在部分模式（如 7 段年份、`?` 通配）会内部崩溃；
 * 这类用例记 null，Rust 侧跳过反向对拍（正向 `nextRun` 仍然全覆盖）。
 */
function previousRuns(cronExpr: string, from: number, count: number): number[] | null {
  try {
    return new Cron(cronExpr)
      .previousRuns(count, new Date(from))
      .map((date) => date.getTime());
  } catch {
    return null;
  }
}

/** 确定性 LCG：随机组合的 cron 表达式，扩大与 croner 的覆盖交集。 */
function createRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state * 1_664_525 + 1_013_904_223) >>> 0;
    return state / 0x1_0000_0000;
  };
}

const FIELD_POOLS = {
  minute: ["0", "5", "*/5", "*/15", "10,20,30", "0-30/10", "1,2,3"],
  hour: ["0", "9", "*/6", "8-18", "0,12"],
  dom: ["*", "1", "15", "L", "LW", "1W", "15W", "1,15", "10-20"],
  month: ["*", "1", "6", "JAN", "*/3", "1,7,12"],
  dow: ["*", "1-5", "MON-FRI", "0", "7", "5L", "1#2", "1,3,5", "?"],
} as const;

function fuzzCases() {
  const random = createRandom(20_261_030);
  const pick = <T,>(values: readonly T[]): T => values[Math.floor(random() * values.length)] as T;
  const cases: Array<{ expr: string; runs: Array<{ from: number; next: number | null }> }> = [];
  for (let index = 0; index < 200; index += 1) {
    const shape = random();
    const fields = [
      pick(FIELD_POOLS.minute),
      pick(FIELD_POOLS.hour),
      pick(FIELD_POOLS.dom),
      pick(FIELD_POOLS.month),
      pick(FIELD_POOLS.dow),
    ];
    const expr =
      shape < 0.7
        ? fields.join(" ")
        : shape < 0.9
          ? ["0", ...fields].join(" ")
          : ["0", ...fields, pick(["*", "2027", "2027,2029"])].join(" ");
    const valid = isValidCronExpr(expr);
    const runs = valid
      ? [0, 1, 2].map(() => {
          const from = local(2026, 1, 1) + Math.floor(random() * 1_200 * 86_400_000);
          return { from, next: computeNextRunAt(expr, from) };
        })
      : [];
    cases.push({ expr, valid, runs });
  }
  return cases;
}

function buildZoneCases() {
  const anchor = local(2026, 3, 10, 9, 30);
  return {
    fromPoints: FROM_POINTS,
    oneShot: oneShotCases(),
    definitions: scheduleRuleCases().map((rule) => ({
      rule,
      expected: scheduleRuleDefinition(rule as never),
    })),
    relativeDelay: [0, 1, 59, 60, 90, 1440, -5].map((delayMinutes) => ({
      delayMinutes,
      from: anchor,
      expected: buildRelativeDelaySchedule(delayMinutes, anchor),
    })),
    inferMinute: [
      "*/5 * * * *",
      "*/1 * * * *",
      "*/120 * * * *",
      "0 9 * * *",
      "*/0 * * * *",
      " */10  *  * * * ",
      "*/5 9 * * *",
      "*/5 * * * SUN",
    ].map((cronExpr) => ({
      cronExpr,
      anchorAt: anchor,
      expected: inferMinuteIntervalScheduleRule(cronExpr, anchor) ?? null,
    })),
    buildIntervalRules: (
      [
        ["minute", 5, "*/5 * * * *"],
        ["minute", 90, "0 9 * * *"],
        ["hourly", 50, "0 0 * * *"],
        ["hourly", 2, "15 9 * * *"],
        ["daily", 40, "30 7 * * *"],
        ["daily", 3, "45 18 * * *"],
        ["weekly", 2, "0 20 * * 1,5"],
        ["weekly", 1, "0 8 * * MON,WED"],
        ["monthly", 13, "0 9 1,15 * *"],
        ["monthly", 1, "0 9 * * *"],
        ["yearly", 2, "6 15 30 11 *"],
        ["yearly", 1, "0 9 * * *"],
      ] as const
    ).map(([unit, interval, cronExpr]) => ({
      unit,
      interval,
      cronExpr,
      anchorAt: anchor,
      expected: buildIntervalScheduleRule(unit, interval, cronExpr, anchor),
    })),
    cron: CRON_EXPRESSIONS.map((expr) => ({
      expr,
      runs: FROM_POINTS.map((from) => ({
        from,
        next: computeNextRunAt(expr, from),
        previous: previousRuns(expr, from, 3),
      })),
    })),
    scheduleRuleNext: scheduleRuleCases().map((rule) => ({
      rule,
      runs: FROM_POINTS.map((from) => ({
        from,
        next: computeScheduleRuleNextRunAt(rule as never, from),
      })),
    })),
    initialNext: initialNextCases(),
    automationNext: scheduleRuleCases().slice(0, 6).flatMap((scheduleRule) => [
      {
        cronExpr: "0 9 * * *",
        scheduleRule,
        from: anchor,
        next: computeAutomationNextRunAt({ cronExpr: "0 9 * * *", scheduleRule } as never, anchor),
      },
      {
        cronExpr: "0 9 * * *",
        scheduleRule: null,
        from: anchor,
        next: computeAutomationNextRunAt({ cronExpr: "0 9 * * *", scheduleRule: null } as never, anchor),
      },
    ]),
    validCron: [...CRON_EXPRESSIONS, ...INVALID_CRON].map((expr) => ({
      expr,
      expected: isValidCronExpr(expr),
    })),
    fuzz: fuzzCases(),
    carrier: carrierCases(),
    forceRecurring: [
      { title: "x", recurring: false, maxRuns: 1, scheduleRule: { unit: "minute", interval: 5 } },
      { recurring: true, maxRuns: null },
      {},
    ].map((input) => ({ input, expected: forceIntervalCarrierRecurring(input as never) })),
  };
}

function main(): void {
  mkdirSync(OUTPUT_DIR, { recursive: true });
  for (const zone of ZONES) {
    process.env.TZ = zone.iana;
    const offsetMinutes = new Date(0).getTimezoneOffset();
    if (-offsetMinutes * 60 !== zone.offsetSecs) {
      throw new Error(
        `时区切换失败：${zone.iana} 期望偏移 ${zone.offsetSecs}s，实得 ${-offsetMinutes * 60}s`,
      );
    }
    const fixture = { zone: zone.name, iana: zone.iana, offsetSecs: zone.offsetSecs, cases: buildZoneCases() };
    const output = path.join(OUTPUT_DIR, `schedule-${zone.name}.json`);
    writeFileSync(output, `${JSON.stringify(fixture, null, 2)}\n`);
    process.stdout.write(`wrote ${path.relative(ROOT_DIR, output)}\n`);
  }
}

main();
