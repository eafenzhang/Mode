# Spec：ZAI Start Plan balance 纯逻辑（Rust `zai-start-plan-billing`）

状态：2026-11-02 第一版。
真源：`packages/services/src/model-provider/zaiStartPlanBilling.ts` 的
`buildZaiStartPlanBalanceUrlFor`、`resolveZaiStartPlanBalanceModelIds`、
`normalizeStartPlanExpiry`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `build_zai_start_plan_balance_url_for` | `buildZaiStartPlanBalanceUrlFor` |
| `resolve_zai_start_plan_balance_model_ids` | `resolveZaiStartPlanBalanceModelIds` |
| `normalize_start_plan_expiry` | `normalizeStartPlanExpiry` |
| `build_zai_start_plan_subscription` | `buildZaiStartPlanSubscription` |
| `normalize_zai_start_plan_balance_limits` | `normalizeZaiStartPlanBalanceLimits` |
| `build_zai_start_plan_quota` | `buildZaiStartPlanQuota` |
| `build_zai_start_plan_remaining` | `buildZaiStartPlanRemaining` |
| `pick_current_zai_start_plan` | `pickCurrentZaiStartPlan` |
| `pick_zai_start_plan_billing_cycle` | `pickZaiStartPlanBillingCycle` |

网络请求、in-flight 缓存、HTTP Date 读取与 `Date.now` 由调用方负责；Rust 入口注入
`response_time` 与 `now_secs`。

## 2. URL

`app_version` 写入 query：已存在时原位替换（保留其它参数顺序），不存在时追加；
空 path 的 origin 输出尾部 `/`。非法 URL 返回错误。

## 3. modelIds

- 遍历 `data.balances[].capabilities`，`model:` 前缀大小写不敏感，取后缀 trim；
  没有有效 capability 时回退 `show_name`。
- 通过官方 GLM 规范 ID 列表做大小写归一；未知 ID 原样保留。
- 按小写 key 去空、去重，保持首次出现顺序。

## 4. 过期归一化

- `now = response_time ?? 合法 server_time ?? now_secs`（秒）。
- plan 状态 trim 后小写为 `active`，且 `Number(ends_at)` 有限、>0、<= now 时，
  status 改为 `expired`；其它值保持不变。
- balance 归属：双方都有非空 `user_plan_id` 时按它匹配，否则严格比较 `plan_id`
  （缺失与 null 不相等）。
- 有归属 plan 且全部 `expired` 的 balance 被过滤；无归属、或任一同归属 plan 未过期时保留。
- `data` 存在时始终输出 `plans` 数组（缺失补 `[]`）；`balances` 缺失保持缺失；
  未知字段原样保留。`data` 缺失时原样返回。

## 5. Subscription / Quota / Remaining

- subscription：只保留 status trim 小写为 `active` 的 plan；`plan_id` 缺省空串，
  `name` 缺省“编程套餐”；时间字段按 Unix 秒转 ISO，非正数/非法值输出 null；
  entitlements 只保留非空 `entitlement_id`，`showName` 始终输出（可为 null）。
- balance limits：至少 total/used/remaining 之一有效才保留；`type` 依次取
  entitlement_id → capabilities 拼接 → meter → `model_usage`；数量字段为 undefined 时
  省略；percentage 仅在 total>0 且 remaining 存在时输出；period/periodStart/periodEnd
  由 plan entitlement 与 balance 字段推导。
- capabilities 前缀 `model:` 大小写敏感地剥离，保留原始空白（usageDetails 的
  modelCode 不 trim）；capabilities 为空时 usageDetails 为空。
- remaining：limits 为空返回 null；count/percentage 按 number/remaining 求和；
  nextResetTime 取最小值，缺失为 null。
- quota：limits 为空返回 null，否则 `{level:"Start", limits}`。
- `pickCurrentZaiStartPlan`：status active 且 plan_id/name 小写包含 `start-plan` 或
  `start plan`；billingCycle 取第一个非空 entitlement period。

## 6. 已知差异

1. JS `Number` 的十六进制/`Infinity` 字符串解析未复刻；夹具覆盖普通十进制字符串、
   number、bool、null。
2. TS 的 HTTP Date 与 in-flight 缓存未迁；Rust 接收已解析的响应时间。

## 7. 验收

- `pnpm zai-start-plan-billing:fixtures` →
  `crates/zai-start-plan-billing/fixtures/billing.json`
  - 3 组 URL（追加、原位替换、origin 补 `/`）；
  - 2 组 modelIds（capabilities/回退/大小写/未知/空值、缺 data）；
  - 9 组过期归一化（responseTime/server_time/now、字符串 ends_at、边界相等、
    非过期值、user_plan_id 归属与过滤、plans 缺失、data 缺失、未知字段保留）。
- `pnpm zai-start-plan-quota:fixtures` →
  `crates/zai-start-plan-billing/fixtures/quota.json`
  - subscription 全字段/空输入；limits 全字段、0 额度、非法数量跳过、entitlement
    周期、user/plan 归属、未知 plan；quota/remaining 空输入；plan 选择与 billingCycle。
- `cargo test -p zai-start-plan-billing`（3 条 Rust 单测 + 2 条 TS 回放）。
- `cargo clippy -p zai-start-plan-billing --all-targets -- -D warnings`。
