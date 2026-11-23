# LT-16：Node Host 退役专项（2026-11-22 立项）

> 上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §10.1、
> `docs/plans/2026-11-21-lt15-ecosystem-closeout.md` P6、`.plan/accept-w6/README.md`。
> 目标：Node Host 独占频道全部迁入 Rust 后，`w6:readiness --fail-on-blockers` 返回
> READY，开物理删除窗口（删 stdioFrames/stdoutGuard、host-standalone、
> host-sidecar 发运集、mode.cjs 回退随包）。

## 0. 进度摘要（2026-11-23 14:14 复核）

| 子任务 | 状态 | 依据 |
| --- | --- | --- |
| 16.1 mode-task 读面 | ✅ 已收口 | `f5a7595a1` 首切 + `e8ce1f0a7` archiveWorkspaceTasks |
| 16.2a 编排面 | 🟡 主链已落 | `2190dd482`、`e723a2285`；剩 v4 createSession 分支 |
| 16.2b 命令队列/交互 | ✅ 已收口 | `574877ab1`、`42d16bef5`（209 测试全过，当期） |
| 16.2c snapshot/事件/写面 | 🟡 大部分已落 | `02d76edce` → `62c15bdd5`（修正虚报）→ 后续 10 提交；剩 rename 的 v4 同步与 3 个旧 facade 命令 |
| 16.3 window-controller | 🟡 主链已落 | `fa39214f6`…`df94025dd`；剩 remote source 聚合与 registerSource 对外接线 |
| 16.4a lan-access | 🟡 首切 + supervisor 已落，**未翻转** | `a7c78bfed`…`86bf08fbc`；inprocess 仍 `lan_host: None` |
| 16.4b remote-desktop | 🟡 首切已落（设置面/授权/能力探测），**未翻面** | `12f6692b8`；会话面待 `RemoteDesktopPort` 生产实现 |
| 16.4c remote-v3 | ⬜ 未开始 | 仅调查拆分（`a249f1229`） |
| 16.4d remote workspaces | 🟡 频道 + LAN transport 全通，**未翻面** | `9ea0e5058` 之后 LAN 端口已落（见 spec §5）；只剩 SSH/WSL/Docker（需 russh）与 v3（需 snow）的端口 |
| 16.5 频道收口 | 🟡 5 个频道已迁 + **provider-settings 阶段 0–4 全落** | 见下「剩余 live 频道」 |
| G2 automation 宿主 | 🟡 开关化装配已落（默认关） | `b342f2a7f`；G2.6 off-peak 已按政策回退（`03649fef7`） |
| 16.6 物理删除 | ⬜ 未开窗 | `node scripts/w6-readiness.mjs` = NOT READY（6/6/119/95） |

**剩余 live 频道（3）**：`remote-v3`（~5.6k）、`remote` workspaces、`remote-desktop` 会话面续段。
另外两项已裁决不按频道迁：

- **`bots` → P1 裁决（2026-11-23，采纳）**：永久留在 Node sidecar，等嵌入 JS 引擎统一承载。
  依据 `docs/specs/bots-migration-decision.md`：bots 依赖第三方 npm SDK
  （`@larksuiteoapi/node-sdk` / `@wecom/aibot-node-sdk`），按 `full-rust-rewrite.md` §1
  属「不可改写的第三方负载」，与插件负载同类；Rust 重写等于推翻 2026-11-20 的
  「停止自实现四通道」裁决。16.6 删除窗口**不包含** bots。
- **`cua-permission` → 并入 16.4b 会话端口**：见 `docs/specs/cua-permission-migration.md`。

`provider-settings` 阶段 4 与 `remote-desktop` 首切已于 2026-11-23 落地。

## 1. 现状（2026-11-21 立项时；11-23 差异见 §0）

- W2+S2 翻面已执行（`dc095d729`）：默认集接管全部白名单频道含 mode-agent。
- renderer 仍消费 Node Host **独占面**（不在 inprocess 白名单）：
  - `mode-task`（任务系统）：`packages/ui` 的 useModeTaskService/conversationMirrorSync/
    taskListMembershipSets/archivedTaskDeletion 等大量消费。
  - `window-controller`（窗口控制）：windowController 子系统 ~1716 行。
  - `remote-desktop` / `remote-v3` / `lan-access` / `remote`（远程面）。
  - `mode-task` 的实现面：`packages/services/src/session/modeTaskService.ts`（746 行编排）
    + `packages/services/src/mode-agent/`（modeAgentService ~5k 行，任务↔agent 桥）。
  - `host-standalone` 骨架（~4.4k 行 TS）+ `windowController`（~1.7k 行）。
