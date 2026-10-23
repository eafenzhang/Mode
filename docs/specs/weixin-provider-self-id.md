# 微信 providerUserId 误写拆分：providerSelfId（机器人自身 id）

状态：已拍板（2026-10-11，用户确认"修根源：providerUserId 误写"方向）。关联：[bot-binding-menu-display.md](./bot-binding-menu-display.md)（展示降级不动）、[bot-state-ownership.md](./bot-state-ownership.md)（锁内原子 RMW）。

## 背景

`BotConfig.providerUserId` 的语义是**绑定用户 id**（`/bind` 写入真实用户，
`botsService.ts` handleBind；预判对话、心跳目标、群授权门、UI 绑定展示都按这个语义读）。
但微信扫码注册（`BotsDialog.tsx` pollWeixinRegistration success 分支）把上游返回的
`ilink_bot_id`（机器人**自身** id）写进了 `providerUserId`——它真正需要的消费点只有一个：
微信出站协议的 `from_user_id`（`providers/weixinProvider.ts` send）。

实机证据（2026-10-11，`C:\Users\Administrator\.mode\v2\`）：

- 配置：`providerUserId = "8b43889be073@im.bot"`，与 `displayName` 相同（注册双写特征）；
- 状态：存在幻影对话 `private:8b43889be073@im.bot`（`mode: task`，绑着桌面会话），
  真实对话是 `private:o9cq80…@im.wechat`；
- 日志（2026-10-08 ~ 10-11 全量 grep）：入站 user 只有 `o9cq80…@im.wechat`，
  **从未**有以 bot 自身 id 发来的消息——幻影对话只可能来自预判/绑定默认键/心跳合成。

误写的连锁后果：

1. `listBotConversations` 预判分支产出 `private:<bot 自身 id>` → 绑定菜单多出一条
   永远收不到消息的"私聊"（用户问题 2）；
2. `resolveBotProactiveUserId` 首选 `boundUserId` → 心跳/主动投递目标是 bot 自己；
3. `bindBotToTask` 无 conversationKey 时的默认键 = `private:<boundUserId>` → 会话被
   绑到幻影对话上（绿点亮着但 IM 消息走另一条对话）；
4. UI 把它显示成"绑定用户"（BotPrivateChatCard 主绑定徽标、BotSummaryCard 已绑定态）。

## 产品规则

1. **新字段 `providerSelfId`**：`BotConfig` 增加 `providerSelfId?: string`，语义为
   渠道侧机器人自身 id。**必须同时登记进 `botConfigSchema`（`.strict()`）**，
   否则保存直接抛 ZodError。当前只有微信写它。
2. **注册写入**：微信扫码成功改为 `providerSelfId: result.botId`，**不再写
   `providerUserId`**。`displayName` 的既有写入不动（本次非目标）。
3. **出站兼容**：`weixinProvider.send` 的 `from_user_id` 改为
   `bot.providerSelfId ?? bot.providerUserId ?? ""`——存量未迁移配置不断流。
4. **启动迁移**（纯函数规划 + 服务层应用，幂等）：
   - 判别（只对 `provider === "weixin"` 且 `providerSelfId` 为空）：
     `providerUserId === displayName` **或** `providerUserId` 以 `@im.bot` 结尾
     → 判定为注册误写，挪进 `providerSelfId` 并清空 `providerUserId`；
   - `providerUserId` 是真实用户（`/bind` 写入，如 `o9cq80…@im.wechat`，
     两个判别都不命中）→ **不动**，保留其绑定用户语义；
   - 幻影对话清理：迁移产生 `providerSelfId` 后，状态里
     `private:<providerSelfId>` 槽位**删除**——先 `writeDraftContext`（撤流订阅、
     `activeTaskId` 置空，与解绑同语义，正在运行的桌面任务不终止、只失镜像），
     再 `mutateState` 删键；真实对话（`private:o9cq80…`）不受影响；
   - 顺序：在 `ensureBotStorageMigrated` 链内**先** `rekeyLegacyConversations`
     （保持既有语义）**再**本迁移；v3 legacy 重映射到 `private:<自身 id>` 的产物被
     本迁移删除——该键真实入站永远不会命中，其镜像投递目标也是自己（本就不可达），
     桌面会话与转写不受影响，用户可从绑定菜单对真实对话重新绑定。
5. **心跳/主动投递目标回落真实对端**：`resolveBotProactiveUserId` 在
   `boundUserId` 判断之后、`privateChatMode` 判断**之前**增加：对话是私聊 → 返回
   `conversation.conversationId`。迁移后微信（`boundUserId` 为空）心跳打到真实私聊；
   其它 provider 因 `boundUserId` 首选不受影响。不变量：非微信 bound_users 下
   存在私聊对话 ⇒ `providerUserId` 必已设置（入站鉴权要求匹配），该分支对它们不可达。
6. **媒体投递兜底**：`deliverBotMedia` 的私聊目标链变为
   `chatId → providerUserId → conversation.conversationId`（既有 conversation 查找
   已兜底到最近对话）。仅在 `providerUserId` 为空时生效（即迁移后的微信），
   此前该场景直接抛 "no bound chat"。
7. **重复绑定校验不回退**：`validateBotConfig` 的同 provider 重复校验扩展到
   `providerSelfId`（迁移前该保护由 `providerUserId` 承担，迁移后不能丢）。
8. **UI 门槛**：
   - `BotSummaryCard` `isBound` 改为 `Boolean(providerUserId || providerSelfId)`——
     微信注册后仍显示运行态而非"未绑定"；
   - `/bind` 成功检测（BotsDialog 绑定码轮询 `targetBot?.providerUserId`）**不变**——
     它检测的是 /bind 写入的真实用户，与注册无关；
   - `BotPrivateChatCard` / `BotGroupChatCard` / `ProviderSettingsCard` 的绑定展示语义不变：
     微信注册后"已绑定用户"列表如实为空（可手动 /bind 绑真实用户）。存量注记：2026-10-21 起
     微信私聊缺省为 `all_users`（[bot-weixin-open-dm.md](./bot-weixin-open-dm.md)），
     仅适用于无 `accessMode` 的存量配置；2026-10-23 起新建渠道缺省 `whitelist`
     （[bot-access-mode.md](./bot-access-mode.md) R5）。存量 free 配置下默认不渲染绑定区；
     显式切到 `whitelist` / `bound_users` 时才出现上述空列表与 `/bind` 入口。群描述
     `descriptionUnbound` 与群授权门实际行为一致（迁移后 `boundUserId` 为空，
     bound_users 群消息静默忽略——与迁移前"自身份 id 永不等于发送者"行为相同）。
9. **已知边界（接受）**：微信 bound_users 且**仅剩群对话、无私聊对话**时心跳不再触发
   （迁移前依赖 bot 自身 id 通过群授权门，是幻影身份的产物）；需要群心跳用
   `all_users` 或 /bind 真实用户。`findBoundUser`/`findAuthorizedBot` 对微信的既有
   放行（按 botId 定位）不动，入站鉴权无变化。

## 状态所有者

```text
注册（BotsDialog）──► bot-config.v3.json：providerSelfId（repo.writeConfig）
启动迁移：planWeixinSelfIdMigration（纯函数，包外可测）
            │  changed → repo.writeConfig（挪字段）
            │  removedConversations → writeDraftContext（撤订阅）
            │                        → mutateState 删槽位（bot-state.v3.json）
