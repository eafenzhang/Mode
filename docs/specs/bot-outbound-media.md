# IM Bot 出站文件：群聊 `/send` 与 agent 侧 SendFile

状态：已批准（2026-10-21，用户「MyAgents 同一个企微 bot 可以发消息也可以发文件，我们的不行」）。
关联：[bot-outbound-target.md](./bot-outbound-target.md)（出站目标唯一函数与媒体目标链）、
[im-bot-agent-channel-aplus.md](./im-bot-agent-channel-aplus.md)（群命令过滤与 `tool_only` 规则）、
[wecom-reply-feedback.md](./wecom-reply-feedback.md)（占位流收口口径）。

## 背景

企微智能机器人平台与官方 SDK 都支持出站媒体：`uploadMedia`（临时素材）
+ `sendMediaMessage` / `replyMedia`。本仓库的传输层也已接好——
`packages/services/src/bots/providers/wecomProvider.ts` 的 `sendMedia`、服务层
`deliverBotMedia` 的路径白名单与大小限额、`/send <路径>` 命令。

但用户实机对比 MyAgents 后反馈「同一个企微 bot 可以发消息也可以发文件，我们的不行」。
源码核对后确认两个产品缺口：

1. **群聊 `/send` 被静默丢弃**：A+ 群命令过滤只放行 message/stop/approve/deny，
   `/send` 不在白名单里（`botsService.handleInboundMessage`），用户 @ 机器人发
   `/send` 没有任何响应。
2. **机器人不会自己发文件**：`BotOutboundMessage` 只有 `text`，回合回复没有附件
   通道；`sendMedia` 的唯一调用点是用户手动 `/send`。agent 的工具面里也没有任何
   “发送文件”工具，因此“把刚生成的报告发给我”这类对话式请求无法完成。
   这也让 `deliveryMode: "tool_only"` 名不副实——普通输出被抑制，却不存在可用的
   显式发送工具（`botsService.sendOutbound` 的 tool_only 早退）。

## 产品规则

1. **`/send` 在私聊与群聊都可用**。它是用户显式发起的消息级动作，不属于群管理
   命令；群聊过滤放行 `send`，授权仍走 `withAuthorizedContext(message, "message")`，
   目标仍是 `resolveOutboundTarget(actor)`（群回群、私聊回本人），不新增第二套路由。
2. **agent 侧新增 `SendFile` 工具**，只在 bot 入站回合可见：
   - 工具输入只有 `path`（工作区相对路径或绝对路径）和可选 `caption`；
     **投递目标不进模型参数**，由 Host 注入的 `botDeliveryTarget` 决定。
   - 工具执行经 Host RPC 回到 `botsService`，复用 `deliverBotMedia`：
     realpath 解析、工作区/应用数据/系统临时目录白名单、图片 10MB / 文件 50MB
     上限、群聊回群、私聊回绑定用户。agent 进程不接触平台凭据，也不自行选择会话。
   - 非 bot 回合（桌面 UI、自动化、子代理）不注册/不可见该工具：runtime 按
     prompt 的 `botDeliveryTarget` 在当轮 `toolDisallowlist` 中隐藏 `SendFile`。
   - 工具结果返回 `{ status, file, kind }`；失败返回可读错误，不抛出未处理异常。
3. **`tool_only` 语义落地**：`tool_only` 仍抑制 AI 自动文本/流式卡片，但
   `SendFile` 属于显式发送，必须继续工作。用户在 tool_only 模式下可得到文件而
   不是整轮静默。
4. **企微 caption 与占位流**：企微媒体消息没有内联 caption 字段，`caption` 以一条
   独立文本在文件之前发送；不把说明拼进文件名，也不静默丢弃。媒体出站同样属于
   “真正出站”，必须收口等待期占位流——有 caption 时用 caption 原地替换，没有时用
   文件名收口；否则 `tool_only` 回合抑制自动文本后会留下永久 loading 的占位消息。
5. **一个发送所有者**：目标解析、路径校验、限额与 provider 分发仍只由
   `botsService.deliverBotMedia` 拥有；`SendFile` 与 `/send` 是同一实现的两个入口。
   企微有该会话的入站帧时优先 `replyMedia`（文件挂在触发消息下），没有帧才回落
   `sendMediaMessage` 主动推送；目标仍由服务层决定，provider 不自选收件人。
6. **远端 workspace 边界**：`SendFile` 的 Host RPC 由承载 agent 会话的 Host
   处理。若 bot 配置不在该 Host（例如远端 workspace 的 Host 没有本地 bot 记录），
   工具返回明确错误（bot 不存在 / 未配置），不回落本机路径或另起发送通道。

## 状态所有者与时序

```text
用户 /send <path> ──▶ botsService.handleSendMedia ──┐
                                                    ├─▶ deliverBotMedia（唯一所有者）
bot 回合 agent ──▶ SendFile 工具 ──▶ Host RPC ──────┘        │
  path+caption（模型只能给这些）   bots/sendMedia            ├─ 目标：resolveOutboundTarget(actor)
                                                             ├─ 路径：realpath + 白名单
                                                             ├─ 限额：图片 10MB / 文件 50MB
                                                             └─ provider.sendMedia（企微 uploadMedia + sendMediaMessage）
```

- `botDeliveryTarget` 由 `botsService` 随每次 bot prompt 下发，Host 只在该轮工具面
  放行 `SendFile`，轮末恢复为上一轮值；模型无法跨轮或跨会话继承发送能力。
- 同一 `botId` 的发送仍经 provider connection registry；失败按 `deliverBotMedia`
  既有语义冒泡为工具错误，不进入文本停放补投队列（文件不做重放，避免重复外发）。

## 验收场景

1. 单测（services）：群聊命令过滤放行 `/send`，仍静默丢弃其它管理命令；
   `deliverBotMedia` 的目标链、白名单与限额不变。
2. 单测（services）：企微 `sendMedia` 带 `caption` 时先发一条 markdown 再发媒体；
   不带 `caption` 时行为与现状一致。
3. 单测（mode-cli）：`SendFile` 只在 `botDeliveryTarget` 存在的回合可用；
   无目标时工具不可见/返回明确错误；路径与 caption 原样传给端口。
4. 单测（services）：`bots/sendMedia` 校验 bot 存在、provider 匹配，并调用
   `deliverBotMedia`。
5. **待实机**：
   - 企微私聊 `/send` 发出文件；
   - 企微群 @ 机器人后 `/send` 发到群里（不是发送者私聊）；
   - 对话式“把刚生成的报告发我”触发 `SendFile` 并收到文件；
   - `tool_only` 模式下正文不外发，但 `SendFile` 的文件仍送达；
   - 超过 50MB 或越界路径返回可读拒绝。

## 非目标

- 不改变 `/send` 的路径白名单与大小限额，不引入新的外部存储或网盘中转。
- 不把 agent 生成的每个文件自动外发；是否发送由模型显式调用 `SendFile` 决定。
- 不为飞书/钉钉新增 caption 语义（沿用现状），也不改变企微流式卡片与占位流。
- 不为远端 workspace 增加跨 Host 文件回传；该场景先返回明确错误。