- **底层已迁**：任务库 schema/迁移（`crates/tasks-db`）、任务索引 Repo
  （`crates/task-index-repo`：query_task_list/list_task_metas/grouped 视图/归档清理/
  分组管理/syncTaskMetaAtGroupedTop，全部与 TS 对拍）。

## 2. 子任务拆解与排序

| # | 子任务 | 内容 | 依赖 | 出口 |
| --- | --- | --- | --- | --- |
| 16.1 | **mode-task 读面进壳** | `host-channels` 增 `mode-task` 频道（首切只读 + 分组管理写面）：listTasks/listPinnedTaskIds/listPinnedTasks/listDeletedTaskIds/listTaskList/listGroupedTaskViewStructure/applyGroupedTaskViewOrder/listArchivedTasks/archiveStaleTasks/archiveWorkspaceTasks + 分组 CRUD（create/rename/color/delete group，底层 repo 全齐） | 无（repo 已迁） | 频道对拍（TS 形状逐字段）+ 真机任务列表渲染一致 |
|  |  | ✅ **已收口（2026-11-22）**：`f5a7595a1` 首切（读面 + 分组 CRUD）、`e8ce1f0a7` 补 archiveWorkspaceTasks；频道已进 assembly/inprocess 默认集；真机渲染验收随 `.plan/accept-w6/` 回归清单执行（未回填）。 |
| 16.2 | **mode-task 编排面进壳** | 生命周期/命令队列/permission/elicitation/close/resume/snapshot 族 | 16.1 | 20 任务清单中任务系统场景全过 |

16.2 细拆（调查结论 2026-11-22）：编排真源 = `modeTaskServiceAdapter.ts`（5768 行，
任务↔agent 会话桥状态机）+ `modeTaskService.ts`（746 行）。**按依赖排序三片**：

- **16.2a 会话桥接状态机**：🟡 命令面已落（2026-11-22）——`mode_task.rs` 编排
  分发（async）+ 读面分发（spawn_blocking）双轨：createTask（ensure_provider_config
  → create_session → snapshotToMeta → sync_task_meta + grouped top →
  initialSlashCommands；automation/闲时 deferred persistence + 盖章）、closeTask
  （close_session + deleted patch）、resumeTask（resume_session → meta sync）、
  sendPrompt（sessionSend 直转发）；无控制面装配时显式 unsupported。真实 DB 测试
  186 全过。**2026-11-23 复核**：draftSessionId 复用已落（`e723a2285`，
  `mode_task.rs:1444/1463/1469`）；**剩余**：v4 createSession 分支（现有 v4 调用只到
  `v4::ConversationCommandEnvelope` 的 resolveInteraction）、readyOutcome 事件源（16.2c 已接）。
- **16.2b 命令队列**：🟡 队列状态机已落（2026-11-22）——`task_command_queue.rs`
  （owner 门控三态：NO_ACTIVE_TASK_OWNER/STALE_TASK_OWNER_COMMAND/缺省跳过；
  enqueue 队尾追加 accepted → promote 队首 accepted 化 → cancel not_found 容忍；
  drain 单发消费：active-run/running 门控跳过 → mark running → 锁外 send executor
  发送 → 成功移除/失败标 failed 留队；per-key tokio Mutex 串行 = TS Promise 链
  等价；send executor 注入（生产 = control.send_prompt 包装，测试 = fake））；
  mode-task 频道接 enqueueTaskCommand/promoteTaskCommand/cancelTaskCommand 三方法
  （owner-run/clientId/clientLabel 透传）。9 队列用例（门控/状态转换/drain 成败）
  + 209 测试全过。**permission/elicitation 已接（2026-11-22）**：respondPermission/respondElicitation
