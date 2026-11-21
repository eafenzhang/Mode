# Spec：Tauri Host 的 window-controller 服务端（窗口任务聚合权威）

状态：2026-10-24 修复回归并验证（typecheck / lint / 新增单测通过）。
关联：[tauri-renderer-host.md](./tauri-renderer-host.md)（Host sidecar 契约与派发守卫）、
[remote-workspace-identity.md](./remote-workspace-identity.md)（workspace 身份键）、
[remote-workspace-conversation-mirror.md](./remote-workspace-conversation-mirror.md)（镜像不在本服务边界内）。

## 背景与回归成因

Renderer 的侧栏任务视图自 v4 起分两条数据链：

1. **tasks-index 行**（本地/远端 `modeTaskService.listTasks/listPinnedTasks/listArchivedTasks`）：
   项目视图 workspace 行、grouped 视图行、归档视图的持久行来源。
2. **sessions-index / Controller 投影**（`IWindowControllerService.listTaskList` + V4 订阅帧）：
   「任务」分区（timeline）、置顶区、时间线视图、命令中心最近任务，以及 grouped/timeline 的
   实时 phase/preview/pendingInteraction enrich。

旧 Electron Host 在 `packages/desktop/src/host/windowHostControllerService.ts`
（+ Projection + SessionsIndexObserver）实现了第 2 条链的 Host 端。
2026-10-20 提交 `d71ed992`（移除旧 Electron 线收口到 Tauri）把这三个文件随旧线一起删除，
但 Tauri host-sidecar（`packages/desktop/src/host-standalone`）**没有补上等价实现**；
renderer 消费方全部保留。结果：`windowControllerService` 是指向未注册频道的代理，
`listTaskList` 每次都以 `Unknown channel: window-controller` 失败（`packages/rpc/src/channelServer.ts`
对未注册频道 reject），`useGlobalTaskList` 捕获后把列表置空——

- 「任务」分区永远显示「还没有任务」（对话工作区的会话明明存在）；
- 置顶区、时间线视图、命令中心最近任务同样为空；
- grouped 视图仍可见（tasks-index 行是左表），形成「历史不见了、切到分组才能找到」。

本 spec 规定恢复后的服务端边界。实现位置：`packages/desktop/src/host-standalone/windowController/`
（`windowHostControllerService.ts` / `windowHostControllerProjection.ts` /
`windowHostSessionsIndexObserver.ts`，自旧实现移植，逻辑不重设计）。

## 目标与非目标

**做**：

- Tauri host-sidecar 注册 `window-controller` 频道，实现 `IWindowControllerService` 全部契约方法：
  `listTaskList` / `mutateTask` / `deleteArchivedTask` / `deleteArchivedTasks` /
  `subscribeControllerV4` / `resyncControllerV4` / `unsubscribeControllerV4` /
  `onDynamicControllerFrame`。
- source 路由：按查询 scope（`workspaceIdentity?.trim() || workspacePath`）把 local /
  remote 会话的 tasks-index 三分区 + sessions-index 观察聚合成一份内存投影。
- 生命周期：远程连接断开 → `disconnectSource`（冻结最后可信快照为 offline）；
  session 释放 → `removeSource`；`bindContext` 换绑 → 摘除旧 scope。
- 注册进本地 `services` 集合：窗口 renderer（MessagePort 服务面）与手机 Web 远控
  （`/ws/host`，`packages/server` `setupChannelServer`）自动获得同一频道。

**不做（登记，不在本次范围）**：

- 不做旧 Electron 线的 per-attachment controller 实例（`createAttachmentService` 的多实例接线）：
  共享单实例的 `onDynamicControllerFrame` 会把所有订阅的帧广播给每个连接，客户端按 address
  过滤（`queryContainsWorkspace`），跨连接串帧只产生额外过滤开销，不产生错误数据。
- 不实现重连时的 `replaceDisconnectedSource` 原子换源接线（runtime 保留该 API；
  当前 `bindContext` 只摘旧 scope，重连后的短暂空档由下一次 refresh 收敛）。
- 不改变 tasks-index 与 sessions-index 的数据所有权；不在 Controller 内做 mirror/去重。

## 状态与所有权

| 状态                       | 唯一所有者                                | 说明                                                            |
| -------------------------- | ----------------------------------------- | --------------------------------------------------------------- |
| source 注册表/快照/overlay | Controller 投影（内存）                   | 每次 refresh 从 source 的 tasks-index 三分区重建；断连冻结      |
| sessions-index live facts  | `WindowHostSessionsIndexObserver`（内存） | 只用 `existing-only` 订阅；runtime 缺席时 dormant，不拉起 Agent |
| 远程会话拓扑               | `windowRemoteConnectionRegistry`          | Controller 通过 `resolveControllerRemoteSource` 读，不持有会话  |
| 任务行持久事实             | 各 source 的 tasks-index                  | Controller 只是聚合投影与写路由，不落盘                         |

## 接口与路由规则

