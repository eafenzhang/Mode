# 远程工作区对话记录镜像（Conversation Mirror）

状态：已实现。挂载点已随 v0.0.43 发版；2026-10-15 补修「冷会话读不出消息」导致镜像导入恒为 0 的缺口（见下节）。

## 实现状态（2026-10-15 核查与修复）

链路各段均已落地：协议 `packages/shared/src/mode-protocol/index.ts`（`source: z.literal("remoteMirror")`）、
共享逻辑 `packages/shared/src/conversation-mirror.ts`、UI 编排 `packages/ui/src/lib/conversationMirrorSync.ts`、
CLI 通用持久化 `apps/mode-cli/packages/bootstrap/src/mode-protocol/server-operations.ts` 的
`persistImportedSessionHistory`（`remoteMirror` 走通用分支，无专属分支）。

**曾经不生效的原因**：唯一触发组件 `packages/ui/src/WorkspaceConversationMirrorStatus.tsx` 的挂载点
被 `8242a576`（在 v0.0.36 基线上重建主线）摘掉——该提交重写 `packages/ui/src` 时删掉了
`WorkspaceSidebarItem.tsx` 里对它的 import 与渲染，组件沦为孤儿，于是
`useWorkspaceConversationMirror` → `runConversationMirrorSync` → `session/create` + `importedHistory`
整条链永不触发（功能代码在、调用点没了；typecheck / lint 不会报，因为孤儿组件是合法代码）。
2026-10-15 已在 `WorkspaceSidebarItem.tsx` 的 `CollapsibleContent` 内、`TaskList` 之后补回该挂载，
与重建前的旧树（`backup/main-before-rebuild`）一致。

**该挂载是整条链的唯一触发点**，且位于 `CollapsibleContent` 内——Radix 折叠时会卸载子树，所以自动同步
实际由「工作区行展开」门控。这条门控方向上与不变量 7（只处理用户正在使用的工作区）一致，而「激活工作区
必然展开」（`tabStore` 的 `activateTab → ensureWorkspaceExpanded`）保证了连上远端就会同步一轮，因此挂载位置
保持不动；代价是改动该处结构时必须重新确认这条依赖——孤儿组件是合法代码，typecheck / lint 都不会报。

**版本事实**：v0.0.39 / v0.0.40 含该挂载（重建前主线）；v0.0.41、v0.0.42（= `origin/main`）不含。
所以本修复必须重新发版才在客户端生效，只改工作区对已安装的 app 没有影响。

**测试**：UI 编排单测已补（`packages/ui/test/conversationMirrorSync.test.ts`，8 例）。
CLI 侧仍缺 `remoteMirror` 源的单测，见「测试」一节。

## 2026-10-15 冷读修复（镜像此前恒导入 0）

**症状**：链路全部就位、挂载点也在，但一轮同步导入 0 条，且 UI 与日志都看不出异常。实测（v0.0.43，双机互连）：本机 host 当天处理 239 次 `mode-session.readSessionMessages`——该 RPC 全仓只有镜像在调用——**0 次成功**，全部 `Session is not active`；CLI 侧对应 140 条 `mode_protocol.session.require_missing`（`activeSessionCount: 0`、`hasSessionStore: true`），`session/create` 为 0。

**根因**：`readMessages` 先调 `requireSession`，而它只查 `context.sessions`（活跃 runtime），冷会话直接抛错。数据其实一直都在 store 上——同一文件里的本地 helper `readSessionMessages` 本来就是 `sessionStore.messages(...)`。于是「对端会话没被 resume 过」时，①方向必然逐条读失败、`continue`，本轮 `importedCount: 0` 且仍返回 `ready`。

**修法**：`readMessages` 改为冷读容忍——命中活跃 runtime 走原路径，未命中则只读 session store（`sessionStore.getSession` + `sessionStore.messages`）再套既有投影。**不物化 runtime**，因此不变量 7 原样成立。

**两处必须如实记录的语义边界**：

- 冷读**不等价于**活跃读：事件存储是内存态（`create-app` 的 `createInMemorySessionEventStore`），冷会话没有 rewind 事件可读，所以冷读只保留「落盘 revert」那一层投影。镜像只需要 role + 文本 + 时间戳，可接受；但不要把两者当同一份结果。
- 冷读刻意用 `sessionStore.getSession` 而非 `getPersistedSession`：后者会顺带修复旧版远端工作区路径，属于写操作，被动读取不该触发对端数据变更。

**仍未补的一处（不阻断主链）**：`WorkspaceConversationMirrorSection.tsx`、`ConversationMirrorRecordDialog.tsx`
只存在于 `.plan/recovered/`（会话库恢复产物），源码树没有；本 spec 的设计不需要它们
（镜像会话本身进任务列表，无需记录列表视图）。

## 产品规则

### 范围

- 覆盖桌面窗口发起的**全部远端连接类型**（SSH / WSL / Docker / LAN）。
- 「对话记录」= 对端机器在**同一工作区路径**下、以**本机本地 scope**（`workspaceIdentity` 为空）产生的会话。
- **镜像产物是真会话，不是只读投影**：镜像记录以 `session/create` + `importedHistory` 导入成目标 scope 下的真实会话，
  与本地对话完全同质——可继续对话、可置顶 / 归档 / 改名 / 删除、进任务搜索与 `@` 提及。
