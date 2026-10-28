# Spec：许可证表达式的 AND/OR 分类

状态：2026-10-28 实现。关联：
[2026-10-18-m4-third-party-ledger.md](../plans/2026-10-18-m4-third-party-ledger.md)（第三方台账）、
`scripts/licenses.mjs`（许可门禁）。

## 目的

`package.json` 的 `license` 字段既可能是单个标识，也可能是 `OR` / `AND` 组合表达式。
组合表达式的分类必须按许可证语义计算，不能取第一个成员兜底：`MIT AND GPL-3.0` 需要同时
满足 MIT 与 GPL-3.0，不能因为第一个成员是 MIT 就判为 green。

## 规则

1. **唯一所有者**：`scripts/lib/license-classification.mjs` 负责表达式解析与分类；
   `scripts/licenses.mjs` 只消费结果，不再保留第二套分类实现。
2. **OR 取最宽松**：满足任意一个分支即可，选择限制最少的分支。
   例如 `MIT OR GPL-3.0` → green。
3. **AND 取最严格**：所有分支都必须满足，取限制最多的分支。
   例如 `MIT AND GPL-3.0` → red-gpl；`MIT AND Apache-2.0` → green。
4. **括号递归**：先按顶层 `OR` 拆分，再按顶层 `AND` 拆分；括号内递归计算。
   `MIT AND (GPL-3.0 OR Apache-2.0)` → green（AND 的另一项可以选 Apache-2.0）。
5. **`WITH` 例外**：按左侧基础许可证分类；例外不降低基础许可证的限制级别，保持保守判断。
6. **未知与缺失**：`(missing)`、`UNLICENSED`、`SEE LICENSE IN ...`、`UNKNOWN`、`N/A`
   归为 `unresolved`，不得视为 green；无法识别的标识归为 `review`。
7. **严重度顺序**（低 → 高）：
   `green` < `yellow-cc` < `yellow-weak` < `yellow-lgpl` < `review` <
   `red-nc` < `red-semiopen` < `red-gpl` < `red-agpl` < `unresolved`。
8. **门禁行为不变**：`scripts/licenses.mjs check` 仍只允许 green；yellow 仅在
   `WEAK_ALLOW` 登记的构建依赖上放行；red/review/unresolved 一律失败。

## 失败语义

- 表达式无法解析时不抛异常到门禁外层；按 `review` 处理并让 `check` 失败，避免解析器错误
  变成静默放行。
- 空字符串与只含括号/空白的表达式按 `unresolved` 处理。

## 验收

1. `MIT AND GPL-3.0` → `red-gpl`，不再误判 green。
2. `MIT AND Apache-2.0` → `green`。
3. `MIT OR GPL-3.0` → `green`。
4. `MIT AND (GPL-3.0 OR Apache-2.0)` → `green`。
5. `GPL-3.0 AND (MIT OR Apache-2.0)` → `red-gpl`。
6. `AGPL-3.0 AND MIT` → `red-agpl`；`LGPL-2.1-only AND MIT` → `yellow-lgpl`。
7. `UNLICENSED`、`SEE LICENSE IN LICENSE` → `unresolved`；`FooLicense` → `review`。
8. `node --test scripts/test/license-classification.test.mjs` 通过。
