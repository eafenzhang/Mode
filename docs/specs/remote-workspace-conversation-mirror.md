# 远程工作区对话记录镜像（Conversation Mirror）

状态：已实现。挂载点已随 v0.0.43 发版；2026-10-15 补修「冷会话读不出消息」导致镜像导入恒为 0 的缺口（见下节）；
同日第二轮修复「`session/messages` 返回值不是 wire 形状导致读取必失败 / ②方向来源 scope 取错 /
只在挂载时跑一轮 / 导入时间线被压平」四处（见「2026-10-15 第二轮修复」）；
同日第三轮按产品口径把单轮窗口提到**至少 10 条**，并把导入会话的**活跃 record 时间线回填**，
让镜像在侧栏按来源的真实时间轴排列（见「2026-10-15 第三轮调整」）。

## 2026-10-15 第三轮调整（一轮 10 条 + 按真实时间轴排序）

两条产品口径：

**① 一轮至少同步 10 条**。窗口基准原本直接取侧栏可见条数，而它默认只有 5
（`WORKSPACE_TASK_PAGE_SIZE`），于是刚连上的远端工作区只搬过来 5 条对话，剩下 5 条要等用户点一次
「展开更多」。现在窗口口径收敛到 shared 的唯一函数 `resolveConversationMirrorWindowSize`：
**下限 10 条**（`CONVERSATION_MIRROR_LIMITS.minWindowSize`）、仍随可见条数增长、上限 20 条
（`maxRecords` 不变，远端 RPC 参数有界）。「展开更多」依旧把更旧的记录带进来（验收场景 9 不变）。

**② 同步过来的对话按来源的真实时间轴排序**。会话列表的 `updatedAt` 取自 sessions-index 的
`lastActivityAt`，而它的唯一事实源是活跃 record 的 `record.updatedAt`（v4-bridge `getSessionIndexMeta`），
再经 `modeTaskIndexSyncer` 落到 tasks-index 的 `updatedAt`、最后由
`compareModeTaskListItems(..., "updated")` 决定侧栏顺序。问题出在
`materializeSessionRecord` 把 `createdAt/updatedAt` 写死成 `Date.now()`：导入会话不回填，
`lastActivityAt` 就是**导入那一刻**，同一轮导入的 10 条于是按**导入顺序**（`selectMirrorSourceTasks`
是 updatedAt 降序，即从新到旧）排列，落到侧栏正好**新旧颠倒**；重启后会话变冷、改读
`session.time.updated`（真实时间）又会翻回来，同一批会话在两端 / 两次启动间顺序不一致。
修法：`persistImportedSessionHistory` 落盘后调 `applyImportedHistoryTimelineToRecord`，把
`createdAt/updatedAt` 回填成 `resolveImportedHistoryTimestamps` 的结果——与 `resumeSession`
对冷会话的回填同一裁决（那边早已这么做，注释见 `server-operations.ts`）。此后用户在镜像会话里
续聊，正常活动事件照旧把 `record.updatedAt` 推到现在，语义不变。

**仍未实机验证**：需要两端都升级才能观察到 10 条窗口与镜像的真实时间轴排序。

## 2026-10-15 第二轮修复（同步失败 / 两端不一致 / 单边可见 / 无分页）

线上反馈四条互相关联的症状：①本机同步远端对话记录**失败**；②本机在远端工作区发起的对话，对端看不到；
③对端发起的对话，本机看不到（或只在连接那一刻看得到）；④同步完两端不一致。日志（`~/.mode/v2/logs`）
与源码定位出四个独立成因：

**成因 0（`session/messages` 返回值不是 wire 形状 → 症状①，本轮的主因）**：
`readMessages` 把 session store 的内部行直接当响应返回——store 行是 `info.id` / `part.messageID`，
而 wire 合同 `modeSessionMessagesResultSchema` 要的是 `info.messageId`。调用方
（`modeAgentService.readSessionMessages` 的 `resolveResponse`）逐条校验，于是**只要会话里有消息就必定
失败**（只有空会话碰巧通过）：

```text
mode-session.readSessionMessages FAIL
{"name":"ZodError","message":"[... {\"code\":\"invalid_type\",
  \"path\":[\"messages\",0,\"info\",\"messageId\"],
  \"message\":\"Invalid input: expected string, received undefined\"} ...]"}
```

