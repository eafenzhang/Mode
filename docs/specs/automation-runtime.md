# Spec：Automation 运行时（Tauri 单进程派发链）

前置：`docs/specs/legacy-electron-eol.md`（Electron 线删除范围）、`packages/services/src/session/automationRepo.ts`（调度状态机契约）。

## 背景

Electron 时代的 automation 执行链由三部分组成：`packages/desktop/src/main` fork 的独立
scheduler 进程（轮询 `claimDue`/`claimManualRuns`）、main 的 CronRun 消息路由、host 内的
`dispatchCronRun`（createTask/sendPrompt/终态回写）。`d71ed992`（移除旧 Electron 线）把这三部分
全部删除，但 `AutomationRepo`/`AutomationService`/UI 保留在 services 层。后果：

- UI「立即运行」→ `modeAgentService.runAutomationNow` → 需要 `onAutomationManualRunRequested`
  派发器；Tauri host 从未注入 → 必抛 "Automation immediate dispatcher is unavailable."；
- 定时触发没有任何轮询者 → 任务永远停在「下次运行」，`runCount` 恒为 0。

本 spec 定义在 Tauri host-standalone（单 Node sidecar 进程）内重建这条链的行为契约。

## 架构与状态所有者

```
UI(立即运行) ─RPC→ modeAgentService.runAutomationNow
                       │ repo.runNow：占用 single-flight(automations.running=1) + 写 manual run
                       ▼ onAutomationManualRunRequested（host 注入）
              AutomationRuntime.dispatchManualRun ──┐
                                                     │ dispatchRun
scheduler 轮询(20s) ─ claimDue / claimManualRuns ────┤
                                                     ▼
                              resolveTargetServices(workspace) → ServiceCollection
                                ├ remote registry 命中 → scoped services
                                ├ remote identity 无会话 → throw（禁止回退本地）
                                └ 否则 → 本地 services
                              createTask/resumeTask + sendPrompt(traceId=runId)
                              终态订阅 → markRunOutcome + manual claim 释放
```

- **调度状态机**的唯一写入方仍是 `AutomationRepo`（`tasks-index.sqlite`）；runtime 只按其
  doc 契约调用，不新增状态字段。同一契约已有 Rust 实现 `crates/automation-repo`
  （70 步与 TS 对拍，见 `docs/specs/automation-repo.md`），但尚未接入本运行时：
  在频道接线完成前，实际写入方仍是 TS `AutomationRepo`。
  调度计算（cron 与 `scheduleRule` 推进）同样已有 Rust 实现
  `crates/automation-schedule`（见 `docs/specs/automation-schedule.md`），本运行时仍调用
  TS `computeAutomationNextRunAt`；两者在下一次派发后各自推进 `next_run_at`，必须同时翻面，
  否则会出现「谁写库谁按另一套规则推进」的漂移。
- **AutomationRuntime**（`packages/services/src/session/automationRuntime.ts`）是进程内模块，
  持有自己的 `AutomationRepo` 连接；由 host-standalone 在服务面就绪后创建并 `start()`，
  dispose 时 `stop()`（停轮询、释放在途认领、退订终态订阅）。
- **目标路由**由 host 注入 `resolveTargetServices`：本地 workspace 返回本地
  `ServiceCollection`；remote identity 已连接返回该会话的 scoped services，未连接必须抛错，
  不得静默回退本地（远程模型/Registry 不可用时本地派发是错误行为）。

## 行为契约

### 派发（dispatchRun）

1. 目标 services 缺 `IModeTaskService`/`IModelSelectionService` → 抛错（计入派发失败）。
2. 模型选择解析顺序（与 repo doc 一致）：run 已固定的 `modelSelection` →
   `repo.getModelSelectionForDispatch`（undefined=跟随 workspace 首选）→
   `IModelSelectionService.getView` 校验（`selectionIssue` 或无 `effectiveSelection` → 抛
   "Automation 模型选择不可用，请重新选择模型与思考档位"）→ workspace `preferredSelection`
   （缺失 → 抛 "Automation 无法从目标 Host 解析首选模型"）。首次派发成功后用
   `repo.fixRunModelSelection` 固化；重试复用已固定值，不重新解释。
3. `targetTaskId` 存在 → `resumeTask` + `setAutomationSessionConfig`；否则 `createTask`
   （带 `automationId` 归因与 `mode`）。
4. Bot 回推是 best-effort：`IBotsService.watchAutomationRun` 失败只记 warn，不阻断派发。
5. `sendPrompt` 的 `traceId` 即 `runId`，`clientMode="desktop-continuous"`，归因 `automationId`。
6. 派发前订阅 `onDynamicTaskTerminalOutcome(taskId)`：`inputId===runId` 时回写
   `markRunOutcome(runId, outcome)`、`setTaskUnread`、manual 释放 single-flight claim、
   停止心跳并退订。manual 派发后启动 60s `touchManualClaim` 心跳，防长任务被僵尸回收。

### 结算

| 触发 | 派发成功 | 派发失败 |
| --- | --- | --- |
| schedule | `markRunDispatch(dispatched)` + `markDispatched`（推进 `next_run_at`） | `markRunDispatch(failed_to_dispatch)` + `markDispatchFailed(kind=transient)`（退避重试） |
| manual | `markManualRunDispatched`（幂等；**不释放 claim**，等真实 turn 终态） | `markRunDispatch(failed_to_dispatch)` + `releaseManualClaim`，并向调用方 rethrow（UI 显示「触发运行失败」） |

### 轮询与 misfire

- 20s 轮询：`claimDue` 认领到期任务（含重试 `retry_at`）、`claimManualRuns` 恢复崩溃的
  manual 派发（attempts=0 或超时）。
- 首次认领（`dispatchAttempts=0`）且 `nextRunAt` 早于 now 超过 5 分钟 → `skipAndReschedule`
  记 skipped 不补跑；纯一次性任务（`isOneShotAutomation`）错过即终态，不再排后续周期。
- `wake()`：manual run 落库后立即触发一轮认领，避免等下一个轮询周期。

## 验收场景

1. 创建「每10分钟」任务且到点后，20s 内自动建 task 并 sendPrompt，`runCount` 递增、
   `nextRunAt` 前推、历史出现 dispatched run。
2. 编辑页「立即运行」→ 任务真实启动（task 出现在会话列表），历史出现 manual dispatched run，
   `runCount` 递增；再次点击不产生并发（single-flight）。
3. 目标模型选择损坏时「立即运行」失败：toast 显示触发失败、claim 释放、run 记
   failed_to_dispatch。
4. remote workspace 的 automation：远端会话在线时派发到远端 scoped services；离线时派发失败
   且不落到本地 services。
5. app 重启后，过期窗口的待触发任务被记 skipped 并重排下一周期（一次性任务转 completed）。

## 已知缺口

- 闲时任务（off-peak）派发链同样随 Electron 线删除，本 spec 不恢复；`OffPeakTaskRepo` 的
  claim/派发/结算为独立后续工作。
- keep-awake（运行会话时阻止系统休眠）依赖 Electron `powerSaveBlocker`，Tauri 线无对应接线。
