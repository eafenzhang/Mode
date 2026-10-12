# 企微机器人等待期反馈（静态占位流）

## 背景与根因

用户在企业微信里给机器人发消息后，等待回复期间**没有任何状态显示**（其他渠道都有：
飞书是 Typing reaction、Telegram 是 chatAction、微信是 `/sendtyping`）。逐层排查后根因有三条：

1. `wecomProvider` 没有实现 `BotProviderAdapter` 的任何可选 typing 钩子
   （`sendTyping` / `startTyping`），`botsService` 的 `if (adapter?.sendTyping)` 门禁直接跳过；
2. 企微 SDK（`@wecom/aibot-node-sdk`）没有原生「正在输入」命令——`WsCmd` 只有
   respond/send/upload/heartbeat，也没有消息撤回 API；
3. 默认回复粒度曾是 `assistant_changes`（2026-10-12 起企微默认已改为 `streaming_card`，
   见 [bot-reply-granularity.md](./bot-reply-granularity.md)），不创建流式卡片；即便用户选了 `streaming_card`，
   首帧也要等句号边界或工具活动才发（`syncStreamingCardReply` 的首帧门槛），
   纯思考期依然是零反馈。

企微平台给出反馈的唯一通道是**非终态 `replyStream` 占位帧**——官方 SDK README 的示例就是
收到消息立刻 `replyStream(frame, streamId, '正在思考中...', false)`，之后同 `streamId`
更新、终帧 `finish=true` 收口。同流刷新的替换语义（每次发的是完整当前文本）也由既有
流式卡片证实。

> 演进：初版用 500ms 循环打点（`· → ·· → ···`）模拟桌面三点气泡；后按用户要求对齐
> MyAgents 企微插件（`wecom-openclaw-plugin` 的 `THINKING_MESSAGE`）实机观感——
> 占位改为开流一次的静态文案（见 `WECOM_TYPING_PLACEHOLDER`），不再循环刷新。

## 产品规则

- 长任务接单（`startTyping`）与同步短命令（`sendTyping`）都**立刻**开一条非终态占位流；
  首帧即发一次 `WECOM_TYPING_PLACEHOLDER`（与 MyAgents 企微插件 `THINKING_MESSAGE`
  逐字一致的空 think 标签对），思考期间**不发任何刷新帧**，直到正文/卡片原地接管。
  **占位内容是静态的，但呈现是动态的**：企微 PC 客户端对未收口的流显示原生 loading
  （插件 `finishThinkingStream` 注释实证「不发可见字符 finish 帧会一直 loading」），
  正文到来后同流原地增长——与 MyAgents 观感一致。循环打点动画（可见 `···` 文本帧会
  把内容显示成静态小点、遮掉原生动感）已随「对齐 MyAgents」一并移除。
- 占位流的收口**归属给下一条真正出站的消息**：
  - `send()`：认领占位流（先等进行中的打开收敛），正文首片以 `finish=true` 收口同一条消息，
    余片继续分片推送；
  - `createStreamingReplyCard()`：认领占位流，卡片首帧复用同一 `streamId`，
    占位文案被卡片渲染原地替换，用户侧不会出现两条消息；
  - 收口失败（通常是帧过期、该流已不可寻址）：放弃占位流并回落主动推送——内容不丢，
    下一次 typing 用新入站帧开新占位流。归还失败占位流只会永久卡死后续打开，所以不归还。
- `stopTyping` **不收口**（静态占位没有动画可停）：任务完成/权限请求/问答挂起时占位保持
  打开，等紧随其后的出站接管。这里 finish 会把占位文案留成永久消息，所以绝不收口；
  保留空实现是为了维持上层 start/stop 契约与失败 warn 口径。
- 占位键为 `${botId}:${chatKey}`：provider 实例按 provider 名共享，一个实例服务全部企微
  bot，不带 botId 会让同会话里的两个 bot 互相收口对方的占位流。
- 打开是异步的，出站/建卡在认领前**先等进行中的打开收敛**（in-flight promise）：
  `sendTyping` 由 `botsService` 以 `void` 触发，若认领不等打开，会出现
  「回复先落地、占位帧后到」——回复之后残留一条永远显示的占位消息。