消费点：出站 from_user_id（providerSelfId ?? providerUserId）
        预判对话/心跳目标/媒体兜底（providerUserId，迁移后微信为空 → 回落真实对话）
```

- 配置唯一写入方不变（`repo.writeConfig` / `saveBot`）；状态唯一写入方
  `writeDraftContext` + `repo.mutateState`（bot-state-ownership 规则 1/3）。
- 迁移幂等：`providerSelfId` 已设置或判别不命中 → 规划为空，重复执行无写入。

## 验收场景

1. 单测（`packages/services/test/weixin-provider-self-id.test.ts`，新建）：
   - 规划纯函数：注册双写（`=== displayName`）→ 挪移 + 清空 + 幻影键入删除清单；
     `@im.bot` 后缀（displayName 不同）→ 同上；真实用户 id → 不动；
     非微信 → 不动；已有 `providerSelfId` → 幂等无操作；删除清单只含
     `private:<自身 id>`，真实对话键不出现。
   - schema：`botConfigSchema.parse` 接受 `providerSelfId`（当前 `.strict()` 会抛 → RED）。
   - 源码契约（沿用 bot-workspace-binding 的闭包内源码契约模式）：
     `weixinProvider.ts` send 的 `from_user_id` 使用 `providerSelfId ?? providerUserId`；
     `resolveBotProactiveUserId` 私聊回落早于 `privateChatMode` 判断；
     `validateBotConfig` 校验含 `providerSelfId`。
2. 单测（`packages/services/test/bot-workspace-binding.test.ts` 追加，Q1）：
   - `bindBotToWorkspace` 体内 `writeDraftContext` 出现在
     `focusBotDraftConversationsOnWorkspace` 之前；
   - 解绑条件含 task 模式与 workspaceKey 不等判断；
   - 体内不含 `isContextActiveTaskRunning`（切换解绑不检查运行态）。
3. 单测（`packages/ui/test/`，源码契约）：`BotSummaryCard` 的 `isBound` 含
   `providerSelfId`。
4. 实机：迁移后 `bot-config.v3.json` `providerSelfId` 有值、`providerUserId` 为空；
   `bot-state.v3.json` 无 `private:<自身 id>` 键；绑定菜单只剩真实私聊一条；
   心跳/镜像投递目标为 `o9cq80…`。
5. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、services 测试通过。

## 非目标

- 不改 `displayName` 的注册写入与语义；不改 conversationKey 格式与绑定存储；
- 不改 `bot-binding-menu-display.md` 的展示降级（真实对话无昵称仍走 token）；
- 不动 `findBoundUser` / `findAuthorizedBot` 的微信放行，不改群授权门结构
  （微信 bound_users 群行为迁移前后一致，见规则 9）；
- 不处理其它 provider 的 `providerSelfId`（字段通用但只有微信写入）；
- 不做 webhook/企微/钉钉的身份拆分（它们的 `providerUserId` 语义正确）。
