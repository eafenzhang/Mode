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

## Tauri 开关化装配（G2.5，默认关闭）

开关：`MODE_RUST_AUTOMATION=1`（`apps/desktop-tauri/src-tauri/src/automation.rs`，
只接受精确 `"1"`）。

- **Rust 侧**：`start_if_enabled` 在 Tauri 进程内装配
  - provider 快照路径复用 `agent_runtime::resolve_provider_snapshot_paths`（与 agent
    进程注入同口径：resources builtin + data_root personal），
    `provider_config_source::read_snapshot` → `ProviderConfigModelSelectionPort`；
  - `AutomationRepo` 与 Node 同库：`<data_root>/v2/tasks-index.sqlite`；
  - `ModeTaskChannel` 复用 `InprocessState` 的同一 `agent_control` 与
    `task_events`（事件总线按进程共享，终态订阅才能收到 mode-task 发射）；
  - 宿主 log 走壳的 `shell_log`；`utc_offset_secs` 用本机时区。
- **Node 侧**：`serviceAccessor` 同开关下**不创建** `automationRuntime`（不是只不
  `start()`）：两个调度器共用 tasks-index.sqlite，只停轮询仍可能被 manual-run 触达。
  manual-run 在该开关下留痕并显式报错（Rust 侧暂未暴露 manual 入口）。
- **默认关闭**：未设置开关时 Node 装配与行为完全不变。开关只用于真机验收与后续
  翻转；翻转完成后再删除 Node 侧代码。
- **已知风险（验收观察项）**：开关打开时 Rust 与 Node 两个进程仍同时持有
  `tasks-index.sqlite`（Node 其余服务照常写索引），并发写可能触发 SQLite busy。
  这是「双进程共存」阶段的固有限制，不是运行时逻辑；翻转（Node 不再起）后才消失。
- **已知限制（G2.4 Bot 回推未接）**：TS dispatch 内 `getOptional(IBotsService)` +
  `watchAutomationRun` 的 best-effort 回推在 Rust 侧没有对应端口（等价 services
  缺省 → 跳过）。`AutomationRepo::get_bot_delivery_target` 已实现，缺的是
  botsService 迁移；开关打开且配置 Bot 回推的自动化会丢回推，默认关闭不受影响。

## 验收

`cargo test -p host-channels --lib automation_host`（宿主构造 + mark unread 索引/事件）+
`cargo test -p automation-runtime`（状态机 fake-port 对拍）+
`cargo test -p mode-tauri automation`（开关判定）。
