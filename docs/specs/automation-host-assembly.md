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
- `ModelSelectionPort`：`host-channels::model_selection_port` 提供生产实现
  `ProviderConfigModelSelectionPort`（每次 getView 读 provider 配置快照）：
  - `ProviderConfigResolver` → `registry_providers`（TS Registry 的可执行子集）；
  - preferred：按顺序取首个非 hidden provider 的首个有档位 model，选最高档；
  - effective：provider 缺失/hidden → issue + null；model 缺失 → issue + null；
    档位缺失/不支持 → issue 且只保留模型身份（不带 options）；完整 → 原样返回；
  - 输入组装：`provider-config-source::read_snapshot`（builtin/personal/active 路径由
    宿主提供）。
  **已知差距**：账号 provider 分类（account-plan/account-offpeak）与 account overlay
  未接（一律 ordinary）、legacy reasoning 归一化未迁、configured-default 源未接。

## 所有权与翻转

`create_automation_host` 只构造不启动；启动/停止所有权由宿主决定。翻转时必须先停
Node `automationRuntime.start()`（双跑会双派发），并保持单实例。

## 验收

`cargo test -p host-channels --lib automation_host`（宿主构造 + mark unread 索引/事件）+
`cargo test -p automation-runtime`（状态机 fake-port 对拍）。
