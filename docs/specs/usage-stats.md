# Spec：usage-stats 时间范围与坐标轴（Rust `usage-stats`）

状态：2026-11-02 第一版（时间范围子集）。
真源：`packages/services/src/usage-stats/providers/bigmodelUsageMonitorRange.ts`。

## 1. 范围

| Rust 入口                               | TS 对应                              |
| --------------------------------------- | ------------------------------------ |
| `resolve_usage_time_range`              | `resolveUsageTimeRange`              |
| `resolve_coding_plan_usage_time_range`  | `resolveCodingPlanUsageTimeRange`    |
| `UsageTimeRange` / `CodingPlanUsageTimeRange` | 同名返回结构                  |
| `CodingPlanRangeRequest` / `UsageTimeRangeError` | 请求与错误码投影            |
| `normalize_limits` / `pick_primary_limit` | BigModel quota limits 投影与主额度选择 |
| `pick_current_subscription_from_list` | BigModel 订阅列表 → 当前 Coding Plan 摘要 |
| `build_usage_stats_snapshot_from_monitor` | legacy BigModel monitor → `UsageStatsSnapshot` |
| `build_coding_plan_usage_snapshot_from_monitor` | BigModel credit-usage monitor → `CodingPlanUsageSnapshot` |

时间由调用方注入：`now_ms` 与固定本地偏移 `offset_secs`。本 crate 不读系统时钟、
不解析 IANA 时区、不发网络请求。

## 2. App Usage 时间范围

`range=7d` 取 7 天，`all` / `30d` 都取 30 天。以本地日为单位：

- `endTime` 是当前本地日的 `23:59:59`；
- `startTime` 是从当前日往回 `min(days, 30) - 1` 天的本地 `00:00:00`；
- 输出 `YYYY-MM-DD HH:mm:ss`，不附时区。

## 3. Coding Plan 时间范围

先按注入偏移求当前本地日期 `endDateKey`：

| range  | startDateKey        | granularity | xTime                    |
| ------ | ------------------- | ----------- | ------------------------ |
| today  | endDateKey          | hour        | `00:00:00` … `23:00:00`  |
| 7d     | endDateKey - 6 天   | day         | 7 个日期                 |
| 30d    | endDateKey - 29 天  | day         | 30 个日期                |
| custom | 规整后的起始日期    | day         | 起止含端点的逐日日期     |

- `startTime` / `endTime` 固定为 `YYYY-MM-DD 00:00:00` / `YYYY-MM-DD 23:59:59`。
- 自定义范围先 `trim`，再要求 `^\d{4}-\d{2}-\d{2}$`；缺省或形状不符分别返回
  `coding_plan_usage_custom_start_date_invalid` / `..._end_date_invalid`。
- 日期差按 `Date.UTC(year, month - 1, day)` 语义计算：月份/日期溢出会滚动
  （例如 `2026-13-01` 变为 2027-01-01）；输出的 `startDateKey` / `endDateKey`
  仍保留规整后的原始字符串。
- 天数 ≤ 0 → `coding_plan_usage_custom_range_invalid_order`；> 30 →
  `coding_plan_usage_custom_range_too_long`。
- 日轴逐项对原始 startDateKey 做 `+N 天` 的 UTC 日期滚动，因此首项可能已被归一化。

## 4. BigModel quota 投影

- `normalize_limits`：非数组返回空；只保留 `type` 为非空字符串的项；数值字段仅在服务端
  给出 number 时输出；`usageDetails` 只保留 `modelCode` 为字符串的项，`displayName`
  trim 后非空才输出，`usage` 非 number 时为 0。
- `pick_primary_limit`：优先 `TIME_LIMIT`，其次第一条带 `remaining`，否则第一条；
  空列表为 `null`。
- `pick_current_subscription_from_list`：找第一条 active personal Coding Plan
  （`productId` 或 `productName` 含 `coding`、`status=VALID`、`inCurrentPeriod=true`）；
  该条 `productId` 为空则整体返回 `null`，不回退到后面的订阅。
- 摘要字段：`productName` 缺省回退 `productId`（空字符串保留）；`billingCycle` trim
  后为空则 `null`；`autoRenew=true` 或数字 `1` 时 `nextRenewTime` 作为 `renewTime`，
  否则作为 `expireTime`；没有可用 `nextRenewTime` 时从 `valid` 中取最后一个日期作为
  `expireTime`。
- 日期解析：先 trim；含 `T` 或无空格时按原样，否则把第一个空格替换成 `T`；
  `YYYY-MM-DD` 按 UTC，日期时间无时区时按注入偏移解释，支持 `Z` 与 `±HH:MM`；
  输出标准 UTC ISO 字符串。

## 5. Legacy monitor 快照

`build_usage_stats_snapshot_from_monitor(request, modelData, toolData, generated_at)`
只做投影，不读时钟：

