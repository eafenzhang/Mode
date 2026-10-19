# IM Bot A+：Agent Channel + 四通道统一语义

状态：已批准（2026-10-20，用户确认 A+；替代 [im-bot-rework.md](./im-bot-rework.md) 的待决草案）。

## 决策

- **D1 会话作用域**：不升话题级。群会话按 `chatId`，私聊按用户；飞书 thread 只作为来源元数据，不进入会话键。
- **D2 配置模型**：引入 Agent 拥有 Channel。Agent 持有工作区、默认模型/Runtime/权限、Heartbeat；Channel 持有凭据、私聊/群聊规则、投递模式和回复粒度。
- **D3 接入 UI**：保留现有 `BotsDialog` 卡片式交互，不改成向导。新增 Agent 归属、群授权列表、投递模式；微信隐藏群聊。
- **D4 运行时**：保留 `packages/services/src/bots` 的 Node 运行时，不迁 Rust。A+ 的 transport worker 是后续可选隔离层，Core 仍拥有业务状态。
- **通道集合**：只保留 `weixin / wecom / feishu / dingtalk`。Telegram、webhook、discord、astrbot 退出产品面；历史配置按迁移策略处理，不新增第 5 个通道。
- **微信群聊**：按参照行为仅私聊（`groups: none`），移除微信群聊入口与群消息处理。

## 参照与许可边界

- 参照项目 MyAgents 为 AGPL-3.0。**只对齐可观察行为与步骤集合，不复制、不翻译其实现代码，不把其源码写入本仓库。**
- 本仓库为 MIT。实现必须是 clean-room 独立实现；接口面、类型、测试断言和本 spec 是允许输入。
- 与 `packages/services/src/bots` 下上游衍生文件的关系继续遵守 `clean-room-policy.md`；本 spec 不授权成片搬运这些实现体。

## 状态所有权

| 状态                                                 | 唯一 Owner                    | 持久化                               |
| ---------------------------------------------------- | ----------------------------- | ------------------------------------ |
| Agent 配置、工作区、默认模型/Runtime/权限、Heartbeat | `BotsRepo` / Agent 配置层     | `bot-config.v4.json`（迁移自 v3）    |
| Channel 凭据                                         | `credentialService`           | `credentials.json`                   |
| Channel 私聊/群聊规则、投递模式、回复粒度            | `BotsRepo`                    | `bot-config.v4.json`                 |
| 群授权记录                                           | `BotsRepo`（Channel 内）      | `bot-config.v4.json`                 |
| 群历史                                               | 群历史所有者                  | 独立文件 `bot-group-history.v1.json` |
| peer binding、草稿/任务、排队、pending interaction   | `BotsRepo.mutateState`        | `bot-state.v4.json`                  |
| 去重、游标                                           | `BotsRepo.mutateState`        | `bot-state.v4.json`                  |
| 长连接、SDK、发送                                    | Channel runtime / 后续 worker | 不持久化业务状态                     |

不变式：

1. 所有状态 RMW 继续走 `BotsRepo.mutateState`；禁止 readState…writeState 快照覆盖。
2. 群历史不进入 `bot-state`，避免每条未触发群消息触发全量写盘。
3. 群授权与 Channel 配置同文件写入，但只由配置写入路径修改。
4. worker 只拥有连接，不拥有 Agent/Channel/Session/群授权/群历史。

## 私聊规则

1. 会话键：`agent:{agentId}:{provider}:private:{peerId}`。
2. 准入模式：`bound_users`（绑定用户或 `allowedUsers`）或 `all_users`。
3. 空名单 + `bound_users`：拒绝，直到 `/bind` 成功；不复制参照项目 transport 的空名单放行差异。
4. 微信不再绕过准入；微信、企微、飞书、钉钉共用同一套判定。
5. `/bind` 只允许私聊，TTL 5 分钟，单次使用；已绑定用户重复绑定码静默忽略。
6. `/new` 只允许私聊；创建新 Session 并轮换 peer binding，历史保留。
7. 私聊是 Heartbeat、Task、Cron 的唯一投递目标；群活动不得覆盖。
8. 命令核心集：`/new`、`/model`、`/status`、`/help`、`/start`、`/bind`。现有扩展命令保留时必须在群聊中静默丢弃。
9. 私聊每条消息注入渠道上下文；不注入群历史。
10. `auto` 模式自动推送；`tool_only` 模式普通输出不外发，显式发送与必要控制消息仍可用。

## 群聊规则

