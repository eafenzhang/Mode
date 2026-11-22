# Spec：mode-task 事件面（workspace / ready / terminal）

真源：`IModeTaskService` 事件与 `modeTaskServiceAdapter.ts` + `modeTaskIndexSyncer.ts`
（sessions-index phase 迁移驱动终态）。Rust：`host-channels::mode_task::TaskEventBus` +
`host-channels::sessions_observer`。

## 事件与载荷

| 事件名 | 参数 | 载荷 | 过滤 |
| --- | --- | --- | --- |
| `onDynamicWorkspaceEvent` | `workspace`（string path 或 `{workspacePath, workspaceIdentity}`） | `{type:"workspace_task_list_changed", workspacePath, workspaceIdentity?, taskId?, reason, taskMeta?}` | workspaceKey（`identity.trim()||path`） |
| `onDynamicTaskReady` | `taskId` | `{taskId, reason: "prompt_completed"|"prompt_failed"}` | taskId |
| `onDynamicTaskTerminalOutcome` | `taskId` | `{taskId, inputId?, outcome: "succeeded"|"failed"}` | taskId |

进程内监听者（window-controller 投影刷新等）与 wire 监听者共用同一 notify 点。

## terminal/ready 触发源（2026-11-22）

- **唯一事实源是 sessions-index phase 迁移**：observer 维护每会话 phase；
  `completedSuccess/completedInterrupted → succeeded`、`error → failed`。
  只在「非终态 → 终态」迁移发射（初始快照/新行终态不发射，避免启动时误收口 automation）。
- **顺序**：先 terminal（收口当前 input），再 ready（放行下一条），与 TS
  `emitTerminalAndReady` 一致。
- **inputId 对账**：`sendPrompt({inputId})` 在 dispatch 前记录
  `workspaceKey\0taskId → inputId`；终态发射时取出并清除，terminal 载荷携带该
  inputId（automation/bots 用 `inputId === runId` 对账）。
- 观察器未装配任务事件总线时（独立频道/测试）不发事件。

## 已知差距

- `onDynamicStreamEvent`/`onDynamicTaskEvent`（任务流式投影）仍只有 Node 侧；
  消费方是 bots/automation 旧链路，随 G2 处理。
- phase 迁移在 observer 断流/重建期间可能漏一次终态（resync 后快照直接为终态时
  不补发）；与 TS 的 recovery 行为差异登记。

## 验收

`cargo test -p host-channels --lib sessions_observer`（terminal 迁移判定 +
terminal→ready 顺序/inputId 消费）、`--test mode_task_channel`（listen 过滤与载荷）。