- `daily` 与 `x_time` 等长，缺失 tokens/calls 补 0；`totalCharacters` 按 token × 3，
  调用数 > 0 时 `sessionCount=1`。
- 汇总使用 `totalUsage.totalTokensUsage/totalModelCallCount`；activeDays、mostActiveDay
  （并列取第一条）、首末活跃日期均由 daily 推导。
- 模型列表优先 `totalUsage.modelSummaryList`，为空/缺失回退 `modelSummaryList`；
  丢弃空 `modelName`，按总 tokens 降序稳定排序，`share=token/totalTokens`（total=0 时为 0）。
- `heatmap` 保留空 `weeks/monthLabels`，start/end/maxActivityScore 从 daily 推导。
- tools 固定输出 network-search/web-reader/zread；total 优先服务端字段，否则按日累加；
  仅当 `totalSearchMcpCount > 0` 追加 search-mcp。
- `source="bigmodel-monitor"`，`timeZone` 缺省 `"UTC"`。

## 6. Coding Plan credit-usage 快照

`build_coding_plan_usage_snapshot_from_monitor(params, generated_at)`：

- 同时支持 legacy `modelData/toolData` 与 credit-usage `modelDetailData/toolDetailData`；后者存在时优先。
- 数值序列支持 number/字符串/空值，统一转有限数字；序列按规范化时间轴（hour 取
  `HH:MM:SS`，day 取 `YYYY-MM-DD`）对齐到目标 `xTime`；源轴为空且长度一致时直接复用。
- token/credit 序列按 total → 名称序列 → 缓存/未缓存/输出分段求和 的优先级挑选。
- credit 模型行过滤“缓存/未缓存/输出”计费拆分桶；总量在存在真实模型行时忽略桶，
  只有桶时按桶兜底。
- 模型/工具列表按 `sortOrder`（缺省为过滤后下标）稳定排序；summary 列表从对应 data
  list 投影。
- activity 生成按日 heatmap：非法/不可解析日期过滤，周从周日开始，level 按
  `ceil(token/max*4)` 夹在 1..4；summary 缺失字段补 0，favoriteModelName 取模型
  summary 第一条。
- detail 指标 value/trend 允许字符串数字；缺失 value 时 total/平均值补 0、trend 为
  null。health 的 xTime 优先 `x_time`，速度序列逐项转数字，非法值补 0。
- quota/provider/xTime/granularity/range 原样透传；`generatedAt` 注入。

## 7. 已知差异

1. TS 使用系统本地时区或 `Intl` 的 IANA 时区；Rust 使用调用方传入的固定偏移。
   跨夏令时跃迁的日期/范围可能不同，与 `automation-schedule` 的口径一致。
2. `^\d{4}-\d{2}-\d{2}$` 只校验形状，不校验真实日历；非法月份/日期按 JS
   `Date.UTC` 归一化，夹具已覆盖月/日溢出。
3. 本 crate 只覆盖时间范围与 quota 投影；monitor mapper、service 编排、缓存和网络
   数据源仍在 Host。
4. 服务端返回 `null` limit/usageDetail 时 TS 会抛 TypeError；Rust 跳过非对象项。
   夹具不覆盖该非法输入，正常对象/原始值行为一致。
5. legacy monitor 与 Coding Plan credit-usage 快照的 `generatedAt` 由调用方注入；
   TS 使用 `Date.now()`。
6. JS `Number("0x10")` / `Number("Infinity")` 等扩展字符串形式未复刻；夹具只覆盖
   普通十进制字符串与 number。

## 8. 验收

- `pnpm usage-stats:fixtures` → `crates/usage-stats/fixtures/usage-time-ranges.json`
  - 27 条 App Usage 范围：all/7d/30d × UTC/+8/-5 × 日间/年末/月初时刻；
  - 31 条 Coding Plan 范围：today/7d/30d 三时区、自定义 1/30/31 天、倒序、trim、
    月/日溢出、闰年、跨年、缺失与形状非法日期；
  - quota：5 组 limits（非数组、空、全部可选字段、usageDetails 过滤、非法条目）、
    4 组主额度选择、11 组订阅摘要（自动续费 true/1/false、时区/UTC/偏移日期、
    无效 nextRenew 回退、多日期 valid、缺失 valid、非 Coding/失效/非当前周期、
    空 productId 短路、空 productName、空白 billingCycle）；
  - 6 组 legacy monitor 快照（完整 payload、无 total 回退、工具按日累加、并列活跃日
    与稳定排序、空 payload、长短不齐序列）；
  - 5 组 Coding Plan credit-usage 快照（legacy 对齐、credit 分段/拆分桶、仅拆分桶、
    空参数、hour 轴与非法/溢出日期 heatmap）。
- `cargo test -p usage-stats`（2 条 TS 深比较回放 + 4 条 Rust 单测）。
- `cargo clippy -p usage-stats --all-targets -- -D warnings`。