- 「双向」= 一轮同步同时做两件事：① 对端记录 → **本机当前视图 scope**；② 本机记录 → **对端本地 scope**。
- 「每次连接时自动同步」= 远端 workspace attach 就绪（`rpcReady`）时自动跑一轮。触发点挂在工作区行内，
  Radix 折叠会卸载子树，所以「激活工作区必然展开」是这条自动同步的隐含前提（见「实现状态」）。

### 归属与执行位置（本 spec 的核心裁决）

| 方向                | 目标 scope（会话落在哪）                           | 之后续聊时 agent 跑在哪    |
| ------------------- | -------------------------------------------------- | -------------------------- |
| 对端记录 → 本机视图 | **对端机器**上、当前 attach 使用的 workspace scope | 对端机器（工作区所在机器） |
| 本机记录 → 对端视图 | **对端机器**的本地 scope（不带 identity）          | 对端机器（工作区所在机器） |

- 镜像会话一律落在**承载该工作区的机器**上，因为续聊要在有代码的那台机器执行；本机不为远端工作区另建本地副本。
- 由此继承两条例证：① 断开连接后镜像会话与远端工作区普通会话一样不可见（不是本地离线副本）；
  ② 对端机器打开该工作区时，会看到本机记录导入进来的会话（这就是「互相看到」）。
- **一个来源记录只会被导入到一个 scope**（按方向固定），因此确定性 id 不需要带 scope，重连换端口也不会产生重复。
- ②方向的**来源 scope 是本机在同一路径下的本地 scope**（`tab.workspacePath`，**不是** `localWorkspacePath`）。
  `localWorkspacePath` 是「本地 → 远端」的配置同步锚点（技能 / MCP / 插件），两侧通常不是同一个工程；
  拿它当会话来源会把 A 工程的对话灌进 B 工程的工作区，因此不采用。代价：双机路径不同的连接
  （如远端 `D:\gas-hub-xp` 对本地 `D:\ZCodium`）②方向恒为空，只有①方向生效。

### 状态所有者

| 状态                                     | 所有者                                                                                 |
| ---------------------------------------- | -------------------------------------------------------------------------------------- |
| 镜像会话内容                             | 承载工作区那台机器的 CLI session store（普通会话，无特殊所有权）                       |
| 任务列表状态（置顶/归档/删除/未读/标题） | 该机器的 tasks-index（镜像导入**不写** task meta，产品态由用户操作独占）               |
| 导入编排与同步状态                       | Renderer hook `useWorkspaceConversationMirror` + `conversationMirrorStore`（纯 UI 态） |
| 「是否已导入」判据                       | 目标 scope 自己的 task meta `updatedAt`（无需额外台账）                                |

### 不变量

1. **镜像只导入、不复制产品态**：导入只调用 `session/create`（含 `importedHistory`），不改 task meta。
   因此重同步不会覆盖用户已设置的置顶 / 归档 / 删除 tombstone / 标题覆盖（tasks-index 只补缺失行，负向 membership 不复活）。
2. **只镜像本机本地 scope 的记录**：带 `workspaceIdentity` 的行属于「以对端身份打开的工作区」，转发它等于转发对端内容。
3. **镜像副本不再被转发**：镜像会话 id 统一为 `mirror-<originSessionId>`（直接拼来源 id，不做哈希——
   Renderer 侧不能依赖 `node:crypto`，且日志与库里能直接看出镜像来源），读取对端本地 scope 时跳过该前缀。
   这同时挡住「本机副本被对端搬回来」的回声和第三台机器副本的扩散。
4. **幂等**：确定性 id + CLI 的 `removePreviousImportedSessionHistory`（按 `metadata.migrationSource` 清理旧导入消息）
   保证重复导入只覆盖导入消息、**保留用户后续在镜像会话里真实聊出的消息**。
5. **有界**：单轮最多 20 条会话；每条最多 200 条消息、单条文本 ≤ 4000 字符、单条记录 ≤ 256 KiB（超限丢最旧消息并置 `truncated`），
   为远端 RPC 物理帧（1 MiB）留余量。
6. **失败不阻断**：导入失败只记 warn + 视图状态，不阻断 workspace 连接、任务列表与会话主链路；
   对端 CLI 不认识 `remoteMirror` 时判为「对端版本过低」，本轮不再重试，UI 给出可读提示。
7. **不启动额外 runtime**：只处理用户正在使用的工作区；列举走任务列表（被动观察者），
   消息读取走既有 `session/messages`。冷会话由 CLI **只读 session store** 提供（不 resume、不物化 runtime，
   见「2026-10-15 冷读修复」），因此读取结果不再取决于「对端该会话是否活跃」。

### 事件顺序

