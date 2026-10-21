# Bot 出站目标统一：群回群、私聊回本人

状态：已批准（2026-10-12，用户「参照 MyAgents 优化」）。
关联：[wecom-reply-feedback.md](./wecom-reply-feedback.md)（流式卡片与占位流的既有「已知限制」由本轮改写）、
[weixin-provider-self-id.md](./weixin-provider-self-id.md)（`resolveBotProactiveUserId` 心跳/身份语义**不动**）、
[bot-reply-granularity.md](./bot-reply-granularity.md)（粒度默认值不动）、
[bot-outbound-delivery.md](./bot-outbound-delivery.md)（停放补投目标链不动）。

## 背景

实机反馈（2026-10-12）：企微群里发消息，回复落在发送者的**私聊**里；私聊文本与 Mode
桌面端会话对不上；群里只剩「思考中…」占位。

根因（源码 + spec 交叉验证，未实机复现）：

1. 企微默认回复粒度自 2026-10-12 起为 `streaming_card`，群回合正文全部走流式卡片；
2. 流式卡片状态的目标写成 `actor.providerUserId`（发送者），是**唯一**不按
   `chatId ?? providerUserId` 路由的出站路径——`createOutbound`、typing、排队消息、
   停放补投、媒体目标链都是对的，两条表达式随时间漂移产生了本次事故；
3. 企微入站帧按 `chatid` 存键，卡片按 `userid` 找帧必然落空 → 降级「仅终稿推送」→
   `sendMessage(发送者)` 进私聊；占位流按 chatid 开、卡片按 userid 认领，键对不上，
   群里占位无人收口；
4. 桌面输入镜像的群分支调 `resolveBotProactiveUserId`，该函数按
   [weixin-provider-self-id.md](./weixin-provider-self-id.md) 规则 5 首选绑定用户（私聊），
   与该处注释「谁持有这个会话就往谁的对话镜像：群聊回群、私聊回本人」自相矛盾。

参照实现（MyAgents 企微插件）：`sendWeComReply` 一律 `replyStream(frame)` 回在入站帧所在
会话；只有事件回调（不能 replyStream）才降级主动发送，且目标是
`body.chatid || body.from.userid`（**群 id 优先**）；reqId 按 chatId 存键。其 `im-mirror`
由绑定通道把桌面侧 user/assistant 文本投回**同一条**会话，不换目标。

## 产品规则

1. **唯一出站目标**：面向会话的文本、卡片、媒体投递，目标 = 所属会话 =
   `actor.chatId ?? actor.providerUserId`（群 → 群 chatId；私聊 → 对方 userId）。
   - 新增共享函数 `resolveOutboundTarget(actor)`，`createOutbound` 与流式卡片状态都用它；
     禁止再手写第二份表达式（本次事故即两条路径漂移）；
   - `BotStreamingReplyCardState.providerUserId` 语义定为「最终投递目标」，与
     `BotProviderAdapter.sendMedia` 注释同一措辞；provider 只把它当 chatKey /
     receive_id / chat_id 使用，不得再假设它是发送者身份。
2. **桌面输入镜像回原会话**：`notifyDesktopUserMessage` 的目标 =
   `context.conversationId`（私聊 = 对方 userId，群 = 群 chatId），函数体内不再引用
   `resolveBotProactiveUserId`。
3. **不改动的边界**：
   - `resolveBotProactiveUserId` 的判定顺序（boundUserId → 私聊对话 → all_users 回落）
     维持 weixin-provider-self-id 规则 5，只继续充当心跳身份与门槛；
   - `queuedMessages`、`pendingDeliveryQueue`、媒体目标链、typing 目标既有 chatId 语义不动；
   - 群授权门、replyMode 默认值、卡片内容构成（正文 + 变更摘要）不动。
4. **失败降级**：拿不到会话帧时不再「攒到终稿一次推送」，而是由服务层回退逐块气泡，
   仍发往**同一目标**（企微走主动推送 `aibot_send_msg`）——见
   [bot-reply-granularity.md](./bot-reply-granularity.md)「流式不可用时回退逐块气泡」；
   「降级」不允许改变会话——推不动就换帧或改走主动推送，不换收件人。
5. ** provider 无关**：本规则对全部 provider 生效。飞书 `oc_` 前缀解析为 `receive_id_type=chat_id`、
   钉钉 `group:` 前缀走群发送、Telegram `chat_id` 直传、企微 chatid 直传，均已在各自
   provider 内按目标形态分流，无需在服务层做平台分支。

## 状态所有者与时序（群回合）