收敛 v4 resolveInteraction（ConversationCommandEnvelope + ResolveInteractionPayload，
`answer.optionId` / `answer.action+content` additive 扩展，与 TS 同语义）；
getTaskTarget 的索引归属回读抽为 `resolve_task_workspace` helper。
16.2b 全收口（209 测试全过，clippy 0）。
- **16.2c snapshot 族 + 事件面**：🟡 首切已落（2026-11-22）——getTaskSnapshot
  （resume 快照 → messageLimit 尾部截断 → syncTaskIndexMeta 回源 → 返回
  snapshot+meta；TS snapshotToMode 投影 rows/history/stats 族未迁，形状差异记录
  spec）+ TaskEventBus 事件面（onWorkspaceTaskListChanged/onDynamicTaskReady
  subscribe/notify，createTask→task_created、closeTask→task_meta_changed；
  broadcast 频道同模式；readyOutcome 的 agent 事件触发源待 16.2c 续接）。
  192 测试全过。
  **事件面 v2（2026-11-22）**：事件名/载荷对齐 TS——wire 事件为
  `onDynamicWorkspaceEvent(workspace)`（scope 按 `identity?.trim()||path` 过滤）
  载荷 `{type:"workspace_task_list_changed", workspacePath, workspaceIdentity?,
  taskId?, reason, taskMeta?}`；`onDynamicTaskReady(taskId)` 按 task 过滤。
  emit 点补齐：createTask/closeTask、applyGroupedTaskViewOrder、分组 CRUD
  rename/color/delete（按 scope）、archiveStaleTasks（逐任务带 meta）、
  archiveWorkspaceTasks（单次 scope 事件）；window-controller mutation 成功后向
  同一总线回发 task_pinned/task_unpinned/task_archived/task_unarchived/
  task_deleted/task_meta_changed（CAS 未命中不广播）。**剩余**：
  getTaskSnapshotBody/Ref/ToolCallsSlice 族（TS 空桩恒 null，已同语义）、
  readyOutcome 事件源已接（sessions-index phase 迁移，2026-11-22）、status/title/分组等其余 TS 发射点。
  **状态写面（2026-11-22）**：renderer 直接调用的 mode-task 写方法已落——
  renameTask（title+titleOverridden+updatedAt，task_title_changed）、deleteTask、
  deleteArchivedTask(s)（批量去重/单次广播）、setTaskPinned、setTaskUnread
  （expectedUnreadAt CAS：未命中不写不广播）、archiveTask/unarchiveTask、
  getTaskMeta；均按 TS reason 回发 workspace 事件。
  **会话只读/命令补充（2026-11-22）**：getTaskSessionFilePath（TS 同款路径投影、
  恒 exists=false）、stopGeneration（v4 Stop host envelope）、getTaskTokenUsage
  （v4/conversation/usage，ExistingOnly 不拉起 runtime）、
  releaseWorkspacePreparation/restartWorkspaceProcess（renderer 关闭 workspace/
  重载会话入口 → `dispose_workspace`，2s 宽限与 mode-agent 同口径）、initialize
  （控制面预热 → {available, version}）、getModelTrajectory
  （`model_trajectory.rs`：model-io tail 读取 / delta 展开 / 消息与工具调用映射，
   spec `model-trajectory-reader.md`；4 单测 + 频道分发用例）。**Claude 导入 Stage 1+2（2026-11-22）**：`scanImportableClaudeSessions` 与 `importClaudeSessions` 均已落（扫描/head 过滤、完整解析、文件复制、createSession(importedHistory)、legacy 快照过滤、索引回写与事件；spec `claude-native-session-import.md`）。**legacy 设置面（2026-11-22 全落）**：configOptions 投影 + getTaskConfigOptions/getTaskModelSelection/setMode/setConfigOption(全部 configId)/setModel/setAutomationSessionConfig/getTaskNativeSessionLogFile；setModel 走 provider config 同步 + v4 CAS（TS legacy 差异已注明）。**剩余（2026-11-23 复核，已剔重）**：rename 的
  v4 renameSession 同步（best-effort，`mode_task.rs:1053` 注释确认仍只写 tasks-index）与
  updateIndexedTaskState 的「索引缺失时 resume seed」兜底；`compactSession`/`goalSession`
  经 `mode_agent.rs:700/716` 承载；`deliverSessionMessage`/`sendSessionMessageDeliveryResult`/
  `branchTaskFromPrompt` 三个旧 facade 命令在 `host-channels` **零实现**（renderer 无引用，
  登记为已知差异）。

