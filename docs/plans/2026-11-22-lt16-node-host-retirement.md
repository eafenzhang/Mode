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
| 16.2 | **mode-task 编排面进壳** | 生命周期/命令队列/permission/elicitation/close/resume/snapshot 族：依赖 AgentControlClient（agent-server 已是 Rust）+ 事件桥（sendPrompt/deliverSessionMessage/readyOutcome） | 16.1 | 20 任务清单中任务系统场景全过 |
| 16.3 | **window-controller 进壳** | windowController 子系统（~1716 行）：窗口会话/Host sessions-index observer | 无强依赖 | 真机窗口操作一致 |
| 16.4 | **remote 面进壳** | remote-desktop（~1031 行独立子系统）/ remote-v3（v3 ControlLink）/ lan-access / remoteWorkspaceService（959 行） | 16.2 | 手机远控验收 |
| 16.5 | **host-standalone 收口** | 骨架（~4.4k 行）随上述面退役：stdioFrames/stdoutGuard/scopedAttachments/rendererConnectionScope 退场 | 16.1–16.4 | `w6:readiness` 四组 blocker 归零 |
| 16.6 | **物理删除窗口** | 删 host-standalone、host-sidecar 发运集/脚本、mode.cjs 回退随包、bundled-agents；四仪表终值复测 | 16.5 | `--fail-on-blockers` READY；体积/性能仪表落档 |

## 3. 执行原则

- 每个子任务先补该面 spec（真源文件:行级对照），再迁移；迁移完进
  `host_channels::assembly`（16.1 起逐个加入白名单与默认集）。
- 真机回归清单（`.plan/accept-w6/`）每子任务落地后追加对应验收项。
- mode.cjs 回退随包**最后删**（16.6）——它是 agent 启动的安全网。
