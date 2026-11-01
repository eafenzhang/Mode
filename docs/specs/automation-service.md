# Spec：定时任务管理服务（Rust `automation-service`）

状态：2026-11-01 首版（8/10 方法；create/update 待下一片）。
真源：`packages/services/src/session/automationService.ts`、`automationRepo.ts`；
底层已在 `crates/automation-repo` 与 `crates/automation-schedule` 与 TS 逐步对拍。

## 1. 范围

| Rust 方法                  | TS 方法                  | 说明                                                          |
| -------------------------- | ------------------------ | ------------------------------------------------------------- |
| `list(scope)`              | `list`                   | repo.list + `ModeAutomation` 投影                             |
| `list_all()`               | `list`（无 scope）       | 同上                                                          |
| `delete`                   | `delete`                 | 返回是否真的删除                                              |
| `set_enabled`              | `setEnabled`             | 失败/恢复，不重算 nextRunAt                                   |
| `restart`                  | `restart`                | 仅 `lifecycle_status == "failed"`；按 cron/规则重算 nextRunAt |
| `run_now`                  | `runNow`                 | repo 的 manual claim，返回 `{automation, run}`                |
| `list_runs` / `delete_run` | `listRuns` / `deleteRun` | 按 workspace key 过滤                                         |

**未迁移**：`create` / `update` 的 carrier 归一化、锚点、生命周期重算（下一片）；
`hasTaskBinding` 由 repo 直接提供，未在服务层暴露。

## 2. 投影

`ModeAutomation` 去掉 repo 的 `workspaceKey/workspacePath/workspaceIdentity/locationKind/
dispatch*` 等内部字段，保留频道 schema 的
`automationId/title/cronExpr/prompt/modelSelection?/mode?/targetTaskId?/enabled/
lifecycleStatus/nextRunAt?/lastRunAt?/runCount/recurring/maxRuns?/scheduleRule?`。
`scheduleRule` 从 repo 的 JSON 反序列化为 `automation_schedule::ScheduleRule`；解析失败按
缺省处理（与 TS 侧结构化投影一致）。`ModeAutomationRun` 同样去掉 `workspaceKey/attempts`。

## 3. 作用域

`scope_key` 与 TS `resolveScopeKey` 一致：没有 `workspacePath` → `None`；否则
`workspaceIdentity.trim()` 非空优先，否则用 `workspacePath`。

## 4. 已知差异 / 未完成

1. **create/update 未迁**：这两条承载调度语义（relative delay、interval carrier、
   minute 锚点、cron 变更清规则、终态复活），需要 TS oracle 逐项对拍后再落。
2. **无 TS 对拍夹具**：当前只有 Rust 单测（临时 SQLite 覆盖 list/setEnabled/runNow/
   runs/restart/delete）。下一步补 `pnpm automation-service:fixtures`，用固定时钟与
   `AutomationRepo(dbPath)` 直接对拍投影与状态。
3. **时钟与时区**：`restart` 的 `now` 由调用方注入；`offsetSecs` 在服务构造时注入
   （与 `automation-schedule` 口径一致），避免读进程本地时区导致跨机器差异。
4. **频道未接管**：`mode-agent` 的 automation 方法仍由 Host 的 TS 服务提供；
   本 crate 是服务层迁移，不改变当前所有权。

## 5. 验收

- `cargo test -p automation-service`（2 条）：list/listAll、setEnabled、runNow →
  listRuns → deleteRun、delete；restart 只对 failed 生效并写回 active + nextRunAt。
- `cargo clippy -p automation-service --all-targets -- -D warnings`。
