# Bot 切换工作区时显式解绑原工作区的任务对话

状态：已拍板（2026-10-11，用户确认"实现自动解绑"方向）。关联：[bot-state-ownership.md](./bot-state-ownership.md)（锁内原子 RMW）、[im-bot-remote-workspace-binding.md](./im-bot-remote-workspace-binding.md)（绑定语义）。

## 背景

`bindBotToWorkspace`（`packages/services/src/bots/botsService.ts`）切换工作区时只做三件事：
绑定表摘旧加新、`allowedWorkspaces` 收敛、**草稿**对话钉到新工作区
（`focusBotDraftConversationsOnWorkspace` 显式跳过 `activeTaskId` 非空的对话）。
`mode: task` 的对话不被触碰，原工作区的任务绑定因此残留：

- 实机状态（2026-10-11）：bot 绑定表已收敛到 `D:\ZCodium`，但 `bot-state.v3.json`
  里两条对话仍是 `mode: task`、各自 `activeTaskId` 指向远端工作区的旧任务。
- 残留绑定只有两条惰性出口，时机都不可预期：
  1. `BotsDialog` 绑定后紧接着 `bindBotToTask`（不带 conversationKey）会把**最近活跃**的
     那条对话换绑到新任务——旧绑定被覆盖，但前提是旧任务没在跑；
  2. 下一条入站消息进 `readContext` 时按绑定集做漂移纠正（改写为 draft、
     `activeTaskId: null`），此前旧绑定一直挂在 UI 上（绿点/解绑行）。
- 中间态还可能携带脏 identity：`focusBotToWorkspace` 写入时目标工作区没有
  `workspaceIdentity` 不清旧值，出现 `workspacePath` 与 `workspaceIdentity` 指向不同
  工作区的字段漂移（key 计算 identity 优先，订阅/镜像按 key 匹配会漏）。

## 产品规则

1. **切换即解绑**：`bindBotToWorkspace` 在绑定表写入与 `allowedWorkspaces` 收敛之后、
   `focusBotDraftConversationsOnWorkspace` 之前，遍历该 bot 全部对话：凡
   `mode === "task"` 且对话的 `getWorkspaceKey(workspacePath, workspaceIdentity)`
   **不等于目标 workspaceKey** 的，调用 `writeDraftContext` 回到草稿。
2. **解绑语义与 `unbindBotFromTask` 完全一致**：`activeTaskId: null`、清 pending
   interaction/排队消息/停放队列、撤该会话上该 bot 的流订阅（`writeContext` 的
   disposeTarget 机制），不停止旧工作区桌面上正在跑的任务——解绑只断 IM 镜像。
3. **不检查旧任务运行态**：与 `readContext` 漂移纠正（1064 行附近）口径一致；切换
   工作区是用户显式动作，运行中的旧任务保留在桌面侧继续跑，只是不再镜像到 IM。
4. **顺序**：先解绑、后 draft 聚焦。`focusBotDraftConversationsOnWorkspace` 随后把
   刚解绑的对话钉到新工作区，最终态是「草稿 + 新工作区」，与 `readContext` 纠正后的
   终态一致。
5. **目标工作区内的任务绑定不动**：key 等于目标 workspaceKey 的 `mode: task` 对话
   保持绑定（例如绑定前就在该工作区工作的会话）。
6. 解绑产生的状态写入走 `writeDraftContext` → `mutateState` 锁内原子 RMW，
   广播沿用既有 `bots:task` 投影刷新（BotsDialog 绑定后已强制刷新绑定投影）。

## 状态所有者

```text
bindBotToWorkspace
  │ 1. 绑定表（bot-bindings.v3.json）+ allowedWorkspaces（bot-config.v3.json）
  │ 2. 解绑：对话 workspaceKey ≠ 目标 → writeDraftContext（bot-state.v3.json 槽位）
  │ 3. focusBotDraftConversationsOnWorkspace：全部草稿 → 钉到目标工作区
  ▼
bot-state.v3.json（唯一事实，锁内 RMW）
```

- 唯一写入入口：`writeDraftContext`（复用，不新增写路径）。
- 副作用（撤流订阅、停 typing）由 `writeContext` 在锁外执行，符合 bot-state-ownership
  规则 3。

## 验收场景

1. 单测（`packages/services/test/bot-workspace-binding.test.ts`，沿用该文件的源码契约
   模式——`bindBotToWorkspace` 在 createBotsService 闭包内，行为级测试需整套服务图）：
   - `bindBotToWorkspace` 函数体内，`writeDraftContext` 调用出现在
     `focusBotDraftConversationsOnWorkspace` 之前（先解绑后聚焦的顺序）；
   - 解绑条件包含 `mode` 为 task 与 workspaceKey 不等的判断；
   - 该函数体内不存在 `isContextActiveTaskRunning` 守卫（规则 3：不检查运行态）。
2. 实机：远端工作区的 bot 绑定切到本地工作区后，旧工作区任务行绿点立即消失，
   `bot-state.v3.json` 中对应对话 `mode: draft`、`activeTaskId: null`、
   `workspacePath`/`workspaceIdentity` 指向新工作区。
3. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 通过。

## 非目标

- `unbindBotFromWorkspace`（从工作区摘除 bot）**不**自动解绑任务对话——本次范围是
  「切换」；摘除路径的残留对话仍走 `readContext` 惰性纠正。
- 不改 `focusBotToWorkspace` 的 identity 写入展开顺序（脏 identity 由本规则的解绑 +
  随后的 draft 聚焦覆盖修复，另行观察）。
- 不改 `readContext` 漂移纠正的既有语义；不新增广播事件类型。