后果正是「本机同步远端对话记录失败」：镜像逐条读失败 → 本轮 0 导入 → 状态降级。
这同时说明 2026-10-15 的冷读修复只解决了「读不到冷会话」，没解决「读到了也发不出去」——两处缺口叠加，
单独修任何一处都仍然是 0 导入。
修法：`readMessages` 在切片之后统一过 `mapMessageWithParts`（快照路径一直在用这同一个 mapper，
`session-mapper.ts`），活跃路径与冷读路径同时修正。
这条 RPC 全仓只有镜像在调用，因此影响的正是本功能。

**成因 A（②方向来源 scope 取错 → 症状②）**：②方向原本只读「本机在同路径下的本地 scope」
（`tab.workspacePath`）。双机路径不同（远端 `D:\gas-hub-xp` 对本地 `D:\ZCodium`）时该 scope 恒为空，
于是 **本机连上远端工作区后在其中发起的对话永远到不了对端**——那些会话落在「以对端身份打开的那个
scope」（`{workspacePath, workspaceIdentity: 本机 attach 身份}`）里，原本被 `isMirrorEligibleTask`
（要求无 identity）直接过滤掉了。
修法：②方向的来源改为**本机在该工作区的全部记录**= 本机本地 scope（无 identity）∪ 本机视图 scope
（带 identity），两者合并去重；`mirror-` 前缀仍跳过，防回声与第三方扩散不变。
①方向不变：来源仍必须是对端本地 scope（带 identity 的行是对端的内容，转发即泄漏，见不变量 2）。

**成因 B（只在挂载时跑一轮 → 症状②③）**：`useWorkspaceConversationMirror` 只在
`workspaceKey + 连接代次 + 窗口`变化时跑一轮，而 `WorkspaceConversationMirrorStatus` 在 `ready` 下
渲染 null（连手动刷新入口都没有）。结果是连接之后任一端新开 / 续聊的对话都同步不过去。
修法：订阅两个来源 scope 的 `workspace_task_list_changed`（对端本地 scope 是①来源、本机视图 scope
是②来源），防抖 1.2s 后补跑一轮；相邻两轮另有 20s 间隔下限——镜像自身的写入也会发事件，而
「水位是否前进」取决于对端 CLI（见成因 C），间隔下限保证对端为旧版本时最坏情况是低频空转而不是持续重导。

**成因 C（导入时间线被压平 → 症状①④）**：CLI 的 `persistImportedSessionHistory` 里
`updatedAt` 只对 `claudeCode` 透传，其余来源（含 `remoteMirror`）一律退化成 `createdAt`。
而目标 scope 的 task meta `updatedAt` 正是「已同步到哪一版」的水位，于是：镜像会话在两端的
创建 / 更新时间与排序不一致（两端不一致）；且水位永远停在来源会话的创建时刻，**每轮同步都判定
「来源更新」而重导一遍**——配合成因 B 的自动重跑会变成持续重导的活锁。
修法：抽出 `resolveImportedHistoryTimestamps`，`claudeCode` 与 `remoteMirror` 同等对待
（透传 `updatedAt`，缺失时用末条消息时间兜底）。这是成因 B 能安全上线的前提。

**成因 D（无分页 → 症状「同步最近 5 条 / 展开更多再加 5 条」）**：单轮固定按
`CONVERSATION_MIRROR_LIMITS.maxRecords = 20` 灌满。现改为按调用方传入的 **`windowSize`** 处理，
窗口取侧栏该工作区的可见条数（`WORKSPACE_TASK_PAGE_SIZE = 5` 起，`expand more` 每次 +5）：
窗口变大时 `useWorkspaceConversationMirror` 的 runKey 变化，自动按新窗口重跑一轮。

**仍未实机验证**：本改动需要两端都升级才能观察到 `mirror-` 会话双向落地；单测覆盖的是编排与投影。

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

**测试**：UI 编排单测已补（`packages/ui/test/conversationMirrorSync.test.ts`，11 例，见「测试」一节）。
CLI 侧有 `resolveImportedHistoryTimestamps` 的水位单测；`session/create` 端到端落盘用例仍缺。

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
- 「每次连接时自动同步」= 远端 workspace attach 就绪（`rpcReady`）时自动跑一轮；之后**任一来来源 scope
  发生变更（新开 / 续聊任务）时防抖补跑一轮**，因此连接之后新产生的对话也会传过去。
