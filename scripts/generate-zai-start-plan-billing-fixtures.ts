/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 ZAI Start Plan 纯逻辑矩阵。 */
/**
 * 生成 `zai-start-plan-billing` 的 TS 对拍数据。
 *
 * 真源：`packages/services/src/model-provider/zaiStartPlanBilling.ts` 的
 * `buildZaiStartPlanBalanceUrlFor`、`resolveZaiStartPlanBalanceModelIds`、
 * `normalizeStartPlanExpiry`。
 *
 * ```bash
 * pnpm zai-start-plan-billing:fixtures
 * cargo test -p zai-start-plan-billing
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildZaiStartPlanBalanceUrlFor,
  normalizeStartPlanExpiry,
  resolveZaiStartPlanBalanceModelIds,
  type ZaiStartPlanBalanceEnvelope,
} from "../packages/services/src/model-provider/zaiStartPlanBilling.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "zai-start-plan-billing",
  "fixtures",
  "billing.json",
);

const urlCases = [
  {
    name: "adds-app-version",
    baseUrl: "https://example.com/api/billing/balance",
    appVersion: "2.0.0",
  },
  {
    name: "replaces-existing-app-version",
    baseUrl: "https://example.com/api/billing?x=1&app_version=old&y=2",
    appVersion: "1.2.3",
  },
  {
    name: "keeps-no-query-slash",
    baseUrl: "https://example.com",
    appVersion: "3.0.0",
  },
];

const modelIdCases = [
  {
    name: "capabilities-and-fallback",
    payload: {
      data: {
        balances: [
          { capabilities: ["model:glm-5", "model:GLM-5", " other"] },
          { capabilities: [], show_name: "glm-4.7" },
          { capabilities: ["MODEL: GLM-4.5 "] },
          { show_name: "unknown-model" },
          { capabilities: [""], show_name: "" },
        ],
      },
    },
  },
  {
    name: "missing-data",
    payload: {},
  },
];

const expiryCases = [
  {
    name: "response-time-expires-active",
    responseTime: 100,
    now: 0,
    payload: {
      data: {
        plans: [{ plan_id: "p", user_plan_id: "u", status: " ACTIVE ", ends_at: 100 }],
        balances: [{ plan_id: "p", remaining_units: 1 }],
      },
    },
  },
  {
    name: "server-time-used-when-no-response-time",
    responseTime: null,
    now: 0,
    payload: {
      data: {
        server_time: 50,
        plans: [{ plan_id: "p", status: "active", ends_at: "49" }],
        balances: [{ plan_id: "p" }],
      },
    },
  },
  {
    name: "now-used-when-no-server-time",
    responseTime: null,
    now: 200,
    payload: {
      data: {
        plans: [{ plan_id: "p", status: "active", ends_at: 200 }],
        balances: [{ plan_id: "p" }],
      },
    },
  },
  {
    name: "string-end-and-boundary-equal",
    responseTime: null,
    now: 10,
    payload: {
      data: {
        plans: [{ plan_id: "p", status: "active", ends_at: "10" }],
        balances: [{ plan_id: "p" }],
      },
    },
  },
  {
    name: "non-expiring-values",
    responseTime: null,
    now: 100,
    payload: {
      data: {
        plans: [
          { plan_id: "zero", status: "active", ends_at: 0 },
          { plan_id: "null", status: "active", ends_at: null },
          { plan_id: "missing", status: "active" },
          { plan_id: "already", status: "expired", ends_at: 1 },
        ],
        balances: [{ plan_id: "zero" }, { plan_id: "null" }, { plan_id: "missing" }],
      },
    },
  },
  {
    name: "user-plan-ownership-and-filtering",
    responseTime: 100,
    now: 0,
    payload: {
      data: {
        plans: [
          { plan_id: "same", user_plan_id: "u1", status: "active", ends_at: 1 },
          { plan_id: "same", user_plan_id: "u2", status: "active", ends_at: 999 },
          { plan_id: "fallback", status: "expired", ends_at: 1 },
        ],
        balances: [
          { user_plan_id: "u1", plan_id: "same" },
          { user_plan_id: "u2", plan_id: "same" },
          { plan_id: "fallback" },
          { plan_id: "no-plan" },
          {},
        ],
      },
    },
  },
  {
    name: "plans-missing-keeps-balances",
    responseTime: 100,
    now: 0,
    payload: {
      data: {
        balances: [{ plan_id: "p" }],
      },
    },
  },
  {
    name: "no-data-unchanged",
    responseTime: 100,
    now: 0,
    payload: { code: 0, msg: "ok" },
  },
  {
    name: "preserves-unknown-fields",
    responseTime: 100,
    now: 0,
    payload: {
      code: 1,
      extra: { keep: true },
      data: {
        server_time: 5,
        extraData: "keep",
        plans: [{ plan_id: "p", status: "active", ends_at: 1, extraPlan: 1 }],
        balances: [{ plan_id: "p", extraBalance: 2 }],
      },
    },
  },
];

function withNowSeconds<T>(now: number, task: () => T): T {
  const originalNow = Date.now;
  Date.now = () => now * 1000;
  try {
    return task();
  } finally {
    Date.now = originalNow;
  }
}

const fixture = {
  urls: urlCases.map((testCase) => ({
    ...testCase,
    expected: buildZaiStartPlanBalanceUrlFor(testCase.baseUrl, testCase.appVersion),
  })),
  modelIds: modelIdCases.map((testCase) => ({
    name: testCase.name,
    payload: testCase.payload,
    expected: resolveZaiStartPlanBalanceModelIds(testCase.payload as never),
  })),
  expiry: expiryCases.map((testCase) => ({
    name: testCase.name,
    payload: testCase.payload,
    responseTime: testCase.responseTime,
    now: testCase.now,
    expected: withNowSeconds(testCase.now, () =>
      normalizeStartPlanExpiry(
        testCase.payload as ZaiStartPlanBalanceEnvelope,
        testCase.responseTime ?? undefined,
      ),
    ),
  })),
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
