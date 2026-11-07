# Spec：Coding Plan 权益解析（Rust `coding-plan-entitlement`）

状态：2026-11-05 第一版。
真源：`packages/services/src/bigmodel/codingPlanEntitlement.ts` 与
`codingPlanProviderAvailability.ts` 的 `validateSubscriptionListAvailability`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `parse_personal_coding_plan_entitlement` | `fetchPersonalCodingPlanEntitlement` 的纯解析 |
| `parse_team_coding_plan_entitlement` | `fetchTeamCodingPlanEntitlement` 的纯解析 |
| `resolve_subscription_list_availability` | `validateSubscriptionListAvailability` 的结果映射 |
| `is_coding_plan_product` / `is_active_personal_coding_plan` | 同名判定 |

**不做**：HTTP 请求、Authorization 头、超时、401/403 分类；由调用方注入 payload。

## 2. 行为

- 业务信封：对象；`success === false`、`code` 非 0/200、或字段类型不符 → `Unknown`。
- 个人列表：
  - `data` 必须是数组；
  - 逐条严格解析 `{productId?, productName?, status, inCurrentPeriod, autoRenew?, ...}`；
  - 命中 `status === "VALID" && inCurrentPeriod === true` 且 productId/productName
    含 `coding` → `Available`；
  - 疑似 Coding 条目但结构损坏 → `Unknown`；无关条目损坏不影响结论；
  - 否则 `Unavailable { reason: None }`。
- 团队详情：
  - `hasSubscription === false` → `Unavailable`；
  - `status === "EXPIRED"` → `Unavailable(Expired)`；
  - `EFFECTIVE + UNASSIGNED` → `Unavailable(Unassigned)`；
  - 其它未识别状态 → `Unknown`；`EFFECTIVE + VALID` → `Available`。
- 可用性映射：无 API Key → `Unknown`；`Unavailable` → overlay 的
  `coding_plan_not_entitled`；`Available` → `Available { models: None }`。

## 3. 差异

1. zod `optional()` 不接受 null；serde `Option` 接受 null（API 未返回该形状，按宽容处理）。
2. 未镜像 `autoRenew` 之外的未来字段；未知键忽略（与 zod strip 一致）。

## 4. 验收

- `cargo test -p coding-plan-entitlement`：6 条（个人可用/不可用、损坏 Coding 条目
  vs 无关损坏、业务失败/非数组、团队四态、无 Key 与映射）。