- **窗口有界且随可见列表增长**：一轮只处理每个来源最近 `windowSize` 条会话。`windowSize` 以侧栏该工作区
  当前可见的会话条数（默认 5，`展开更多` 每次 +5）为基准，经 `resolveConversationMirrorWindowSize`
  归一为**至少 10 条、至多 20 条**：刚连上就同步最近 10 条，窗口变大时按新窗口重跑，
  不会一次性把远端历史灌满。
- **同步过来的对话按来源的真实时间轴排序**：导入会话的 `createdAt/updatedAt` 取来源会话的真实时间线
  （不是导入时刻），因此侧栏顺序与来源机器一致，也与重启后的冷启动顺序一致（见「第三轮调整」②）。

### 归属与执行位置（本 spec 的核心裁决）

| 方向                | 目标 scope（会话落在哪）                           | 之后续聊时 agent 跑在哪    |
| ------------------- | -------------------------------------------------- | -------------------------- |
| 对端记录 → 本机视图 | **对端机器**上、当前 attach 使用的 workspace scope | 对端机器（工作区所在机器） |
| 本机记录 → 对端视图 | **对端机器**的本地 scope（不带 identity）          | 对端机器（工作区所在机器） |

- 镜像会话一律落在**承载该工作区的机器**上，因为续聊要在有代码的那台机器执行；本机不为远端工作区另建本地副本。
- 由此继承两条例证：① 断开连接后镜像会话与远端工作区普通会话一样不可见（不是本地离线副本）；
  ② 对端机器打开该工作区时，会看到本机记录导入进来的会话（这就是「互相看到」）。
- **一个来源记录只会被导入到一个 scope**（按方向固定），因此确定性 id 不需要带 scope，重连换端口也不会产生重复。
- ②方向的**来源**是「本机在该工作区的记录」，由两个 scope 合并：本机本地 scope（无 identity，同路径本地工程）
  与**本机视图 scope**（`{workspacePath, workspaceIdentity: 本机 attach 身份}`）。后者是本机连上远端工作区后
  在其中发起的对话——只取本地 scope 会让②方向在双机路径不同时恒为空，本机发起的远端对话就到不了对端
  （见「2026-10-15 第二轮修复」成因 A）。
  明确**不采用** `localWorkspacePath`：它是「本地 → 远端」的配置同步锚点（技能 / MCP / 插件），
  两侧通常不是同一个工程，拿它当会话来源会把 A 工程的对话灌进 B 工程的工作区。

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
2. **按下方向过滤带 `workspaceIdentity` 的行**：①方向的来源只能是对端机器的本地 scope，
   带 identity 的行属于「以对端身份打开的工作区」，转发它等于转发对端内容；
   ②方向的来源反过来**必须包含本机视图 scope 的 identity 行**（那正是本机在远端工作区发起的对话）。
   两个方向的过滤条件不同，不能共用一个谓词（见「2026-10-15 第二轮修复」成因 A）。
3. **镜像副本不再被转发**：镜像会话 id 统一为 `mirror-<originSessionId>`（直接拼来源 id，不做哈希——
   Renderer 侧不能依赖 `node:crypto`，且日志与库里能直接看出镜像来源），读取对端本地 scope 时跳过该前缀。
   这同时挡住「本机副本被对端搬回来」的回声和第三台机器副本的扩散。
4. **幂等**：确定性 id + CLI 的 `removePreviousImportedSessionHistory`（按 `metadata.migrationSource` 清理旧导入消息）
   保证重复导入只覆盖导入消息、**保留用户后续在镜像会话里真实聊出的消息**。
5. **有界**：单轮每个方向处理 `windowSize` 条会话，`windowSize` = 侧栏可见条数取下限 10、上限 20
   （见「2026-10-15 第三轮调整」①，成因 D 定的是窗口可随可见列表增长这件事）；
   每条最多 200 条消息、单条文本 ≤ 4000 字符、单条记录 ≤ 256 KiB（超限丢最旧消息并置 `truncated`），
   为远端 RPC 物理帧（1 MiB）留余量。
