/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 ZAI Start Plan quota 矩阵。 */
/**
 * 生成 `zai-start-plan-billing` 的 subscription/quota/remaining 对拍数据。
 *
 * 真源：`packages/services/src/usage-stats/providers/bigmodelUsageQuotaProvider.ts`
 * 导出的 Zai Start Plan helper。
 *
 * ```bash
 * pnpm zai-start-plan-quota:fixtures
 * cargo test -p zai-start-plan-billing
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildZaiStartPlanQuota,
  buildZaiStartPlanRemaining,
  buildZaiStartPlanSubscription,
  normalizeZaiStartPlanBalanceLimits,
  pickCurrentZaiStartPlan,
  pickZaiStartPlanBillingCycle,
} from "../packages/services/src/usage-stats/providers/bigmodelUsageQuotaProvider.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "zai-start-plan-billing",
  "fixtures",
  "quota.json",
);

const plans = [
  {
    plan_id: "plan-a",
    user_plan_id: "u1",
    name: " Start Plan ",
    status: " ACTIVE ",
    starts_at: 1_700_000_000,
    ends_at: "1730000000",
    entitlements: [
      {
        entitlement_id: "e1",
        show_name: " Show ",
        period: " monthly ",
        effective_at: 1_690_000_000,
      },
      { show_name: "skip" },
      { entitlement_id: "e2", period: "", effective_at: null },
    ],
  },
  {
    plan_id: "p2",
    name: "daily plan",
    status: "active",
    starts_at: "bad",
    ends_at: 0,
    entitlements: [{ entitlement_id: "e3", period: "daily", effective_at: "1700000000" }],
  },
  {
    plan_id: "expired-plan",
    name: "Start Plan",
    status: "expired",
    ends_at: 1_730_000_000,
  },
];

const balances = [
  {
    bucket_id: "b1",
    user_plan_id: "u1",
    plan_id: "plan-a",
    entitlement_id: "e1",
    show_name: " GLM-5 ",
    meter: " tokens ",
    unit_type: "credits",
    capabilities: ["model:GLM-5", " GLM-4 ", "other"],
    total_units: "100",
    used_units: 25,
    remaining_units: 75,
    period_start: "1690000000",
    period_end: "1700000000",
    expires_at: "1700000000",
  },
  {
    plan_id: "p2",
    entitlement_id: "e3",
    total_units: 0,
    used_units: 0,
    remaining_units: 0,
    capabilities: [],
  },
  {
    plan_id: "skip",
    total_units: null,
    used_units: "",
    remaining_units: "bad",
  },
  {
    user_plan_id: "u1",
    plan_id: "plan-a",
    total_units: 10,
    used_units: 4,
    remaining_units: 6,
    capabilities: ["model: GLM-4.5 "],
    expires_at: 1_710_000_000,
  },
  {
    user_plan_id: "u-other",
    plan_id: "missing",
    entitlement_id: "ex",
    total_units: "10.5",
    used_units: "5",
    remaining_units: "5.25",
    capabilities: ["raw"],
  },
  {
    user_plan_id: "u-mismatch",
    plan_id: "p2",
    entitlement_id: "e3",
    total_units: 8,
    used_units: 2,
    remaining_units: 6,
  },
];

const fixture = {
  plans,
  balances,
  subscription: buildZaiStartPlanSubscription(plans as never),
  subscriptionEmpty: buildZaiStartPlanSubscription(undefined),
  limits: normalizeZaiStartPlanBalanceLimits(balances as never, plans as never),
  limitsWithoutPlans: normalizeZaiStartPlanBalanceLimits(balances as never),
  quota: buildZaiStartPlanQuota(balances as never, plans as never),
  quotaEmpty: buildZaiStartPlanQuota([] as never, plans as never),
  remaining: buildZaiStartPlanRemaining(balances as never),
  remainingEmpty: buildZaiStartPlanRemaining([] as never),
  pick: {
    plans,
    current: pickCurrentZaiStartPlan(plans as never),
    none: pickCurrentZaiStartPlan([] as never),
    nonStart: pickCurrentZaiStartPlan([
      { plan_id: "other", name: "Other", status: "active" },
    ] as never),
  },
  billingCycle: pickZaiStartPlanBillingCycle(plans[0] as never),
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