1. 会话键：`agent:{agentId}:{provider}:group:{chatId}`；一个群一个 Session。
2. 微信：不支持群聊，群入口隐藏，群消息不进入 AI。
3. 企微、钉钉：仅 @ 模式；平台只下发 @ 消息。飞书：`mention` / `always` / `disabled`。
4. 群授权记录：`{ groupId, groupName, platform, status: pending|approved, discoveredAt, addedBy? }`。
5. 群发现：首次群消息或平台 bot-added/成员事件创建 `pending`；UI 批准/拒绝/移除。
6. 发送者准入：群已批准，或发送者在 `allowedUsers` 中；否则只写群历史并静默。
7. `mention` 模式：没有 mention 证据只写历史；`always` 模式每条触发，模型可用 `NO_REPLY` 静默。
8. 群历史：单群 30 条、单条 200 字符、最多 500 群；未触发和未授权消息都记录；触发时读取快照并注入，读取不清空。
9. 群管理命令静默丢弃，不交给 AI、不写 binding。
10. 群名只用于展示；授权、路由、去重继续使用群 ID。
11. Bot 被移出群：失效群授权、清空群历史、停止发送。
12. 群活动不更新 `lastActivePrivateTarget`。

## 回合准入

1. 同一会话（群或私聊）的入站与 bot 回合由 `enqueueInboundProcessing` 按对话键串行。
2. 单服务实例内同时运行的 bot 回合总数 ≤ `BOT_GLOBAL_TURN_CONCURRENCY_CAP`（默认 8）。
3. 超出上限按全局 FIFO 等待；首次挂起向发起方发一次 `turnQueued` 提示。
4. 不再使用 workspace 级上限 2；不同群/私聊在全局槽位内并行。

## 平台能力

| 平台     | 私聊 | 群聊      | mention    | 出站媒体  |
| -------- | ---- | --------- | ---------- | --------- |
| weixin   | 是   | 否        | 不适用     | 当前无    |
| wecom    | 是   | 仅 @      | 平台不变量 | 图片/文件 |
| feishu   | 是   | 全部/仅 @ | mentions   | 图片/文件 |
| dingtalk | 是   | 仅 @      | isInAtList | 当前无    |

能力表是 UI、admission 和 runtime 的单一事实来源；不能只靠 provider 名称在 UI 与运行时各写一套。

## A+ Worker

第一阶段保持 transport 在 Core 进程内，但按 `ChannelTransport` 接口隔离。第二阶段可把每个 Channel 的 transport 移入受监督 worker：

- Core 拥有业务状态、准入、Session、群历史、去重。
- Worker 拥有 SDK 连接、入站解析、出站发送、重连。
- 游标只在 Core 接纳后提交；worker 崩溃重投的消息由 Core 去重。
- 单 Channel worker 崩溃不影响其他 Channel。

## 迁移

1. 读取 `bot-config.v3.json`，写 `bot-config.v4.json`；旧文件保留用于回滚。
2. 默认一 Bot 一 Agent 一 Channel：Agent id = `agent-{botId}`，Channel id = `botId`。
3. 工作区取 `bot-bindings.v3.json`；无法确定唯一工作区时标记待绑定，不自动猜测。
4. `heartbeat`、`currentOptions` 移到 Agent；`privateChatMode`、`allowedUsers`、`providerUserId`、`groupChat`、凭据、`replyMode` 留在 Channel。
5. `lark` 迁移为 `feishu`（保留 domain 信息）；telegram/webhook/discord/astrbot 记录标记为 retired，不进入产品面。
6. 状态文件 `bot-state.v3.json` → `bot-state.v4.json`；conversation key 保持 `private:/group:`，增加 agent/channel 归属。

## 分阶段

- **阶段 1**：规格、四通道类型/能力表、私聊准入统一、群授权 schema + 闸门 + 服务端操作。
- **阶段 2**：UI 群授权列表、微信隐藏群聊、投递模式；迁移 v3→v4。
- **阶段 3**：ChannelTransport 接口与 worker 隔离；出站 per-conversation 队列、全局公平准入。
- **阶段 4**：删除 Telegram/webhook/discord 运行时与 HTTP 入口；回归测试。

## 验收

1. 新建通道只出现四个；微信无群聊。
2. 同一 Bot 多用户私聊互不串话；微信不再无条件放行。
3. 群未批准只记历史；批准后任何成员可 @ 触发；拒绝/移除清空历史。
4. 群管理命令静默；`NO_REPLY` 不产生空消息。
5. 群活动不覆盖私聊 Heartbeat 目标。
6. 迁移可回滚；旧配置不可解析时明确报错，不静默丢数据。
7. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 通过；相关 services/UI 测试全绿。

## 非目标

- 不升话题级（D1）。
- 不迁 Rust（D4）。
- 不引入 OpenClaw Plugin Bridge。
- 不新增第 5 个通道。
- 不复制 MyAgents 实现代码。