6. **失败不阻断**：导入失败只记 warn + 视图状态，不阻断 workspace 连接、任务列表与会话主链路；
   对端 CLI 不认识 `remoteMirror` 时判为「对端版本过低」，本轮不再重试，UI 给出可读提示。
7. **不启动额外 runtime**：只处理用户正在使用的工作区；列举走任务列表（被动观察者），
   消息读取走既有 `session/messages`。冷会话由 CLI **只读 session store** 提供（不 resume、不物化 runtime，
   见「2026-10-15 冷读修复」），因此读取结果不再取决于「对端该会话是否活跃」。

### 事件顺序

```text
A 连接 B 的远端工作区 W（rpcReady + 工作区行已展开 → 挂载镜像状态组件）
  │   触发：连接就绪 / 窗口（侧栏可见条数）变大 / 两个来源 scope 的
  │        workspace_task_list_changed（防抖 1.2s）/ 手动重试
  │
  ├─ ① 对端记录 → A 的视图 scope（会话落在 B 上）
  │   remote.modeTaskService.listTaskList({workspacePath: W})               ← 取 B 的本地 scope 行
  │     └ 过滤：带 identity 的行（对端身份）与 mirror- 前缀（副本）都跳过；取最新 windowSize 条
  │   remote.modeTaskService.listTaskList({workspacePath: W, identity: R})  ← 目标 scope 现有 meta
  │     └ 仅当 origin.updatedAt > 目标 meta.updatedAt 才继续（未变则跳过）
  │   remote.modeSessionService.readSessionMessages({W, sessionId}) → 提取文本 + 截断
  │   remote.modeSessionService.createSession({
  │       sessionId: mirror-<originSessionId>, persistence: "immediate",
  │       workspace: {workspacePath: W, workspaceIdentity: R},
  │       importedHistory: {source: "remoteMirror", title, createdAt, updatedAt, originSessionId, messages}})
  │     └ CLI：upsert 会话 → 清理旧导入消息 → 写消息/part(migrationSource) → app.resume()（可续聊）
  │
  └─ ② 本机记录 → B 的本地 scope（会话落在 B 上）
      来源 = remote.listTaskList({W, identity: R}) ← 合并
             base.listTaskList({W})               （A 同路径本地工程）
        └ 过滤：只跳过 mirror- 前缀（identity 行是本机自己发起的对话，必须保留）；取最新 windowSize 条
      消息回各自所在的机器读（视图 scope 的行在 B 上，本地 scope 的行在 A 上）
      remote.modeSessionService.createSession({sessionId: mirror-<originId>,
        workspace: {workspacePath: W},       ← 不带 identity = B 的本地 scope
        importedHistory: {source: "remoteMirror", ...}})
      → B 打开该工作区时看到 A 的对话，同样可续聊 / 可操作
```

### 错误处理（fail-soft，全部可读文案）

| 场景                                     | 行为                                                                 |
| ---------------------------------------- | -------------------------------------------------------------------- |
| 对端 CLI 不认识 `remoteMirror`（旧版本） | 视图标 unsupported +「对端版本过低，无法同步对话记录」，本轮不再重试 |
| 单条记录读取 / 导入失败                  | 跳过该条 + warn，其余继续                                            |
| 有记录可读但**本轮全部读取失败**         | 视图标 degraded + 重试入口（对端 CLI 无冷读能力时即此状态），不得伪装成 ready |
| 本机在该路径下无本地记录                 | ②方向只跳过这部分来源（另一个来源 scope 照常处理），不导入空内容      |
| 记录内容为空（全是工具调用 / 隐藏消息）  | 跳过该条（不导入空会话）                                             |
| 目标 meta 更新时间 ≥ 来源更新时间        | 跳过（无需台账即可幂等，重连不重复导入）                             |
| 超限                                     | 丢最旧消息 + `truncated`                                             |

## 验收场景

1. A 连接 B 的 `D:/proj`：A 的任务列表出现 B 的本地会话，且能**继续对话**（消息发到 B 执行）。
2. 上条会话可**置顶 / 归档 / 改名 / 删除**，删除后重连不复活（tasks-index tombstone）。
3. 该会话出现在**任务搜索**与 `@` 提及里（导入不写 migrationSource 到 task meta）。
4. **A 在远端工作区 `W` 发起的对话，B 打开 `W` 时也看得到**（②方向来源含本机视图 scope），
   同样可续聊 / 可操作。
