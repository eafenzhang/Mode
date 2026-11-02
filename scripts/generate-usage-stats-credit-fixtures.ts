/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 Coding Plan monitor 矩阵，拆分会降低可核对性。 */
/**
 * 生成 `usage-stats` 的 Coding Plan credit-usage monitor 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/usage-stats/providers/bigmodelUsageMonitorMapper.ts`
 *
 * ```bash
 * pnpm usage-stats-credit:fixtures
 * cargo test -p usage-stats
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { buildCodingPlanUsageSnapshotFromMonitor } from "../packages/services/src/usage-stats/providers/bigmodelUsageMonitorMapper.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "usage-stats",
  "fixtures",
  "coding-plan-monitor.json",
);
const NOW = 1_700_000_000_000;
const RealDate = Date;

function withFixedTime<T>(timeZone: string, task: () => T): T {
  const previousTimeZone = process.env.TZ;
  const previousDate = globalThis.Date;
  class FixedDate extends RealDate {
    constructor(...args: any[]) {
      if (args.length === 0) super(NOW);
      else super(...(args as [number]));
    }
    static now(): number {
      return NOW;
    }
  }
  process.env.TZ = timeZone;
  globalThis.Date = FixedDate as DateConstructor;
  try {
    return task();
  } finally {
    globalThis.Date = previousDate;
    if (previousTimeZone === undefined) delete process.env.TZ;
    else process.env.TZ = previousTimeZone;
  }
}

const legacyParams = {
  request: { range: "7d" },
  provider: { id: "account:bigmodel-individual-coding-plan", name: "BigModel Coding Plan" },
  quota: { level: "pro", limits: [] },
  granularity: "day",
  xTime: ["2026-01-02", "2026-01-01"],
  startDateKey: "2026-01-01",
  endDateKey: "2026-01-02",
  modelData: {
    x_time: ["2026-01-01", "2026-01-02"],
    modelDataList: [
      { modelName: " A ", sortOrder: 2, tokensUsage: [1, 2], totalTokens: 3 },
      { modelName: "B", tokensUsage: [3, 4] },
    ],
    modelSummaryList: [
      { modelName: " A ", totalTokens: 3, sortOrder: 2 },
      { modelName: "B", totalTokens: 7, sortOrder: 1 },
    ],
    totalUsage: { totalTokensUsage: 10, totalModelCallCount: 5 },
  },
  toolData: {
    x_time: ["2026-01-01", "2026-01-02"],
    toolDataList: [{ toolName: " Web ", usageCount: [4, 5], totalUsageCount: 9 }],
    totalUsage: {},
  },
  activityData: {
    summary: {
      totalTokens: 10,
      peakDailyTokens: 7,
      peakDailyTokensDate: " 2026-01-02 ",
      totalUsageDurationMs: 1234,
      currentStreakDays: 2,
      longestStreakDays: 3,
    },
    series: [
      { date: "2026-01-01", totalTokens: 3, modelCallCount: 1, mcpCalls: 0 },
      { date: "2026-01-02", totalTokens: 7, modelCallCount: 2, mcpCalls: 4 },
    ],
  },
  modelDetailData: { summary: { totalCredits: { value: 2, trend: 0.5 } } },
  toolDetailData: { summary: { cacheHitRate: { value: "0.25" } } },
  healthData: { xTime: ["t1", "t2"], proMaxDecodeSpeed: [1, "bad", 3], liteDecodeSpeed: [0] },
};

const creditParams = {
  request: { range: "30d" },
  provider: { id: "account:bigmodel-individual-coding-plan", name: "BigModel Coding Plan" },
  quota: null,
  granularity: "day",
  xTime: ["2026-01-02", "2026-01-01"],
  startDateKey: "2026-01-01",
  endDateKey: "2026-01-02",
  activityData: {
    summary: { totalTokens: 25, peakDailyTokens: 20, peakDailyTokensDate: "2026-01-02" },
    series: [
      { date: "2026-01-04", totalTokens: 5, modelCallCount: 1, mcpCalls: 2 },
      { date: "2026-01-01", totalTokens: 20, modelCallCount: 2, mcpCalls: 1 },
      { date: "2026-01-03", totalTokens: 0, modelCallCount: 0, mcpCalls: 0 },
      { date: "bad", totalTokens: 99 },
    ],
  },
  modelDetailData: {
    summary: {
      cacheHitRate: { value: "0.5", trend: -0.1 },
      totalCredits: { value: 12, trend: "3" },
      averageDailyCredits: { value: 4 },
    },
    modelUsage: {
      xTime: ["2026-01-01", "2026-01-02"],
      modelDataList: [
        {
          modelName: " GLM 5 ",
          modelCode: "glm-5",
          sortOrder: 1,
          totalTokens: 10,
          totalCredits: 2,
          totalTokensUsage: [1, 2],
          totalCreditsUsage: [0.5, 1],
          cachedInputTokensUsage: [1],
          uncachedInputTokensUsage: [2],
          outputTokensUsage: [3],
          cachedInputCreditsUsage: [1],
          uncachedInputCreditsUsage: [2],
          outputCreditsUsage: [3],
        },
        { modelCode: "cached_input", totalTokens: 99 },
        { modelName: "bucket", modelCode: "output", totalTokens: 88 },
        {
          modelName: "split",
          tokensUsage: [4, 5],
          cachedInputTokensUsage: [1, 2],
          uncachedInputTokensUsage: [3, 4],
          outputTokensUsage: [5, 6],
          totalTokens: 15,
        },
      ],
    },
  },
  toolDetailData: {
    summary: { totalCredits: { value: 1 }, averageDailyCredits: { value: "0.5" } },
    mcpUsage: {
      xTime: ["2026-01-01", "2026-01-02"],
      mcpDataList: [
        {
          mcpName: " Web ",
          mcpCode: "web",
          sortOrder: 2,
          mcpCallCount: [1, 2],
          creditsUsage: [0.5, 1],
          totalCredits: 3,
        },
        { toolCode: "tool-only", usageCount: [5] },
        { mcpName: "no-code", usageCount: [7, 8], totalUsageCount: 20 },
      ],
    },
  },
  healthData: { x_time: ["a"], proMaxDecodeSpeed: ["2"], liteDecodeSpeed: [] },
};

const cases = [
  { name: "legacy-model-and-tool-data", params: legacyParams },
  { name: "credit-usage-with-breakdowns", params: creditParams },
  {
    name: "credit-breakdown-only",
    params: {
      request: { range: "today" },
      provider: { id: "account:bigmodel-individual-coding-plan", name: "BigModel Coding Plan" },
      quota: null,
      granularity: "day",
      xTime: ["2026-01-01"],
      startDateKey: "2026-01-01",
      endDateKey: "2026-01-01",
      activityData: { summary: null, series: [] },
      modelDetailData: {
        summary: null,
        modelUsage: {
          xTime: ["2026-01-01"],
          modelDataList: [
            { modelCode: "cached_input", totalTokens: 3 },
            { modelName: "缓存", totalTokens: 4 },
            { modelCode: "output_tokens", totalTokens: 5 },
          ],
        },
      },
      toolDetailData: { summary: null, mcpUsage: null },
      healthData: {},
    },
  },
  {
    name: "empty-params",
    params: {
      request: { range: "today" },
      provider: { id: "account:bigmodel-individual-coding-plan", name: "BigModel Coding Plan" },
      quota: null,
      granularity: "day",
      xTime: [],
      startDateKey: "2026-01-01",
      endDateKey: "2026-01-01",
    },
  },
  {
    name: "hourly-axis-alignment",
    params: {
      request: { range: "today" },
      provider: { id: "account:bigmodel-individual-coding-plan", name: "BigModel Coding Plan" },
      quota: null,
      granularity: "hour",
      xTime: ["00:00:00", "01:00:00"],
      startDateKey: "2026-01-01",
      endDateKey: "2026-01-01",
      modelData: {
        x_time: ["2026-01-01 01:00", "2026-01-01 00:00"],
        modelDataList: [{ modelName: "A", tokensUsage: [9, 8], totalTokens: 17 }],
        modelSummaryList: [{ modelName: "A", totalTokens: 17 }],
      },
      toolData: {
        x_time: ["2026-01-01 00:00", "2026-01-01 01:00"],
        toolDataList: [{ toolCode: "t", usageCount: [1, 2] }],
      },
      activityData: {
        series: [
          { date: "2026-02-31", totalTokens: 6, modelCallCount: 1, mcpCalls: 0 },
          { date: "2026-13-01", totalTokens: 99 },
          { date: "2026-03-01", totalTokens: 1, modelCallCount: 0, mcpCalls: 0 },
        ],
      },
      healthData: { x_time: [], liteDecodeSpeed: [] },
    },
  },
];

const fixture = cases.map((testCase) => ({
  name: testCase.name,
  params: testCase.params,
  expected: withFixedTime("UTC", () =>
    buildCodingPlanUsageSnapshotFromMonitor(testCase.params as never),
  ),
}));

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ now: NOW, cases: fixture }, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