- 无入站帧（主动触达、后端推送）时静默降级：开不了流就不开，回复链路不受影响。
- 占位是 best-effort：打开失败只影响展示，不参与回复成败。

## 状态所有者与时序

占位流状态唯一所有者是 `wecomProvider` 闭包（`pendingTypingStreams`），不落盘、不进协议。

```
用户发消息
  │ wecomChannelRuntime: rememberFrame(chatKey) → processProviderCallback
  ▼
botsService 接单
  ├─ 长任务：startTyping ────────► ensureTypingPlaceholder
  └─ 同步命令：sendTyping ───────►   │（幂等：in-flight + pending 双保险）
        │                            ▼
        │              replyStream(首帧 WECOM_TYPING_PLACEHOLDER, finish=false)
        │              思考期零刷新帧——企微 PC 原生 loading 呈现动态（MyAgents 同款）
        ▼
  WeCom 聊天里立刻出现静态思考占位
  │
  ├─ 流式卡片首帧（句号边界/工具活动/终稿 force）
  │     createStreamingReplyCard ──认领（先等打开收敛）──► 同 streamId 首帧原地替换占位
  │     updateStreamingReplyCard(终稿) ──► finish=true 收口
  │
  ├─ 普通回复（默认粒度/命令回执/错误提示）
  │     send() ──认领──► 正文首片 finish=true 收口同一条消息 → 余片分片推送
  │
  └─ 中途挂起（task_complete / permission_request / elicitation）
        stopTyping ──► 无操作（静态占位无动画可停），流保持打开等出站接管
```

## 已知限制

- 占位流绑定打开它的入站帧 `req_id`；帧过期后该流不可寻址，平台无撤回 API，
  这条占位消息只能被平台侧超时收敛（与既有流式卡片的帧过期语义一致）。
- 静态占位文案是逐字对齐 MyAgents 的实现细节；若参照产品日后调整其 `THINKING_MESSAGE`，
  需同步更新 `WECOM_TYPING_PLACEHOLDER` 与对应单测。
- 群聊 + `streaming_card` 的帧缓存键错位已修复（2026-10-12，见
  [bot-outbound-target.md](./bot-outbound-target.md)）：卡片目标与占位流同为
  `resolveOutboundTarget(actor)` = 群 chatid——能取到群帧就在群里同流接管占位，
  取不到才走「仅终稿推送」，且推送目标仍是 chatid（**降级不换收件人**）。
  修复前卡片按发送者 `providerUserId` 找帧，群帧必然落空 → 终稿推进发送者私聊、
  群里占位无人收口，是本轮实机问题的根因。
- 企微群聊只有 mention 语义（`capabilities.always` 为 false），不存在「沉默回合」，
  因此占位流不会破坏群聊全静音的产品规则。

## 验收场景

1. `startTyping` 立即发出一条非终态占位帧（内容为 `WECOM_TYPING_PLACEHOLDER`，
   与 MyAgents `THINKING_MESSAGE` 逐字一致），重复触发只开一条（`bot-wecom-typing.test.ts`）。
2. 静态占位：不随时间发出任何刷新帧（无循环打点）；出站接管后同样无残留刷新。
3. 无入站帧时不开占位流、不抛错，出站照常主动推送。
4. 出站正文接管占位流：首片 `finish=true` 落在同一 `streamId`，余片分片推送；
   占位已消费后不再重复收口。
5. 流式卡片接管占位流：`createStreamingReplyCard` 返回占位流的 `streamId`，终稿 finish 同流。
6. 同会话里另一个企微 bot 的出站不收口本 bot 的占位流（键带 botId）。
7. `stopTyping` 不 finish 占位流（静态占位无可停动画）；之后出站仍能接管收口；
   stopTyping 后再次 startTyping 保持幂等单流。
8. typing 打开与出站并发：出站等待打开收敛后接管同一条流，回复之后不残留占位帧。
9. 群聊键一致：卡片 `state.providerUserId` 与占位流 chatKey 同取
   `resolveOutboundTarget(actor)`（`bot-outbound-target.test.ts` 契约 +
   `bot-wecom-typing.test.ts` 群键用例）——群里占位由卡片同流收口，终稿落群 chatid。
10. `pnpm typecheck`、`pnpm lint`、services 测试全绿。
