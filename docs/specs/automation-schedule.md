# Spec：定时任务调度计算（Rust `automation-schedule`）

状态：2026-10-30 实现完成，与 TS 真源逐步对拍通过；**尚未接入运行时**（调度写入方仍是 TS）。

## 1. 范围

本 spec 固定 `crates/automation-schedule` 的行为：给定一条定时任务定义与一个基准时刻，
算出「下一次 / 上一次触发」。它只做纯计算，不读时钟、不碰数据库、不发请求。

不在本 spec 内：`AutomationRepo` 的认领与退避（见 `docs/specs/automation-repo.md`）、
`AutomationRuntime` 的派发链（见 `docs/specs/automation-runtime.md`）、频道接线。

## 2. 行为真源

clean-room：只以本仓库 TS 为 oracle。

- `packages/services/src/session/automationCron.ts`：`computeNextRunAt`、
  `computeScheduleRuleNextRunAt`、`computeInitialAutomationNextRunAt`、
  `computeAutomationNextRunAt`、`buildRelativeDelaySchedule`、`buildIntervalScheduleRule`、
  `inferMinuteIntervalScheduleRule`、`isOneShotAutomation`、`scheduleRuleDefinition`。
- `packages/services/src/session/automationCronValidation.ts`：`isValidCronExpr`。
- `packages/services/src/session/automationIntervalCarrier.ts`：
  `assertValidAutomationIntervalCarrier`、`forceIntervalCarrierRecurring`。
- `packages/shared/src/automation-types.ts`：`ModeAutomationScheduleRule` 字段语义。
- cron 表达式语义以仓库依赖的 **croner 10** 的黑盒行为为准（`packages/services/package.json`）。

## 3. 公开面

| Rust | TS 对应 | 说明 |
| --- | --- | --- |
| `is_one_shot_automation(recurring, max_runs)` | `isOneShotAutomation` | 非循环且至多一次 |
| `schedule_rule_definition(&rule)` | `scheduleRuleDefinition` | 展示字段判等用的规范串 |
| `build_relative_delay_schedule(minutes, from, offset)` | `buildRelativeDelaySchedule` | 「几分钟后」→ 兼容 cron + 锚点规则 |
| `infer_minute_interval_schedule_rule(expr, anchor, offset)` | 同名 | `*/N * * * *` → 分钟规则 |
| `build_interval_schedule_rule(unit, interval, expr, anchor, offset)` | 同名 | carrier → 权威规则 |
| `compute_schedule_rule_next_run_at(&rule, from, offset)` | 同名 | 规则推进 |
| `compute_next_run_at(expr, from, offset)` | `computeNextRunAt` | cron 推进 |
| `compute_previous_runs(expr, from, count, offset)` | croner `previousRuns` | 供一次性窗口判定 |
| `compute_automation_next_run_at(expr, rule, from, offset)` | 同名 | 有规则走规则，否则走 cron |
| `compute_initial_automation_next_run_at(...)` | 同名 | 首次触发；陈旧目标返回 `StaleOneShotScheduleError` |
| `is_valid_cron_expr(expr)` | `isValidCronExpr` | 解析成功即合法 |
| `assert_valid_automation_interval_carrier(map)` | 同名 | 错误文案逐字一致 |
| `force_interval_carrier_recurring(map)` | 同名 | `recurring=true`、`maxRuns=null` |

所有入口都额外接收 **本地偏移秒数** `offset`（见 §7）。

## 4. cron 语法（对齐 croner 10）

- 段数 5/6/7：`分 时 日 月 周`；6 段在首位补秒；7 段末位是年份。
- 通配与修饰：`*`、`?`（等价 `*`）、`a-b`、`*/n`、`a-b/n`、`a,b,c`。
- 名称：`JAN-DEC`、`SUN-SAT`（大小写不限）；`7` 等同周日。
- 特殊：`L`（月末）、`LW`（月末工作日）、`nW`（距 n 最近的工作日）、`nL`（当月最后一个周 n）、
  `n#k`（当月第 k 个周 n）。
- 昵称：`@yearly`/`@annually`/`@monthly`/`@weekly`/`@daily`/`@midnight`/`@hourly`；
  `@reboot` 与 `@weekdays`/`@weekends` 非法。
- 非法：段数不在 5-7、倒序范围（`50-10`）、数字前缀步长（`5/15`）、步长为 0 或超过字段长度、
  越界值（分 60、时 24、日 0/32、月 0/13、周 8、年 0/10000）。

组合语义：

- 「日」与「周」**都由用户限定**（字段不是 `*`；`?` 不算通配）→ 取 **或**。
- 只有一方受限 → 按 **与**（另一方视为任意）。
- `0 0 ? * 1` 因此每天命中（`?` 参与「都受限」判定且匹配任意日），与 croner 一致。

## 5. `scheduleRule` 推进

