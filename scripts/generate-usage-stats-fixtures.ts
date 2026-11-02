/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明时间范围矩阵，拆分会降低可核对性。 */
/**
 * 生成 `usage-stats` 的时间范围对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/usage-stats/providers/bigmodelUsageMonitorRange.ts`
 *
 * 通过临时替换全局 Date 与 TZ 固定「当前时间」和本地时区。
 *
 * ```bash
 * pnpm usage-stats:fixtures
 * cargo test -p usage-stats
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  resolveCodingPlanUsageTimeRange,
  resolveUsageTimeRange,
} from "../packages/services/src/usage-stats/providers/bigmodelUsageMonitorRange.js";
import {
  normalizeLimits,
  pickCurrentSubscriptionFromList,
  pickPrimaryLimit,
} from "../packages/services/src/usage-stats/providers/bigmodelUsageQuotaMapper.js";
import { buildUsageStatsSnapshotFromMonitor } from "../packages/services/src/usage-stats/providers/bigmodelUsageMonitorMapper.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "usage-stats", "fixtures", "usage-time-ranges.json");

const RealDate = Date;

function withFixedTime<T>(now: number, timeZone: string, task: () => T): T {
  const previousTimeZone = process.env.TZ;
  const previousDate = globalThis.Date;
  class FixedDate extends RealDate {
    constructor(...args: ConstructorParameters<typeof Date>) {
      if (args.length === 0) {
        super(now);
      } else {
        super(...(args as [number]));
      }
    }
    static now(): number {
      return now;
    }
  }
  process.env.TZ = timeZone;
  globalThis.Date = FixedDate as DateConstructor;
  try {
    return task();
  } finally {
    globalThis.Date = previousDate;
    if (previousTimeZone === undefined) {
      delete process.env.TZ;
    } else {
      process.env.TZ = previousTimeZone;
    }
  }
}

function withTimeZone<T>(timeZone: string, task: () => T): T {
  const previousTimeZone = process.env.TZ;
  process.env.TZ = timeZone;
  try {
    return task();
  } finally {
    if (previousTimeZone === undefined) {
      delete process.env.TZ;
    } else {
      process.env.TZ = previousTimeZone;
    }
  }
}

const zones = [
  { timeZone: "UTC", offsetSecs: 0 },
  { timeZone: "Asia/Shanghai", offsetSecs: 8 * 60 * 60 },
  { timeZone: "Etc/GMT+5", offsetSecs: -5 * 60 * 60 },
];
const rerangeTimes = [
  Date.UTC(2026, 0, 2, 12, 0, 0),
  Date.UTC(2026, 11, 31, 23, 30, 0),
  Date.UTC(2026, 2, 1, 0, 0, 0),
];

const usageRanges = rerangeTimes.flatMap((now) =>
  zones.flatMap((zone) =>
    (["all", "7d", "30d"] as const).map((range) => ({
      now,
      offsetSecs: zone.offsetSecs,
      timeZone: zone.timeZone,
      range,
      expected: withFixedTime(now, zone.timeZone, () => resolveUsageTimeRange({ range })),
    })),
  ),
);

interface CodingCase {
  name: string;
  now: number;
  offsetSecs: number;
  timeZone: string;
  request: {
    range: "today" | "7d" | "30d" | "custom";
    customStartDate?: string | null;
    customEndDate?: string | null;
    timeZone?: string;
  };
}

const codingCases: CodingCase[] = [
  ...([Date.UTC(2026, 0, 2, 0, 30, 0), Date.UTC(2026, 11, 31, 23, 30, 0)] as const).flatMap(
    (now) =>
      zones.flatMap((zone) =>
        (["today", "7d", "30d"] as const).map((range) => ({
          name: `${range}-${now}-${zone.timeZone}`,
          now,
          offsetSecs: zone.offsetSecs,
          timeZone: zone.timeZone,
          request: { range, timeZone: zone.timeZone },
        })),
      ),
  ),
  {
    name: "custom-one-day",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-01-01", customEndDate: "2026-01-01" },
  },
  {
    name: "custom-thirty-days",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-01-01", customEndDate: "2026-01-30" },
  },
  {
    name: "custom-thirty-one-days-too-long",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-01-01", customEndDate: "2026-01-31" },
  },
  {
    name: "custom-inverted",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-01-02", customEndDate: "2026-01-01" },
  },
  {
    name: "custom-trims-values",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: " 2026-01-01 ", customEndDate: "2026-01-02 " },
  },
  {
    name: "custom-month-overflow",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-13-01", customEndDate: "2027-01-02" },
  },
  {
    name: "custom-day-overflow",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-02-31", customEndDate: "2026-03-05" },
  },
  {
    name: "custom-leap-year",
    now: Date.UTC(2024, 1, 29),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2024-02-28", customEndDate: "2024-03-01" },
  },
  {
    name: "custom-cross-year",
    now: Date.UTC(2026, 11, 31),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-12-31", customEndDate: "2027-01-02" },
  },
  {
    name: "custom-missing-start",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customEndDate: "2026-01-02" },
  },
  {
    name: "custom-missing-end",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-01-01" },
  },
  {
    name: "custom-invalid-start-shape",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-1-01", customEndDate: "2026-01-02" },
  },
  {
    name: "custom-invalid-end-shape",
    now: Date.UTC(2026, 0, 2),
    offsetSecs: 0,
    timeZone: "UTC",
    request: { range: "custom", customStartDate: "2026-01-01", customEndDate: "2026-01-2" },
  },
];

const codingRanges = codingCases.map((testCase) => {
  const result = withFixedTime(testCase.now, testCase.timeZone, () => {
    try {
      return {
        ok: true as const,
        value: resolveCodingPlanUsageTimeRange(testCase.request),
      };
    } catch (error) {
      return {
        ok: false as const,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  });
  return {
    name: testCase.name,
    now: testCase.now,
    offsetSecs: testCase.offsetSecs,
    timeZone: testCase.timeZone,
    request: testCase.request,
    expected: result,
  };
});

const limitCases = [
  { name: "non-array-null", limits: null },
  { name: "non-array-object", limits: {} },
  { name: "empty", limits: [] },
  {
    name: "full-fields-and-details",
    limits: [
      {
        type: "TIME_LIMIT",
        unit: 1,
        number: 100,
        usage: 25.5,
        currentValue: 3,
        remaining: 74.5,
        percentage: 25.5,
        nextResetTime: 1_700_000_000_000,
        usageDetails: [
          { modelCode: "glm-5", displayName: " GLM 5 ", usage: 10 },
          { modelCode: "glm-4", displayName: "   ", usage: 2.5 },
          { modelCode: "glm-3", usage: "bad" },
          { modelCode: "", displayName: "", usage: 0 },
        ],
      },
      { type: "TOKENS_LIMIT", unit: 2, usageDetails: "bad" },
    ],
  },
  {
    name: "filters-invalid-limits",
    limits: [
      { type: "" },
      { type: 42 },
      { type: "TOKENS_LIMIT", unit: "1", number: null, usageDetails: [{ displayName: "x" }] },
    ],
  },
];
const pickPrimaryCases = [
  { name: "time-limit-wins", limits: [{ type: "TOKENS_LIMIT", remaining: 1 }, { type: "TIME_LIMIT" }] },
  { name: "remaining-fallback", limits: [{ type: "A" }, { type: "B", remaining: 0 }] },
  { name: "first-fallback", limits: [{ type: "A" }, { type: "B" }] },
  { name: "empty", limits: [] },
];
const subscriptionCases: Array<{
  name: string;
  timeZone: string;
  offsetSecs: number;
  payload: unknown;
}> = [
  {
    name: "auto-renew-true",
    timeZone: "Asia/Shanghai",
    offsetSecs: 8 * 60 * 60,
    payload: [
      {
        productId: "coding-plan-personal",
        productName: "Coding Plan",
        status: "VALID",
        inCurrentPeriod: true,
        autoRenew: true,
        nextRenewTime: "2026-01-02 03:04:05",
        valid: "2026-01-01 00:00:00 - 2026-02-01 00:00:00",
        billingCycle: " MONTHLY ",
      },
    ],
  },
  {
    name: "auto-renew-one",
    timeZone: "Etc/GMT+5",
    offsetSecs: -5 * 60 * 60,
    payload: [
      {
        productId: "coding-plan-personal",
        status: "VALID",
        inCurrentPeriod: true,
        autoRenew: 1,
        nextRenewTime: "2026-02-01T00:00:00Z",
        valid: "2026-01-01 00:00:00 - 2026-02-15 00:00:00",
      },
    ],
  },
  {
    name: "not-auto-renew",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [
      {
        productId: "coding-plan-personal",
        productName: "Coding Plan",
        status: "VALID",
        inCurrentPeriod: true,
        autoRenew: false,
        nextRenewTime: "2026-01-02T03:04:05+08:00",
        valid: "2026-01-01 00:00:00 - 2026-03-01 00:00:00",
      },
    ],
  },
  {
    name: "invalid-next-renew-falls-back-to-valid",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [
      {
        productId: "coding-plan-personal",
        status: "VALID",
        inCurrentPeriod: true,
        autoRenew: true,
        nextRenewTime: "not-a-date",
        valid: "2026-01-01 00:00:00 - 2026-02-01 00:00:00 - 2026-03-01 00:00:00",
      },
    ],
  },
  {
    name: "no-valid-period",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [
      {
        productId: "coding-plan-personal",
        status: "VALID",
        inCurrentPeriod: true,
        autoRenew: false,
        nextRenewTime: "2026-01-02T03:04:05Z",
      },
    ],
  },
  {
    name: "date-only-utc",
    timeZone: "Asia/Shanghai",
    offsetSecs: 8 * 60 * 60,
    payload: [
      {
        productId: "coding-plan-personal",
        status: "VALID",
        inCurrentPeriod: true,
        autoRenew: false,
        nextRenewTime: "2026-01-02",
      },
    ],
  },
  {
    name: "non-coding-product",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [{ productId: "api-plan", productName: "API", status: "VALID", inCurrentPeriod: true }],
  },
  {
    name: "expired-status",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [
      { productId: "coding-plan-personal", status: "EXPIRED", inCurrentPeriod: true },
    ],
  },
  {
    name: "not-current-period",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [
      { productId: "coding-plan-personal", status: "VALID", inCurrentPeriod: false },
    ],
  },
  {
    name: "empty-product-id-first",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [
      { productId: "", productName: "Coding Plan", status: "VALID", inCurrentPeriod: true },
      { productId: "coding-plan-later", status: "VALID", inCurrentPeriod: true },
    ],
  },
  {
    name: "empty-product-name-kept",
    timeZone: "UTC",
    offsetSecs: 0,
    payload: [
      {
        productId: "coding-plan-personal",
        productName: "",
        status: "VALID",
        inCurrentPeriod: true,
        billingCycle: "   ",
        autoRenew: false,
        nextRenewTime: "2026-01-02T03:04:05Z",
      },
    ],
  },
];

const quota = {
  limits: limitCases.map((testCase) => {
    const expected = normalizeLimits(testCase.limits);
    return {
      name: testCase.name,
      limits: testCase.limits,
      expected,
    };
  }),
  primary: pickPrimaryCases.map((testCase) => {
    const limits = normalizeLimits(testCase.limits);
    return {
      name: testCase.name,
      limits: testCase.limits,
      expected: pickPrimaryLimit(limits),
    };
  }),
  subscriptions: subscriptionCases.map((testCase) => ({
    name: testCase.name,
    timeZone: testCase.timeZone,
    offsetSecs: testCase.offsetSecs,
    payload: testCase.payload,
    expected: withTimeZone(testCase.timeZone, () =>
      pickCurrentSubscriptionFromList(testCase.payload as never),
    ),
  })),
};

const monitorCases = [
  {
    name: "full-legacy-monitor",
    request: { range: "7d", timeZone: "Asia/Shanghai" },
    modelData: {
      x_time: ["2026-01-01", "2026-01-02", "2026-01-03"],
      tokensUsage: [100, 0, 50],
      modelCallCount: [2, 0, 1],
      totalUsage: {
        totalTokensUsage: 150,
        totalModelCallCount: 3,
        modelSummaryList: [
          { modelName: "glm-5", totalTokens: 100 },
          { modelName: "glm-4", totalTokens: 50 },
          { modelName: "", totalTokens: 9 },
        ],
      },
    },
    toolData: {
      networkSearchCount: [1, 2],
      webReadMcpCount: [0, 1],
      totalUsage: {
        totalNetworkSearchCount: 3,
        totalWebReadMcpCount: 1,
        totalZreadMcpCount: 0,
        totalSearchMcpCount: 4,
      },
    },
  },
  {
    name: "model-summary-fallback-without-totals",
    request: { range: "all" },
    modelData: {
      x_time: ["2026-01-01", "2026-01-02"],
      tokensUsage: [7, 3],
      modelCallCount: [1, 0],
      modelSummaryList: [
        { modelName: "a", totalTokens: 5 },
        { modelName: "a", totalTokens: 5 },
      ],
    },
    toolData: {},
  },
  {
    name: "tool-totals-from-arrays",
    request: { range: "30d", timeZone: "UTC" },
    modelData: {},
    toolData: {
      networkSearchCount: [1, 2, 3],
      webReadMcpCount: [0, 4],
      zreadMcpCount: [5],
    },
  },
  {
    name: "tie-most-active-and-stable-model-sort",
    request: { range: "7d", timeZone: "UTC" },
    modelData: {
      x_time: ["d1", "d2", "d3"],
      tokensUsage: [5, 5, 3],
      modelCallCount: [1, 1, 1],
      totalUsage: {
        totalTokensUsage: 13,
        totalModelCallCount: 3,
        modelSummaryList: [
          { modelName: "first", totalTokens: 5 },
          { modelName: "second", totalTokens: 5 },
        ],
      },
    },
    toolData: {
      networkSearchCount: [0],
      webReadMcpCount: [0],
      zreadMcpCount: [0],
      totalUsage: { totalSearchMcpCount: 0 },
    },
  },
  {
    name: "empty-payloads",
    request: { range: "all", timeZone: "UTC" },
    modelData: {},
    toolData: {},
  },
  {
    name: "ragged-series",
    request: { range: "7d", timeZone: "UTC" },
    modelData: {
      x_time: ["d1", "d2", "d3", "d4"],
      tokensUsage: [2],
      modelCallCount: [1, 2],
      totalUsage: { totalTokensUsage: 2, totalModelCallCount: 3, modelSummaryList: [] },
    },
    toolData: {
      networkSearchCount: [],
      webReadMcpCount: [2],
      totalUsage: { totalSearchMcpCount: -1 },
    },
  },
];

const monitor = monitorCases.map((testCase) => ({
  name: testCase.name,
  request: testCase.request,
  modelData: testCase.modelData,
  toolData: testCase.toolData,
  expected: withFixedTime(1_700_000_000_000, "UTC", () =>
    buildUsageStatsSnapshotFromMonitor(
      testCase.request as never,
      testCase.modelData as never,
      testCase.toolData as never,
    ),
  ),
}));

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(
  OUTPUT,
  `${JSON.stringify({ usageRanges, codingRanges, quota, monitor }, null, 2)}\n`,
  "utf8",
);
process.stdout.write(`wrote ${OUTPUT}\n`);
