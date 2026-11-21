# LT-16：Node Host 退役专项（2026-11-22 立项）

> 上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §10.1、
> `docs/plans/2026-11-21-lt15-ecosystem-closeout.md` P6、`.plan/accept-w6/README.md`。
> 目标：Node Host 独占频道全部迁入 Rust 后，`w6:readiness --fail-on-blockers` 返回
> READY，开物理删除窗口（删 stdioFrames/stdoutGuard、host-standalone、
> host-sidecar 发运集、mode.cjs 回退随包）。

## 1. 现状（2026-11-21）

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
| 16.2 | **mode-task 编排面进壳** | 生命周期/命令队列/permission/elicitation/close/resume/snapshot 族 | 16.1 | 20 任务清单中任务系统场景全过 |

16.2 细拆（调查结论 2026-11-22）：编排真源 = `modeTaskServiceAdapter.ts`（5768 行，
任务↔agent 会话桥状态机）+ `modeTaskService.ts`（746 行）。**按依赖排序三片**：

- **16.2a 会话桥接状态机**：🟡 命令面已落（2026-11-22）——`mode_task.rs` 编排
  分发（async）+ 读面分发（spawn_blocking）双轨：createTask（ensure_provider_config
  → create_session → snapshotToMeta → sync_task_meta + grouped top →
  initialSlashCommands；automation/闲时 deferred persistence + 盖章）、closeTask
  （close_session + deleted patch）、resumeTask（resume_session → meta sync）、
  sendPrompt（sessionSend 直转发）；无控制面装配时显式 unsupported。真实 DB 测试
  186 全过。**剩余**：draftSessionId 复用（readSession→setModel/setThoughtLevel
  同步）与 v4Create 分支、readyOutcome 事件（16.2c）。
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
  readyOutcome 事件源（prompt 生命周期跟踪）、status/title/分组等其余 TS 发射点。
  **状态写面（2026-11-22）**：renderer 直接调用的 mode-task 写方法已落——
  renameTask（title+titleOverridden+updatedAt，task_title_changed）、deleteTask、
  deleteArchivedTask(s)（批量去重/单次广播）、setTaskPinned、setTaskUnread
  （expectedUnreadAt CAS：未命中不写不广播）、archiveTask/unarchiveTask、
  getTaskMeta；均按 TS reason 回发 workspace 事件。
  **会话只读/命令补充（2026-11-22）**：getTaskSessionFilePath（TS 同款路径投影、
  恒 exists=false）、stopGeneration（v4 Stop host envelope）、getTaskTokenUsage
  （v4/conversation/usage，ExistingOnly 不拉起 runtime）、
  releaseWorkspacePreparation/restartWorkspaceProcess（renderer 关闭 workspace/
  重载会话入口 → `dispose_workspace`，2s 宽限与 mode-agent 同口径）。**剩余**：rename 的
  v4 renameSession 同步（best-effort）与 updateIndexedTaskState 的「索引缺失时
  resume seed」兜底；会话设置面（initialize/releaseWorkspacePreparation/
  getTaskConfigOptions/getTaskModelSelection/setMode/setConfigOption/setModel/
  setAutomationSessionConfig）与 compactSession/goalSession/
  deliverSessionMessage/sendSessionMessageDeliveryResult/branchTaskFromPrompt
  等旧 facade 命令。