- `resolveSource(scope)`（装配在 `serviceAccessor.ts`）：
  1. `remote.resolveControllerRemoteSource(scope)`（进程内方法，不进 RPC 面）——命中已绑定
     workspace 的远程会话时：online → 该会话 scoped services 的 `IModeTaskService`（必填）与
     `IModeAgentService`（可选）；offline → 冻结占位。
  2. 远程身份（`isRemoteWorkspaceIdentity`）**绝不回落本地 tasks-index**——同路径不同 authority
     会串读（沿用旧实现与 Workspace Identity 纪律）。
  3. 其余 scope 回落本地 `services` 的 `IModeTaskService` / `IModeAgentService`。
- 写路由（`mutateTask` 等）：`resolveTaskAddress` 先物化 source 并要求投影行唯一匹配，
  fail-closed；远端 offline 时拒绝列表写操作。
- 事件驱动刷新：source 的 `onDynamicWorkspaceEvent` 触发该 source 的 single-flight refresh；
  多事件合并为下一轮（事件在读取途中到达时等待在途结束）。

## 失败语义

| 场景                      | 行为                                                                          |
| ------------------------- | ----------------------------------------------------------------------------- |
| 频道未注册（本修复前）    | `Unknown channel: window-controller`，`useGlobalTaskList` 置空并记 error      |
| 某 remote source 读取失败 | 只影响该 source；其他 source 的可信投影保留（`onSourceError` 记日志）         |
| 远程 source 离线          | 投影冻结为 offline 快照，列表行带 `sourceAvailability: "offline"`；写操作拒绝 |
| source refresh 代际失效   | 丢弃本次快照，不写投影（避免旧代覆盖新代）                                    |

## Rust 侧实现（LT-16.3 首切 + 帧订阅）

契约方法由 `crates/host-channels/src/window_controller.rs` 的
`WindowControllerChannel` 承载（`window-controller` 频道），先落在 **local 单源**：

- **列表/变更**：`listTaskList` 直查 `task-index-repo`（`total`/`has_more` 用 repo 的
  全量计数，不按截断结果重算）；`mutateTask` 的 8 种 mutation 映射到索引 patch，
  统一返回 meta 或 `null`（与 TS `mutateTask` 同语义）；`deleteArchivedTask(s)` 单条/批量。
  remote address 在壳内 local 单源不可达，写操作按 TS 文案拒绝。
- **controller 帧**：`subscribeControllerV4` / `resyncControllerV4` /
  `unsubscribeControllerV4` 三个 RPC + `onDynamicControllerFrame` 事件。
  - 两个 topic：`controller/tasks-index`、`controller/workspaces`；各自独立
    `logEpoch`（uuid v4）与单调 `seq`。
  - 订阅 ack `{subscriptionId, mode: "snapshot", logEpoch}`；订阅/重同步后向事件
    监听者推送 snapshot 帧（`fromSeq = 0`，`toSeq = 当前 seq`，`snapshot.logEpoch`
    与帧一致）。
  - 每次 refresh 与上次发布的快照做结构 diff：`task.removed` / `task.upserted`、
    `workspace.removed` / `workspace.upserted`；有 delta 才 `seq += 1` 并广播，
    帧 `fromSeq` 恒等于上一帧 `toSeq`（Renderer 的 gap 判定依赖此连续性）。
  - **refresh 触发点**：本频道每次读写调用前后（含 mutation 与归档删除）；壳内暂无
    sessions-index observer/DB 变更观察者，其他频道写入索引后的收敛依赖下一次
    本频道调用——这是与 TS「事件驱动 single-flight refresh」的已知差距。
  - 快照行 = 索引全量未删除任务三分区合并（active/pinned/archived，archived 行
    membership 固定 `pinned:false, archived:true`，与 TS `readSourceTaskIndex` 同）；
    workspace facts 由索引中出现的 workspace 去重派生（`online/online`），空 workspace
    不会出现（TS 由 source 注册表驱动，壳内当前无注册面）。
  - `liveStatus` 在无 sessions-index overlay 时只投影 `completed`/`error`，其余
    一律 `idle`：SQLite 里的 running/waiting 可能是上次进程残留，不能重新解释成
    当前运行事实（TS `defaultLiveStatus` 同规则）。

**未覆盖（16.3 续 / 16.4）**：sessions-index overlay（liveStatus/activity/
pendingInteraction/title 富化）、remote source 聚合与重连换源、事件驱动的增量收敛
（当前为调用时 refresh）、`registerSource`/`replaceDisconnectedSource` 的对外接线。

## 验收场景

1. 打开桌面端，侧栏「任务」分区显示对话工作区的活跃会话（不再是「还没有任务」）。
2. 对任一任务 pin / 归档 / 标未读后，「任务」分区与 workspace 行在同一轮事件内收敛。
3. 远程 workspace 连接断开时，其任务行变为 offline 展示而非消失；重新连接后恢复 online。
4. 手机 Web 远控（`/ws/host`）的任务列表与桌面一致（同一 Controller 投影）。
5. `pnpm typecheck` / `pnpm lint` 通过；`window-host-controller.test.mts` 覆盖
   投影 delta、membership 过滤、scope 路由与离线冻结。