`scheduleRule` 是调度权威，`cronExpr` 只做兼容展示。`interval = max(1, floor(rule.interval))`。

| unit | 推进方式 |
| --- | --- |
| `minute` | 从固定 `anchorAt` 按 `interval` 分钟步进：`anchor + max(1, floor((from-anchor)/step)+1) * step`。不从墙钟刻度起算，避免晚派发累积漂移。 |
| `hourly` | 锚点当天、`rule.minute` 对齐后按小时步进；`hour` 不参与计算。 |
| `daily` | 从锚点日起按 `interval` 天，命中 `hour:minute`；最多向前找 36 600 天。 |
| `weekly` | 以锚点所在周的周一为起点，按 `interval` 周；`weekdays` 缺省 `[1]`，0=周日。 |
| `monthly` | 从锚点月按 `interval` 月（上限 1200 次偏移）；`monthlyMode="weekday"` 取当月第一个 `weekdays[0]`（缺省周一），否则取排序后的 `monthDays`（缺省 `[1]`），溢出月（如 2/30）跳过。 |
| `yearly` | 目标月取 `months[0]`（1-12，缺省锚点月），目标日取 `monthDays[0]`（缺省锚点日）；月溢出（2/30、非闰年 2/29）跳过该年；最多 400 次偏移。 |

`buildRelativeDelaySchedule` 的 `cronExpr` 是 `分 时 日 月 *`（不补零），实际触发仍由带
`anchorAt` 的分钟规则决定，避免五段 cron 提前/延后整分钟。

## 6. 一次性任务的错过窗口

仅当「非循环 + 无 `scheduleRule` + cron 是固定月日（`分 时 日 月 *`）」时判定：

| 情形 | 结果 |
| --- | --- |
| 目标刚过去 < 60 s，且下一次要滚到 30 min 之后 | 返回 `from`（立即补执行） |
| 目标过去 60 s–30 min，且下一次要滚到 30 min 之后 | `StaleOneShotScheduleError { target_at }` |
| 目标过去 > 30 min，或下一次仍在 30 min 内 | 返回正常的下一次 |

错误消息里的日期用本地时间渲染（`YYYY/M/D HH:MM:SS`）；与 TS 的 `toLocaleString()` 文案
**不逐字节对齐**，对拍只比较错误类型与 `target_at`。

## 7. 时区契约与已知差异

- 调用方传入 **固定本地偏移秒数**；整个计算内偏移不变。UTC 与 `+08:00` 两种偏移都进对拍。
- **夏令时**：固定偏移意味着跨 DST 跃迁时可能偏差一小时（下一次派发后会重新计算）。
  这与 croner 的逐时刻偏移换算不同，属已知差异；中国区无 DST，暂不阻塞。
- 未实现 croner 的 `+` 前缀（「日/周都受限时取与」）与 `alternativeWeekdays`（Quartz 周编号），
  两者都不在产品调用面上出现。
- 搜索上界：cron 最多向前/向后扫 1200 年；规则按各 unit 的循环上限。

## 8. 验收

```bash
pnpm automation-schedule:fixtures          # 同一进程切 TZ，产出 UTC 与 Asia/Shanghai 两份
cmd //c "D:\tools\cargo-msvc.cmd test -p automation-schedule"
```

fixture（`crates/automation-schedule/fixtures/schedule-*.json`）覆盖：

- 42 条手写 cron 表达式 × 5 个基准时刻（含 `L`/`LW`/`nW`/`nL`/`n#k`/名称/6 段/7 段/`?`/昵称），
  正向 `nextRun` 与反向 `previousRuns` 各 3 条。
- 200 条确定性随机组合表达式 × 3 个时刻（固定 LCG 种子），覆盖字段组合与非法判定。
- 19 条 `scheduleRule` × 5 个基准时刻（跨 anchor 前后、月溢出、年溢出、`monthlyMode` 两种）。
- 19 条定义串、7 条相对延迟、8 条 `*/N` 识别、12 条 carrier 归一化、16 条 carrier 校验、
  3 条 `forceIntervalCarrierRecurring`、7 条一次性窗口、54 条合法性判定。

croner 10 的 `previousRuns` 在 7 段年份与 `?` 的组合上会内部崩溃，这类用例记 `null`，
Rust 侧跳过反向对拍（正向仍覆盖）。

## 9. 实现约束

- 不引入时间库：公历换算用 `days_from_civil` / `civil_from_days`，本地时间 = UTC + 固定偏移。
- 计时单位统一毫秒时间戳；「严格晚于 `from`」按秒取整后 +1 s 起步，与 croner 一致。
- 错误类型化：`StaleOneShotScheduleError` 与 `InvalidAutomationIntervalCarrierError` 不靠文本分流。
- `scheduleRuleDefinition` 的数字按 JS `Number` 口径输出（整数不带 `.0`）。