```text
A 连接 B 的远端工作区 W（rpcReady + 工作区行已展开 → 挂载镜像状态组件）
  │
  ├─ ① 对端记录 → A 的视图 scope（会话落在 B 上）
  │   remote.modeTaskService.listTaskList({workspacePath: W})               ← 取 B 的本地 scope 行
  │     └ 过滤：带 identity 的行（对端身份）与 mirror- 前缀（副本）都跳过
  │   remote.modeTaskService.listTaskList({workspacePath: W, identity: R})  ← 目标 scope 现有 meta
  │     └ 仅当 origin.updatedAt > 目标 meta.updatedAt 才继续（未变则跳过）
  │   remote.modeSessionService.readSessionMessages({W, sessionId}) → 提取文本 + 截断
  │   remote.modeSessionService.createSession({
  │       sessionId: mirror-<originSessionId>, persistence: "immediate",
  │       workspace: {workspacePath: W, workspaceIdentity: R},
  │       importedHistory: {source: "remoteMirror", title, createdAt, updatedAt, originSessionId, messages}})
  │     └ CLI：upsert 会话 → 清理旧导入消息 → 写消息/part(migrationSource) → app.resume()（可续聊）
  │
  └─ ② 本机记录 → B 的本地 scope
      本机 base 服务读本地 scope 记录 → createSession（workspace 不带 identity）
      → B 打开该工作区时看到 A 的对话，同样可续聊 / 可操作
```

### 错误处理（fail-soft，全部可读文案）

| 场景                                     | 行为                                                                 |
| ---------------------------------------- | -------------------------------------------------------------------- |
| 对端 CLI 不认识 `remoteMirror`（旧版本） | 视图标 unsupported +「对端版本过低，无法同步对话记录」，本轮不再重试 |
| 单条记录读取 / 导入失败                  | 跳过该条 + warn，其余继续                                            |
| 有记录可读但**本轮全部读取失败**         | 视图标 degraded + 重试入口（对端 CLI 无冷读能力时即此状态），不得伪装成 ready |
| 本机在该路径下无本地记录                 | 跳过②方向（不导入空内容）                                            |
| 记录内容为空（全是工具调用 / 隐藏消息）  | 跳过该条（不导入空会话）                                             |
| 目标 meta 更新时间 ≥ 来源更新时间        | 跳过（无需台账即可幂等，重连不重复导入）                             |
| 超限                                     | 丢最旧消息 + `truncated`                                             |

## 验收场景

1. A 连接 B 的 `D:/proj`：A 的任务列表出现 B 的本地会话，且能**继续对话**（消息发到 B 执行）。
2. 上条会话可**置顶 / 归档 / 改名 / 删除**，删除后重连不复活（tasks-index tombstone）。
3. 该会话出现在**任务搜索**与 `@` 提及里（导入不写 migrationSource 到 task meta）。
4. B 打开 `D:/proj` 时能看到 A 的本地会话，同样可续聊 / 可操作。
5. 反复重连：不产生重复会话，也不丢失用户在镜像会话里续聊出的消息。
6. 对端为旧版本（CLI 不认 `remoteMirror`）→ 明确提示，连接与任务列表不受影响。
7. 断开远端连接后镜像会话不可见（属于对端机器），重新连接后仍在。

## 测试

- 单测（shared）：确定性 id 派生稳定、镜像前缀判定、本地 scope 过滤、消息提取（跳过隐藏 synthetic / 空文本）、
  有界化（会话数 / 消息数 / 文本长度 / 单记录字节预算 → `truncated`）、导入 payload 构造。
- 单测（ui）：`packages/ui/test/conversationMirrorSync.test.ts`（8 例）——夹具按双机拓扑搭，远端机的本地 scope
  同时是①方向的来源与②方向的目标，避免两边共用一份服务桩而掩盖方向接错。覆盖：两个方向各落一个 scope
  （视图带 identity / 反向不带）、目标不更旧不重导、mirror 前缀不被当新来源也不被再搬一次、对端不支持时整轮降级、
  空记录不导入、单条读取失败只跳过该条、本地工作区不做任何镜像。
- CLI 侧（`apps/mode-cli`）：**仍缺**。`session/create` 带 `importedHistory.source="remoteMirror"` 应能落盘为
  可续聊会话，且重导入保留后续真实消息。注意本 spec 早先引用的 `mode-protocol.test.ts` 在当前源码树里不存在
  （只在 `node_modules` 里有上游旧副本），补测前要先确认现有 bootstrap 测试骨架（`tsx --test test/*.test.mts`）能驱动
  `persistImportedSessionHistory`。
- E2E（交互改动必需）：远端工作区连接后镜像会话进入任务列表并可续聊。
- 验证：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`，目标包测试按实际入口执行并如实报告。

## 不做

- **不复制工具调用 / 附件字节**：镜像只有角色 + 文本 + 时间戳（工具调用与附件留在来源机器的工作区）。
- **不做实时帧推送**：只在连接就绪 / 激活 / 手动刷新时同步。
- **不为远端工作区建本地离线副本**：归属固定在工作区所在机器（见上表）。
- **不做冲突合并**：同一来源记录按 `updatedAt` 单向覆盖导入消息（用户续聊消息不受影响）。
- 不含手机远控（`web-remote-replayable`）专项；不含子 agent / 工作流子会话。
- 不做工作区内落盘交换目录（`.mode-share` 式）。
