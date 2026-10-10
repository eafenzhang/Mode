# 企微机器人等待期反馈（三点打点动画占位流）

## 背景与根因

用户在企业微信里给机器人发消息后，等待回复期间**没有任何状态显示**（其他渠道都有：
飞书是 Typing reaction、Telegram 是 chatAction、微信是 `/sendtyping`）。逐层排查后根因有三条：

1. `wecomProvider` 没有实现 `BotProviderAdapter` 的任何可选 typing 钩子
   （`sendTyping` / `startTyping`），`botsService` 的 `if (adapter?.sendTyping)` 门禁直接跳过；
2. 企微 SDK（`@wecom/aibot-node-sdk`）没有原生「正在输入」命令——`WsCmd` 只有
   respond/send/upload/heartbeat，也没有消息撤回 API；
3. 默认回复粒度是 `assistant_changes`，不创建流式卡片；即便用户选了 `streaming_card`，
   首帧也要等句号边界或工具活动才发（`syncStreamingCardReply` 的首帧门槛），
   纯思考期依然是零反馈。

企微平台给出反馈的唯一通道是**非终态 `replyStream` 占位帧**——官方 SDK README 的示例就是
收到消息立刻 `replyStream(frame, streamId, '正在思考中...', false)`，之后同 `streamId`
更新、终帧 `finish=true` 收口。同流刷新的替换语义（每次发的是完整当前文本）也由既有
流式卡片证实，因此可以用它模拟动画。

## 产品规则

- 长任务接单（`startTyping`）与同步短命令（`sendTyping`）都**立刻**开一条非终态占位流；
  首帧是 `·`，随后每 500ms 用 `replyStreamNonBlocking` 在**同一条消息**上刷新
  `· → ·· → ···` 循环——与桌面三点气泡同观感的打点动画（企微没有动画素材通道，
  同流刷新是唯一的动画手段；上一帧未 ack 时 SDK 自动跳过，天然节流）。
- 占位流的收口**归属给下一条真正出站的消息**：
  - `send()`：认领占位流（先等进行中的打开收敛），正文首片以 `finish=true` 收口同一条消息，
    余片继续分片推送；
  - `createStreamingReplyCard()`：认领占位流，卡片首帧复用同一 `streamId`，
    打点文案被卡片渲染原地替换，用户侧不会出现两条消息；
  - 收口失败（通常是帧过期、该流已不可寻址）：放弃占位流并回落主动推送——内容不丢，
    下一次 typing 用新入站帧开新占位流。归还失败占位流只会永久卡死后续打开，所以不归还。
- `stopTyping` **只停动画、不收口**：任务完成/权限请求/问答挂起时打点冻结（机器人确实
  不在输出了），流保持打开等紧随其后的出站接管；任务恢复（问答通过后再次 `startTyping`）
  会把停掉的动画续上。这里 finish 会把打点文案留成永久消息，所以绝不收口。
- 占位键为 `${botId}:${chatKey}`：provider 实例按 provider 名共享，一个实例服务全部企微
  bot，不带 botId 会让同会话里的两个 bot 互相收口对方的占位流。
- 打开是异步的，出站/建卡在认领前**先等进行中的打开收敛**（in-flight promise）：
  `sendTyping` 由 `botsService` 以 `void` 触发，若认领不等打开，会出现
  「回复先落地、占位帧后到」——回复之后残留一条永远显示的打点消息。
- 无入站帧（主动触达、后端推送）时静默降级：开不了流就不开，回复链路不受影响。
- 占位与动画都是 best-effort：打开/刷新失败只影响展示，不参与回复成败。

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
        │              replyStream(首帧 "·", finish=false)
        │              setInterval 500ms：· → ·· → ··· 同流刷新（NonBlocking 节流）
        ▼
  WeCom 聊天里立刻出现打点动画（桌面三点气泡同观感）
  │
  ├─ 流式卡片首帧（句号边界/工具活动/终稿 force）
  │     createStreamingReplyCard ──认领（先等打开收敛，停动画）──► 同 streamId 首帧
  │     updateStreamingReplyCard(终稿) ──► finish=true 收口
  │
  ├─ 普通回复（默认粒度/命令回执/错误提示）
  │     send() ──认领（停动画）──► 正文首片 finish=true 收口同一条消息 → 余片分片推送
  │
  └─ 中途挂起（task_complete / permission_request / elicitation）
        stopTyping ──► 只停动画（点冻结），流保持打开等出站接管；
                       任务恢复 startTyping 续动画
```

## 已知限制

- 占位流绑定打开它的入站帧 `req_id`；帧过期后该流不可寻址，平台无撤回 API，
  这条占位消息只能被平台侧超时收敛（与既有流式卡片的帧过期语义一致）。
- 打点动画 500ms 一帧是 WS 开销与观感的折中；企微客户端对高频流更新的渲染细节
  （是否平滑、是否自带生成光标）需要真机确认。
- 群聊 + `streaming_card`：卡片按 `state.providerUserId`（发送者）找帧是既有行为，
  群聊帧缓存键是 chatid，卡片仍走「仅终稿推送」降级；此时占位流（chatid 键）
  由下一次 `send()`（providerUserId = chatId ?? providerUserId）收口。
- 企微群聊只有 mention 语义（`capabilities.always` 为 false），不存在「沉默回合」，
  因此占位流不会破坏群聊全静音的产品规则。

## 验收场景

1. `startTyping` 立即发出一条非终态占位帧，重复触发只开一条（`bot-wecom-typing.test.ts`）。
2. 打点动画：首帧 `·`，每 500ms 按 `· → ·· → ···` 循环刷新同一条流；
   出站接管后动画停止。
3. 无入站帧时不开占位流、不抛错，出站照常主动推送。
4. 出站正文接管占位流：首片 `finish=true` 落在同一 `streamId`，余片分片推送；
   占位已消费后不再重复收口。
5. 流式卡片接管占位流：`createStreamingReplyCard` 返回占位流的 `streamId`，终稿 finish 同流。
6. 同会话里另一个企微 bot 的出站不收口本 bot 的占位流（键带 botId）。
7. `stopTyping` 只停动画、不 finish 占位流；之后出站仍能接管收口。
8. typing 打开与出站并发：出站等待打开收敛后接管同一条流，回复之后不残留占位帧。
9. `pnpm typecheck`、`pnpm lint`、services 测试全绿。