```text
入站群消息 ──▶ actor { chatId=群, providerUserId=发送者 }
                 │
startTyping/sendTyping ──▶ 占位流（键 = resolveOutboundTarget(actor) = chatId）
                 │
watchTaskStream 流式卡片 ──▶ state.providerUserId = resolveOutboundTarget(actor)
                 │            ├─ 有群帧 ──▶ replyStream 在群里原地接管占位（同 streamId）
                 │            └─ 无帧   ──▶ 回退逐块气泡 sendMessage(chatId)（仍在群里，不等到终稿）
task 终态 ──▶ 卡片终稿收口；命令/错误/澄清/媒体走 createOutbound(chatId)（群）

桌面发起回合 ──▶ buildConversationActor(chatId) ──▶ 同一卡片路径（群）
桌面输入镜像 ──▶ adapter.send(providerUserId = context.conversationId)（群/私聊各回各家）
```

单所有者：出站目标只由服务层（`botsService`）决定；provider 是执行者，不自选收件人。

## 验收场景

1. 单测 `packages/services/test/bot-outbound-target.test.ts`（源码契约）：
   - `syncStreamingCardReply` 构造的 `state.providerUserId` 取自 `resolveOutboundTarget(actor)`，
     该 state 构造处不再出现裸 `actor.providerUserId`；
   - `notifyDesktopUserMessage` 的镜像目标取 `context.conversationId`，函数体内不引用
     `resolveBotProactiveUserId`（`bot-conversation-session.test.ts` 的既有镜像契约
     同步收敛到这一口径）；
   - `resolveOutboundTarget` 与 `createOutbound` 共用同一函数（不存在第二份 `??` 表达式）。
2. 单测 `packages/services/test/bot-wecom-typing.test.ts`（provider 契约）：卡片以
   `state.providerUserId`（= 会话键）认领占位流、查找入站帧、推送终稿——三处同键。
3. 回归全绿：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed`。
4. **待实机**：企微群 @机器人，回复出现在**群里**且与桌面端会话正文一致；群里占位被同一条
   流收口（不再残留「思考中…」）；桌面端发消息时镜像进群而非绑定用户私聊。

## 桌面输入镜像的附件口径（2026-10-16 决策并落地）

**原缺口**：`notifyDesktopUserMessage` 的参数只有 `text`，镜像只发文本，所以桌面端带附件发消息时
IM 侧只看到文字、文件没过去。反方向是完整的：IM → 桌面走 `prepareBotMessageContent`，
附件会下载、缓存并作为 prompt 附件进入回合（`cacheResolvedAttachment`）。

**决策：附件不进镜像，但必须在 IM 侧显式告知未同步。** 两个可选口径里选了后者，理由各自成立：

- **静默跳过——否决**。IM 侧的读者会以为这次提问是完整的，属**可见的错**。本项目在出站投递上
  已立过同一条规矩（`bot-outbound-delivery.md`：失败必达、用户可见），镜像没有理由更宽松。
- **自动镜像附件——否决**。把用户的本地文件上传到 IM 平台是**一次隐式的数据外发**，群聊场景下
  还意味着文件到了其他人手里。用户把附件加在**桌面会话**上，不等于同意把它发给 IM 平台，
  这个同意不该由镜像代签。日后若要按通道开放（大概率只有私聊合理），是独立决策。

**已实现（2026-10-16）**：

- `IBotsService.notifyDesktopUserMessage` 增加 `attachmentCount?: number`（`packages/services/src/bots/bots.ts`）；
- 服务层把未同步提示拼在镜像正文**之后**（同一条消息，不另发一条），文案 `desktopPromptMirrorAttachmentsSkipped`；
- 渲染层两处镜像调用点（`SessionPane.tsx`：新建会话 / 既有会话）传 `readyAttachments.length`。

**集成点（将来真要传附件时直接用）**：

1. UI 侧 `readyAttachments` 已在发送回调作用域内，取清单不需要新的状态；
2. `AttachmentRef` 是引用不是字节——本地文件带 `localPath`（零拷贝 ready），其余形态要靠 UI 的
   `attachmentRead` / `attachmentReadRange`，而那是 **UI 侧能力、服务层拿不到**；
3. 投递侧已具备：`deliverBotMedia` + `providers[].sendMedia`，目标复用 `resolveOutboundTarget`。

**验收**：桌面端在绑定会话里带 1 个以上附件发消息 → IM 侧文本末尾出现「（另有 N 个附件未同步到本会话）」；
不带附件时不出现该提示。两端都有单测钉住（`services/test/bot-outbound-target.test.ts`、
`ui/test/sessionPaneMirrorAttachments.test.ts`）。**待实机**。

## 非目标

- 不改回复粒度默认值、卡片渲染构成与分片（工具/思考不进卡片、终稿只含正文 + 变更摘要，
  是 `bot-reply-granularity` 既定行为）；「内容不一致」中属于**渲染通道差异**的部分不在此轮，
  待实机确认是否另有诉求。
- 不改 `resolveBotProactiveUserId`、心跳目标选择、群授权门与绑定模型。
- 不引入 MyAgents 的 assistant 侧 im-mirror 增强（Mode 已有 `watchTaskStream` 等价镜像）。