window-controller（16.3）依赖 16.2a+b（多源聚合消费 IModeTaskService 事件与
sessions-index observer）——**排序 16.2 先于 16.3**。
| 16.3 | **window-controller 进壳** | windowController 子系统（~1716 行）：窗口会话/Host sessions-index observer | 无强依赖 | 真机窗口操作一致 |
|  |  | 🟡 首切 + 帧订阅已落（2026-11-22）：`window_controller.rs` local 单源——listTaskList（repo total/has_more 直用）、mutateTask 8 种 mutation（pin/archive/delete/delete-archived[-batch]/mark-read[CAS]/mark-unread/open/resume → 索引 patch，统一返回 meta\|null）、deleteArchivedTask(s)；V4 帧（subscribeControllerV4/resyncControllerV4/unsubscribeControllerV4 + onDynamicControllerFrame）：tasks-index/workspaces 两 topic、独立 logEpoch+单调 seq、snapshot/delta（fromSeq 恒等上一帧 toSeq）、refresh 结构 diff、退订后静默；liveStatus 对齐 TS defaultLiveStatus（仅 completed/error，其余 idle）；assembly/inprocess/频道名守卫接线；7 用例。**剩余**：sessions-index overlay（activity/pendingInteraction/title 富化）、remote source 聚合与重连换源、事件驱动收敛（mode-task `onDynamicWorkspaceEvent` 进程内事件桥已接，createTask/closeTask 后立即推 delta；仍无 sessions-index observer/直接改库观察者）、registerSource 对外接线。 |
| 16.4 | **remote 面进壳** | remote-desktop（~1031 行独立子系统）/ remote-v3（v3 ControlLink）/ lan-access / remoteWorkspaceService（959 行） | 16.2 | 手机远控验收 |
|  |  | 🟡 调查拆分（2026-11-22）。TS 规模：lan-access 适配器 377 + LAN server 591 + 对端 registry/attach 892 ≈ 1.9k；remote-desktop 面依赖 `@mode/client` 主控 controller + `remoteDesktopServer` + 原生媒体（`crates/mode-cua-ax/rd_media` 已存在）；remoteV3 客户端 ~5.6k（`packages/desktop/src/host/remoteV3/`）；remoteWorkspace 959 + services/remote 接口 109。Rust 既有底座：`lan-pairing`（clients/token/配对码/远程桌面授权）、`lan-http`（pair + 六管理端点 + serve）、`lan-discovery`、`crates/server/src/lan.rs`（axum 桥）、`ws_host`（每连接 ChannelServer）、`remote-desktop-streaming`（自适应码率纯状态机）。<br>按依赖拆四片：**16.4a lan-access 管理面**（本机 server supervisor：loopback/LAN 双模式、/ws host 复用、静态资源/中继；getState/setEnabled/createPairCode/removeClient/resetTokens/onDidChange + 对端 side：discoverPeers/pairPeer/listPairedPeers/getPeerWorkspaces/remove/rename）→ **16.4b remote-desktop 频道**（设置/同意/被控状态 + 主控会话 open/probe/signal，复用原生媒体）→ **16.4c remote-v3 频道**（设备身份/登录/ControlLink/tunnels/E2EE，最大块）→ **16.4d remote workspaces**（connect/cancel/bind/dispose/attach/detach + remoteWorkspaceService）。<br>**状态**：16.4a 首切已落（2026-11-22）——`lan-access` 频道对端配对 registry
（listPairedPeers v1→v2 归一化/公网 HTTPS 过滤/pairedAt 排序、removePairedPeer
幂等、renamePairedPeer 1–64 展示名）；server supervisor 与网络 side 显式报错，
channel 已进 assembly/inprocess 默认集；6 用例。**16.4a 剩余**：本机 server
supervisor（loopback/LAN 双模式、/ws host 复用、静态资源/中继）、getState/
setEnabled/配对码/clients/reset、discoverPeers/pairPeer/getPeerWorkspaces、
onDidChange。`w6:readiness` 四组 blocker（stdioFrames/stdoutGuard/host-sidecar/
node-agent-fallback）均为 Node Host 发运物，随 16.6 删除归零。 |
| 16.5 | **host-standalone 收口** | 骨架（~4.4k 行）随上述面退役：stdioFrames/stdoutGuard/scopedAttachments/rendererConnectionScope 退场 | 16.1–16.4 | `w6:readiness` 四组 blocker 归零 |
| 16.6 | **物理删除窗口** | 删 host-standalone、host-sidecar 发运集/脚本、mode.cjs 回退随包、bundled-agents；四仪表终值复测 | 16.5 | `--fail-on-blockers` READY；体积/性能仪表落档 |

## 3. 执行原则

- 每个子任务先补该面 spec（真源文件:行级对照），再迁移；迁移完进
  `host_channels::assembly`（16.1 起逐个加入白名单与默认集）。
- 真机回归清单（`.plan/accept-w6/`）每子任务落地后追加对应验收项。
- mode.cjs 回退随包**最后删**（16.6）——它是 agent 启动的安全网。
