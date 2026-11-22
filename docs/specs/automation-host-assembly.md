# Spec：Automation 宿主装配（G2.2/G2.3）

真源：`packages/services/src/session/automationRuntime.ts`（dispatchRun/trackRunOutcome）
与 `packages/services/src/mode-agent/modeTaskServiceAdapter.ts`（createTask/resumeTask/
setAutomationSessionConfig/sendPrompt）。Rust：`automation-runtime`（状态机）+
`host-channels::automation_host`（宿主装配）。

## 组成

- `AutomationRepo`：与 tasks-index 同库（`tasks-index.sqlite`），单一调度状态写入方。
- `AutomationRuntime`：poll/claim/heartbeat/dispatch/settle（crate 内已与 TS 对拍）。
- `AutomationDispatch`（生产 `DispatchPort`）：**只经 mode-task 频道公开 call 面**编排，
  不复制第二套索引写入路径：
  - `targetTaskId` 存在 → `resumeTask`（thoughtLevel）→ `setAutomationSessionConfig`
    （modelSelection/thoughtLevel/mode；v4 CAS switchModelConfig ×2 + mode CAS + resume
    快照 + 索引回写）。
  - 否则 `createTask`（model/thoughtLevel/mode/automationId；频道内部按 automation
    使用 deferred persistence + titleGeneration=false + cronAutomationId 盖章）。
  - **终态订阅先于 sendPrompt**：`TaskEventBus::subscribe_terminal(taskId)`，载荷
    `inputId` 缺省回退 runId；sendPrompt 失败立即退订。
  - `sendPrompt` 携带 `inputId=runId` 与 `automationId`（CronCreate 暴露面不丢失）。
  - `mark_task_unread`：best-effort 异步 `setTaskUnread(unread:true)`（失败不影响结算）。
- `ModelSelectionPort`：宿主注入。provider selection facade（getView 的
  effective/preferred/issue 解析）尚未迁 Rust；实现方必须给出真实解析，宿主侧不伪造。

## 所有权与翻转

`create_automation_host` 只构造不启动；启动/停止所有权由宿主决定。翻转时必须先停
Node `automationRuntime.start()`（双跑会双派发），并保持单实例。

## 验收

`cargo test -p host-channels --lib automation_host`（宿主构造 + mark unread 索引/事件）+
`cargo test -p automation-runtime`（状态机 fake-port 对拍）。