window-controller（16.3）依赖 16.2a+b（多源聚合消费 IModeTaskService 事件与
sessions-index observer）——**排序 16.2 先于 16.3**。
| 16.3 | **window-controller 进壳** | windowController 子系统（~1716 行）：窗口会话/Host sessions-index observer | 无强依赖 | 真机窗口操作一致 |
|  |  | 🟡 首切 + 帧订阅已落（2026-11-22）：`window_controller.rs` local 单源——listTaskList（repo total/has_more 直用）、mutateTask 8 种 mutation（pin/archive/delete/delete-archived[-batch]/mark-read[CAS]/mark-unread/open/resume → 索引 patch，统一返回 meta\|null）、deleteArchivedTask(s)；V4 帧（subscribeControllerV4/resyncControllerV4/unsubscribeControllerV4 + onDynamicControllerFrame）：tasks-index/workspaces 两 topic、独立 logEpoch+单调 seq、snapshot/delta（fromSeq 恒等上一帧 toSeq）、refresh 结构 diff、退订后静默；liveStatus 对齐 TS defaultLiveStatus（仅 completed/error，其余 idle）；assembly/inprocess/频道名守卫接线；7 用例。**sessions-index overlay 投影（2026-11-22）**：`SessionOverlayStore` + 行/列表富化
（title/titleOverridden/updatedAt/status/pendingInteraction/liveStatus/activity，
searchSnippets 透传）已落；**分片装配器（2026-11-22）**：`agent-protocol::v4_wire_assembler`（ordinal tombstone/recovery supersede/预算/crc32/严格 UTF-8+schema，6 单测）已移植；**观察器泵（2026-11-22）**：`sessions_observer.rs` existing-only 订阅 + 装配器 + summary→overlay 映射 + list/subscribe 触发 + 故障清空重试已落；**观察器恢复（2026-11-22）**：游标连续性校验 + resync(forceSnapshot/existing-only) + 30s 超时重建 + 流结束退避重试 + on_runtime_restarted 清空重订阅；**剩余**：remote source 聚合与重连换源、事件驱动收敛（mode-task `onDynamicWorkspaceEvent` 进程内事件桥已接，createTask/closeTask 后立即推 delta；仍无 sessions-index observer/直接改库观察者）、registerSource 对外接线。 |
| 16.4 | **remote 面进壳** | remote-desktop（~1031 行独立子系统）/ remote-v3（v3 ControlLink）/ lan-access / remoteWorkspaceService（959 行） | 16.2 | 手机远控验收 |
|  |  | 🟡 调查拆分（2026-11-22）。TS 规模：lan-access 适配器 377 + LAN server 591 + 对端 registry/attach 892 ≈ 1.9k；remote-desktop 面依赖 `@mode/client` 主控 controller + `remoteDesktopServer` + 原生媒体（`crates/mode-cua-ax/rd_media` 已存在）；remoteV3 客户端 ~5.6k（`packages/desktop/src/host/remoteV3/`）；remoteWorkspace 959 + services/remote 接口 109。Rust 既有底座：`lan-pairing`（clients/token/配对码/远程桌面授权）、`lan-http`（pair + 六管理端点 + serve）、`lan-discovery`、`crates/server/src/lan.rs`（axum 桥）、`ws_host`（每连接 ChannelServer）、`remote-desktop-streaming`（自适应码率纯状态机）。<br>按依赖拆四片：**16.4a lan-access 管理面**（本机 server supervisor：loopback/LAN 双模式、/ws host 复用、静态资源/中继；getState/setEnabled/createPairCode/removeClient/resetTokens/onDidChange + 对端 side：discoverPeers/pairPeer/listPairedPeers/getPeerWorkspaces/remove/rename）→ **16.4b remote-desktop 频道**（设置/同意/被控状态 + 主控会话 open/probe/signal，复用原生媒体）→ **16.4c remote-v3 频道**（设备身份/登录/ControlLink/tunnels/E2EE，最大块）→ **16.4d remote workspaces**（connect/cancel/bind/dispose/attach/detach + remoteWorkspaceService）。<br>**状态**：16.4a 首切已落（2026-11-22）——`lan-access` 频道对端配对 registry
（listPairedPeers v1→v2 归一化/公网 HTTPS 过滤/pairedAt 排序、removePairedPeer
幂等、renamePairedPeer 1–64 展示名）；server supervisor 与网络 side 显式报错，
channel 已进 assembly/inprocess 默认集；另 `getState` 按 TS
`readDisabledLanAccessState` 输出真实 offline 快照（clients 读凭据并归一化）；
7 用例。**管理面端口（2026-11-22）**：`LanAccessHostPort` 注入后
setEnabled/createPairCode/removeClient/resetTokens/onDidChange 全部委托 supervisor，
getState 读实时状态；未注入时显式报错/offline 快照。**宿主监督器（2026-11-22）**：`mode-server` 的
`LanHostSupervisor` 已落——loopback/LAN 双模式 bind/serve（端口占用回落）、复用
`router_with_lan` + `ws_host_router`、enabled 持久化回调、配对码/clients/reset、
`onDidChange` 快照、`LanAccessHostPort` 同步端口；`ChannelAssemblyOptions.lan_host`
注入频道；`WsHostDeps.agent_control` 补 LAN 客户端 agent 频道。**16.4a 剩余**：
inprocess 装配 supervisor（ServerInfo/工作区动态投影、设置持久化、Host 代重建）、
静态 Web 资源与公网中继、discoverPeers（lan-discovery 复用）、pairPeer/getPeerWorkspaces（lan_peer_client：配对换令牌/公网端点自动生成/合并、端点优先级探测）均已接；**仅剩宿主 supervisor 装配与翻转**。
**翻转约束（必须先解决再接线）**：Node Host 的 `lanAccessService` 仍按
`restoreOnStart` 启动自己的 loopback/LAN 监听并持有配对状态；Rust supervisor
一旦同时上电会出现双监听（45880 被 Node 占用→回落随机端口）与状态分叉。
翻转必须是一个原子改动：先停/删 Node 侧 LAN server 启动点，再由**进程级单例**
（不是每 window 一个）Rust supervisor 接管；需要真机验证「允许局域网直连」与
桌面↔桌面挂接。工作区列表投影（ServerInfoConfig.workspaces）也需从当前打开的
workspace 集合注入，不能沿用 from_env 的 cwd 兜底。`w6:readiness` 四组 blocker（stdioFrames/stdoutGuard/host-sidecar/
node-agent-fallback）均为 Node Host 发运物，随 16.6 删除归零。 |
|  |  | 🟡 收口盘点（2026-11-22，实测）。**host-standalone 6120 行/25 文件**：已由 Rust 接管但仍留 TS——windowController（801+556+359=1716；RPC 已翻面）；仍唯一由 Node 提供——remoteWorkspaceService 959、remoteDesktopService 575、lanAccessService 377（Rust 频道已接管 RPC，supervisor 未接线、Node 仍起监听）、serviceAccessor 502（automation/bots/registry 编排）、index 474（sidecar 入口/stdio/stdoutGuard）；骨架约 1.5k（startupMapping 299、nativeDeps 245、channelGuard 122、scopedAttachments 110、logFile 109、dataRoot 107、stdioFrames 97、rendererConnectionScope 93、bundledAgentRuntime 93、stdoutGuard 87、runtimeResources 54、remoteAssets 54、webAssets 24、internalServiceMembers 23）。<br>**Node-only 频道（相对 Rust 29 个频道常量）共 17 个**：bots、coding-plan-subscription、conversation-share、cua-permission、cua-pip-session、mode-session、model-selection、oauth、off-peak-task、onboarding-record、orcarouter、plugin-management、plugins、provider-provisioning-target、provider-settings、remote、remote-desktop。renderer/platform 实测消费：remoteV3（SettingsPage onDidChange/listHosts）、remote-desktop（desktopPlatform/desktopLocalDisplays 桥）、remote（远程工作区管理）、lan-access（WebRemoteControlDialog/desktopPlatform）；其余多为 Node 内部服务或已被 mode-agent/mode-task 方法面覆盖，删除频道前需逐项复核 ServiceChannels 引用。<br>**Node 内部运行时**（无独立频道、随 serviceAccessor 启动）：AutomationRuntime（`crates/automation-runtime` 已有 start，Tauri 未接线）、Bot 桥（`crates/bots-core` 存在，服务装配未接）、LAN server（Rust supervisor 已备）、remoteV3/remoteDesktop 运行时。<br>**readiness 四组 blocker → 删除条件**：stdioFrames(6)/stdoutGuard(6)=host-standalone index + sidecar-host + `rpc-wire::stdio`，需 Tauri 停止 spawn sidecar；host-sidecar(119 product/22 文件)=build-host-sidecar/staging/harness/bench，同一条件；node-agent-fallback(95/34)=bundled-agents + prepare/stage 脚本 + node.ts 回退查找，需 agent CLI 全原生且移除回退随包。<br>**16.5 门槛**：G1 16.4 全家 Rust 化 + WebRTC 方案；G2 automation/bots/off-peak 宿主迁移（crates 已有 runtime/仓储，缺进程内装配）；G3 LAN supervisor 进程级单例 + 停 Node lanAccessService；G4 mode-task 旧 facade 残余（setModel/compact/goal/stream 事件，仅 bots/automation 用）随 G2 处理；G5 移除 sidecar 启动链（host.rs/build-host-sidecar/staging）→ 四组归零。<br>**量化目标**：删除 ≈ 6.1k TS + host-sidecar 组 22 文件 + node-agent-fallback 34 文件。 |
| 16.5 | **host-standalone 收口** | 骨架（~4.4k 行）随上述面退役：stdioFrames/stdoutGuard/scopedAttachments/rendererConnectionScope 退场 | 16.1–16.4 | `w6:readiness` 四组 blocker 归零 |
|  |  | 🟡 **G2 可行性（2026-11-22）**：Rust `automation-runtime` 状态机完整（poll/claim/heartbeat/dispatch/settle + fake-port 测试），缺的是生产端口与所有权装配——`DispatchPort`（建/恢复 task + **终态订阅** + sendPrompt(inputId=runId) + mark unread）、`ModelSelectionPort`（model-selection + provider-config）、`AutomationRepo` 实例（与 Node 同库）；Bot 回推 `botsService.watchAutomationRun` 属未迁的 botsService（8k+ 行；`bots-core` 仅 433 行 adapter/types），首切可按 TS `getOptional` 跳过但会缺 Bot 会话回推语义。**G2.1 已落（2026-11-22）**：sessions-index phase 迁移 → `onDynamicTaskTerminalOutcome`/`onDynamicTaskReady`（非终态→终态才发射；terminal 先于 ready；`sendPrompt(inputId)` 本地对账并在终态载荷携带/清除；spec `mode-task-events.md`）。**G2 建议顺序**：G2.1 终态事件（已落）→ G2.2/G2.3（2026-11-22 已落：`automation_host.rs`——AutomationRepo（tasks-index 同库）+ 生产 DispatchPort 经 mode-task 频道公开方法编排（create/resume/setAutomationSessionConfig/sendPrompt(inputId=runId,automationId)，终态订阅先于 sendPrompt，mark unread best-effort）+ `ModelSelectionPort` 宿主注入；`setAutomationSessionConfig` 频道方法落地，sendPrompt 透传 automationId）→ G2.4 Bot 回推（可空）→ G2.5 原子翻转（Node `automationRuntime.start()` 停，双跑防护）。**G2.5 开关化装配已落（2026-11-22）**：`MODE_RUST_AUTOMATION=1` 时 Tauri 进程内构造并启动 Rust AutomationRuntime（`apps/desktop-tauri/src-tauri/src/automation.rs`：provider 快照路径复用 agent_runtime resolver → ProviderConfigModelSelectionPort；tasks-index.sqlite 同库；ModeTaskChannel 复用 inprocess 的 agent_control/task_events；宿主 log 走 shell_log），Node `serviceAccessor` 同开关下整条不创建（不是只不 start，双跑防护）；默认关闭、行为与现状一致。**G2 剩余**：开关真机验收（自动触发的端到端 run）、翻转后删 Node automationRuntime、Bot 回推、off-peak 所有权；ModelSelectionPort 已落（provider-config-source → resolver → preferred/effective 解析；账号分类/overlay/legacy reasoning 未接，一律 ordinary——spec 登记）。**G2.4 Bot 回推盘点（2026-11-22）**：TS 在 dispatch 内 `getOptional(IBotsService)` best-effort 调 `watchAutomationRun`（`packages/services/src/session/automationRuntime.ts`，订阅失败只 warn）；Rust `AutomationDispatch` 无 Bot 端口 = 等价 services 缺省（跳过），`AutomationRepo::get_bot_delivery_target` 已备；真正缺口是 `botsService.watchAutomationRun`（botsService 8k+ 行未迁）。**已知限制**：开关打开且配置了 Bot 回推的自动化会丢回推（仅开关验收期，默认关闭不受影响）。**off-peak 所有权核对（2026-11-22）**：Node 侧属主是 `packages/services/src/node.ts`（注册 `OffPeakTaskService` 并 `startSync()`），对外经 Node-only 频道 `off-peak-task` 与 mode-agent `offPeak/create|list` handler 暴露；Rust 已有 `off-peak-repo`（1093 行）+ `off-peak-service`（lib 355 + server 103 + remote 503，含 `start_sync`/`run_sync_cycle` 与 fixtures 测试）+ Tauri keep-awake 读库。**off-peak 执行器缺失复核（2026-11-22）**：`d71ed992b`（移除旧 Electron 线）删除了 `packages/desktop/src/scheduler/`（index 433 + offPeakDispatchSettlement 85 + manualClaimRelease 38 + schedulerProtocol 80）、`src/host/offPeakDispatchPlan.ts` 48、`src/main/desktopCronScheduler.ts` 230，未在 Tauri 重建。实测 `OffPeakTaskRepo.claimDue`/`markRunning` 在 TS/Rust 均无生产调用方，`off-peak-scheduler-wake-request` 频道无发送者——**off-peak 任务当前不会被执行**（既有产品缺口，非 LT-16 引入；Node 也只做管理/取号/轮询）。因此 G2.6 不能按「接线已有执行器」推进：需先定所有权（是否在 Rust 恢复 scheduler：claim→建会话→prompt→终态结算）。管理面 `off-peak-task` 频道 + `startSync` 轮询可独立迁移（Rust service 已备，缺 server client/凭据/coding-plan 支持装配）。**G2.6 执行侧已落（2026-11-22）**：`crates/off-peak-runtime`（claim→建/恢复会话→sendPrompt→终态结算，含启动 recover_interrupted、stop 释放认领/退订、派发失败重试上限、ticket-expired 回队；7 条真实 SQLite 用例）+ `host-channels::off_peak_host`（经 mode-task 频道派发，init/resume、终态订阅先于 sendPrompt、failed 时从 task-index lastError 取错误文本）+ mode-task `sendPrompt` 透传 `offPeakTaskId`/`offPeakRunType` 与 `OffPeakCreate` denylist、`createTask(offPeakTaskId)` 用 deferred persistence + Tauri `MODE_RUST_OFF_PEAK=1` 装配（默认关闭；Node 无 scheduler，无双派发）。**产品边界复核（2026-11-22，重要）**：off-peak 属政策下线功能——`packages/shared/src/officialPlatformPolicy.ts` 的 `assertOfficialServiceRemoved("offPeak")` 在凭证读取与网络请求前短路（`offPeakRuntimeModel.resolveOffPeakCredentials` / `offPeakServerClient.request`），灰度配置 fail-closed，UI 入口隐藏。因此 **G2.6 不应迁移 HTTP/凭据、`off-peak-task` 管理频道与 mode-agent `offPeak/*` handler**；Node 侧 off-peak 服务随 16.6 与 Node Host 一起删除。**已回退（2026-11-22 裁决）**：`off-peak-runtime`、`off_peak_host`、Tauri `MODE_RUST_OFF_PEAK` 开关与 mode-task 透传改动全部撤销，避免为已下线功能保留可误触发的派发路径；文档保留政策结论。 |
|  |  | 🟡 **Node-only 频道处置盘点（2026-11-22，16.5 输入）**：按证据分三类。**①政策下线，删除不迁（4）**：`coding-plan-subscription`（`bigmodelCodingPlanSubscriptionProvider.ts:1275` `assertOfficialServiceRemoved("codingPlan")`）、`conversation-share`（`conversationShareService.ts:777/1330/1843` `assertConversationShareRemoved`）、`oauth`（`oauthService.ts` account 下线，读取恒 signed-out/空）、`off-peak-task`（`offPeakRuntimeModel.ts:140` offPeak 下线）；UI 入口均已 fail-closed，Node 服务随 16.6 删除。**②Node 内部 facade（1）**：`mode-session`。**已切 3 个纯 passthrough 到 `mode-agent`（2026-11-22）**：`readWorkspacePresentation`（workspace prepare 全链路）、`readSessionMessages`（conversation mirror 读侧）与 `closeSession`（草稿失效/远端插件同步），Rust `mode-agent` 均已实现；mirror 的 `createSession` 仍保留在 mode-session（Node 侧含 MCP 解析与 trace id，避免行为变化）。**readSession 已由 Rust `mode-session` 频道接管（2026-11-22）**：`host-channels::mode_session` 实现 readSession（apiRetry 快照暂存 + Claude 导入历史修复，spec `mode-session-channel.md`），进入 inprocess 默认集；renderer 已不依赖 Node mode-session，Node 服务随 16.6 删除。**③已迁（2）**：`onboarding-record`（2026-11-22：`host-channels::onboarding` + 装配 + inprocess 默认集，spec onboarding-record-service §2.1）与 `plugin-management`（2026-11-22：`host-channels::plugin_management` 薄别名转发 mode-agent，overview 补 officialMarketplaceEnabled，spec plugin-management-channel.md）；真机验收见 16.5 清单。**live，需迁移（3）**：`provider-settings`（已落）、`remote-desktop`（首切已落）、`remote-v3`（自托管 `MODE_REMOTE_V3_URL`，不依赖官方账号；~5.6k 行，最大块）；`remote` workspaces（16.4d）。**`bots` 经裁决移出本清单**（见下方 P1 裁决）。`lan-access` RPC 已由 Rust 接管（supervisor 待接）。**①补充（2026-11-22 复核）**：`cua-pip-session` 无任何 renderer/remote 消费者（仓库内只有 descriptor 与 node.ts 内部 publish，`packages/ui`/`web`/`desktop` 无引用），属 Node 内部 PiP 投影残留，**频道删除不迁**（内部 publish 不依赖频道）。`plugins` 也是退役通道——`pluginsService.ts` 的 getOverview 恒返回空/unsupported，六个写方法恒抛 "Legacy plugin management has been retired"；renderer 侧只有 `usePlugins`/`pluginStore` 引用，且没有任何组件使用 `usePlugins`（活路径已是 plugin-management），因此**删除不迁**，归入①的下线/退役组（共 6 个）。
**model-selection 已迁（2026-11-22）**：`host-channels::model_selection` 实现 getView/saveConfiguredDefault/onDidChange（生产口径 ordinary：生产 facade 未传 classifier）。数据源 provider-config-source 快照 + resolver；configuredDefault/变更通知/写回走 PersonalConfigRepository；View 投影用 `ProviderConfig::to_json`。桌面 inprocess 由壳注入 bundled 路径，进入默认集；无 bundled 路径（server-mode）回落 Node。spec `model-selection-channel.md` 记录边界（builtin 静态不订阅、account-*/legacy reasoning 显式不做）。
**非桌面 Host 能力（2026-11-23 复核）**：`provider-provisioning-target` 只在 `isDesktopAttachedRemote` 或 `providerProvisioningTargetEnabled` 时注册；全仓唯一开启点是 `packages/server/src/entry-http.ts`（`Boolean(authToken)`）。桌面本地 Host 不注册该频道，**不属于 LT-16 桌面 Node 删除范围**，Node 实现保留给 server/SSH remote Environment。
**orcarouter 已全量迁入（2026-11-23，阶段 1–5）**：`crates/orcarouter-service`（凭据/PKCE/目录/overlay/service 编排，24 条测试）+ `host-channels::orcarouter` 频道（12 方法，4 条测试）+ inprocess 默认集装配；spec `orcarouter-service.md`。真机验收待回填（手填 key/PKCE/目录降级/断开 fail closed）。
**provider-settings 进展（2026-11-23，阶段 0–4 全落）**：阶段 0–3——精确规则访问器、View 投影、10 个 provider/model 写入方法（15 条用例）、`mode-agent testModelConnectivity`、`provider-model-listing` reqwest 分页 executor（两页用例）。**阶段 4（2026-11-23）**：`host-channels::provider_settings_channel` 15 方法频道 + `onDidChange` + `assembly`/`inprocess` 默认集装配；为「校验失败不落盘」新增 `PersonalConfigRepository::try_update`；9 条频道契约用例 + 1 条仓库用例。剩真机走查。
**剩余 live 长任务顺序（2026-11-23 17:00 再修订）**：~~`orcarouter`~~、~~`provider-settings` 阶段 4~~、~~`remote-desktop` 首切~~ 均已落。**`bots` 走 P1（2026-11-23 采纳）**：永久留在 Node sidecar，等嵌入 JS 引擎统一承载——依据 `docs/specs/bots-migration-decision.md`（依赖第三方 npm SDK，属 §1 的「不可改写第三方负载」，重写等于推翻 2026-11-20 的「停止自实现四通道」）；16.6 删除窗口不含 bots。**`cua-permission` 与 16.4b 会话端口同批**（`docs/specs/cua-permission-migration.md`）。当前顺序：①`remote` workspaces（16.4d，`remoteWorkspaceService` 917 行，体量最小）→ ②`remote-v3`（~5.6k，16.4c，最大块）→ ③`remote-desktop` 会话面续段 + `cua-permission`（需 `RemoteDesktopPort` 生产实现）。
**结论**：删除 Node Host 前需迁的 live 面收敛为 3 个（remote workspaces / remote-v3 / remote-desktop 会话面）+ 1 个并入项（cua-permission）；**bots 移出删除前置**（P1：随 sidecar 存活，等嵌入引擎）；6 个下线/退役频道不投入迁移；onboarding-record / plugin-management / model-selection / mode-session / orcarouter / provider-settings 已迁。 |
| 16.6 | **物理删除窗口** | 删 host-standalone、host-sidecar 发运集/脚本、mode.cjs 回退随包、bundled-agents；四仪表终值复测 | 16.5 | `--fail-on-blockers` READY；体积/性能仪表落档 |

## 3. 执行原则

- 每个子任务先补该面 spec（真源文件:行级对照），再迁移；迁移完进
  `host_channels::assembly`（16.1 起逐个加入白名单与默认集）。
- 真机回归清单（`.plan/accept-w6/`）每子任务落地后追加对应验收项。
- mode.cjs 回退随包**最后删**（16.6）——它是 agent 启动的安全网。