5. 反复重连：不产生重复会话，也不丢失用户在镜像会话里续聊出的消息。
6. 对端为旧版本（CLI 不认 `remoteMirror`）→ 明确提示，连接与任务列表不受影响。
7. 断开远端连接后镜像会话不可见（属于对端机器），重新连接后仍在。
8. **连接之后**任一端新开 / 续聊的对话，另一端无需重连即可看到（工作区事件触发的防抖重同步）。
9. **窗口**：一轮至少同步每端最近 10 条（侧栏默认只可见 5 条也一样）；侧栏「展开更多」后窗口 +5，
   更旧的记录随之被同步，上限 20 条。
10. **排序**：镜像会话在侧栏按来源的**真实时间轴**排列（不是导入顺序），且与重启后冷启动的顺序一致；
    两端同一来源会话的创建 / 更新时间一致（CLI 透传 `updatedAt` 并回填活跃 record）。

## 测试

- 单测（shared）：`packages/shared/test/conversation-mirror.test.ts`——确定性 id 派生稳定、镜像前缀判定、
  ①方向过滤 identity 行 / ②方向保留 identity 行（两个谓词不同）、窗口取最新 N 条、
  窗口口径归一（可见 5 条 → 10 条；随可见条数增长并以 20 条封顶；未传 / NaN / 0 取下限）、
  多来源按 taskId 合并去重、消息提取（跳过隐藏 synthetic / 空文本）、
  有界化（会话数 / 消息数 / 文本长度 / 单记录字节预算 → `truncated`）、导入 payload 构造。
- 单测（ui）：`packages/ui/test/conversationMirrorSync.test.ts`（12 例）——夹具按双机拓扑搭，远端机的本地 scope
  同时是①方向的来源与②方向的目标、视图 scope 是①的目标与②的来源，避免两边共用一份服务桩而掩盖方向接错。
  覆盖：两个方向各落一个 scope（视图带 identity / 反向不带）、②方向处理本机在远端工作区发起的对话、
  窗口至少 10 条且变大后更旧的记录才进来、目标不更旧不重导、mirror 前缀不被当新来源也不被再搬一次、
  对端不支持时整轮降级、空记录不导入、单条读取失败只跳过该条、本地工作区不做任何镜像。
- CLI 侧（`apps/mode-cli/packages/bootstrap/test/`）：
  `session-messages-cold-read.test.mts` 除冷读语义外，**必须**断言返回值通过
  `modeSessionMessagesResultSchema`（成因 0 的回归守卫——只断言文本内容就漏掉了形状错误，
  这正是它上一轮漏过 ZodError 的原因）；
  `imported-history-timestamps.test.mts` 钉住 `remoteMirror` 透传 `createdAt/updatedAt`
  （这是镜像「已同步到哪一版」的水位，被压成 `createdAt` 会导致两端不一致 + 每轮重导），
  以及同一份时间线回填到活跃 record（`applyImportedHistoryTimelineToRecord`）——不回填则
  sessions-index `lastActivityAt` 落在导入那一刻，镜像按导入顺序排在侧栏最前）。
  **仍缺**：`session/create` 带 `importedHistory.source="remoteMirror"` 端到端落盘为可续聊会话的用例。
- E2E（交互改动必需）：远端工作区连接后镜像会话进入任务列表并可续聊；两端各发起一条对话后对方可见。
- 验证：`pnpm typecheck`（另需 `pnpm --filter @mode/bootstrap typecheck`）、`pnpm lint`、
  `pnpm architecture:check --changed`，目标包测试按实际入口执行并如实报告。

## 不做

- **不复制工具调用 / 附件字节**：镜像只有角色 + 文本 + 时间戳（工具调用与附件留在来源机器的工作区）。
- **不做实时帧推送**：只在连接就绪 / 窗口变化 / 来源 scope 事件（防抖 + 20s 间隔下限）/ 手动刷新时同步。
- **不为远端工作区建本地离线副本**：归属固定在工作区所在机器（见上表）。
- **不做冲突合并**：同一来源记录按 `updatedAt` 单向覆盖导入消息（用户续聊消息不受影响）。
- 不含手机远控（`web-remote-replayable`）专项；不含子 agent / 工作流子会话。
- 不做工作区内落盘交换目录（`.mode-share` 式）。
