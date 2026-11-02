# Spec：定时任务管理服务（Rust `automation-service`）

状态：2026-11-01 完成 10/10 方法（含 create/update 调度语义与双时区 TS 对拍）。
真源：`packages/services/src/session/automationService.ts`、`automationRepo.ts`；
底层已在 `crates/automation-repo`、`crates/automation-schedule` 与 TS 逐步对拍。

## 1. 范围

| Rust 方法                  | TS 方法                  | 说明                                                                 |
| -------------------------- | ------------------------ | -------------------------------------------------------------------- |
| `create(params, scope, now)` | `create`               | relative delay / interval carrier / 规则校验 / 首次触发时间 / endAt 终态 |
| `update(id, params, scope, now)` | `update`           | cron 变更、规则锚点、recurring/maxRuns、endAt、终态复活               |
| `list(scope)`              | `list`                   | repo.list + `ModeAutomation` 投影                                     |
| `list_all()`               | `list`（无 scope）       | 同上                                                                 |
| `delete`                   | `delete`                 | 返回是否真的删除                                                     |
| `set_enabled`              | `setEnabled`             | 失败/恢复，不重算 nextRunAt                                          |
| `restart`                  | `restart`                | 仅 `lifecycle_status == "failed"`；按 cron/规则重算 nextRunAt        |
| `run_now`                  | `runNow`                 | repo 的 manual claim，返回 `{automation, run}`                       |
| `list_runs` / `delete_run` | `listRuns` / `deleteRun` | 按 workspace key 过滤                                                |

`hasTaskBinding` 由 repo 直接提供，未在服务层暴露（TS 侧同样只做透传）。

## 2. create 的调度语义

校验顺序与 TS 一致：

1. `relativeDelayMinutes`：必须是 1–525600 的整数；`recurring=true`、`maxRuns`、
   `scheduleRule` 均与相对时间互斥。
2. interval carrier（`intervalUnit` + `interval`）：必须配对、1–200 整数、与
   relative delay / scheduleRule 互斥、`recurring` 不得为 false、不得带 `maxRuns`。
3. 归一化：
   - relative delay → `buildRelativeDelaySchedule` 生成兼容 cron + minute 规则，
     `recurring=false`；
   - carrier → `buildIntervalScheduleRule`，强制 `recurring=true`、清空 `maxRuns`。
4. cron 合法性（`isValidCronExpr` 等价）；显式 `scheduleRule` 走完整规则校验
   （unit/interval/hour/minute/weekdays/monthDays/months/monthlyMode）。
5. 显式规则的 `anchorAt` 统一改写为本次创建时刻；无显式规则时对 `*/N * * * *`
   补 minute 锚点。
6. `compute_initial_automation_next_run_at`：一次性固定月日 cron 刚错过目标分钟时
   仅在 60 秒内补执行，其它已过去目标返回 stale 错误。
7. `endAt` 早于首次触发 → `nextRunAt = null`、`lifecycleStatus = "completed"`
   （repo 同时置 `enabled = false`）。

## 3. update 的调度语义

- `scheduleRule: null` 是显式清除，与「未提交」区分；清除后恢复 cron 语义。
- cron 变更且未提交显式规则：`*/N * * * *` 重新推断 minute 规则并锚定本次保存；
  其它 cron 清空旧规则。
- 显式规则内容（`scheduleRuleDefinition`）未变时保留原 `anchorAt`；真正改频率时
  重置为本次保存时刻。carrier 提交总是重新锚定。
- `maxRuns: null` 必须与 `recurring=true` 同一次提交；`recurring=true` 时不得设置
  有限 `maxRuns`。只提交 `recurring=true` 时领域层原子清空旧上限。
- 生命周期：显式清空 `endAt` 或编辑后重新可跑，会把 `completed`/`failed` 复活为
  `active` 并补算 `nextRunAt`；有限次任务按 `scheduledRunCount` 判定耗尽。
- 更新不存在的 automation 返回 `None`。

## 4. 投影

`ModeAutomation` 去掉 repo 的 `workspaceKey/workspacePath/workspaceIdentity/locationKind/
dispatch*` 等内部字段，保留频道 schema 的
`automationId/title/cronExpr/prompt/modelSelection?/mode?/targetTaskId?/enabled/
lifecycleStatus/nextRunAt?/lastRunAt?/runCount/recurring/maxRuns?/scheduleRule?`。
`scheduleRule` 从 repo 的 JSON 反序列化为 `automation_schedule::ScheduleRule`；解析失败按
缺省处理（与 TS 侧结构化投影一致）。`ModeAutomationRun` 同样去掉 `workspaceKey/attempts`。

入参 `modelSelection` / `botDeliveryTarget` 用本地 `Deserialize` 形状接收（repo 类型只
实现 `Serialize`），再转换为 repo 类型。

## 5. 作用域与时钟

`scope_key` 与 TS `resolveScopeKey` 一致：没有 `workspacePath` → `None`；否则
`workspaceIdentity.trim()` 非空优先，否则用 `workspacePath`。

`now` 由调用方注入；`offsetSecs` 在服务构造时固定（与 `automation-schedule` 口径一致），
避免读进程本地时区造成跨机器差异。

## 6. 对拍夹具

`scripts/generate-automation-service-fixtures.ts` → `crates/automation-service/fixtures/
service-{utc,asia-shanghai}.json`（各 14 条用例，逐 op 记录期望）。

- 每条用例独立临时 SQLite（`new AutomationRepo(dbPath)`），`Date.now` 固定为用例 `now`；
- 覆盖 create 的 relative delay / carrier / 显式规则 / endAt / 非法入参，update 的
  cron、规则清除与锚点、maxRuns/recurring、endAt/复活、carrier 冲突，以及
  list/listAll/setEnabled/runNow/listRuns/deleteRun/delete 全链路；
- 自动化 ID 与 runId 归一化为 `<automation-id>` / `<run-id>`；数字比较按 f64
  （TS `10` 与 Rust `10.0` 等价）。

## 7. 已知差异

1. **stale 错误文案**：TS 用 `toLocaleString()`（随系统语言变化），Rust 用固定
   `YYYY/M/D HH:mm:ss`。夹具只断言「是 stale 错误」，文案差异不复刻。
2. **未知枚举值**：TS 在规则校验阶段报 `unit 不受支持` / `monthlyMode 不受支持`；
   Rust 在反序列化阶段即拒绝（serde 错误）。合法输入的行为完全一致。
3. **频道未接管**：`mode-agent` 的 automation 方法仍由 Host 的 TS 服务提供；
   本 crate 是服务层迁移，切换所有权见 `docs/specs/host-ownership-switch.md` §6。

## 8. 验收

- `cargo test -p automation-service`：Rust 单测 + 双时区逐 op 对拍。
- `cargo clippy -p automation-service --all-targets -- -D warnings`。
