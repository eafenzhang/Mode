# 远程工作区对话记录镜像（Conversation Mirror）

## 产品规则

### 范围

- 覆盖桌面窗口发起的**全部远端连接类型**（SSH / WSL / Docker / LAN）。
- 「对话记录」= 对端机器在**同一工作区路径**下、以**本机本地 scope**（`workspaceIdentity` 为空）产生的会话。
- **镜像产物是真会话，不是只读投影**：镜像记录以 `session/create` + `importedHistory` 导入成目标 scope 下的真实会话，
  与本地对话完全同质——可继续对话、可置顶 / 归档 / 改名 / 删除、进任务搜索与 `@` 提及。
- 「双向」= 一轮同步同时做两件事：① 对端记录 → **本机当前视图 scope**；② 本机记录 → **对端本地 scope**。
- 「每次连接时自动同步」= 远端 workspace attach 就绪（`rpcReady`）或该 workspace 被激活时自动跑一轮。

### 归属与执行位置（本 spec 的核心裁决）

| 方向                | 目标 scope（会话落在哪）                           | 之后续聊时 agent 跑在哪    |
| ------------------- | -------------------------------------------------- | -------------------------- |
| 对端记录 → 本机视图 | **对端机器**上、当前 attach 使用的 workspace scope | 对端机器（工作区所在机器） |
| 本机记录 → 对端视图 | **对端机器**的本地 scope（不带 identity）          | 对端机器（工作区所在机器） |

- 镜像会话一律落在**承载该工作区的机器**上，因为续聊要在有代码的那台机器执行；本机不为远端工作区另建本地副本。
- 由此继承两条例证：① 断开连接后镜像会话与远端工作区普通会话一样不可见（不是本地离线副本）；
  ② 对端机器打开该工作区时，会看到本机记录导入进来的会话（这就是「互相看到」）。
- **一个来源记录只会被导入到一个 scope**（按方向固定），因此确定性 id 不需要带 scope，重连换端口也不会产生重复。

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
3. **镜像副本不再被转发**：镜像会话 id 统一为 `mirror-<sha256(originSessionId)[0:24]>`，读取对端本地 scope 时跳过该前缀。
   这同时挡住「本机副本被对端搬回来」的回声和第三台机器副本的扩散。
4. **幂等**：确定性 id + CLI 的 `removePreviousImportedSessionHistory`（按 `metadata.migrationSource` 清理旧导入消息）
   保证重复导入只覆盖导入消息、**保留用户后续在镜像会话里真实聊出的消息**。
5. **有界**：单轮最多 20 条会话；每条最多 200 条消息、单条文本 ≤ 4000 字符、单条记录 ≤ 256 KiB（超限丢最旧消息并置 `truncated`），
   为远端 RPC 物理帧（1 MiB）留余量。
6. **失败不阻断**：导入失败只记 warn + 视图状态，不阻断 workspace 连接、任务列表与会话主链路；
   对端 CLI 不认识 `remoteMirror` 时判为「对端版本过低」，本轮不再重试，UI 给出可读提示。
7. **不启动额外 runtime**：只处理用户正在使用的工作区；列举走任务列表（被动观察者），
   消息读取走既有 `session/messages`（冷会话由 CLI 自行从 store 恢复）。

### 事件顺序

```text
A 连接 B 的远端工作区 W（rpcReady / 激活）
  │
  ├─ ① 对端记录 → A 的视图 scope（会话落在 B 上）
  │   remote.modeTaskService.listTaskList({workspacePath: W})               ← 取 B 的本地 scope 行
  │     └ 过滤：带 identity 的行（对端身份）与 mirror- 前缀（副本）都跳过
  │   remote.modeTaskService.listTaskList({workspacePath: W, identity: R})  ← 目标 scope 现有 meta
  │     └ 仅当 origin.updatedAt > 目标 meta.updatedAt 才继续（未变则跳过）
  │   remote.modeSessionService.readSessionMessages({W, sessionId}) → 提取文本 + 截断
  │   remote.modeSessionService.createSession({
  │       sessionId: mirror-<hash>, persistence: "immediate",
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
- 单测（ui）：来源记录变更才导入（meta 更新时间比较）、mirror 前缀不转发、两个方向的 workspace ref 正确
  （视图 scope 带 identity，对端本地 scope 不带）、对端不支持时降级、空记录不导入、单条失败不影响其余。
- CLI 侧（`apps/mode-cli`）：`session/create` 带 `importedHistory.source="remoteMirror"` 能落盘为可续聊会话，
  重导入保留后续真实消息（沿用既有 `mode-protocol.test.ts` 导入用例模式）。
- E2E（交互改动必需）：远端工作区连接后镜像会话进入任务列表并可续聊。
- 验证：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`，目标包测试按实际入口执行并如实报告。

## 不做

- **不复制工具调用 / 附件字节**：镜像只有角色 + 文本 + 时间戳（工具调用与附件留在来源机器的工作区）。
- **不做实时帧推送**：只在连接就绪 / 激活 / 手动刷新时同步。
- **不为远端工作区建本地离线副本**：归属固定在工作区所在机器（见上表）。
- **不做冲突合并**：同一来源记录按 `updatedAt` 单向覆盖导入消息（用户续聊消息不受影响）。
- 不含手机远控（`web-remote-replayable`）专项；不含子 agent / 工作流子会话。
- 不做工作区内落盘交换目录（`.mode-share` 式）。
